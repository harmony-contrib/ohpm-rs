//! Argument types shared by the native command entry points and the CLI
//! parser.

use std::path::PathBuf;

use crate::Arch;

#[derive(Debug, Clone)]
pub struct InitArgs {
    /// `--package`: ohpm package name; defaults to the project name.
    pub package_name: Option<String>,
    /// Project directory name.
    pub name: String,
    /// Directory that will contain the new project.
    pub base: PathBuf,
}

#[derive(Debug, Clone)]
pub struct BuildArgs {
    pub dist: String,
    pub arch: Option<Vec<Arch>>,
    pub release: bool,
    pub copy_static: bool,
    pub cargo_args: Option<Vec<String>>,
    pub skip_libs: bool,
    pub dts_cache: bool,
    pub target_dir: Option<String>,
    pub skip_check: bool,
    pub zigbuild: bool,
    pub bisheng: bool,
    pub package: Option<String>,
    pub skip_napi_check: bool,
    pub soname: Option<String>,
    pub sta_atomic: bool,
    pub dyn_atomic: bool,
    /// `--sdk-native`: explicit OpenHarmony `native` directory.
    pub sdk_native: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ArtifactArgs {
    pub dist: String,
    pub name: String,
    pub skip_libs: bool,
    pub package: Option<String>,
    pub no_workspace: bool,
}

#[derive(Debug, Clone)]
pub struct CargoArgs {
    pub arch: Option<Vec<Arch>>,
    pub bisheng: bool,
    pub disable_target: bool,
    pub args: Vec<String>,
    pub package: Option<String>,
    pub soname: Option<String>,
    pub sta_atomic: bool,
    pub dyn_atomic: bool,
    /// `--sdk-native`: explicit OpenHarmony `native` directory.
    pub sdk_native: Option<String>,
}
