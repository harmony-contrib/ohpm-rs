//! Project scaffolding: turn [`InitOptions`] into a file plan and write it.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use ohpm_core::package::validate::is_standard_package_name;

use super::{png, templates};

/// The kind of skeleton `init` creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitKind {
    /// Only `oh-package.json5` (the reference `ohpm init` behavior).
    Package,
    /// Standalone static-library (HAR) project.
    Har,
    /// Standalone shared-library (HSP) project.
    Hsp,
    /// Application project with an `entry` HAP module.
    Project,
}

impl InitKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "package" | "pkg" => Ok(Self::Package),
            "har" => Ok(Self::Har),
            "hsp" => Ok(Self::Hsp),
            "project" | "app" => Ok(Self::Project),
            other => bail!("unknown --type \"{other}\"; expected package, har, hsp, or project"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Har => "har",
            Self::Hsp => "hsp",
            Self::Project => "project",
        }
    }

    /// Human-readable name used in the success message.
    pub fn title(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Har => "HAR",
            Self::Hsp => "HSP",
            Self::Project => "HarmonyOS",
        }
    }
}

/// Inputs shared by the flag path and the TUI.
pub struct InitOptions {
    pub kind: InitKind,
    /// Absolute directory the files are written to.
    pub dir: PathBuf,
    /// Full package name for `har`/`hsp` (optionally scoped); display name
    /// for `project`.
    pub name: String,
    pub bundle_name: Option<String>,
    /// `--api-version` value; defaults to `5.0.0(12)`.
    pub api_version: Option<String>,
    pub force: bool,
}

/// One file in the scaffold.
pub struct PlannedFile {
    pub path: PathBuf,
    pub contents: Vec<u8>,
}

/// Everything the scaffold will create, relative to `InitOptions::dir`.
pub struct Plan {
    pub files: Vec<PlannedFile>,
    pub dirs: Vec<PathBuf>,
    /// Resolved application bundle name.
    pub bundle_name: Option<String>,
}

impl Plan {
    fn file(&mut self, path: impl Into<PathBuf>, contents: impl Into<Vec<u8>>) {
        self.files.push(PlannedFile {
            path: path.into(),
            contents: contents.into(),
        });
    }

    fn dir(&mut self, path: impl Into<PathBuf>) {
        self.dirs.push(path.into());
    }
}

/// Resolved values used by the template builders.
pub(super) struct Prepared {
    pub kind: InitKind,
    /// Full package name (har/hsp) or project name (project).
    pub name: String,
    /// Value of `AppScope/resources/base/element/string.json#app_name`.
    pub label: String,
    pub module_name: String,
    pub bundle_name: String,
    pub sdk: Sdk,
}

/// SDK/model versions used across the generated manifests.
pub(super) struct Sdk {
    /// `targetSdkVersion` / `compatibleSdkVersion`, e.g. `5.0.0(12)`.
    pub version: String,
    /// `modelVersion` in root manifests, e.g. `5.0.0`.
    pub model: String,
    /// Numeric API level.
    pub api: u32,
    /// `@ohos/hypium` version paired with this SDK.
    pub hypium: &'static str,
}

impl Sdk {
    pub fn uses_layered_icon(&self) -> bool {
        self.api >= 11
    }

    pub fn normalizes_ohm_url(&self) -> bool {
        self.api >= 12
    }
}

const DEFAULT_API_VERSION: &str = "5.0.0(12)";

pub fn plan(options: &InitOptions) -> Result<Plan> {
    let prepared = prepare(options)?;
    let mut plan = Plan {
        files: Vec::new(),
        dirs: Vec::new(),
        bundle_name: Some(prepared.bundle_name.clone()),
    };

    push_root(&mut plan, &prepared);
    match prepared.kind {
        InitKind::Project => push_project(&mut plan, &prepared),
        InitKind::Har => push_har(&mut plan, &prepared),
        InitKind::Hsp => push_hsp(&mut plan, &prepared),
        InitKind::Package => unreachable!("package manifests are written separately"),
    }
    Ok(plan)
}

