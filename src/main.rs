use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use easytier_stun::{Config, Server};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the TOML configuration file.
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    /// Logging filter, for example "info" or "easytier_stun=debug".
    #[arg(long, default_value = "info", env = "RUST_LOG")]
    log: String,

    /// Parse and validate configuration, then exit.
    #[arg(long)]
    check_config: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&args.log)?)
        .with_target(false)
        .compact()
        .init();

    let config = Config::from_path(&args.config)?;
    if args.check_config {
        println!("configuration is valid");
        return Ok(());
    }
    Server::new(config)?.run().await
}
