//! ohpm CLI entry point.

mod cli;
mod commands;
mod logger;
pub mod output;

use anyhow::Result;
use owo_colors::OwoColorize;

use cli::Command;

#[tokio::main]
async fn main() {
    logger::init();
    let command = cli::cli().run();
    if let Err(message) = cli::validate(&command) {
        eprintln!("{}", format!("ohpm error: {message}").red());
        std::process::exit(2);
    }
    if let Err(e) = run(command).await {
        eprintln!("{}", format!("ohpm error: {e:#}").red());
        std::process::exit(1);
    }
}

async fn run(command: Command) -> Result<()> {
    match command {
        Command::PrintVersion => {
            println!("ohpm-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Publish(args) => commands::publish::run(&args).await,
        Command::Prepublish(args) => commands::prepublish::run(&args).await,
        Command::Pack(args) => commands::pack::run(&args).await,
        Command::Init(args) => commands::init::run(&args).await,
        Command::Config(args) => commands::config::run(&args).await,
        Command::Login(args) => commands::login::run(&args).await,
        Command::Unpublish(args) => commands::unpublish::run(&args).await,
        Command::Info(args) => commands::info::run(&args).await,
        Command::List(args) => commands::list::run(&args).await,
        Command::Ping(args) => commands::ping::run(&args).await,
        Command::Root => commands::root::run().await,
        Command::Version(args) => commands::version::run(&args).await,
        Command::Cache(args) => commands::cache::run(args.action, &args.pkg).await,
        Command::Clean(args) => commands::clean::run(&args).await,
        Command::Install(args) => commands::install::run(&args).await,
        Command::Update(args) => commands::update::run(&args).await,
        Command::Uninstall(args) => commands::uninstall::run(&args).await,
        Command::Run(args) => commands::run::run(&args).await,
        Command::Build(args) => ohpm_native::build::build(args),
        Command::Artifact(args) => ohpm_native::artifact::artifact(args),
        Command::Cargo(args) => ohpm_native::cargo::cargo(args),
        Command::Doctor => ohpm_native::doctor::doctor(),
    }
}
