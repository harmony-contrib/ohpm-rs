//! Registry packument fetching, mirroring
//! `lib/core/dependency/dep-fetcher/implementor/RegistryMetaDataFetcherImpl.js`
//! and the registry helpers in `lib/core/registry/registry.js`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::default::types;
use crate::config::Config;
use crate::error::{OhpmError, Result};
use crate::install::semver::{get_version_by_dist_tags, semver_max_satisfying};
use crate::registry::auth;
use crate::registry::RegistryClient;

/// `dist` of a published version.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dist {
    #[serde(default)]
    pub tarball: String,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub shasum: Option<String>,
    #[serde(rename = "resolved_hsp", default)]
    pub resolved_hsp: Option<String>,
    #[serde(rename = "integrity_hsp", default)]
    pub integrity_hsp: Option<String>,
}

/// One published version's metadata (a registry packument `versions` entry).
///
/// `resolved`/`integrity`/`shasum` sit at the top level for lockfile-derived
/// entries (the reference's `getLockPkgFromMetaData` lock packages carry them
/// as direct fields; registry entries carry them under `dist`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(rename = "packageType", default)]
    pub package_type: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dynamic_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub resolved: String,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub shasum: Option<String>,
    #[serde(rename = "_ohpmVersion", default)]
    pub ohpm_version: Option<String>,
    #[serde(rename = "hspType", default)]
    pub hsp_type: Option<String>,
    #[serde(rename = "isDebugHsp", default)]
    pub is_debug_hsp: Option<bool>,
    #[serde(default)]
    pub dist: Option<Dist>,
    /// Unknown fields round-trip.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The wire shape of a packument (registry responses).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPackument {
    #[serde(default)]
    name: String,
    #[serde(rename = "dist-tags", default)]
    dist_tags: BTreeMap<String, String>,
    #[serde(default)]
    versions: BTreeMap<String, VersionMeta>,
    #[serde(rename = "packageType", default)]
    package_type: Option<String>,
}

/// A registry packument (`{name, dist-tags, versions}`).
#[derive(Debug, Clone, Default)]
pub struct Packument {
    /// The requested package name.
    pub name: String,
    /// The `name` field of the packument document.
    pub actual_name: String,
    /// `dist-tags`.
    pub dist_tags: BTreeMap<String, String>,
    pub versions: BTreeMap<String, VersionMeta>,
    /// "ohpm" | "local" (npm white-list deferred).
    pub registry_type: String,
    pub is_from_lock_file: bool,
    /// The packument-level `packageType` (HSP marker).
    pub package_type: Option<String>,
    /// The response `package-type` header (checkRegistry signal).
    pub package_type_header: Option<String>,
}

impl Packument {
    pub fn version_keys(&self) -> Vec<String> {
        self.versions.keys().cloned().collect()
    }

    /// `checkRegistry` — reject a registry whose resolved version lacks the
    /// `_ohpmVersion` marker when the `package-type` header is not "ohpm".
    /// 6.0.1's white-list is `[""]`, so no real registry is whitelisted and
    /// this can never pass in production — kept for reference parity.
    pub fn check_registry(&self, meta: &VersionMeta, registry: &str, whitelist: &[String]) -> Result<()> {
        if !whitelist.contains(&registry.to_string())
            && meta.ohpm_version.is_none()
            && self.package_type_header.as_deref() != Some("ohpm")
        {
            return Err(OhpmError::check_registry_failed(registry));
        }
        Ok(())
    }
}

/// Memoized packument fetches, keyed `name@registry` (mirrors
/// `RegistryMetaDataFetcherImpl.fetchResultMap`; failed fetches are memoized
/// too, like the reference's `undefined` cache entries).
#[derive(Debug, Default)]
pub struct PackumentCache {
    inner: Mutex<HashMap<String, Option<Arc<Packument>>>>,
}