/// Validate the options and resolve names/versions.
pub(super) fn prepare(options: &InitOptions) -> Result<Prepared> {
    if options.dir.exists() && !options.dir.is_dir() {
        bail!("{} is not a directory.", options.dir.display());
    }

    let sdk = parse_sdk(
        options
            .api_version
            .as_deref()
            .unwrap_or(DEFAULT_API_VERSION),
    )?;

    match options.kind {
        InitKind::Package => unreachable!("package manifests are written separately"),
        InitKind::Project => {
            let name = options.name.trim().to_string();
            if name.is_empty() {
                bail!("The project name must not be empty.");
            }
            if name.starts_with('@') || name.contains('/') || name.contains('\\') {
                bail!("The project name \"{name}\" must not be scoped or contain path separators.");
            }
            let bundle_name = resolve_bundle_name(options.bundle_name.as_deref(), &name)?;
            Ok(Prepared {
                kind: options.kind,
                label: name.clone(),
                name,
                module_name: "entry".to_string(),
                bundle_name,
                sdk,
            })
        }
        InitKind::Har | InitKind::Hsp => {
            if !is_standard_package_name(&options.name) {
                bail!(
                    "The package name \"{}\" is invalid. It must be a valid ohpm package name \
                     (lowercase letters, digits, '-', '_', '.'; at most 128 chars).",
                    options.name
                );
            }
            let module_name = module_identifier(&options.name);
            let bundle_name = resolve_bundle_name(options.bundle_name.as_deref(), &module_name)?;
            Ok(Prepared {
                kind: options.kind,
                label: module_name.clone(),
                module_name,
                bundle_name,
                name: options.name.clone(),
                sdk,
            })
        }
    }
}

fn parse_sdk(input: &str) -> Result<Sdk> {
    let value = input.trim();
    if value.is_empty() {
        bail!("--api-version must not be empty.");
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '(' | ')' | '-' | '_'))
    {
        bail!(
            "invalid --api-version \"{value}\"; expected characters from letters, digits, '.', \
             '(', ')', '-', and '_'"
        );
    }

    let (version, api, model) = if let Some((model, rest)) = value.split_once('(') {
        let api = rest
            .strip_suffix(')')
            .ok_or_else(|| anyhow!("invalid --api-version \"{value}\"; expected e.g. 5.0.0(12)"))?
            .trim()
            .parse::<u32>()
            .map_err(|_| {
                anyhow!("invalid API level in --api-version \"{value}\"; expected e.g. 5.0.0(12)")
            })?;
        if model.trim().is_empty() {
            bail!("invalid --api-version \"{value}\"; expected e.g. 5.0.0(12)");
        }
        (value.to_string(), api, model.trim().to_string())
    } else if value.chars().all(|c| c.is_ascii_digit()) {
        match value {
            "12" => ("5.0.0(12)".to_string(), 12, "5.0.0".to_string()),
            "20" => ("6.0.0(20)".to_string(), 20, "6.0.0".to_string()),
            "26" => ("26.0.0".to_string(), 26, "26.0.0".to_string()),
            _ => bail!(
                "unsupported bare API version \"{value}\"; pass a full SDK version such as \
                 5.0.0(12), or one of 12, 20, 26"
            ),
        }
    } else {
        let first = value.split('.').next().unwrap_or_default();
        let api = first.parse::<u32>().map_err(|_| {
            anyhow!("invalid --api-version \"{value}\"; expected 5.0.0(12), 20, or 26.0.0")
        })?;
        (value.to_string(), api, value.to_string())
    };

    let hypium = if api >= 26 {
        "1.0.25"
    } else if api >= 20 {
        "1.0.24"
    } else {
        "1.0.19"
    };

    Ok(Sdk {
        version,
        model,
        api,
        hypium,
    })
}

fn module_identifier(package_name: &str) -> String {
    let local = package_name.rsplit('/').next().unwrap_or(package_name);
    let mut ident: String = local
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if ident.is_empty() {
        ident.push_str("library");
    }
    if !ident
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        ident.insert(0, '_');
    }
    ident
}

pub(super) fn default_bundle_name(name: &str) -> String {
    format!("com.example.{}", bundle_segment(name))
}

fn resolve_bundle_name(bundle: Option<&str>, name: &str) -> Result<String> {
    match bundle {
        Some(bundle) => {
            if !is_valid_bundle_name(bundle) {
                bail!(
                    "The bundle name \"{bundle}\" is invalid. Use dot-separated segments of \
                     letters, digits, and '_', each starting with a letter."
                );
            }
            Ok(bundle.to_string())
        }
        None => Ok(default_bundle_name(name)),
    }
}

fn bundle_segment(name: &str) -> String {
    let mut segment: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if segment.is_empty() {
        segment.push_str("app");
    }
    if !segment
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        segment.insert_str(0, "app");
    }
    segment
}

fn is_valid_bundle_name(bundle: &str) -> bool {
    bundle.split('.').all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && segment
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
    })
}

