pub mod cli;
pub mod client;
pub mod commands;
pub mod model;
pub mod storage;
pub mod tui;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run() -> anyhow::Result<()> {
    cli::run().await
}