/// `getRegistryList.js` — the ordered candidate registries: the `@scope:registry`
/// override, then the comma-separated `registry` config, then the default.
/// All normalized with a trailing slash.
pub fn registry_list(config: &Config, name: &str) -> Vec<String> {
    let mut list = Vec::new();
    if let Some(scope) = name.split('/').next() {
        if name.starts_with('@') && !scope.is_empty() {
            let scoped = config.get_string(&format!("{scope}:registry"));
            for entry in scoped.split(',') {
                let entry = entry.trim();
                if !entry.is_empty() {
                    list.push(crate::config::ensure_trailing_slash(entry));
                }
            }
        }
    }
    if list.is_empty() {
        // The reference falls back to `resources/.default-registry`.
        let raw = config.get_string(types::REGISTRY);
        let raw = if raw.trim().is_empty() {
            crate::constants::DEFAULT_REGISTRY.to_string()
        } else {
            raw
        };
        for entry in raw.split(',') {
            let entry = entry.trim();
            if !entry.is_empty() {
                list.push(crate::config::ensure_trailing_slash(entry));
            }
        }
    }
    list
}

/// `RegistryMetaDataFetcherImpl.fetchMetaDataFromRegistry` — try each registry
/// in order; the first whose packument resolves the fetch spec wins.
pub async fn fetch_packument(
    client: &RegistryClient,
    config: &Config,
    name: &str,
    fetch_spec: &str,
    cache: &PackumentCache,
) -> Result<Packument> {
    let list = registry_list(config, name);
    if list.is_empty() {
        return Err(OhpmError::fetcher_registry_fetch_pkg_info_failed(
            name,
            fetch_spec,
            "",
        ));
    }
    let timeout = config.get_number(types::FETCH_TIMEOUT) as u64;
    for registry in &list {
        let key = format!("{name}@{registry}");
        let cached = {
            let guard = cache.inner.lock().await;
            guard.get(&key).cloned()
        };
        let packument = match cached {
            Some(Some(p)) => p.clone(),
            Some(None) => continue,
            None => {
                match fetch_one(client, config, name, registry, timeout).await {
                    Ok(Some(p)) => {
                        let p = Arc::new(p);
                        cache.inner.lock().await.insert(key, Some(p.clone()));
                        p
                    }
                    Ok(None) => {
                        cache.inner.lock().await.insert(key, None);
                        continue;
                    }
                    Err(e) => {
                        cache.inner.lock().await.insert(key, None);
                        log::warn!("fetch meta info of package '{name}' failed: {e}");
                        continue;
                    }
                }
            }
        };
        // `e.versions && (semverMaxSatisfying(...) || getVersionByDistTags(...))`
        let keys: std::collections::BTreeSet<String> =
            packument.version_keys().into_iter().collect();
        let resolves = !packument.versions.is_empty()
            && (semver_max_satisfying(&packument.version_keys(), fetch_spec).is_some()
                || get_version_by_dist_tags(fetch_spec, &packument.dist_tags, &keys).is_some());
        if resolves {
            log::info!("fetch meta info of package '{name}' success: {registry}{name}");
            // `checkRegistry` — the winning version's metadata must carry the
            // ohpm marker (no-op in production: the white-list is [""]).
            let pinned = semver_max_satisfying(&packument.version_keys(), fetch_spec)
                .or_else(|| get_version_by_dist_tags(fetch_spec, &packument.dist_tags, &keys));
            let whitelist: Vec<String> = crate::constants::REGISTRY_WHITE_LIST
                .iter()
                .map(|s| s.to_string())
                .collect();
            if let Some(p) = pinned {
                if let Some(meta) = packument.versions.get(&p) {
                    packument.check_registry(meta, registry, &whitelist)?;
                }
            }
            return Ok((*packument).clone());
        }
        log::debug!(
            "fetch meta info of package '{name}' success, but version '{fetch_spec}' not exist: {registry}{name}"
        );
    }
    let list_str = list.join(", ");
    Err(OhpmError::fetcher_registry_fetch_pkg_info_failed(
        name,
        fetch_spec,
        &list_str,
    ))
}

