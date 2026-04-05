use anyhow::{Context, Result, bail};

use crate::cli::DeleteArgs;
use crate::client::transfer::TransferClient;
use crate::storage::config::AppConfig;
use crate::storage::db::HistoryStore;
use crate::storage::paths::AppPaths;

pub async fn run(server_override: Option<String>, args: DeleteArgs) -> Result<()> {
    let paths = AppPaths::discover()?;
    let config = AppConfig::load_or_create(&paths)?;
    let store = HistoryStore::new(&paths)?;
    let server = config.resolve_server_url(server_override.as_deref());
    let transfer = TransferClient::new(&server)?;

    let record = match store.find_by_id_or_url(&args.id_or_url)? {
        Some(record) => record,
        None => bail!("no history record matched '{}'", args.id_or_url),
    };

    transfer
        .delete(&record.delete_url)
        .await
        .with_context(|| format!("failed to delete remote file {}", record.remote_name))?;
    store.mark_deleted(&record.id)?;

    println!("Deleted remote file: {}", record.original_name);
    Ok(())
}