fn push_root(plan: &mut Plan, prepared: &Prepared) {
    plan.file(".gitignore", templates::GITIGNORE);
    plan.file("code-linter.json5", templates::CODE_LINTER);
    plan.file("hvigorfile.ts", templates::HVIGORFILE_APP);
    plan.file(
        "oh-package.json5",
        templates::root_oh_package(&prepared.sdk),
    );
    plan.file(
        "hvigor/hvigor-config.json5",
        templates::hvigor_config(&prepared.sdk),
    );
    plan.file("AppScope/app.json5", templates::app_json5(prepared));
    plan.file(
        "AppScope/resources/base/element/string.json",
        templates::app_string_json(&prepared.label),
    );

    if prepared.sdk.uses_layered_icon() {
        plan.file(
            "AppScope/resources/base/media/background.png",
            png::background(),
        );
        plan.file(
            "AppScope/resources/base/media/foreground.png",
            png::foreground(),
        );
        plan.file(
            "AppScope/resources/base/media/layered_image.json",
            templates::LAYERED_IMAGE,
        );
    } else {
        plan.file(
            "AppScope/resources/base/media/app_icon.png",
            png::app_icon(),
        );
    }

    plan.file(
        "build-profile.json5",
        templates::root_build_profile(prepared),
    );
}

fn push_project(plan: &mut Plan, prepared: &Prepared) {
    let module = &prepared.module_name;
    let root = PathBuf::from(module);

    plan.file(root.join(".gitignore"), templates::MODULE_GITIGNORE);
    plan.file(
        root.join("build-profile.json5"),
        templates::hap_build_profile(),
    );
    plan.file(root.join("hvigorfile.ts"), templates::HVIGORFILE_HAP);
    plan.file(
        root.join("obfuscation-rules.txt"),
        templates::OBFUSCATION_RULES,
    );
    plan.file(root.join("oh-package.json5"), templates::ENTRY_OH_PACKAGE);
    plan.file(
        root.join("src/main/module.json5"),
        templates::entry_module_json5(&prepared.sdk),
    );
    plan.file(
        root.join("src/main/ets/entryability/EntryAbility.ets"),
        templates::entry_ability(&prepared.sdk),
    );
    plan.file(
        root.join("src/main/ets/entrybackupability/EntryBackupAbility.ets"),
        templates::ENTRY_BACKUP_ABILITY,
    );
    plan.file(
        root.join("src/main/ets/pages/Index.ets"),
        templates::index_page(&prepared.sdk),
    );

    for (name, contents) in templates::entry_resources(&prepared.sdk) {
        plan.file(root.join("src/main/resources").join(name), contents);
    }

    plan.dir(root.join("src/main/resources/rawfile"));
    plan.file(
        root.join("src/mock/mock-config.json5"),
        templates::MOCK_CONFIG,
    );

    push_tests(plan, &root, prepared, "phone");
}

fn push_har(plan: &mut Plan, prepared: &Prepared) {
    let module = &prepared.module_name;
    let root = PathBuf::from(module);

    plan.file(root.join(".gitignore"), templates::MODULE_GITIGNORE);
    plan.file(
        root.join("build-profile.json5"),
        templates::library_build_profile(),
    );
    plan.file(root.join("hvigorfile.ts"), templates::HVIGORFILE_HAR);
    plan.file(
        root.join("obfuscation-rules.txt"),
        templates::OBFUSCATION_RULES,
    );
    plan.file(root.join("consumer-rules.txt"), templates::CONSUMER_RULES);
    plan.file(
        root.join("oh-package.json5"),
        templates::har_oh_package(&prepared.name),
    );
    plan.file(root.join("Index.ets"), templates::HAR_INDEX);
    plan.file(
        root.join("src/main/module.json5"),
        templates::har_module_json5(module),
    );
    plan.file(
        root.join("src/main/ets/components/MainPage.ets"),
        templates::MAIN_PAGE,
    );
    plan.file(
        root.join("src/main/resources/base/element/string.json"),
        templates::HAR_STRING_JSON,
    );
    plan.file(
        root.join("src/main/resources/base/element/float.json"),
        templates::FLOAT_JSON,
    );
    plan.dir(root.join("src/main/resources/rawfile"));
    plan.file(
        root.join("src/mock/mock-config.json5"),
        templates::MOCK_CONFIG,
    );

    push_tests(plan, &root, prepared, "default");
}

