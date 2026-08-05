//! Constants mirroring `lib/common/Constants.js`.

pub const PM: &str = "ohpm";
pub const PM_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const PM_RC_DIR: &str = "resources";
pub const PM_RC: &str = ".ohpmrc";
pub const PM_DIR: &str = ".ohpm";
pub const DEFAULT_REGISTRY_FILE: &str = ".default-registry";
pub const MTIME_CACHE_DIR: &str = ".mtime";

pub const MY_PACKAGE_JSON: &str = "oh-package.json5";
pub const MY_MODULES: &str = "oh_modules";
pub const TMP_DIR_NAME: &str = ".tmp";
pub const LOCK_JSON: &str = "oh-package-lock.json5";
pub const BUILD_PROFILE: &str = "build-profile.json5";

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

/// Node user-agent components used in `getUserAgent()`: `ohpm/<ver> node/<node-ver>`.
pub fn user_agent() -> String {
    format!("{PM}/{PM_VERSION} rust/{}", std::env::consts::ARCH)
}
