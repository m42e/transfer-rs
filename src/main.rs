#[tokio::main]
async fn main() -> anyhow::Result<()> {
    transfer_rs::run().await
}
