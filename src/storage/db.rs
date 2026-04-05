use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{EncryptionMode, UploadRecord};
use crate::storage::paths::AppPaths;

pub struct HistoryStore {
    db_path: std::path::PathBuf,
}

impl HistoryStore {
    pub fn new(paths: &AppPaths) -> Result<Self> {
        let store = Self {
            db_path: paths.db_path.clone(),
        };
        store.init()?;
        Ok(store)
    }

    pub fn insert_record(&self, record: &UploadRecord) -> Result<()> {
        let connection = self.open()?;
        connection.execute(
            "INSERT INTO uploads (
                id,
                original_name,
                remote_name,
                source_path,
                download_url,
                delete_url,
                uploaded_at,
                size_bytes,
                encryption_mode,
                is_deleted,
                deleted_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                record.id,
                record.original_name,
                record.remote_name,
                record.source_path,
                record.download_url,
                record.delete_url,
                record.uploaded_at.to_rfc3339(),
                record.size_bytes as i64,
                record.encryption_mode.as_str(),
                record.is_deleted as i64,
                record.deleted_at.map(|value| value.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn list_records(&self, show_deleted: bool) -> Result<Vec<UploadRecord>> {
        let connection = self.open()?;
        let mut statement = if show_deleted {
            connection.prepare(
                "SELECT id, original_name, remote_name, source_path, download_url, delete_url, uploaded_at, size_bytes, encryption_mode, is_deleted, deleted_at
                 FROM uploads
                 ORDER BY uploaded_at DESC",
            )?
        } else {
            connection.prepare(
                "SELECT id, original_name, remote_name, source_path, download_url, delete_url, uploaded_at, size_bytes, encryption_mode, is_deleted, deleted_at
                 FROM uploads
                 WHERE is_deleted = 0
                 ORDER BY uploaded_at DESC",
            )?
        };

        let rows = statement.query_map([], row_to_record)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    pub fn find_by_download_url(&self, url: &str) -> Result<Option<UploadRecord>> {
        let connection = self.open()?;
        connection
            .query_row(
                "SELECT id, original_name, remote_name, source_path, download_url, delete_url, uploaded_at, size_bytes, encryption_mode, is_deleted, deleted_at
                 FROM uploads
                 WHERE download_url = ?1
                 LIMIT 1",
                [url],
                row_to_record,
            )
            .optional()
            .context("failed to lookup history by download URL")
    }

    pub fn find_by_id_or_url(&self, id_or_url: &str) -> Result<Option<UploadRecord>> {
        let connection = self.open()?;
        connection
            .query_row(
                "SELECT id, original_name, remote_name, source_path, download_url, delete_url, uploaded_at, size_bytes, encryption_mode, is_deleted, deleted_at
                 FROM uploads
                 WHERE id = ?1 OR download_url = ?1
                 LIMIT 1",
                [id_or_url],
                row_to_record,
            )
            .optional()
            .context("failed to lookup history record")
    }

    pub fn mark_deleted(&self, id: &str) -> Result<()> {
        let connection = self.open()?;
        connection.execute(
            "UPDATE uploads
             SET is_deleted = 1, deleted_at = ?2
             WHERE id = ?1",
            params![id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn delete_local(&self, id: &str) -> Result<()> {
        let connection = self.open()?;
        connection.execute("DELETE FROM uploads WHERE id = ?1", [id])?;
        Ok(())
    }

    fn init(&self) -> Result<()> {
        let connection = self.open()?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS uploads (
                id TEXT PRIMARY KEY,
                original_name TEXT NOT NULL,
                remote_name TEXT NOT NULL,
                source_path TEXT,
                download_url TEXT NOT NULL UNIQUE,
                delete_url TEXT NOT NULL,
                uploaded_at TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                encryption_mode TEXT NOT NULL,
                is_deleted INTEGER NOT NULL DEFAULT 0,
                deleted_at TEXT
            );",
        )?;
        Ok(())
    }

    fn open(&self) -> Result<Connection> {
        Connection::open(&self.db_path)
            .with_context(|| format!("failed to open {}", self.db_path.display()))
    }
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<UploadRecord> {
    let uploaded_at: String = row.get(6)?;
    let deleted_at: Option<String> = row.get(10)?;
    Ok(UploadRecord {
        id: row.get(0)?,
        original_name: row.get(1)?,
        remote_name: row.get(2)?,
        source_path: row.get(3)?,
        download_url: row.get(4)?,
        delete_url: row.get(5)?,
        uploaded_at: parse_timestamp(&uploaded_at)?,
        size_bytes: row.get::<_, i64>(7)? as u64,
        encryption_mode: EncryptionMode::from_db(&row.get::<_, String>(8)?),
        is_deleted: row.get::<_, i64>(9)? != 0,
        deleted_at: deleted_at.as_deref().map(parse_timestamp).transpose()?,
    })
}

fn parse_timestamp(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{HistoryStore, parse_timestamp};
    use crate::model::{EncryptionMode, UploadRecord};
    use crate::storage::paths::AppPaths;
    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use rusqlite::Connection;
    use tempfile::{TempDir, tempdir};

    fn test_store() -> Result<(TempDir, AppPaths, HistoryStore)> {
        let root = tempdir()?;
        let config_dir = root.path().join("config");
        let data_dir = root.path().join("data");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        let paths = AppPaths::from_dirs(config_dir, data_dir);
        let store = HistoryStore::new(&paths)?;
        Ok((root, paths, store))
    }

    fn sample_record(id: &str, url: &str) -> UploadRecord {
        UploadRecord {
            id: id.to_owned(),
            original_name: format!("{id}.txt"),
            remote_name: format!("{id}.txt.age"),
            source_path: Some(format!("/tmp/{id}.txt")),
            download_url: url.to_owned(),
            delete_url: format!("{url}/delete"),
            uploaded_at: Utc
                .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
                .single()
                .expect("valid time"),
            size_bytes: 512,
            encryption_mode: EncryptionMode::Identity,
            is_deleted: false,
            deleted_at: None,
        }
    }

    #[test]
    fn insert_list_find_mark_and_delete_records() -> Result<()> {
        let (_root, _paths, store) = test_store()?;
        let first = sample_record("one", "https://example.invalid/one");
        let mut second = sample_record("two", "https://example.invalid/two");
        second.uploaded_at = Utc
            .with_ymd_and_hms(2024, 1, 3, 3, 4, 5)
            .single()
            .expect("valid time");
        second.encryption_mode = EncryptionMode::Passphrase;

        store.insert_record(&first)?;
        store.insert_record(&second)?;

        let active = store.list_records(false)?;
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].id, "two");
        assert_eq!(active[1].id, "one");

        let by_url = store
            .find_by_download_url("https://example.invalid/one")?
            .expect("record by url");
        assert_eq!(by_url.id, "one");
        assert_eq!(by_url.encryption_mode, EncryptionMode::Identity);

        let by_id = store.find_by_id_or_url("two")?.expect("record by id");
        assert_eq!(by_id.download_url, "https://example.invalid/two");

        store.mark_deleted("one")?;

        let active_after_delete = store.list_records(false)?;
        assert_eq!(active_after_delete.len(), 1);
        assert_eq!(active_after_delete[0].id, "two");

        let all = store.list_records(true)?;
        let deleted = all
            .iter()
            .find(|record| record.id == "one")
            .expect("deleted record present");
        assert!(deleted.is_deleted);
        assert!(deleted.deleted_at.is_some());

        store.delete_local("two")?;
        assert!(store.find_by_id_or_url("two")?.is_none());
        Ok(())
    }

    #[test]
    fn find_methods_return_none_for_unknown_entries() -> Result<()> {
        let (_root, _paths, store) = test_store()?;

        assert!(
            store
                .find_by_download_url("https://missing.invalid")?
                .is_none()
        );
        assert!(store.find_by_id_or_url("missing")?.is_none());
        Ok(())
    }

    #[test]
    fn row_to_record_rejects_invalid_timestamps() -> Result<()> {
        let (_root, paths, _store) = test_store()?;
        let connection = Connection::open(&paths.db_path)?;
        connection.execute(
            "INSERT INTO uploads (
                id, original_name, remote_name, source_path, download_url, delete_url,
                uploaded_at, size_bytes, encryption_mode, is_deleted, deleted_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                "broken",
                "broken.txt",
                "broken.txt",
                Option::<String>::None,
                "https://example.invalid/broken",
                "https://example.invalid/broken/delete",
                "definitely-not-a-timestamp",
                1_i64,
                "none",
                0_i64,
                Option::<String>::None,
            ],
        )?;

        let store = HistoryStore::new(&paths)?;
        let error = store
            .list_records(true)
            .expect_err("invalid timestamps should fail");

        assert!(!error.to_string().is_empty());
        Ok(())
    }

    #[test]
    fn parse_timestamp_accepts_rfc3339_values() {
        let parsed = parse_timestamp("2024-01-02T03:04:05Z").expect("valid timestamp");
        assert_eq!(
            parsed,
            Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
                .single()
                .expect("valid time")
        );
    }
}
