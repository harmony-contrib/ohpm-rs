//! ohpm CLI entry point.

mod cli;
mod commands;
mod logger;
pub mod output;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};

#[tokio::main]
async fn main() {
    logger::init();
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("\x1b[31mohpm error: {e:#}\x1b[0m");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
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
        Command::Cache(args) => commands::cache::run(args.action).await,
        Command::Clean(args) => commands::clean::run(&args).await,
        Command::Install(args) => commands::install::run(&args).await,
        Command::Update(args) => commands::update::run(&args).await,
        Command::Uninstall(args) => commands::uninstall::run(&args).await,
    }
}
