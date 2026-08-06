//! Constants mirroring `lib/common/Constants.js`.

pub const PM: &str = "ohpm";

/// The reference ohpm version this tool is wire-compatible with. The registry
/// validates `_ohpmVersion` in the publish metadata and may gate the
/// user-agent on known client versions, so the protocol identity uses the
/// reference version (6.0.1, the DevEco Studio 6.1.1.280 bundle).
pub const PM_VERSION: &str = "6.0.1";

/// The reference's `RECOMMENDED_NODE_VERSION`, reported in `_nodeVersion` and
/// the user-agent (there is no Node here, but the registry expects the field).
pub const NODE_VERSION: &str = "18.0.0";

/// This crate's own version, used for display (`--version`, `config list`).
pub const PM_DISPLAY_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const PM_RC_DIR: &str = "resources";
pub const PM_RC: &str = ".ohpmrc";
pub const PM_DIR: &str = ".ohpm";
pub const DEFAULT_REGISTRY_FILE: &str = ".default-registry";
pub const MTIME_CACHE_DIR: &str = ".mtime";

pub const MY_PACKAGE_JSON: &str = "oh-package.json5";
pub const PACKAGE_JSON: &str = "package.json";
pub const MY_MODULES: &str = "oh_modules";
pub const NODE_MODULES: &str = "node_modules";
pub const TMP_DIR_NAME: &str = ".tmp";
pub const LOCK_JSON: &str = "oh-package-lock.json5";
pub const BUILD_PROFILE: &str = "build-profile.json5";
pub const SIGN_FOLDER_NAME: &str = ".CodeSignature";
pub const HSP_DIR: &str = ".hsp";

/// The "tag:" prefix for tag specs (`tag:<name>`).
pub const TAG_PREFIX: &str = "tag:";

/// The alias prefix (`ohpm:<real-package>@<spec>`), mirroring pnpm's `npm:`.
pub const ALIAS_PREFIX: &str = "ohpm:";

/// The workspace protocol prefix (`workspace:<range>`), mirroring pnpm.
pub const WORKSPACE_PREFIX: &str = "workspace:";

/// The HSP hspType values (`HspType.js`).
pub const HSP_TYPE_CROSS_APP: &str = "cross_app";

/// The cross-process lock file name at the project root (`oh-lock.lock`).
pub const LOCK_FILE_NAME: &str = "oh-lock.lock";

/// 6.0.1 hard-coded registry white-list (`[""]` — no real URL matches, so the
/// npm-registry check can never fire in production).
pub const REGISTRY_WHITE_LIST: [&str; 1] = [""];

/// Path/content compression for store-dir names (see `Constants.compressConfig`).
pub mod compress {
    pub const PATH_LEN: usize = 44;
    pub const PKG_HEAD: &str = "file:";
    pub const ALGORITHM: &str = "sha256";
    pub const ENCODING: &str = "base64";
    pub const REPLACE_TARGET: &str = "+";
    pub const TO_UPPER_CASE: bool = false;
}

/// File suffixes.
pub const TGZ_SUFFIX: &str = ".tgz";
pub const HAR_SUFFIX: &str = ".har";
pub const HSP_SUFFIX: &str = ".hsp";

/// Default (public) registry.
pub const DEFAULT_REGISTRY: &str = "https://ohpm.openharmony.cn/ohpm/";

/// Package size limits, in bytes.
pub const MIN_PACK_SIZE_MB: u64 = 0;
pub const MAX_PACK_SIZE_MB: u64 = 500;
pub const MAX_PACK_SIZE_B: u64 = MAX_PACK_SIZE_MB << 20;

/// HSP package type.
pub const HSP_PACKAGE_TYPE: &str = "InterfaceHar";
pub const HSP_TYPE_BUNDLE_APP: &str = "bundle_app";

pub const LATEST: &str = "latest";

/// Locked-package retry behavior (see `RetryUploaderProxy.js`).
pub const RETRY_CODE: u16 = 598;
pub const MAX_RETRY_TIMES: u32 = 3;
pub const RETRY_INTERVAL_MS: u64 = 30_000;

/// Headers shared by registry requests (see `registry.js` / uploaders).
pub const REQUEST_COMMAND: &str = "publish";
pub const REQUEST_VERSION: &str = "v1";
pub const LOGIN_REQUEST_VERSION: &str = "v1";

/// The registry user-agent, matching the reference format
/// `ohpm/<version> node/<node-version>`.
pub fn user_agent() -> String {
    format!("{PM}/{PM_VERSION} node/{NODE_VERSION}")
}
