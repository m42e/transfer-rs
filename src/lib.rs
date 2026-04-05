pub mod cli;
pub mod client;
pub mod commands;
pub mod model;
pub mod storage;
pub mod tui;

pub async fn run() -> anyhow::Result<()> {
    cli::run().await
}