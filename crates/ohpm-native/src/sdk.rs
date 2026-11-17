use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::Arch;

/// Resolved SDK locations used by the native build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sdk {
    /// Directory containing `llvm`, `sysroot`, and `build` (the `native` dir).
    pub native: PathBuf,
    /// Parent of [`Self::native`]; the OpenHarmony SDK root expected by the
    /// build code.
    pub root: PathBuf,
    /// HarmonyOS (`hms`) SDK root, when the installed SDK includes one.
    pub hms_root: Option<PathBuf>,
}

/// Environment variables consulted when discovering an SDK.
#[derive(Debug, Clone, Default)]
pub struct SdkEnv {
    pub ohos_sdk_native: Option<String>,
    pub ohos_ndk_home: Option<String>,
    pub ohos_base_sdk_home: Option<String>,
    pub hos_ndk_home: Option<String>,
    pub deveco_sdk_home: Option<String>,
}

impl SdkEnv {
    pub fn from_env() -> Self {
        let read = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            ohos_sdk_native: read("OHOS_SDK_NATIVE"),
            ohos_ndk_home: read("OHOS_NDK_HOME"),
            ohos_base_sdk_home: read("OHOS_BASE_SDK_HOME"),
            hos_ndk_home: read("HOS_NDK_HOME"),
            deveco_sdk_home: read("DEVECO_SDK_HOME"),
        }
    }
}

/// Locate the OpenHarmony SDK.
///
/// `setup-ohos-sdk` exports `OHOS_SDK_NATIVE` pointing directly at the SDK's
/// `native` directory, while DevEco exports `OHOS_NDK_HOME` pointing at the
/// SDK root (which contains `native`). Both layouts, plus
/// `OHOS_BASE_SDK_HOME`/`DEVECO_SDK_HOME` and an explicit `--sdk-native`
/// override, are accepted.
pub fn resolve_sdk(explicit_native: Option<&str>) -> Result<Sdk> {
    resolve_sdk_with(explicit_native, SdkEnv::from_env())
}

pub fn resolve_sdk_with(explicit_native: Option<&str>, sdk_env: SdkEnv) -> Result<Sdk> {
    let mut candidates = Vec::new();
    let mut tried = Vec::new();

    let sources = [
        ("--sdk-native", explicit_native),
        ("OHOS_SDK_NATIVE", sdk_env.ohos_sdk_native.as_deref()),
        ("OHOS_NDK_HOME", sdk_env.ohos_ndk_home.as_deref()),
        ("OHOS_BASE_SDK_HOME", sdk_env.ohos_base_sdk_home.as_deref()),
        ("DEVECO_SDK_HOME", sdk_env.deveco_sdk_home.as_deref()),
        ("HOS_NDK_HOME", sdk_env.hos_ndk_home.as_deref()),
    ];
    for (name, value) in sources {
        let Some(value) = value else { continue };
        let path = PathBuf::from(value);
        tried.push(format!("{name}={}", path.display()));
        push_native_candidates(&mut candidates, &path);
    }

    for native in candidates {
        if !is_native_dir(&native) {
            continue;
        }
        let root = native
            .parent()
            .ok_or_else(|| anyhow!("SDK native directory has no parent: {}", native.display()))?
            .to_path_buf();
        let hms_root = find_hms_root(&root, &sdk_env);
        return Ok(Sdk {
            native,
            root,
            hms_root,
        });
    }

    Err(anyhow!(
        "Could not locate an OpenHarmony SDK native directory. Set OHOS_SDK_NATIVE to the \
     `native` directory exported by setup-ohos-sdk, or OHOS_NDK_HOME to an SDK root \
     containing `native`. Tried:\n  {}",
        if tried.is_empty() {
            "(no SDK environment variables set)".to_string()
        } else {
            tried.join("\n  ")
        }
    ))
}

