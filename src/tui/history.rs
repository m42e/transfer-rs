use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use arboard::Clipboard;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::client::transfer::TransferClient;
use crate::storage::db::HistoryStore;

pub struct HistoryApp {
    show_deleted: bool,
    records: Vec<crate::model::UploadRecord>,
    selected: usize,
    table_state: TableState,
    status: String,
    store: HistoryStore,
    transfer: TransferClient,
}

impl HistoryApp {
    pub fn new(store: HistoryStore, transfer: TransferClient, show_deleted: bool) -> Self {
        Self {
            show_deleted,
            records: Vec::new(),
            selected: 0,
            table_state: TableState::default(),
            status: String::from(
                "Press q to quit, d to delete, x to remove local record, c to copy URL.",
            ),
            store,
            transfer,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        self.reload()?;
        let mut terminal = TerminalSession::enter()?;

        loop {
            self.sync_selection();
            terminal.terminal.draw(|frame| self.render(frame))?;

            if !event::poll(Duration::from_millis(200))? {
                continue;
            }

            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Down | KeyCode::Char('j') => self.move_down(),
                    KeyCode::Up | KeyCode::Char('k') => self.move_up(),
                    KeyCode::Char('c') => self.copy_selected_url(),
                    KeyCode::Char('x') => self.remove_local_record()?,
                    KeyCode::Char('d') => self.delete_selected_remote().await?,
                    _ => {}
                }
            }
        }