/// `realMetadataFetch` + `handleResponse` — GET `{registry}{name}` (the name
/// is not URL-encoded, like the reference), manual redirect (one hop), auth
/// from the registry's read token, `package-type` header captured. `Ok(None)`
/// on a non-2xx response (the caller tries the next registry).
async fn fetch_one(
    client: &RegistryClient,
    config: &Config,
    name: &str,
    registry: &str,
    timeout: u64,
) -> Result<Option<Packument>> {
    let token = auth::read_token(config, registry);
    let url = format!("{registry}{name}");
    let response = get_with_redirect(client, config, &url, &token, timeout).await?;
    let status = response.status();
    let package_type_header = response.headers()
        .get("package-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if !status.is_success() {
        log::warn!(
            "fetch meta info of package '{name}' failed: - GET {url} {status}"
        );
        return Ok(None);
    }
    let body = response.bytes().await.map_err(OhpmError::from)?;
    let raw: RawPackument = serde_json::from_slice(&body)
        .map_err(|e| OhpmError::request_failed(&format!("invalid packument shape: {e}")))?;
    Ok(Some(Packument {
        name: name.to_string(),
        actual_name: raw.name,
        dist_tags: raw.dist_tags,
        versions: raw.versions,
        registry_type: "ohpm".to_string(),
        is_from_lock_file: false,
        package_type: raw.package_type.or(package_type_header.clone()),
        package_type_header,
    }))
}

/// GET with manual redirect handling (mirrors `getRedirectUrlFromResponse` +
/// `redirectFetch`): a 301/302 with a `location` header is re-fetched once
/// against the redirect URL (with the redirect URL's own auth token).
pub(crate) async fn get_with_redirect(
    client: &RegistryClient,
    config: &Config,
    url: &str,
    token: &str,
    timeout: u64,
) -> Result<reqwest::Response> {
    let mut request = client
        .http()
        .get(url)
        .header("user-agent", RegistryClient::user_agent());
    if !token.is_empty() {
        request = request.header("authorization", token);
    }
    let response = request
        .timeout(std::time::Duration::from_millis(timeout))
        .send()
        .await
        .map_err(OhpmError::from)?;
    let status = response.status();
    let location = if status == reqwest::StatusCode::MOVED_PERMANENTLY
        || status == reqwest::StatusCode::FOUND
    {
        response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    } else {
        None
    };
    if let Some(location) = location {
        let redirect_token = auth::read_token(config, &location);
        let mut request = client
            .http()
            .get(location)
            .header("user-agent", RegistryClient::user_agent());
        if !redirect_token.is_empty() {
            request = request.header("authorization", redirect_token);
        }
        return Ok(request
            .timeout(std::time::Duration::from_millis(timeout))
            .send()
            .await
            .map_err(OhpmError::from)?);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_list_shape() {
        // Covered via the config tests; here only the trailing-slash helper.
        assert_eq!(
            crate::config::ensure_trailing_slash("https://x.y/ohpm"),
            "https://x.y/ohpm/"
        );
        assert_eq!(
            crate::config::ensure_trailing_slash("https://x.y/ohpm/"),
            "https://x.y/ohpm/"
        );
    }

    #[test]
    fn parse_packument_shape() {
        let json = r#"{
            "name": "@ohos/foo",
            "dist-tags": { "latest": "1.2.3", "beta": "1.3.0-beta.1" },
            "versions": {
                "1.2.3": {
                    "name": "@ohos/foo",
                    "version": "1.2.3",
                    "dependencies": { "@ohos/bar": "^1.0.0" },
                    "dist": {
                        "tarball": "https://r/ohpm/@ohos/foo/-/foo-1.2.3.tgz",
                        "shasum": "abc",
                        "integrity": "sha512-XYZ"
                    }
                }
            }
        }"#;
        let raw: RawPackument = serde_json::from_str(json).unwrap();
        let p = Packument {
            name: "@ohos/foo".to_string(),
            actual_name: raw.name,
            dist_tags: raw.dist_tags,
            versions: raw.versions,
            registry_type: "ohpm".to_string(),
            is_from_lock_file: false,
            package_type: raw.package_type,
            package_type_header: None,
        };
        assert_eq!(p.actual_name, "@ohos/foo");
        assert_eq!(p.dist_tags["latest"], "1.2.3");
        let meta = &p.versions["1.2.3"];
        assert_eq!(meta.dependencies["@ohos/bar"], "^1.0.0");
        let dist = meta.dist.as_ref().unwrap();
        assert_eq!(dist.tarball, "https://r/ohpm/@ohos/foo/-/foo-1.2.3.tgz");
        assert_eq!(dist.shasum.as_deref(), Some("abc"));
        assert_eq!(dist.integrity.as_deref(), Some("sha512-XYZ"));
    }
}