/// Find the HarmonyOS `hms` SDK root next to an OpenHarmony SDK root.
pub fn find_hms_root(root: &Path, sdk_env: &SdkEnv) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    let mut push = |path: PathBuf| {
        let path = normalize_sdk_root(path);
        if is_native_dir(&path.join("native")) && !candidates.contains(&path) {
            candidates.push(path);
        }
    };

    if root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("hms"))
    {
        push(root.to_path_buf());
    }
    push(root.join("hms"));
    if let Some(parent) = root.parent() {
        push(parent.join("hms"));
        push(parent.join("default").join("hms"));
    }
    if let Some(value) = sdk_env.hos_ndk_home.as_deref() {
        push(PathBuf::from(value));
    }
    if let Some(value) = sdk_env.deveco_sdk_home.as_deref() {
        push(PathBuf::from(value).join("default").join("hms"));
    }

    candidates.into_iter().next()
}

fn push_native_candidates(out: &mut Vec<PathBuf>, path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    out.push(path.to_path_buf());
    out.push(path.join("native"));
    out.push(path.join("default").join("openharmony").join("native"));
    out.push(path.join("default").join("hms").join("native"));

    // setup-ohos-sdk with `fixup_path` moves components into `<api>/native`;
    // DevEco keeps them under `<sdk>/default/openharmony/native`.
    if let Ok(entries) = fs::read_dir(path) {
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|child| child.is_dir())
            .collect();
        children.sort();
        for child in children {
            out.push(child.join("native"));
            out.push(child.join("openharmony").join("native"));
            out.push(child.join("hms").join("native"));
        }
    }
}

fn normalize_sdk_root(path: PathBuf) -> PathBuf {
    if is_native_name(&path) {
        path.parent().map(Path::to_path_buf).unwrap_or(path)
    } else {
        path
    }
}

fn is_native_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("native"))
}

fn is_native_dir(path: &Path) -> bool {
    path.is_dir()
        && (path.join("llvm").is_dir() || path.join("sysroot").is_dir() || is_native_name(path))
}

#[derive(Debug, Clone, Default)]
pub struct ToolchainPaths {
    pub ranlib: String,
    pub ar: String,
    pub cc: String,
    pub cxx: String,
    pub llvm_as: String,
    pub ld: String,
    pub strip: String,
    pub objdump: String,
    pub readobj: String,
    pub objcopy: String,
    pub nm: String,
    pub bin_dir: String,
    pub lib_dir: String,
}

#[derive(Debug, Clone, Default)]
pub struct HmsPaths {
    pub include: Option<String>,
    pub lib: Option<String>,
}

pub fn resolve_toolchain_paths(root: &str) -> ToolchainPaths {
    let bin_dir = Path::new(root).join("bin");
    let lib_dir = Path::new(root).join("lib");

    #[cfg(target_os = "windows")]
    let exe_suffix = ".exe";
    #[cfg(not(target_os = "windows"))]
    let exe_suffix = "";

    let to_string = |p: std::path::PathBuf| p.to_string_lossy().to_string();
    let tool = |name: &str| to_string(bin_dir.join(format!("{}{}", name, exe_suffix)));

    ToolchainPaths {
        ranlib: tool("llvm-ranlib"),
        ar: tool("llvm-ar"),
        cc: tool("clang"),
        cxx: tool("clang++"),
        llvm_as: tool("llvm-as"),
        ld: tool("ld.lld"),
        strip: tool("llvm-strip"),
        objdump: tool("llvm-objdump"),
        readobj: tool("llvm-readobj"),
        objcopy: tool("llvm-objcopy"),
        nm: tool("llvm-nm"),
        bin_dir: to_string(bin_dir),
        lib_dir: to_string(lib_dir),
    }
}

pub fn resolve_hms_paths(hos_ndk: &str, arch: &Arch) -> HmsPaths {
    if hos_ndk.is_empty() {
        return HmsPaths::default();
    }

    let include_path = Path::new(hos_ndk)
        .join("native")
        .join("sysroot")
        .join("usr")
        .join("include");
    let lib_path = Path::new(hos_ndk)
        .join("native")
        .join("sysroot")
        .join("usr")
        .join("lib")
        .join(arch.c_target());

    HmsPaths {
        include: include_path
            .exists()
            .then(|| include_path.to_string_lossy().to_string()),
        lib: lib_path
            .exists()
            .then(|| lib_path.to_string_lossy().to_string()),
    }
}

