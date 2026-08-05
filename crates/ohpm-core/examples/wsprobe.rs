//! Read-only workspace probe: prints the workspace root, versioning mode, and
//! discovered members for a given directory.
//!
//! Usage: `cargo run -p ohpm-core --example wsprobe -- <dir>`

use ohpm_core::workspace::Workspace;

fn main() {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: wsprobe <dir>");
        std::process::exit(2);
    };
    match Workspace::find(std::path::Path::new(&dir)) {
        Ok(Some(ws)) => {
            println!("workspace root: {}", ws.root.display());
            println!("version mode: {:?}", ws.version_mode);
            for m in &ws.members {
                println!(
                    "  member {}@{}  ({})",
                    m.manifest.name,
                    m.manifest.version,
                    m.dir.display()
                );
            }
        }
        Ok(None) => println!("not inside a workspace"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
