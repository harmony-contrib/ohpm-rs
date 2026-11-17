//! bpaf CLI definitions, mirroring the ohpm command surface plus the native
//! `ohrs` build commands.

use std::path::PathBuf;

use bpaf::{construct, long, positional, pure, Parser};
use ohpm_native::Arch;

#[derive(Debug, Clone)]
pub enum Command {
    PrintVersion,
    Publish(PublishArgs),
    Prepublish(PrepublishArgs),
    Pack(PackArgs),
    Init(InitArgs),
    Config(ConfigArgs),
    Login(LoginArgs),
    Unpublish(UnpublishArgs),
    Info(InfoArgs),
    List(ListArgs),
    Ping(PingArgs),
    Root,
    Version(VersionArgs),
    Cache(CacheArgs),
    Clean(CleanArgs),
    Install(InstallArgs),
    Update(UpdateArgs),
    Uninstall(UninstallArgs),
    Run(RunArgs),
    Build(ohpm_native::BuildArgs),
    Artifact(ohpm_native::ArtifactArgs),
    Cargo(ohpm_native::CargoArgs),
    Doctor,
}

#[derive(Debug, Clone)]
pub struct RunArgs {
    pub alias: String,
    pub args: Vec<String>,
    pub prefix: Option<PathBuf>,
    pub workspace: bool,
    pub filter: Vec<String>,
    pub log_level: Option<String>,
    pub debug: bool,
}

