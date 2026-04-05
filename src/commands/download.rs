use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tempfile::NamedTempFile;

use crate::cli::DownloadArgs;
use crate::client::crypto;
use crate::client::transfer::TransferClient;
use crate::model::EncryptionMode;
use crate::storage::config::AppConfig;
use crate::storage::db::HistoryStore;
use crate::storage::paths::AppPaths;

pub async fn run(server_override: Option<String>, args: DownloadArgs) -> Result<()> {
    let paths = AppPaths::discover()?;
    let config = AppConfig::load_or_create(&paths)?;
    let store = HistoryStore::new(&paths)?;
    let server = config.resolve_server_url(server_override.as_deref());
    let transfer = TransferClient::new(&server)?;
    let record = store.find_by_download_url(&args.url)?;

    let mode = resolve_mode(&args, record.as_ref());

    let output_path = match args.output {
        Some(path) => path,
        None => infer_output_path(&args.url, record.as_ref(), mode)?,
    };

    if output_path.exists() {
        bail!("output file already exists: {}", output_path.display());
    }

    if !mode.is_encrypted() {
        transfer
            .download_to_path(&args.url, &output_path)
            .await
            .context("download failed")?;
        println!("Saved: {}", output_path.display());
        return Ok(());
    }

    let temp = NamedTempFile::new().context("failed to allocate temporary download file")?;
    transfer
        .download_to_path(&args.url, temp.path())
        .await
        .context("download failed")?;

    match mode {
        EncryptionMode::Passphrase => {
            let passphrase = crypto::prompt_passphrase("Download passphrase")?;
            crypto::decrypt_passphrase_file(temp.path(), &output_path, passphrase)?;
        }
        EncryptionMode::Identity => {
            crypto::decrypt_identity_file(temp.path(), &output_path, &paths)?;
        }
        EncryptionMode::None => unreachable!(),
    }

    println!("Saved: {}", output_path.display());
    Ok(())
}

fn infer_output_path(
    url: &str,
    record: Option<&crate::model::UploadRecord>,
    mode: EncryptionMode,
) -> Result<PathBuf> {
    if let Some(record) = record {
        return Ok(PathBuf::from(&record.original_name));
    }

    let last = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .context("could not infer a filename from the URL")?;
    let file_name = if mode.is_encrypted() {
        last.strip_suffix(".age").unwrap_or(last)
    } else {
        last
    };

    Ok(PathBuf::from(file_name))
}

fn infer_encryption_mode(url: &str) -> EncryptionMode {
    if url.trim_end_matches('/').ends_with(".age") {
        EncryptionMode::Passphrase
    } else {
        EncryptionMode::None
    }
}

fn resolve_mode(args: &DownloadArgs, record: Option<&crate::model::UploadRecord>) -> EncryptionMode {
    if args.passphrase {
        EncryptionMode::Passphrase
    } else if args.identity {
        EncryptionMode::Identity
    } else {
        record
            .map(|entry| entry.encryption_mode)
            .unwrap_or_else(|| infer_encryption_mode(&args.url))
    }
}

#[cfg(test)]
mod tests {
    use super::{infer_encryption_mode, infer_output_path, resolve_mode};
    use crate::cli::DownloadArgs;
    use crate::model::{EncryptionMode, UploadRecord};
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn sample_record() -> UploadRecord {
        UploadRecord {
            id: "1".to_owned(),
            original_name: "original.txt".to_owned(),
            remote_name: "original.txt.age".to_owned(),
            source_path: Some("/tmp/original.txt".to_owned()),
            download_url: "https://example.invalid/original.txt.age".to_owned(),
            delete_url: "https://example.invalid/delete/original.txt.age".to_owned(),
            uploaded_at: Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).single().expect("valid time"),
            size_bytes: 12,
            encryption_mode: EncryptionMode::Identity,
            is_deleted: false,
            deleted_at: None,
        }
    }

    fn download_args(url: &str) -> DownloadArgs {
        DownloadArgs {
            url: url.to_owned(),
            output: None,
            passphrase: false,
            identity: false,
        }
    }

    #[test]
    fn infer_output_path_prefers_history_record_name() {
        let record = sample_record();
        let path = infer_output_path(&record.download_url, Some(&record), EncryptionMode::Identity)
            .expect("path from history");

        assert_eq!(path, PathBuf::from("original.txt"));
    }

    #[test]
    fn infer_output_path_strips_age_suffix_for_encrypted_urls() {
        let path = infer_output_path(
            "https://example.invalid/files/archive.tar.age",
            None,
            EncryptionMode::Passphrase,
        )
        .expect("path from encrypted URL");

        assert_eq!(path, PathBuf::from("archive.tar"));
    }

    #[test]
    fn infer_output_path_keeps_plain_filename_for_unencrypted_urls() {
        let path = infer_output_path("https://example.invalid/files/archive.tar", None, EncryptionMode::None)
            .expect("path from plain URL");

        assert_eq!(path, PathBuf::from("archive.tar"));
    }

    #[test]
    fn infer_output_path_rejects_urls_without_filename_segment() {
        let error = infer_output_path("/", None, EncryptionMode::None)
            .expect_err("missing filename should fail");
        assert!(error.to_string().contains("could not infer a filename"));
    }

    #[test]
    fn infer_encryption_mode_detects_age_extension() {
        assert_eq!(infer_encryption_mode("https://example.invalid/file.age"), EncryptionMode::Passphrase);
        assert_eq!(infer_encryption_mode("https://example.invalid/file.txt"), EncryptionMode::None);
    }

    #[test]
    fn resolve_mode_prefers_flags_then_history_then_url() {
        let record = sample_record();
        let args = download_args("https://example.invalid/file.age");
        assert_eq!(resolve_mode(&args, Some(&record)), EncryptionMode::Identity);
        assert_eq!(resolve_mode(&download_args("https://example.invalid/file.age"), None), EncryptionMode::Passphrase);
        assert_eq!(resolve_mode(&download_args("https://example.invalid/file.txt"), None), EncryptionMode::None);

        let mut passphrase = download_args("https://example.invalid/file.txt");
        passphrase.passphrase = true;
        assert_eq!(resolve_mode(&passphrase, Some(&record)), EncryptionMode::Passphrase);

        let mut identity = download_args("https://example.invalid/file.txt");
        identity.identity = true;
        assert_eq!(resolve_mode(&identity, None), EncryptionMode::Identity);
    }
}