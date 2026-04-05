use std::path::Path;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::cli::UploadArgs;
use crate::client::crypto::{self, PreparedUpload};
use crate::client::transfer::TransferClient;
use crate::model::{EncryptionMode, UploadRecord};
use crate::storage::config::AppConfig;
use crate::storage::db::HistoryStore;
use crate::storage::paths::AppPaths;

pub async fn run(server_override: Option<String>, args: UploadArgs) -> Result<()> {
    if !args.file.is_file() {
        bail!("input path is not a file: {}", args.file.display());
    }

    let paths = AppPaths::discover()?;
    let config = AppConfig::load_or_create(&paths)?;
    let store = HistoryStore::new(&paths)?;
    let server = config.resolve_server_url(server_override.as_deref());
    let transfer = TransferClient::new(&server)?;

    let encryption_mode = select_encryption_mode(args.passphrase, args.identity);

    let prepared = prepare_upload(&args, &paths, encryption_mode, crypto::prompt_passphrase)?;
    let upload = transfer
        .upload_file(
            prepared.upload_path(),
            &prepared.remote_name,
            args.max_days,
            args.max_downloads,
        )
        .await
        .context("upload failed")?;

    let source_name = args
        .file
        .file_name()
        .and_then(|value| value.to_str())
        .context("input file name is not valid unicode")?
        .to_owned();
    let size_bytes = std::fs::metadata(&args.file)?.len();
    let record = UploadRecord {
        id: Uuid::new_v4().to_string(),
        original_name: source_name,
        remote_name: upload.remote_name.clone(),
        source_path: Some(args.file.display().to_string()),
        download_url: upload.download_url.clone(),
        delete_url: upload.delete_url.clone(),
        uploaded_at: chrono::Utc::now(),
        size_bytes,
        encryption_mode,
        is_deleted: false,
        deleted_at: None,
    };
    store.insert_record(&record)?;

    println!("Uploaded: {}", record.original_name);
    println!("URL: {}", record.download_url);
    println!("Delete URL: {}", record.delete_url);
    if prepared.mode.is_encrypted() {
        println!("Encryption: {}", prepared.mode);
    }

    Ok(())
}

fn select_encryption_mode(passphrase: bool, identity: bool) -> EncryptionMode {
    if passphrase {
        EncryptionMode::Passphrase
    } else if identity {
        EncryptionMode::Identity
    } else {
        EncryptionMode::None
    }
}

fn prepare_upload<F>(
    args: &UploadArgs,
    paths: &AppPaths,
    mode: EncryptionMode,
    prompt_passphrase: F,
) -> Result<PreparedUpload>
where
    F: FnOnce(&str) -> Result<String>,
{
    let remote_name = match &args.remote_name {
        Some(remote_name) => remote_name.clone(),
        None => file_name(&args.file)?.to_owned(),
    };

    match mode {
        EncryptionMode::None => Ok(PreparedUpload::plain(args.file.clone(), remote_name)),
        EncryptionMode::Passphrase => {
            let passphrase = prompt_passphrase("Upload passphrase")?;
            crypto::prepare_passphrase_upload(&args.file, &remote_name, passphrase)
        }
        EncryptionMode::Identity => crypto::prepare_identity_upload(&args.file, &remote_name, paths),
    }
}

fn file_name(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|value| value.to_str())
        .context("file name is not valid unicode")
}

#[cfg(test)]
mod tests {
    use super::{file_name, prepare_upload, select_encryption_mode};
    use crate::cli::UploadArgs;
    use crate::model::EncryptionMode;
    use crate::storage::paths::AppPaths;
    use anyhow::Result;
    use std::os::unix::ffi::OsStringExt;
    use tempfile::{TempDir, tempdir};

    fn test_paths() -> Result<(TempDir, AppPaths)> {
        let root = tempdir()?;
        let config_dir = root.path().join("config");
        let data_dir = root.path().join("data");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        Ok((root, AppPaths::from_dirs(config_dir, data_dir)))
    }

    fn upload_args(path: std::path::PathBuf) -> UploadArgs {
        UploadArgs {
            file: path,
            remote_name: None,
            max_days: None,
            max_downloads: None,
            passphrase: false,
            identity: false,
        }
    }

    #[test]
    fn select_encryption_mode_prefers_requested_mode() {
        assert_eq!(select_encryption_mode(false, false), EncryptionMode::None);
        assert_eq!(select_encryption_mode(true, false), EncryptionMode::Passphrase);
        assert_eq!(select_encryption_mode(false, true), EncryptionMode::Identity);
    }

    #[test]
    fn prepare_upload_returns_plain_upload_without_encryption() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let file = root.path().join("plain.txt");
        std::fs::write(&file, "plain")?;
        let args = upload_args(file.clone());

        let prepared = prepare_upload(&args, &paths, EncryptionMode::None, |_| unreachable!())?;

        assert_eq!(prepared.remote_name, "plain.txt");
        assert_eq!(prepared.mode, EncryptionMode::None);
        assert_eq!(prepared.upload_path(), file.as_path());
        Ok(())
    }

    #[test]
    fn prepare_upload_uses_prompt_for_passphrase_encryption() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let file = root.path().join("secret.txt");
        let output = root.path().join("decrypted.txt");
        std::fs::write(&file, "sensitive")?;
        let args = upload_args(file);

        let prepared = prepare_upload(&args, &paths, EncryptionMode::Passphrase, |label| {
            assert_eq!(label, "Upload passphrase");
            Ok("secret-passphrase".to_owned())
        })?;

        crate::client::crypto::decrypt_passphrase_file(
            prepared.upload_path(),
            &output,
            "secret-passphrase".to_owned(),
        )?;
        assert_eq!(prepared.remote_name, "secret.txt.age");
        assert_eq!(prepared.mode, EncryptionMode::Passphrase);
        assert_eq!(std::fs::read_to_string(output)?, "sensitive");
        Ok(())
    }

    #[test]
    fn prepare_upload_uses_identity_encryption() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let file = root.path().join("secret.txt");
        let output = root.path().join("decrypted.txt");
        std::fs::write(&file, "identity")?;
        let args = UploadArgs {
            remote_name: Some("renamed.bin".to_owned()),
            ..upload_args(file)
        };

        let prepared = prepare_upload(&args, &paths, EncryptionMode::Identity, |_| unreachable!())?;

        crate::client::crypto::decrypt_identity_file(prepared.upload_path(), &output, &paths)?;
        assert_eq!(prepared.remote_name, "renamed.bin.age");
        assert_eq!(prepared.mode, EncryptionMode::Identity);
        assert_eq!(std::fs::read_to_string(output)?, "identity");
        Ok(())
    }

    #[test]
    fn file_name_returns_unicode_name() -> Result<()> {
        let path = std::path::Path::new("/tmp/example.txt");
        assert_eq!(file_name(path)?, "example.txt");
        Ok(())
    }

    #[test]
    fn file_name_rejects_non_unicode_names() {
        let os_string = std::ffi::OsString::from_vec(vec![0x66, 0x6f, 0x80]);
        let path = std::path::PathBuf::from(os_string);

        let error = file_name(&path).expect_err("non-unicode filename should fail");
        assert!(error.to_string().contains("valid unicode"));
    }
}