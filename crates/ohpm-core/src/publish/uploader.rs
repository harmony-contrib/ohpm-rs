//! Package uploaders: stream (multipart) and attachment (base64 JSON),
//! with locked-package retry and stream-to-attachment fallback.
//! Mirrors `uploader/*` + `RetryUploaderProxy` + `OhpmPublication`.

use std::path::Path;

use base64::Engine;
use serde_json::Value;

use crate::constants;
use crate::error::{OhpmError, Result};
use crate::registry::RegistryClient;

/// Result of a successful publish.
#[derive(Debug, Clone)]
pub struct PublishResult {
    pub body: Value,
}

/// Error carrying the HTTP status so the caller can decide fallback/retry.
#[derive(Debug)]
struct UploadError {
    http_code: Option<u16>,
    message: String,
}

impl UploadError {
    fn from_status(status: u16, body: &str) -> Self {
        Self {
            http_code: Some(status),
            message: format!("HttpCode {status}, {body}"),
        }
    }
}

/// The package files and sizes to upload.
#[derive(Clone)]
pub struct PackageSource<'a> {
    pub har_path: &'a Path,
    pub hsp_path: Option<&'a Path>,
    /// Total package size in bytes (har + hsp), used for the upload threshold.
    pub size_bytes: u64,
    /// Whether this is a `.tgz` bundle (also uploads the `.hsp` attachment).
    pub is_tgz: bool,
}

/// Publish a package to `registry`.
///
/// Chooses stream vs attachment upload based on the total size and the
/// `use_stream_threshold_size` config.
pub async fn publish_package(
    client: &RegistryClient,
    registry: &str,
    token: &str,
    meta: &mut Value,
    source: &PackageSource<'_>,
    stream_threshold_mb: u64,
) -> Result<PublishResult> {
    let use_stream = source.size_bytes > 1024 * stream_threshold_mb * 1024;
    let mut attempts = 0;
    loop {
        let mut attempt_meta = meta.clone();
        match upload_once(client, registry, token, &mut attempt_meta, source, use_stream).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                // Locked (598): retry a bounded number of times.
                if e.http_code == Some(constants::RETRY_CODE) {
                    attempts += 1;
                    if attempts >= constants::MAX_RETRY_TIMES {
                        return Err(OhpmError::pkg_is_locked(meta["name"].as_str().unwrap_or("")));
                    }
                    log::warn!(
                        "package is locked, waiting {}s to retry (attempt {attempts}/{})",
                        constants::RETRY_INTERVAL_MS / 1000,
                        constants::MAX_RETRY_TIMES
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(constants::RETRY_INTERVAL_MS))
                        .await;
                    continue;
                }
                // Stream failure with 404/302/405 -> fall back to attachment once.
                if use_stream && matches!(e.http_code, Some(404 | 302 | 405)) {
                    log::warn!(
                        "stream publish failed ({message}); falling back to the attachment API",
                        message = e.message
                    );
                    return upload_once(client, registry, token, meta, source, false)
                        .await
                        .map_err(|e| {
                            OhpmError::request_failed(&format!(
                                "attachment upload failed: {}",
                                e.message
                            ))
                        });
                }
                return Err(OhpmError::request_failed(&e.message));
            }
        }
    }
}

async fn upload_once(
    client: &RegistryClient,
    registry: &str,
    token: &str,
    meta: &mut Value,
    source: &PackageSource<'_>,
    use_stream: bool,
) -> std::result::Result<PublishResult, UploadError> {
    if use_stream {
        stream_upload(client, registry, token, meta, source.har_path).await
    } else {
        attachment_upload(
            client,
            registry,
            token,
            meta,
            source.har_path,
            source.hsp_path,
            source.is_tgz,
        )
        .await
    }
}