pub fn append_hms_link_flags(base_flags: &mut Vec<String>, hms_paths: &HmsPaths) {
    if let Some(lib) = hms_paths.lib.as_ref() {
        base_flags.push(format!("-L{}", lib));
        base_flags.push(format!("-Wl,-rpath-link,{}", lib));
    }
}

pub fn apply_hms_include_env(
    prepare_env: &mut HashMap<String, String>,
    hms_paths: &HmsPaths,
    rust_target: &str,
) {
    let Some(include) = hms_paths.include.as_ref() else {
        return;
    };

    let (include_flag, needs_shell_escaping) = hms_include_flag(include);

    append_env_with_flag(prepare_env, "TARGET_CFLAGS", &include_flag);
    append_env_with_flag(prepare_env, "TARGET_CXXFLAGS", &include_flag);

    // `cc-rs` splits `*FLAGS` on whitespace unless this is set. Enabling it
    // changes how every pre-existing `CFLAGS`/`TARGET_CFLAGS` value is parsed,
    // so only do it when the include path actually contains characters that
    // need shell escaping.
    if needs_shell_escaping {
        prepare_env.insert(String::from("CC_SHELL_ESCAPED_FLAGS"), String::from("1"));
    }

    let bindgen_target = rust_target.replace('-', "_");
    append_env_with_flag(
        prepare_env,
        &format!("BINDGEN_EXTRA_CLANG_ARGS_{}", bindgen_target),
        &include_flag,
    );
    append_env_with_flag(
        prepare_env,
        &format!("BINDGEN_EXTRA_CLANG_ARGS_{}", bindgen_target.to_uppercase()),
        &include_flag,
    );
}

/// Build the `-I` flag for an HMS include path and report whether it needs
/// shell-escaped parsing.
///
/// Windows SDK paths contain backslashes, which `shlex` treats as escapes, so
/// Windows-style paths are normalized to forward slashes first. The flag is
/// single-quoted only when the path contains shell metacharacters: `cc-rs`
/// (with `CC_SHELL_ESCAPED_FLAGS`) and `bindgen` both parse their flag
/// variables with `shlex`, so a quoted flag survives as one argument.
fn hms_include_flag(include: &str) -> (String, bool) {
    let include = normalize_sdk_path(include);
    let flag = format!("-I{include}");
    let needs_shell_escaping = needs_shell_quoting(&include);
    let flag = if needs_shell_escaping {
        quote_shell_arg(&flag)
    } else {
        flag
    };
    (flag, needs_shell_escaping)
}

fn normalize_sdk_path(path: &str) -> String {
    let windows_style = path.as_bytes().get(1) == Some(&b':') || path.starts_with("\\\\");
    if cfg!(windows) || windows_style {
        path.replace('\\', "/")
    } else {
        path.to_string()
    }
}

/// Join flags for consumers that parse the value with `shlex`, such as
/// OpenCV's `OPENCV_CLANG_ARGS`.
pub fn join_shell_escaped_flags(flags: &[String]) -> String {
    flags
        .iter()
        .map(|flag| quote_shell_arg(flag))
        .collect::<Vec<_>>()
        .join(" ")
}

fn needs_shell_quoting(value: &str) -> bool {
    value.chars().any(|c| {
        !(c.is_alphanumeric()
            || matches!(c, '/' | '.' | '_' | '-' | ':' | '+' | '@' | '%' | ',' | '='))
    })
}

fn quote_shell_arg(value: &str) -> String {
    if !needs_shell_quoting(value) {
        return value.to_string();
    }

    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for c in value.chars() {
        if c == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(c);
        }
    }
    quoted.push('\'');
    quoted
}