fn push_hsp(plan: &mut Plan, prepared: &Prepared) {
    let module = &prepared.module_name;
    let root = PathBuf::from(module);

    plan.file(root.join(".gitignore"), templates::MODULE_GITIGNORE);
    plan.file(
        root.join("build-profile.json5"),
        templates::library_build_profile(),
    );
    plan.file(root.join("hvigorfile.ts"), templates::HVIGORFILE_HSP);
    plan.file(
        root.join("obfuscation-rules.txt"),
        templates::OBFUSCATION_RULES,
    );
    plan.file(root.join("consumer-rules.txt"), templates::CONSUMER_RULES);
    plan.file(
        root.join("oh-package.json5"),
        templates::hsp_oh_package(&prepared.name),
    );
    plan.file(root.join("Index.ets"), templates::HSP_INDEX);
    plan.file(
        root.join("src/main/module.json5"),
        templates::hsp_module_json5(module),
    );
    plan.file(
        root.join("src/main/ets/pages/Index.ets"),
        templates::index_page(&prepared.sdk),
    );
    plan.file(root.join("src/main/ets/utils/Calc.ets"), templates::CALC);
    plan.file(
        root.join("src/main/resources/base/element/string.json"),
        templates::HSP_STRING_JSON,
    );
    plan.file(
        root.join("src/main/resources/base/element/float.json"),
        templates::FLOAT_JSON,
    );
    plan.file(
        root.join("src/main/resources/base/profile/main_pages.json"),
        templates::MAIN_PAGES,
    );
    plan.dir(root.join("src/main/resources/base/media"));
    plan.dir(root.join("src/main/resources/rawfile"));
    plan.file(
        root.join("src/mock/mock-config.json5"),
        templates::MOCK_CONFIG,
    );

    push_tests(plan, &root, prepared, "phone, tablet, 2in1");
}

fn push_tests(plan: &mut Plan, root: &Path, prepared: &Prepared, device_types: &str) {
    plan.file(
        root.join("src/ohosTest/module.json5"),
        templates::test_module_json5(&prepared.module_name, device_types),
    );
    plan.file(
        root.join("src/ohosTest/ets/test/Ability.test.ets"),
        templates::ABILITY_TEST,
    );
    plan.file(
        root.join("src/ohosTest/ets/test/List.test.ets"),
        templates::OHOS_TEST_LIST,
    );
    plan.file(
        root.join("src/test/List.test.ets"),
        templates::UNIT_TEST_LIST,
    );
    plan.file(
        root.join("src/test/LocalUnit.test.ets"),
        templates::LOCAL_UNIT_TEST,
    );
}

/// Write the plan, refusing conflicts unless `options.force` is set.
pub fn write(options: &InitOptions, plan: &Plan) -> Result<()> {
    let conflicts: Vec<String> = plan
        .files
        .iter()
        .filter(|file| options.dir.join(&file.path).symlink_metadata().is_ok())
        .map(|file| options.dir.join(&file.path).display().to_string())
        .collect();

    if !conflicts.is_empty() && !options.force {
        let shown = conflicts
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ");
        let more = if conflicts.len() > 5 {
            format!("\n  ... and {} more", conflicts.len() - 5)
        } else {
            String::new()
        };
        bail!(
            "Refusing to overwrite {} existing file(s) in {} (use --force to overwrite):\n  {}{more}",
            conflicts.len(),
            options.dir.display(),
            shown
        );
    }

    std::fs::create_dir_all(&options.dir)
        .map_err(|e| anyhow!("failed to create {}: {e}", options.dir.display()))?;

    for dir in &plan.dirs {
        std::fs::create_dir_all(options.dir.join(dir))
            .map_err(|e| anyhow!("failed to create {}: {e}", options.dir.join(dir).display()))?;
    }

    for file in &plan.files {
        let path = options.dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!("failed to create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, &file.contents)
            .map_err(|e| anyhow!("failed to write {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_identifier_sanitizes_scopes_and_separators() {
        assert_eq!(module_identifier("@demo/my-lib"), "my_lib");
        assert_eq!(module_identifier("@demo/lib.name"), "lib_name");
        assert_eq!(module_identifier("plain"), "plain");
    }

    #[test]
    fn bundle_segment_is_lowercase_and_starts_with_letter() {
        assert_eq!(bundle_segment("My App!"), "myapp");
        assert_eq!(bundle_segment("123"), "app123");
        assert_eq!(bundle_segment("你好"), "app");
    }

    #[test]
    fn sdk_parses_common_forms() {
        let sdk = parse_sdk("5.0.0(12)").unwrap();
        assert_eq!(
            (sdk.version.as_str(), sdk.api, sdk.model.as_str()),
            ("5.0.0(12)", 12, "5.0.0")
        );

        let sdk = parse_sdk("20").unwrap();
        assert_eq!((sdk.version.as_str(), sdk.api), ("6.0.0(20)", 20));

        let sdk = parse_sdk("26.0.0").unwrap();
        assert_eq!(
            (sdk.version.as_str(), sdk.api, sdk.model.as_str()),
            ("26.0.0", 26, "26.0.0")
        );
    }

    #[test]
    fn sdk_rejects_unknown_bare_api() {
        assert!(parse_sdk("17").is_err());
        assert!(parse_sdk("not-a-version").is_err());
    }
}
