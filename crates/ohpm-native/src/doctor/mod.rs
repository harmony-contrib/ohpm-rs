use owo_colors::OwoColorize;

use render_result::render;
use semver::Version;
use target::{resolve_rust_version, resolve_targets};

use crate::Arch;
mod render_result;
mod target;

pub fn doctor() -> anyhow::Result<()> {
    let targets = resolve_targets()?;
    let origin_version = resolve_rust_version()?;
    let version = Version::parse(&origin_version)?;
    let sdk = crate::sdk::resolve_sdk(None);

    let msvc = Version::parse("1.88.0")?;

    let is_env_ok = sdk.is_ok();
    println!(
        "{}  SDK should be discoverable via {}.",
        render(is_env_ok),
        "OHOS_SDK_NATIVE / OHOS_NDK_HOME / OHOS_BASE_SDK_HOME / DEVECO_SDK_HOME".green()
    );
    match &sdk {
        Ok(sdk) => {
            println!("     OpenHarmony native: {}", sdk.native.display());
            if let Some(hms) = &sdk.hms_root {
                println!("     HarmonyOS hms:      {}", hms.display());
            }
        }
        Err(error) => println!("     {}", error.to_string().red()),
    }
    println!(
        "{}  Rust version should be >= 1.88.0.",
        render(version >= msvc)
    );
    [Arch::ARM64, Arch::ARM32, Arch::X86_64]
        .iter()
        .for_each(|arch| {
            let t = arch.rust_target();
            println!(
                "{}  Rustup target: {} should be installed.",
                render(targets.contains(&t.to_owned())),
                t.green(),
            );
        });
    Ok(())
}