#[derive(Debug, Clone)]
pub struct PublishArgs {
    pub file: Option<String>,
    pub tag: Option<String>,
    pub publish_registry: Option<String>,
    pub publish_id: Option<String>,
    pub key_path: Option<String>,
    pub key_content: Option<String>,
    pub passphrase: Option<String>,
    pub timeout: Option<u64>,
    pub workspace: bool,
    pub filter: Vec<String>,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct PrepublishArgs {
    pub file: Option<String>,
    pub workspace: bool,
    pub filter: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PackArgs {
    pub source: Option<String>,
    pub output: Option<String>,
    pub workspace: bool,
    pub filter: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct InitArgs {
    pub name: Option<String>,
    pub project_type: Option<String>,
    pub group: Option<String>,
    pub bundle_name: Option<String>,
    pub api_version: Option<String>,
    pub path: Option<PathBuf>,
    pub yes: bool,
    pub force: bool,
    /// `--package` for `--type native` (ohpm package name).
    pub package: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConfigArgs {
    pub action: Option<String>,
    pub key: Option<String>,
    pub value: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct LoginArgs {
    pub publish_registry: Option<String>,
    pub publish_id: Option<String>,
    pub key_path: Option<String>,
    pub key_content: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UnpublishArgs {
    pub pkg: Option<String>,
    pub force: bool,
    pub publish_registry: Option<String>,
    pub publish_id: Option<String>,
    pub key_path: Option<String>,
    pub key_content: Option<String>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct InfoArgs {
    pub pkg: String,
    pub field: Option<String>,
    pub registry: Option<String>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ListArgs {
    pub pkg: Option<String>,
    pub depth: Option<u32>,
    pub json: bool,
    pub recursive: bool,
}

#[derive(Debug, Clone)]
pub struct PingArgs {
    pub registry: Option<String>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct VersionArgs {
    pub action: Option<String>,
    pub workspace: bool,
    pub filter: Vec<String>,
    pub preid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CacheArgs {
    pub action: Option<String>,
    pub pkg: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CleanArgs {
    pub keep_lockfile: bool,
    pub workspace: bool,
    pub filter: Vec<String>,
    pub prefix: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstallArgs {
    pub pkg: Vec<String>,
    pub save_dev: bool,
    pub save_prod: bool,
    pub save_dynamic: bool,
    pub no_save: bool,
    pub no_link: bool,
    /// Kept for reference-flag parity; the reference always behaves as if
    /// enabled, so the value is intentionally unused.
    #[allow(dead_code)]
    pub experimental_concurrently_safe: bool,
    pub all: bool,
    pub lockfile_only: bool,
    pub workspace: bool,
    pub filter: Vec<String>,
    pub prefix: Option<String>,
    pub parameter_file: Option<String>,
    pub target_path: Option<String>,
    pub registry: Option<String>,
    pub cache: Option<String>,
    pub fetch_timeout: Option<u64>,
    pub strict_ssl: Option<bool>,
    pub max_concurrent: Option<u64>,
    pub retry_times: Option<u32>,
    pub retry_interval: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct UpdateArgs {
    pub pkg: Vec<String>,
    pub workspace: bool,
    pub filter: Vec<String>,
    pub all: bool,
    pub all_modules: bool,
    pub tag_filter: Option<String>,
    pub prefix: Option<String>,
    pub registry: Option<String>,
    pub cache: Option<String>,
    pub fetch_timeout: Option<u64>,
    pub strict_ssl: Option<bool>,
    pub max_concurrent: Option<u64>,
    pub retry_times: Option<u32>,
    pub retry_interval: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct UninstallArgs {
    pub pkg: Vec<String>,
    pub no_save: bool,
    pub all: bool,
    pub workspace: bool,
    pub filter: Vec<String>,
    pub prefix: Option<String>,
    pub registry: Option<String>,
    pub cache: Option<String>,
    pub fetch_timeout: Option<u64>,
    pub strict_ssl: Option<bool>,
    pub max_concurrent: Option<u64>,
    pub retry_times: Option<u32>,
    pub retry_interval: Option<u64>,
}

fn comma_values(value: String) -> Vec<String> {
    value
        .split(',')
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn filter_parser() -> impl Parser<Vec<String>> {
    long("filter")
        .help("Select workspace packages by exact name (comma-separated).")
        .argument::<String>("pkgs")
        .map(comma_values)
        .many()
        .map(|groups| groups.into_iter().flatten().collect())
}

fn run_parser() -> impl Parser<RunArgs> {
    let alias = positional::<String>("alias").help("Script alias in oh-package.json5");
    let args = positional::<String>("args")
        .help("Append or override script parameters")
        .many();
    let prefix = long("prefix")
        .help("Package directory, or workspace search directory.")
        .argument::<PathBuf>("directory")
        .optional();
    let workspace = long("workspace")
        .help("Run the script in every workspace member that defines it.")
        .switch();
    let filter = filter_parser();
    let log_level = long("log_level")
        .help("Log level for this command: info, debug, warn, error.")
        .argument::<String>("level")
        .optional();
    let debug = long("debug")
        .help("Enable debug logging for this command.")
        .switch();
    construct!(RunArgs {
        prefix,
        workspace,
        filter,
        log_level,
        debug,
        alias,
        args
    })
}

fn publish_parser() -> impl Parser<PublishArgs> {
    let file = positional::<String>("har_or_tgz_file")
        .help("HAR/TGZ file or source directory; omit for the current package.")
        .optional();
    let tag = long("tag")
        .short('t')
        .help("Add a dist-tag to the current version.")
        .argument::<String>("tag")
        .optional();
    let publish_registry = long("publish_registry")
        .help("Registry to publish to.")
        .argument::<String>("registry")
        .optional();
    let publish_id = long("publish_id")
        .help("Publish id generated by the registry.")
        .argument::<String>("id")
        .optional();
    let key_path = long("key_path")
        .help("Path to the private key.")
        .argument::<String>("path")
        .optional();
    let key_content = long("key_content")
        .help("Private key PEM; use --key_content=<pem> when it starts with '-'.")
        .argument::<String>("content")
        .optional();
    let passphrase = long("passphrase")
        .help("Private key passphrase (prefer OHPM_KEY_PASSPHRASE).")
        .argument::<String>("passphrase")
        .optional();
    let timeout = long("timeout")
        .help("Network timeout in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    let workspace = long("workspace")
        .help("Publish every publishable workspace member.")
        .switch();
    let filter = filter_parser();
    let dry_run = long("dry-run").help("Validate without uploading.").switch();
    construct!(PublishArgs {
        tag,
        publish_registry,
        publish_id,
        key_path,
        key_content,
        passphrase,
        timeout,
        workspace,
        filter,
        dry_run,
        file
    })
}

fn prepublish_parser() -> impl Parser<PrepublishArgs> {
    let file = positional::<String>("har_or_tgz_file")
        .help("HAR/TGZ file or source directory; omit for the current package.")
        .optional();
    let workspace = long("workspace")
        .help("Validate every publishable workspace member.")
        .switch();
    let filter = filter_parser();
    construct!(PrepublishArgs {
        workspace,
        filter,
        file
    })
}

fn pack_parser() -> impl Parser<PackArgs> {
    let source = positional::<String>("source_dir")
        .help("Source directory containing oh-package.json5.")
        .optional();
    let output = long("output")
        .help("Output directory for the HAR.")
        .argument::<String>("directory")
        .optional();
    let workspace = long("workspace")
        .help("Pack every publishable workspace member.")
        .switch();
    let filter = filter_parser();
    construct!(PackArgs {
        output,
        workspace,
        filter,
        source
    })
}

fn init_parser() -> impl Parser<InitArgs> {
    let name = positional::<String>("name")
        .help("Package, module, or project name (defaults to the directory name).")
        .optional();
    let project_type = long("type")
        .short('t')
        .help("package | har | hsp | project | native (omit for the TUI).")
        .argument::<String>("type")
        .optional();
    let group = long("group")
        .short('g')
        .help("Package scope: @group/name.")
        .argument::<String>("group")
        .optional();
    let bundle_name = long("bundle-name")
        .help("Bundle name for generated projects.")
        .argument::<String>("bundle")
        .optional();
    let api_version = long("api-version")
        .help("SDK/API version, e.g. 5.0.0(12), 20, or 26.0.0.")
        .argument::<String>("version")
        .optional();
    let path = long("path")
        .short('p')
        .help("Directory to create the files in.")
        .argument::<PathBuf>("directory")
        .optional();
    let yes = long("yes")
        .short('y')
        .help("Accept defaults without prompting.")
        .switch();
    let force = long("force")
        .short('f')
        .help("Overwrite files that already exist.")
        .switch();
    let package = long("package")
        .help("ohpm package name for --type native.")
        .argument::<String>("package")
        .optional();
    construct!(InitArgs {
        project_type,
        group,
        bundle_name,
        api_version,
        path,
        yes,
        force,
        package,
        name
    })
}

fn config_parser() -> impl Parser<ConfigArgs> {
    let action = positional::<String>("action")
        .help("set | get | delete | list | encrypt")
        .optional();
    let key = positional::<String>("key")
        .help("Configuration key.")
        .optional();
    let value = positional::<String>("value")
        .help("Value for `set`.")
        .optional();
    let json = long("json")
        .short('j')
        .help("Print effective values as JSON.")
        .switch();
    construct!(ConfigArgs {
        json,
        action,
        key,
        value
    })
}

fn login_parser() -> impl Parser<LoginArgs> {
    let publish_registry = long("publish_registry")
        .help("Registry to log in to.")
        .argument::<String>("registry")
        .optional();
    let publish_id = long("publish_id")
        .help("Publish id generated by the registry.")
        .argument::<String>("id")
        .optional();
    let key_path = long("key_path")
        .help("Path to the private key.")
        .argument::<String>("path")
        .optional();
    let key_content = long("key_content")
        .help("Private key PEM; use --key_content=<pem> when it starts with '-'.")
        .argument::<String>("content")
        .optional();
    let passphrase = long("passphrase")
        .help("Private key passphrase.")
        .argument::<String>("passphrase")
        .optional();
    construct!(LoginArgs {
        publish_registry,
        publish_id,
        key_path,
        key_content,
        passphrase
    })
}

fn unpublish_parser() -> impl Parser<UnpublishArgs> {
    let pkg = positional::<String>("pkg")
        .help("Package name, optionally with @version.")
        .optional();
    let force = long("force")
        .short('f')
        .help("Unpublish all versions when no version is given.")
        .switch();
    let publish_registry = long("publish_registry")
        .help("Registry to unpublish from.")
        .argument::<String>("registry")
        .optional();
    let publish_id = long("publish_id")
        .help("Publish id generated by the registry.")
        .argument::<String>("id")
        .optional();
    let key_path = long("key_path")
        .help("Path to the private key.")
        .argument::<String>("path")
        .optional();
    let key_content = long("key_content")
        .help("Private key PEM; use --key_content=<pem> when it starts with '-'.")
        .argument::<String>("content")
        .optional();
    let timeout = long("timeout")
        .help("Network timeout in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    construct!(UnpublishArgs {
        force,
        publish_registry,
        publish_id,
        key_path,
        key_content,
        timeout,
        pkg
    })
}

fn info_parser() -> impl Parser<InfoArgs> {
    let pkg = positional::<String>("pkg").help("Package name, optionally with @version.");
    let field = positional::<String>("field")
        .help("Optional field to display.")
        .optional();
    let registry = long("registry")
        .help("Registry to query.")
        .argument::<String>("registry")
        .optional();
    let timeout = long("timeout")
        .help("Network timeout in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    construct!(InfoArgs {
        registry,
        timeout,
        pkg,
        field
    })
}

fn list_parser() -> impl Parser<ListArgs> {
    let pkg = positional::<String>("pkg")
        .help("Package name, optionally with @version.")
        .optional();
    let depth = long("depth")
        .short('d')
        .help("Maximum dependency-graph depth.")
        .argument::<u32>("depth")
        .optional();
    let json = long("json").short('j').help("Output JSON.").switch();
    let recursive = long("recursive")
        .short('r')
        .help("Print the graph for all modules.")
        .switch();
    construct!(ListArgs {
        depth,
        json,
        recursive,
        pkg
    })
}

fn ping_parser() -> impl Parser<PingArgs> {
    let registry = long("registry")
        .help("Registry to ping.")
        .argument::<String>("registry")
        .optional();
    let timeout = long("timeout")
        .help("Network timeout in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    construct!(PingArgs { registry, timeout })
}

fn version_parser() -> impl Parser<VersionArgs> {
    let action = positional::<String>("action")
        .help("newversion | major | minor | patch | premajor | preminor | prepatch | prerelease")
        .optional();
    let workspace = long("workspace")
        .help("Bump every package in the workspace to one version.")
        .switch();
    let filter = filter_parser();
    let preid = long("preid")
        .help("Pre-release identifier for pre* actions.")
        .argument::<String>("id")
        .optional();
    construct!(VersionArgs {
        workspace,
        filter,
        preid,
        action
    })
}

fn cache_parser() -> impl Parser<CacheArgs> {
    let action = positional::<String>("action")
        .help("clean | path | status | add")
        .optional();
    let pkg = positional::<String>("pkg")
        .help("Packages for `add`.")
        .many();
    construct!(CacheArgs { action, pkg })
}

fn clean_parser() -> impl Parser<CleanArgs> {
    let keep_lockfile_long = long("keep-lockfile")
        .help("Do not delete oh-package-lock.json5.")
        .switch();
    let keep_lockfile_alias = long("kl").help("Alias for --keep-lockfile.").switch();
    let keep_lockfile =
        construct!(keep_lockfile_long, keep_lockfile_alias).map(|(long, alias)| long || alias);
    let workspace = long("workspace")
        .help("Clean every workspace member.")
        .switch();
    let filter = filter_parser();
    let prefix = long("prefix")
        .help("Project root, or the directory used to locate the workspace.")
        .argument::<String>("directory")
        .optional();
    construct!(CleanArgs {
        keep_lockfile,
        workspace,
        filter,
        prefix
    })
}

fn install_parser() -> impl Parser<InstallArgs> {
    let pkg = positional::<String>("pkg")
        .help("Packages, folders, or HAR/TGZ files.")
        .many();
    let save_dev = long("save-dev").help("Save to devDependencies.").switch();
    let save_prod = long("save-prod").help("Save to dependencies.").switch();
    let save_dynamic = long("save-dynamic")
        .help("Save to dynamicDependencies.")
        .switch();
    let no_save = long("no-save")
        .help("Do not modify oh-package.json5.")
        .switch();
    let no_link = long("no-link")
        .help("Copy source dependencies instead of linking them.")
        .switch();
    let experimental_concurrently_safe = long("experimental-concurrently-safe")
        .help("Accepted for reference parity; the reference always behaves as enabled.")
        .switch();
    let all = long("all")
        .help("Install dependencies for all modules in the project.")
        .switch();
    let lockfile_only = long("lockfile-only")
        .help("Resolve dependencies and write the lockfile only.")
        .switch();
    let workspace = long("workspace")
        .help("Install every workspace member.")
        .switch();
    let filter = filter_parser();
    let prefix = long("prefix")
        .help("Package root, or the directory used to locate the workspace.")
        .argument::<String>("directory")
        .optional();
    let parameter_file = long("parameter-file")
        .help("Parameter file for parameterized installs.")
        .argument::<String>("path")
        .optional();
    let target_path = long("target_path")
        .help("Install for a dependencyMap.json5 target.")
        .argument::<String>("path")
        .optional();
    let registry = long("registry")
        .help("Override the registry.")
        .argument::<String>("registry")
        .optional();
    let cache = long("cache")
        .help("Override the local store directory.")
        .argument::<String>("directory")
        .optional();
    let fetch_timeout = long("fetch_timeout")
        .help("Network timeout in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    let strict_ssl = long("strict_ssl")
        .help("Verify TLS certificates (true/false).")
        .argument::<bool>("bool")
        .optional();
    let max_concurrent = long("max_concurrent")
        .help("Maximum concurrent tasks.")
        .argument::<u64>("n")
        .optional();
    let retry_times = long("retry_times")
        .help("Network retry count.")
        .argument::<u32>("n")
        .optional();
    let retry_interval = long("retry_interval")
        .help("Network retry interval in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    construct!(InstallArgs {
        save_dev,
        save_prod,
        save_dynamic,
        no_save,
        no_link,
        experimental_concurrently_safe,
        all,
        lockfile_only,
        workspace,
        filter,
        prefix,
        parameter_file,
        target_path,
        registry,
        cache,
        fetch_timeout,
        strict_ssl,
        max_concurrent,
        retry_times,
        retry_interval,
        pkg
    })
}

fn update_parser() -> impl Parser<UpdateArgs> {
    let pkg = positional::<String>("pkg")
        .help("Package names to update (without versions).")
        .many();
    let workspace = long("workspace")
        .help("Update every workspace member.")
        .switch();
    let filter = filter_parser();
    let all = long("all")
        .help("Update a package or all dependencies in the current module.")
        .switch();
    let all_modules = long("all-modules")
        .help("Update all modules in the project.")
        .switch();
    let tag_filter = long("tag-filter")
        .help("Update dependencies whose tag matches this regex.")
        .argument::<String>("regex")
        .optional();
    let prefix = long("prefix")
        .help("Package root, or the directory used to locate the workspace.")
        .argument::<String>("directory")
        .optional();
    let registry = long("registry")
        .help("Override the registry.")
        .argument::<String>("registry")
        .optional();
    let cache = long("cache")
        .help("Override the local store directory.")
        .argument::<String>("directory")
        .optional();
    let fetch_timeout = long("fetch_timeout")
        .help("Network timeout in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    let strict_ssl = long("strict_ssl")
        .help("Verify TLS certificates (true/false).")
        .argument::<bool>("bool")
        .optional();
    let max_concurrent = long("max_concurrent")
        .help("Maximum concurrent tasks.")
        .argument::<u64>("n")
        .optional();
    let retry_times = long("retry_times")
        .help("Network retry count.")
        .argument::<u32>("n")
        .optional();
    let retry_interval = long("retry_interval")
        .help("Network retry interval in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    construct!(UpdateArgs {
        workspace,
        filter,
        all,
        all_modules,
        tag_filter,
        prefix,
        registry,
        cache,
        fetch_timeout,
        strict_ssl,
        max_concurrent,
        retry_times,
        retry_interval,
        pkg
    })
}

fn uninstall_parser() -> impl Parser<UninstallArgs> {
    let pkg = positional::<String>("pkg")
        .help("Package names to uninstall (without versions).")
        .some("At least one package is required");
    let no_save = long("no-save")
        .help("Do not modify oh-package.json5.")
        .switch();
    let all = long("all")
        .help("Uninstall from the current module and reinstall all modules.")
        .switch();
    let workspace = long("workspace")
        .help("Uninstall from every workspace member.")
        .switch();
    let filter = filter_parser();
    let prefix = long("prefix")
        .help("Package root, or the directory used to locate the workspace.")
        .argument::<String>("directory")
        .optional();
    let registry = long("registry")
        .help("Override the registry.")
        .argument::<String>("registry")
        .optional();
    let cache = long("cache")
        .help("Override the local store directory.")
        .argument::<String>("directory")
        .optional();
    let fetch_timeout = long("fetch_timeout")
        .help("Network timeout in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    let strict_ssl = long("strict_ssl")
        .help("Verify TLS certificates (true/false).")
        .argument::<bool>("bool")
        .optional();
    let max_concurrent = long("max_concurrent")
        .help("Maximum concurrent tasks.")
        .argument::<u64>("n")
        .optional();
    let retry_times = long("retry_times")
        .help("Network retry count.")
        .argument::<u32>("n")
        .optional();
    let retry_interval = long("retry_interval")
        .help("Network retry interval in milliseconds.")
        .argument::<u64>("ms")
        .optional();
    construct!(UninstallArgs {
        no_save,
        all,
        workspace,
        filter,
        prefix,
        registry,
        cache,
        fetch_timeout,
        strict_ssl,
        max_concurrent,
        retry_times,
        retry_interval,
        pkg
    })
}

fn arch_parser(default: Vec<Arch>) -> impl Parser<Option<Vec<Arch>>> {
    long("arch")
        .short('a')
        .help("Target architecture: arm64, arm32, x86_64, loongarch64 (repeatable).")
        .argument::<Arch>("ARCH")
        .some("Please provide at least one architecture")
        .optional()
        .fallback(Some(default))
}

fn native_build_parser() -> impl Parser<ohpm_native::BuildArgs> {
    let dist = long("dist")
        .help("Output directory (defaults to `dist`).")
        .argument::<String>("DIST")
        .fallback(String::from("dist"));
    let release = long("release").help("Build with release mode.").switch();
    let arch = arch_parser(vec![Arch::ARM64, Arch::ARM32, Arch::X86_64]);
    let copy_static = long("static")
        .help("Copy static libraries to the output directory.")
        .switch();
    let cargo_args = positional::<String>("CARGO_ARGS")
        .help("Extra arguments passed to cargo build (after `--`).")
        .strict()
        .many()
        .optional();
    let skip_libs = long("skip-libs")
        .help("Do not copy dynamic libraries to the output directory.")
        .switch();
    let dts_cache = long("dts-cache")
        .help("Reuse the d.ts cache (enabled by default).")
        .flag(true, true);
    let skip_check = long("skip-check")
        .help("Skip the napi-ohos version check (enabled by default).")
        .flag(true, true);
    let target_dir = long("target-dir")
        .help("Temporary build directory (defaults to the project directory).")
        .argument::<String>("DIR")
        .optional();
    let zigbuild = long("zigbuild").help("Build with zigbuild.").switch();
    let bisheng = long("bisheng")
        .help("Build with the HarmonyOS BiSheng toolchain.")
        .switch();
    let skip_napi_check = long("skip-napi-check")
        .help("Skip the napi-ohos/napi-derive-ohos dependency check (enabled by default).")
        .flag(true, true);
    let package = long("package")
        .short('p')
        .help("Workspace package to build.")
        .argument::<String>("PACKAGE")
        .optional();
    let soname = long("soname")
        .help("Set the SONAME of the generated shared library.")
        .argument::<String>("SONAME")
        .optional();
    let sta_atomic = long("sta-atomic")
        .help("Statically link the bundled OHOS libatomic.")
        .switch();
    let dyn_atomic = long("dyn-atomic")
        .help("Dynamically link the bundled OHOS libatomic.")
        .switch();
    let sdk_native = long("sdk-native")
        .help("Explicit path to the SDK's `native` directory.")
        .argument::<String>("DIR")
        .optional();
    construct!(ohpm_native::BuildArgs {
        dist,
        arch,
        release,
        copy_static,
        skip_libs,
        dts_cache,
        target_dir,
        skip_check,
        zigbuild,
        bisheng,
        package,
        skip_napi_check,
        soname,
        sta_atomic,
        dyn_atomic,
        sdk_native,
        cargo_args
    })
}

fn artifact_parser() -> impl Parser<ohpm_native::ArtifactArgs> {
    let dist = long("dist")
        .short('d')
        .help("Directory containing the build products (defaults to `dist`).")
        .argument::<String>("DIST")
        .fallback(String::from("dist"));
    let name = long("name")
        .short('n')
        .help("Name of the generated .har file (defaults to `package`).")
        .argument::<String>("NAME")
        .fallback(String::from("package"));
    let skip_libs = long("skip-libs")
        .help("Do not copy dynamic libraries into the .har.")
        .switch();
    let package = long("package")
        .short('p')
        .help("Workspace package to package.")
        .argument::<String>("PACKAGE")
        .optional();
    let no_workspace = long("no-workspace")
        .help("Ignore workspace mode and package the current directory.")
        .switch();
    construct!(ohpm_native::ArtifactArgs {
        dist,
        name,
        skip_libs,
        package,
        no_workspace
    })
}

fn native_cargo_parser() -> impl Parser<ohpm_native::CargoArgs> {
    let arch = arch_parser(vec![Arch::ARM64]);
    let bisheng = long("bisheng")
        .help("Use the HarmonyOS BiSheng toolchain.")
        .switch();
    let disable_target = long("disable-target")
        .help("Do not add the default `--target` argument; run the command once.")
        .switch();
    let args = positional::<String>("CARGO_ARGS")
        .help("Cargo command and arguments (after `--`).")
        .strict()
        .many();
    let package = long("package")
        .short('p')
        .help("Workspace package to run the command for.")
        .argument::<String>("PACKAGE")
        .optional();
    let soname = long("soname")
        .help("Set the SONAME of the generated shared library.")
        .argument::<String>("SONAME")
        .optional();
    let sta_atomic = long("sta-atomic")
        .help("Statically link the bundled OHOS libatomic.")
        .switch();
    let dyn_atomic = long("dyn-atomic")
        .help("Dynamically link the bundled OHOS libatomic.")
        .switch();
    let sdk_native = long("sdk-native")
        .help("Explicit path to the SDK's `native` directory.")
        .argument::<String>("DIR")
        .optional();
    construct!(ohpm_native::CargoArgs {
        arch,
        bisheng,
        disable_target,
        package,
        soname,
        sta_atomic,
        dyn_atomic,
        sdk_native,
        args
    })
}

pub fn cli() -> bpaf::OptionParser<Command> {
    let publish = publish_parser()
        .to_options()
        .command("publish")
        .help("Publish a package to the registry.")
        .map(Command::Publish);
    let prepublish = prepublish_parser()
        .to_options()
        .command("prepublish")
        .help("Pre-verify package content without publishing.")
        .map(Command::Prepublish);
    let pack = pack_parser()
        .to_options()
        .command("pack")
        .help("Build a har package from a source directory.")
        .map(Command::Pack);
    let init = init_parser()
        .to_options()
        .command("init")
        .help("Create an oh-package.json5 file or a HarmonyOS project.")
        .map(Command::Init);
    let config = config_parser()
        .to_options()
        .command("config")
        .help("Manage the ohpm configuration file.")
        .map(Command::Config);
    let login = login_parser()
        .to_options()
        .command("login")
        .help("Log in with a private key and store the access token.")
        .map(Command::Login);
    let unpublish = unpublish_parser()
        .to_options()
        .command("unpublish")
        .help("Unpublish a package from the registry.")
        .map(Command::Unpublish);
    let info = info_parser()
        .to_options()
        .command("info")
        .help("Display information about a package.")
        .map(Command::Info);
    let list = list_parser()
        .to_options()
        .command("list")
        .help("Display the dependency graph (basic).")
        .map(Command::List);
    let ping = ping_parser()
        .to_options()
        .command("ping")
        .help("Test network connectivity to the registry.")
        .map(Command::Ping);
    let root = pure(Command::Root)
        .to_options()
        .command("root")
        .help("Print the effective oh_modules folder.");
    let version = version_parser()
        .to_options()
        .command("version")
        .help("Bump the package version.")
        .map(Command::Version);
    let cache = cache_parser()
        .to_options()
        .command("cache")
        .help("Manage the ohpm cache folder.")
        .map(Command::Cache);
    let clean = clean_parser()
        .to_options()
        .command("clean")
        .help("Delete all 'oh_modules' directories and the lockfile.")
        .map(Command::Clean);
    let cls = clean_parser()
        .to_options()
        .command("cls")
        .help("Delete all 'oh_modules' directories and the lockfile.")
        .map(Command::Clean);
    let install = install_parser()
        .to_options()
        .command("install")
        .help("Install packages from the registry or local sources.")
        .map(Command::Install);
    let update = update_parser()
        .to_options()
        .command("update")
        .help("Update package(s) to their latest version.")
        .map(Command::Update);
    let up = update_parser()
        .to_options()
        .command("up")
        .help("Update package(s) to their latest version.")
        .map(Command::Update);
    let uninstall = uninstall_parser()
        .to_options()
        .command("uninstall")
        .help("Uninstall package(s).")
        .map(Command::Uninstall);
    let run = run_parser()
        .to_options()
        .command("run")
        .help("Run a script defined in oh-package.json5.")
        .map(Command::Run);
    let build = native_build_parser()
        .to_options()
        .command("build")
        .help("Build an OHOS native module (cargo-ohrs build).")
        .map(Command::Build);
    let artifact = artifact_parser()
        .to_options()
        .command("artifact")
        .help("Generate an ohpm .har file from build artifacts.")
        .map(Command::Artifact);
    let cargo = native_cargo_parser()
        .to_options()
        .command("cargo")
        .help("Run a cargo command inside the OHOS environment.")
        .map(Command::Cargo);
    let doctor = pure(Command::Doctor)
        .to_options()
        .command("doctor")
        .help("Verify that the native development environment is ready.");
    let print_version = long("version")
        .short('V')
        .help("Print version information.")
        .req_flag(Command::PrintVersion);

    construct!([
        publish,
        prepublish,
        pack,
        init,
        config,
        login,
        unpublish,
        info,
        list,
        ping,
        root,
        version,
        cache,
        clean,
        cls,
        install,
        update,
        up,
        uninstall,
        run,
        build,
        artifact,
        cargo,
        doctor,
        print_version
    ])
    .to_options()
    .descr("ohpm-rs (OpenHarmony package manager), a Rust reimplementation of ohpm")
}

/// Reject mutually exclusive option combinations.
pub fn validate(command: &Command) -> Result<(), String> {
    match command {
        Command::Install(args)
            if (args.workspace || !args.filter.is_empty())
                && (!args.pkg.is_empty() || args.target_path.is_some()) =>
        {
            Err("--workspace/--filter cannot be combined with packages or --target_path.".into())
        }
        Command::Update(args)
            if (args.workspace || !args.filter.is_empty()) && (args.all || args.all_modules) =>
        {
            Err("--workspace/--filter cannot be combined with --all or --all-modules.".into())
        }
        Command::Uninstall(args) if args.all && (args.workspace || !args.filter.is_empty()) => {
            Err("--all cannot be combined with --workspace or --filter.".into())
        }
        Command::Run(args)
            if args
                .log_level
                .as_deref()
                .is_some_and(|level| !matches!(level, "info" | "debug" | "warn" | "error")) =>
        {
            Err("--log_level must be one of info, debug, warn, error.".into())
        }
        _ => Ok(()),
    }
}