pub fn apply_ohos_cmake_env(
    prepare_env: &mut HashMap<String, String>,
    rust_target: &str,
    ndk: &str,
    arch: &Arch,
) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let target_key = format!("CMAKE_GENERATOR_{}", rust_target);
        let target_key_alt = format!("CMAKE_GENERATOR_{}", rust_target.replace('-', "_"));
        let has_generator = env::var(&target_key).is_ok()
            || env::var(&target_key_alt).is_ok()
            || env::var("TARGET_CMAKE_GENERATOR").is_ok()
            || env::var("CMAKE_GENERATOR").is_ok();

        if !has_generator {
            prepare_env.insert(target_key_alt, String::from("Ninja"));
            prepare_env.insert(
                String::from("TARGET_CMAKE_GENERATOR"),
                String::from("Ninja"),
            );
        }
    }

    let target_toolchain_key = format!("CMAKE_TOOLCHAIN_FILE_{}", rust_target);
    let target_toolchain_key_alt =
        format!("CMAKE_TOOLCHAIN_FILE_{}", rust_target.replace('-', "_"));
    let has_toolchain_file = env::var(&target_toolchain_key).is_ok()
        || env::var(&target_toolchain_key_alt).is_ok()
        || env::var("TARGET_CMAKE_TOOLCHAIN_FILE").is_ok()
        || env::var("CMAKE_TOOLCHAIN_FILE").is_ok();

    if !has_toolchain_file {
        let toolchain_file = write_ohos_cmake_toolchain(ndk, rust_target, arch)?;
        prepare_env.insert(target_toolchain_key_alt, toolchain_file.clone());
        prepare_env.insert(String::from("TARGET_CMAKE_TOOLCHAIN_FILE"), toolchain_file);
    }

    Ok(())
}

fn write_ohos_cmake_toolchain(ndk: &str, rust_target: &str, arch: &Arch) -> io::Result<String> {
    let sdk_toolchain = Path::new(ndk)
        .join("native")
        .join("build")
        .join("cmake")
        .join("ohos.toolchain.cmake");

    if !sdk_toolchain.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "OHOS CMake toolchain file not found: {}",
                sdk_toolchain.to_string_lossy()
            ),
        ));
    }

    let output_dir = env::current_dir()?
        .join("target")
        .join("ohrs")
        .join("cmake");
    fs::create_dir_all(&output_dir)?;

    let wrapper = output_dir.join(format!("ohos-{}.toolchain.cmake", rust_target));
    let sdk_toolchain = cmake_path(&sdk_toolchain);
    let content = format!(
        "set(OHOS_ARCH \"{}\" CACHE STRING \"OHOS target ABI\" FORCE)\ninclude(\"{}\")\n",
        arch.to_arch(),
        sdk_toolchain
    );
    fs::write(&wrapper, content)?;

    Ok(wrapper.to_string_lossy().to_string())
}

