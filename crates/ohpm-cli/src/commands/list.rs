//! `ohpm list [-r/--recursive] [-j/--json] [-d/--depth] [pkg]` — display the
//! dependency graph (mirrors `lib/commands/list.js` + the list service; the
//! graph is built in the `Installed` mode and rendered byte-identically).

use anyhow::Result;
use ohpm_core::install::list::{list, list_recursive, ListOptions};
use ohpm_core::registry::RegistryClient;

use super::{load_config, output, resolve_prefix};
use crate::cli::ListArgs;

pub async fn run(args: &ListArgs) -> Result<()> {
    let config = load_config()?;
    let prefix = resolve_prefix(None, "list")?;
    let opts = ListOptions {
        depth: args.depth,
        json: args.json,
        pkg: args.pkg.clone(),
    };
    let client = RegistryClient::from_config(&config)?;
    let outcome = if args.recursive {
        list_recursive(&client, &config, &prefix, &opts).await?
    } else {
        list(&client, &config, &prefix, &opts).await?
    };
    // The rendered output already carries its line endings.
    print!("{}", outcome.output);
    for problem in &outcome.problems {
        output::warn(problem);
    }
    Ok(())
}
