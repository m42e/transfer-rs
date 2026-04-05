use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

use crate::cli::HistoryArgs;
use crate::client::transfer::TransferClient;
use crate::storage::config::AppConfig;
use crate::storage::db::HistoryStore;
use crate::storage::paths::AppPaths;
use crate::tui::history::HistoryApp;

pub async fn run(server_override: Option<String>, args: HistoryArgs) -> Result<()> {
    run_with(server_override, args, HistoryApp::new, |app| Box::pin(app.run())).await
}

async fn run_with<App, Build, Run>(
    server_override: Option<String>,
    args: HistoryArgs,
    build_app: Build,
    run_app: Run,
) -> Result<()>
where
    Build: FnOnce(HistoryStore, TransferClient, bool) -> App,
    Run: for<'a> FnOnce(&'a mut App) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>,
{
    let paths = AppPaths::discover()?;
    let config = AppConfig::load_or_create(&paths)?;
    let store = HistoryStore::new(&paths)?;
    let server = config.resolve_server_url(server_override.as_deref());
    let transfer = TransferClient::new(&server)?;

    let mut app = build_app(store, transfer, args.show_deleted);
    run_app(&mut app).await
}

#[cfg(test)]
mod tests {
    use super::run_with;
    use crate::cli::HistoryArgs;
    use anyhow::Result;
    use serial_test::serial;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[derive(Default)]
    struct FakeApp;

    #[tokio::test]
    #[serial]
    async fn run_with_builds_app_and_executes_runner() -> Result<()> {
        let home = tempdir()?;
        let previous_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home.path());
        }

        let observed = Arc::new(Mutex::new(None::<(String, bool)>));
        let observed_clone = Arc::clone(&observed);
        run_with(
            Some("https://example.invalid".to_owned()),
            HistoryArgs { show_deleted: true },
            move |_store, transfer, show_deleted| {
                *observed_clone.lock().expect("lock observed state") = Some((
                    format!("{:?}", std::any::type_name_of_val(&transfer)),
                    show_deleted,
                ));
                FakeApp
            },
            |_| Box::pin(async { Ok(()) }),
        )
        .await?;

        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        let observed = observed.lock().expect("lock observed state").clone();
        let (transfer_type, show_deleted) = observed.expect("app was built");
        assert!(transfer_type.contains("TransferClient"));
        assert!(show_deleted);
        Ok(())
    }
}