        Ok(())
    }

    fn render(&mut self, frame: &mut ratatui::Frame<'_>) {
        let layout =
            Layout::vertical([Constraint::Min(5), Constraint::Length(3)]).split(frame.area());
        let rows = self.records.iter().map(|record| {
            Row::new(vec![
                Cell::from(record.original_name.clone()),
                Cell::from(record.uploaded_at.format("%Y-%m-%d %H:%M").to_string()),
                Cell::from(format_bytes(record.size_bytes)),
                Cell::from(record.encryption_mode.to_string()),
                Cell::from(if record.is_deleted {
                    "deleted"
                } else {
                    "active"
                }),
            ])
        });

        let widths = [
            Constraint::Percentage(38),
            Constraint::Length(18),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
        ];
        let title = format!("transfer-rs v{} history", crate::APP_VERSION);
        let table = Table::new(rows, widths)
            .header(
                Row::new(vec!["Name", "Uploaded", "Size", "Encryption", "State"])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            )
            .block(Block::default().title(title).borders(Borders::ALL))
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(table, layout[0], &mut self.table_state);
        let footer = Paragraph::new(vec![Line::from(self.status.clone())])
            .block(Block::default().borders(Borders::ALL).title("Status"));
        frame.render_widget(footer, layout[1]);
    }

    fn move_down(&mut self) {
        if self.records.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.records.len() - 1);
    }

    fn move_up(&mut self) {
        if self.records.is_empty() {
            return;
        }
        self.selected = self.selected.saturating_sub(1);
    }

    fn copy_selected_url(&mut self) {
        self.copy_selected_url_with(|url| {
            Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(url.to_owned()))
                .map_err(|error| error.to_string())
        });
    }

    fn copy_selected_url_with<F>(&mut self, copy_url: F)
    where
        F: FnOnce(&str) -> std::result::Result<(), String>,
    {
        let Some(record) = self.records.get(self.selected) else {
            self.status = String::from("No record selected.");
            return;
        };

        match copy_url(&record.download_url) {
            Ok(()) => self.status = format!("Copied URL for {}", record.original_name),
            Err(error) => self.status = format!("Clipboard failed: {error}"),
        }
    }

    fn remove_local_record(&mut self) -> Result<()> {
        let Some(record) = self.records.get(self.selected) else {
            self.status = String::from("No record selected.");
            return Ok(());
        };

        self.store.delete_local(&record.id)?;
        self.status = format!("Removed local history entry for {}", record.original_name);
        self.reload()?;
        Ok(())
    }

    async fn delete_selected_remote(&mut self) -> Result<()> {
        let Some(record) = self.records.get(self.selected).cloned() else {
            self.status = String::from("No record selected.");
            return Ok(());
        };

        match self.transfer.delete(&record.delete_url).await {
            Ok(()) => {
                self.store.mark_deleted(&record.id)?;
                self.status = format!("Deleted remote file {}", record.original_name);
            }
            Err(error) => {
                self.status = format!("Delete failed for {}: {error}", record.original_name);
            }
        }
        self.reload()?;
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        self.records = self.store.list_records(self.show_deleted)?;
        if self.records.is_empty() {
            self.selected = 0;
            self.status = String::from("No history entries yet.");
        } else if self.selected >= self.records.len() {
            self.selected = self.records.len() - 1;
        }
        Ok(())
    }

    fn sync_selection(&mut self) {
        self.table_state
            .select((!self.records.is_empty()).then_some(self.selected));
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("failed to initialize terminal UI")?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

fn format_bytes(size_bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = size_bytes as f64;
    let mut unit_index = 0;
    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }
    if unit_index == 0 {
        format!("{size_bytes} {}", UNITS[unit_index])
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::{HistoryApp, format_bytes};
    use crate::client::transfer::TransferClient;
    use crate::model::{EncryptionMode, UploadRecord};
    use crate::storage::db::HistoryStore;
    use crate::storage::paths::AppPaths;
    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use mockito::Server;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tempfile::{TempDir, tempdir};

    fn sample_record(id: &str, deleted: bool) -> UploadRecord {
        UploadRecord {
            id: id.to_owned(),
            original_name: format!("{id}.txt"),
            remote_name: format!("{id}.txt"),
            source_path: Some(format!("/tmp/{id}.txt")),
            download_url: format!("https://example.invalid/{id}.txt"),
            delete_url: format!("https://example.invalid/delete/{id}.txt"),
            uploaded_at: Utc
                .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
                .single()
                .expect("valid time"),
            size_bytes: 1536,
            encryption_mode: EncryptionMode::Passphrase,
            is_deleted: deleted,
            deleted_at: deleted.then(|| {
                Utc.with_ymd_and_hms(2024, 1, 3, 3, 4, 5)
                    .single()
                    .expect("valid time")
            }),
        }
    }

    fn test_app() -> Result<(TempDir, HistoryStore, TransferClient, HistoryApp)> {
        let root = tempdir()?;
        let config_dir = root.path().join("config");
        let data_dir = root.path().join("data");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        let paths = AppPaths::from_dirs(config_dir, data_dir);
        let store = HistoryStore::new(&paths)?;
        let transfer = TransferClient::new("https://example.invalid")?;
        let app = HistoryApp::new(
            HistoryStore::new(&paths)?,
            TransferClient::new("https://example.invalid")?,
            false,
        );
        Ok((root, store, transfer, app))
    }

    #[test]
    fn format_bytes_uses_binary_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn new_initializes_default_status() -> Result<()> {
        let (_root, _store, _transfer, app) = test_app()?;
        assert!(app.status.contains("Press q to quit"));
        assert_eq!(app.selected, 0);
        Ok(())
    }

    #[test]
    fn move_selection_handles_empty_and_non_empty_lists() -> Result<()> {
        let (_root, _store, _transfer, mut app) = test_app()?;
        app.move_down();
        app.move_up();
        assert_eq!(app.selected, 0);

        app.records = vec![sample_record("one", false), sample_record("two", false)];
        app.move_down();
        assert_eq!(app.selected, 1);
        app.move_down();
        assert_eq!(app.selected, 1);
        app.move_up();
        assert_eq!(app.selected, 0);
        Ok(())
    }

    #[test]
    fn copy_selected_url_updates_status_for_empty_success_and_error_cases() -> Result<()> {
        let (_root, _store, _transfer, mut app) = test_app()?;
        app.copy_selected_url_with(|_| Ok(()));
        assert_eq!(app.status, "No record selected.");

        app.records = vec![sample_record("one", false)];
        app.copy_selected_url_with(|_| Ok(()));
        assert_eq!(app.status, "Copied URL for one.txt");

        app.copy_selected_url_with(|_| Err("clipboard offline".to_owned()));
        assert_eq!(app.status, "Clipboard failed: clipboard offline");
        Ok(())
    }

    #[test]
    fn copy_selected_url_executes_real_clipboard_path() -> Result<()> {
        let (_root, _store, _transfer, mut app) = test_app()?;
        app.records = vec![sample_record("one", false)];

        app.copy_selected_url();

        assert!(
            app.status == "Copied URL for one.txt" || app.status.starts_with("Clipboard failed:"),
            "unexpected clipboard status: {}",
            app.status
        );
        Ok(())
    }

    #[test]
    fn reload_updates_status_and_selected_index() -> Result<()> {
        let (_root, store, _transfer, mut app) = test_app()?;
        app.reload()?;
        assert_eq!(app.status, "No history entries yet.");
        assert_eq!(app.selected, 0);

        store.insert_record(&sample_record("one", false))?;
        store.insert_record(&sample_record("two", true))?;
        app = HistoryApp::new(
            store,
            TransferClient::new("https://example.invalid")?,
            false,
        );
        app.reload()?;
        assert_eq!(app.records.len(), 1);
        assert_eq!(app.records[0].id, "one");

        app.selected = 99;
        app.show_deleted = true;
        app.reload()?;
        assert_eq!(app.selected, 1);
        Ok(())
    }

    #[test]
    fn sync_selection_reflects_whether_records_exist() -> Result<()> {
        let (_root, _store, _transfer, mut app) = test_app()?;
        app.sync_selection();
        assert!(app.table_state.selected().is_none());

        app.records = vec![sample_record("one", false)];
        app.sync_selection();
        assert_eq!(app.table_state.selected(), Some(0));
        Ok(())
    }

    #[test]
    fn remove_local_record_handles_empty_and_existing_entries() -> Result<()> {
        let (_root, store, _transfer, mut app) = test_app()?;
        app.remove_local_record()?;
        assert_eq!(app.status, "No record selected.");

        let record = sample_record("one", false);
        store.insert_record(&record)?;
        app = HistoryApp::new(
            store,
            TransferClient::new("https://example.invalid")?,
            false,
        );
        app.reload()?;
        app.remove_local_record()?;
        assert_eq!(app.status, "No history entries yet.");
        assert!(app.records.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn delete_selected_remote_handles_empty_success_and_failure() -> Result<()> {
        let root = tempdir()?;
        let config_dir = root.path().join("config");
        let data_dir = root.path().join("data");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        let paths = AppPaths::from_dirs(config_dir, data_dir);
        let store = HistoryStore::new(&paths)?;
        let mut app = HistoryApp::new(
            store,
            TransferClient::new("https://example.invalid")?,
            false,
        );
        app.delete_selected_remote().await?;
        assert_eq!(app.status, "No record selected.");

        let mut success_server = Server::new_async().await;
        let success_record = UploadRecord {
            delete_url: format!("{}/delete/one", success_server.url()),
            ..sample_record("one", false)
        };
        let store = HistoryStore::new(&paths)?;
        store.insert_record(&success_record)?;
        let _success_mock = success_server
            .mock("DELETE", "/delete/one")
            .with_status(200)
            .create_async()
            .await;
        let mut app = HistoryApp::new(store, TransferClient::new(&success_server.url())?, true);
        app.reload()?;
        app.delete_selected_remote().await?;
        assert!(app.status.contains("Deleted remote file one.txt"));
        assert!(app.records[0].is_deleted);

        let mut failure_server = Server::new_async().await;
        let failure_paths =
            AppPaths::from_dirs(root.path().join("config2"), root.path().join("data2"));
        std::fs::create_dir_all(&failure_paths.config_dir)?;
        std::fs::create_dir_all(&failure_paths.data_dir)?;
        let failure_store = HistoryStore::new(&failure_paths)?;
        let failure_record = UploadRecord {
            delete_url: format!("{}/delete/two", failure_server.url()),
            ..sample_record("two", false)
        };
        failure_store.insert_record(&failure_record)?;
        let _failure_mock = failure_server
            .mock("DELETE", "/delete/two")
            .with_status(500)
            .create_async()
            .await;
        let mut failure_app = HistoryApp::new(
            failure_store,
            TransferClient::new(&failure_server.url())?,
            false,
        );
        failure_app.reload()?;
        failure_app.delete_selected_remote().await?;
        assert!(failure_app.status.contains("Delete failed for two.txt"));
        Ok(())
    }

    #[test]
    fn render_draws_history_table_and_status() -> Result<()> {
        let (_root, _store, _transfer, mut app) = test_app()?;
        app.records = vec![sample_record("one", false)];
        app.sync_selection();
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend)?;

        terminal.draw(|frame| app.render(frame))?;
        let buffer = terminal.backend().buffer().clone();
        let rendered = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains(&format!("transfer-rs v{} history", crate::APP_VERSION)));
        assert!(rendered.contains("Status"));
        assert!(rendered.contains("one.txt"));
        Ok(())
    }
}