fn cmake_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn append_env_with_flag(prepare_env: &mut HashMap<String, String>, key: &str, append: &str) {
    let current = env::var(key).unwrap_or_default();
    let merged = if current.is_empty() {
        append.to_string()
    } else {
        format!("{} {}", current, append)
    };
    prepare_env.insert(key.to_string(), merged);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS_INCLUDE: &str =
        r"C:\Program Files\Huawei\DevEco Studio\sdk\default\hms\native\sysroot\usr\include";
    const NORMALIZED_INCLUDE: &str =
        "C:/Program Files/Huawei/DevEco Studio/sdk/default/hms/native/sysroot/usr/include";

    #[test]
    fn hms_include_flag_quotes_windows_paths_with_spaces() {
        let (flag, needs_shell_escaping) = hms_include_flag(WINDOWS_INCLUDE);
        assert!(needs_shell_escaping);
        assert_eq!(flag, format!("'-I{}'", NORMALIZED_INCLUDE));
    }

    #[test]
    fn hms_include_flag_leaves_plain_paths_unquoted() {
        let (flag, needs_shell_escaping) =
            hms_include_flag("/opt/ohos/sdk/native/sysroot/usr/include");
        assert!(!needs_shell_escaping);
        assert_eq!(flag, "-I/opt/ohos/sdk/native/sysroot/usr/include");
    }

    #[test]
    fn apply_hms_include_env_escapes_only_when_needed() {
        let spaced = HmsPaths {
            include: Some(WINDOWS_INCLUDE.to_string()),
            ..Default::default()
        };
        let mut env = HashMap::new();
        apply_hms_include_env(&mut env, &spaced, "aarch64-unknown-linux-ohos");

        assert_eq!(
            env.get("CC_SHELL_ESCAPED_FLAGS").map(String::as_str),
            Some("1")
        );
        for key in [
            "TARGET_CFLAGS",
            "TARGET_CXXFLAGS",
            "BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_linux_ohos",
            "BINDGEN_EXTRA_CLANG_ARGS_AARCH64_UNKNOWN_LINUX_OHOS",
        ] {
            assert!(
                env.get(key)
                    .is_some_and(|value| value.ends_with(&format!("'-I{}'", NORMALIZED_INCLUDE))),
                "{key}: {:?}",
                env.get(key)
            );
        }

        let plain = HmsPaths {
            include: Some("/opt/ohos/sdk/native/sysroot/usr/include".to_string()),
            ..Default::default()
        };
        let mut env = HashMap::new();
        apply_hms_include_env(&mut env, &plain, "aarch64-unknown-linux-ohos");

        assert!(!env.contains_key("CC_SHELL_ESCAPED_FLAGS"));
        assert!(env["TARGET_CFLAGS"].ends_with("-I/opt/ohos/sdk/native/sysroot/usr/include"));
    }

    #[test]
    fn join_shell_escaped_flags_preserves_spaces_and_backslashes() {
        let flags = vec![
            "--target=aarch64-linux-ohos".to_string(),
            r"--sysroot=C:\Program Files\Huawei\DevEco Studio\sdk".to_string(),
            r"-LC:\ohos sdk\lib".to_string(),
        ];

        assert_eq!(
            join_shell_escaped_flags(&flags),
            r#"--target=aarch64-linux-ohos '--sysroot=C:\Program Files\Huawei\DevEco Studio\sdk' '-LC:\ohos sdk\lib'"#
        );
    }

    #[test]
    fn quote_shell_arg_escapes_single_quotes() {
        assert_eq!(quote_shell_arg("a'b"), r#"'a'\''b'"#);
    }

    fn make_native(root: &Path) -> PathBuf {
        let native = root.join("native");
        fs::create_dir_all(native.join("llvm")).unwrap();
        native
    }

    #[test]
    fn resolves_setup_ohos_sdk_native_output() {
        let temp = tempfile::tempdir().unwrap();
        let native = make_native(&temp.path().join("sdk"));
        let sdk = resolve_sdk_with(
            None,
            SdkEnv {
                ohos_sdk_native: Some(native.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(sdk.native, native);
        assert_eq!(sdk.root, temp.path().join("sdk"));
        assert_eq!(sdk.hms_root, None);
    }

    #[test]
    fn resolves_ohos_ndk_home_root_and_api_fixup_layout() {
        let temp = tempfile::tempdir().unwrap();

        let root = temp.path().join("sdk");
        let native = make_native(&root);
        let sdk = resolve_sdk_with(
            None,
            SdkEnv {
                ohos_ndk_home: Some(root.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(sdk.native, native);
        assert_eq!(sdk.root, root);

        let base = temp.path().join("base");
        let native = make_native(&base.join("12"));
        let sdk = resolve_sdk_with(
            None,
            SdkEnv {
                ohos_base_sdk_home: Some(base.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(sdk.native, native);
        assert_eq!(sdk.root, base.join("12"));
    }

    #[test]
    fn resolves_deveco_hms_sibling() {
        let temp = tempfile::tempdir().unwrap();
        let openharmony = temp.path().join("sdk/default/openharmony");
        let native = make_native(&openharmony);
        let hms = make_native(&temp.path().join("sdk/default/hms"));

        let sdk = resolve_sdk_with(
            None,
            SdkEnv {
                ohos_ndk_home: Some(openharmony.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(sdk.native, native);
        assert_eq!(sdk.root, openharmony);
        assert_eq!(sdk.hms_root, Some(hms.parent().unwrap().to_path_buf()));
    }

    #[test]
    fn explicit_native_wins_and_missing_sdk_lists_sources() {
        let temp = tempfile::tempdir().unwrap();
        let explicit = make_native(&temp.path().join("explicit"));
        let env_native = make_native(&temp.path().join("env"));

        let sdk = resolve_sdk_with(
            Some(&explicit.to_string_lossy()),
            SdkEnv {
                ohos_sdk_native: Some(env_native.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(sdk.native, explicit);

        let error = resolve_sdk_with(None, SdkEnv::default()).unwrap_err();
        assert!(error.to_string().contains("Could not locate"));
    }
}
