use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing::error;

use cargo_manage::{HoistConfig, hoist, restore, sort};

#[derive(Parser)]
#[command(name = "cargo-manage")]
#[command(bin_name = "cargo")]
#[command(author, version, about = "Workspace Cargo.toml manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: CargoCommand,
}

#[derive(Subcommand)]
pub enum CargoCommand {
    /// Manage workspace dependencies
    Manage(ManageArgs),
}

#[derive(Parser)]
pub struct ManageArgs {
    #[command(subcommand)]
    pub command: Option<ManageCommand>,

    /// Root directory of the workspace
    #[arg(short, long, global = true, default_value = ".")]
    pub root: PathBuf,

    /// Perform a dry run without modifying files
    #[arg(long, global = true, default_value_t = false)]
    pub dry_run: bool,

    /// Enable verbose output
    #[arg(short, long, global = true, default_value_t = false)]
    pub verbose: bool,

    /// Prefix for priority sorting (deps with this prefix come first)
    #[arg(long, global = true)]
    pub prefix: Option<String>,
}

#[derive(Subcommand)]
pub enum ManageCommand {
    /// Hoist dependency versions to workspace.dependencies (default command)
    Deps,
    /// Sort dependencies in all Cargo.toml files
    Sort,
    /// Restore Cargo.toml files from .bak backups
    Restore,
}

fn main() {
    let cli = Cli::parse();
    let CargoCommand::Manage(args) = cli.command;

    // Initialize tracing with appropriate log level
    let level = if args.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(level.into())
                .from_env_lossy(),
        )
        .with_target(false)
        .init();

    // Build configuration
    let mut config = HoistConfig::new();
    if let Some(prefix) = args.prefix {
        config = config.with_prefix(prefix);
    }

    let result = match args.command {
        Some(ManageCommand::Deps) | None => hoist(&args.root, &config, args.dry_run).map(|_| ()),
        Some(ManageCommand::Sort) => sort(&args.root, &config, args.dry_run).map(|_| ()),
        Some(ManageCommand::Restore) => restore(&args.root, &config, args.dry_run).map(|_| ()),
    };

    if let Err(e) = result {
        error!("{}", e);
        std::process::exit(1);
    }
}