/// POST `{registry}stream/{name}` with multipart `metadata` + `pkg_stream`.
async fn stream_upload(
    client: &RegistryClient,
    registry: &str,
    token: &str,
    meta: &Value,
    har_path: &Path,
) -> std::result::Result<PublishResult, UploadError> {
    let name = meta["name"].as_str().unwrap_or_default();
    let url = format!(
        "{}{}{}",
        crate::config::ensure_trailing_slash(registry),
        "stream/",
        crate::registry::url_encode_pkg_name(name)
    );

    let metadata_json = serde_json::to_string(meta).unwrap_or_else(|_| "{}".into());
    let bytes = tokio::fs::read(har_path).await.map_err(|e| UploadError {
        http_code: None,
        message: e.to_string(),
    })?;

    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata_json)
        .part(
            "pkg_stream",
            reqwest::multipart::Part::bytes(bytes).file_name(har_path.to_string_lossy().into_owned()),
        );

    let resp = client
        .http()
        .post(&url)
        .header("command", constants::REQUEST_COMMAND)
        .header("version", constants::REQUEST_VERSION)
        .header("user-agent", constants::user_agent())
        .header("authorization", token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| UploadError {
            http_code: None,
            message: e.to_string(),
        })?;

    let status = resp.status();
    if status.is_success() {
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        return Ok(PublishResult { body });
    }
    let text = resp.text().await.unwrap_or_default();
    Err(UploadError::from_status(status.as_u16(), &text))
}

/// PUT `{registry}{name}` with a JSON body carrying `_attachments` (base64).
async fn attachment_upload(
    client: &RegistryClient,
    registry: &str,
    token: &str,
    meta: &mut Value,
    har_path: &Path,
    hsp_path: Option<&Path>,
    is_tgz: bool,
) -> std::result::Result<PublishResult, UploadError> {
    let name = meta["name"].as_str().unwrap_or_default().to_string();
    let version = meta["versions"]
        .as_object()
        .and_then(|v| v.keys().next().cloned())
        .unwrap_or_default();
    let url = format!(
        "{}{}",
        crate::config::ensure_trailing_slash(registry),
        crate::registry::url_encode_pkg_name(&name)
    );

    // Build `_attachments`: { "<name>-<ver>.har": { content_type, data, length } }.
    let mut attachments = serde_json::Map::new();
    if let Value::Object(meta_map) = meta {
        let har_key = format!("{name}-{version}{}", constants::HAR_SUFFIX);
        let har_bytes = std::fs::read(har_path).map_err(|e| UploadError {
            http_code: None,
            message: e.to_string(),
        })?;
        attachments.insert(
            har_key,
            attachment_entry(&har_bytes),
        );
        if is_tgz {
            if let Some(hsp) = hsp_path {
                let hsp_key = format!("{name}-{version}{}", constants::HSP_SUFFIX);
                let hsp_bytes = std::fs::read(hsp).map_err(|e| UploadError {
                    http_code: None,
                    message: e.to_string(),
                })?;
                attachments.insert(hsp_key, attachment_entry(&hsp_bytes));
            }
        }
        meta_map.insert("_attachments".into(), Value::Object(attachments));
    }

    // Remove internal-only fields before upload.
    super::meta::clear_unused_fields(meta);

    let body = serde_json::to_vec(meta).map_err(|e| UploadError {
        http_code: None,
        message: e.to_string(),
    })?;

    let resp = client
        .http()
        .put(&url)
        .header("command", constants::REQUEST_COMMAND)
        .header("version", constants::REQUEST_VERSION)
        .header("user-agent", constants::user_agent())
        .header("Authorization", token)
        .header("Content-Type", "application/json; charset=UTF-8")
        .body(body)
        .send()
        .await
        .map_err(|e| UploadError {
            http_code: None,
            message: e.to_string(),
        })?;

    let status = resp.status();
    if status.is_success() {
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        return Ok(PublishResult { body });
    }
    let text = resp.text().await.unwrap_or_default();
    Err(UploadError::from_status(status.as_u16(), &text))
}

fn attachment_entry(bytes: &[u8]) -> Value {
    serde_json::json!({
        "content_type": "application/octet-stream",
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        "length": bytes.len(),
    })
}
