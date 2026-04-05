use std::path::Path;
use std::{io::BufWriter, path::PathBuf};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::cli::UploadArgs;
use crate::client::crypto::{self, PreparedUpload};
use crate::client::transfer::TransferClient;
use crate::model::{EncryptionMode, UploadRecord};
use crate::storage::config::AppConfig;
use crate::storage::db::HistoryStore;
use crate::storage::paths::AppPaths;

pub async fn run(server_override: Option<String>, args: UploadArgs) -> Result<()> {
    let source = resolve_upload_source(&args.file)?;
    let source_path = source.source_path.display().to_string();
    let original_name = source.original_name.clone();
    let size_bytes = source.size_bytes;

    let paths = AppPaths::discover()?;
    let config = AppConfig::load_or_create(&paths)?;
    let store = HistoryStore::new(&paths)?;
    let server = config.resolve_server_url(server_override.as_deref());
    let transfer = TransferClient::new(&server)?;

    let encryption_mode = select_encryption_mode(args.passphrase, args.identity);

    let prepared = prepare_upload(
        &args,
        &paths,
        encryption_mode,
        crypto::prompt_passphrase,
        source,
    )?;
    let upload = transfer
        .upload_file(
            prepared.upload_path(),
            &prepared.remote_name,
            args.max_days,
            args.max_downloads,
        )
        .await
        .context("upload failed")?;

    let record = UploadRecord {
        id: Uuid::new_v4().to_string(),
        original_name,
        remote_name: upload.remote_name.clone(),
        source_path: Some(source_path),
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

struct UploadSource {
    source_path: PathBuf,
    upload_path: PathBuf,
    original_name: String,
    size_bytes: u64,
    temp_file: Option<NamedTempFile>,
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
    source: UploadSource,
) -> Result<PreparedUpload>
where
    F: FnOnce(&str) -> Result<String>,
{
    let remote_name = match &args.remote_name {
        Some(remote_name) => remote_name.clone(),
        None => source.original_name.clone(),
    };

    match mode {
        EncryptionMode::None => match source.temp_file {
            Some(temp_file) => Ok(PreparedUpload::plain_with_temp(
                source.upload_path,
                remote_name,
                temp_file,
            )),
            None => Ok(PreparedUpload::plain(source.upload_path, remote_name)),
        },
        EncryptionMode::Passphrase => {
            let passphrase = prompt_passphrase("Upload passphrase")?;
            crypto::prepare_passphrase_upload(&source.upload_path, &remote_name, passphrase)
        }
        EncryptionMode::Identity => {
            crypto::prepare_identity_upload(&source.upload_path, &remote_name, paths)
        }
    }
}

fn resolve_upload_source(path: &Path) -> Result<UploadSource> {
    if path.is_file() {
        return Ok(UploadSource {
            source_path: path.to_path_buf(),
            upload_path: path.to_path_buf(),
            original_name: file_name(path)?.to_owned(),
            size_bytes: std::fs::metadata(path)?.len(),
            temp_file: None,
        });
    }

    if path.is_dir() {
        let archive_name = archive_name(path)?;
        let archive = create_directory_archive(path)?;
        let size_bytes = archive.as_file().metadata()?.len();
        let upload_path = archive.path().to_path_buf();
        return Ok(UploadSource {
            source_path: path.to_path_buf(),
            upload_path,
            original_name: archive_name,
            size_bytes,
            temp_file: Some(archive),
        });
    }

    if !path.exists() {
        bail!("input path does not exist: {}", path.display());
    }

    bail!("input path must be a file or directory: {}", path.display())
}

fn create_directory_archive(path: &Path) -> Result<NamedTempFile> {
    let archive = NamedTempFile::new().context("failed to create temporary archive")?;
    let archive_file = archive
        .reopen()
        .context("failed to open temporary archive")?;
    let encoder = GzEncoder::new(BufWriter::new(archive_file), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder
        .append_dir_all(file_name(path)?, path)
        .with_context(|| format!("failed to archive directory {}", path.display()))?;
    let encoder = builder
        .into_inner()
        .context("failed to finalize tar archive")?;
    encoder
        .finish()
        .context("failed to finalize gzip archive")?;
    Ok(archive)
}

fn archive_name(path: &Path) -> Result<String> {
    Ok(format!("{}.tar.gz", file_name(path)?))
}

fn file_name(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|value| value.to_str())
        .context("file name is not valid unicode")
}

#[cfg(test)]
mod tests {
    use super::{
        archive_name, file_name, prepare_upload, resolve_upload_source, select_encryption_mode,
    };
    use crate::cli::UploadArgs;
    use crate::model::EncryptionMode;
    use crate::storage::paths::AppPaths;
    use anyhow::Result;
    use flate2::read::GzDecoder;
    use std::collections::BTreeMap;
    use std::io::Read;
    use tempfile::{TempDir, tempdir};

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

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

    fn archive_files(path: &std::path::Path) -> Result<BTreeMap<String, String>> {
        let file = std::fs::File::open(path)?;
        let decoder = GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        let mut files = BTreeMap::new();

        for entry in archive.entries()? {
            let mut entry = entry?;
            if !entry.header().entry_type().is_file() {
                continue;
            }

            let path = entry.path()?.to_string_lossy().into_owned();
            let mut contents = String::new();
            entry.read_to_string(&mut contents)?;
            files.insert(path, contents);
        }

        Ok(files)
    }

    #[test]
    fn select_encryption_mode_prefers_requested_mode() {
        assert_eq!(select_encryption_mode(false, false), EncryptionMode::None);
        assert_eq!(
            select_encryption_mode(true, false),
            EncryptionMode::Passphrase
        );
        assert_eq!(
            select_encryption_mode(false, true),
            EncryptionMode::Identity
        );
    }

    #[test]
    fn prepare_upload_returns_plain_upload_without_encryption() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let file = root.path().join("plain.txt");
        std::fs::write(&file, "plain")?;
        let source = resolve_upload_source(&file)?;
        let args = upload_args(file.clone());

        let prepared = prepare_upload(
            &args,
            &paths,
            EncryptionMode::None,
            |_| unreachable!(),
            source,
        )?;

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
        let source = resolve_upload_source(&file)?;
        let args = upload_args(file);

        let prepared = prepare_upload(
            &args,
            &paths,
            EncryptionMode::Passphrase,
            |label| {
                assert_eq!(label, "Upload passphrase");
                Ok("secret-passphrase".to_owned())
            },
            source,
        )?;

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
        let source = resolve_upload_source(&file)?;
        let args = UploadArgs {
            remote_name: Some("renamed.bin".to_owned()),
            ..upload_args(file)
        };

        let prepared = prepare_upload(
            &args,
            &paths,
            EncryptionMode::Identity,
            |_| unreachable!(),
            source,
        )?;

        crate::client::crypto::decrypt_identity_file(prepared.upload_path(), &output, &paths)?;
        assert_eq!(prepared.remote_name, "renamed.bin.age");
        assert_eq!(prepared.mode, EncryptionMode::Identity);
        assert_eq!(std::fs::read_to_string(output)?, "identity");
        Ok(())
    }

    #[test]
    fn prepare_upload_archives_directories_before_plain_upload() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let directory = root.path().join("bundle");
        let nested = directory.join("nested");
        std::fs::create_dir_all(&nested)?;
        std::fs::write(directory.join("top.txt"), "top-level")?;
        std::fs::write(nested.join("child.txt"), "nested")?;
        let source = resolve_upload_source(&directory)?;
        let args = upload_args(directory.clone());

        let prepared = prepare_upload(
            &args,
            &paths,
            EncryptionMode::None,
            |_| unreachable!(),
            source,
        )?;
        let files = archive_files(prepared.upload_path())?;

        assert_eq!(prepared.remote_name, "bundle.tar.gz");
        assert_eq!(prepared.mode, EncryptionMode::None);
        assert_eq!(files.get("bundle/top.txt"), Some(&"top-level".to_owned()));
        assert_eq!(
            files.get("bundle/nested/child.txt"),
            Some(&"nested".to_owned())
        );
        Ok(())
    }

    #[test]
    fn resolve_upload_source_uses_archive_name_for_directories() -> Result<()> {
        let root = tempdir()?;
        let directory = root.path().join("photos");
        std::fs::create_dir_all(&directory)?;

        let source = resolve_upload_source(&directory)?;

        assert_eq!(source.original_name, "photos.tar.gz");
        assert_eq!(source.source_path, directory);
        assert!(source.size_bytes > 0);
        Ok(())
    }

    #[test]
    fn archive_name_appends_tar_gz_suffix() -> Result<()> {
        let path = std::path::Path::new("/tmp/example");
        assert_eq!(archive_name(path)?, "example.tar.gz");
        Ok(())
    }

    #[test]
    fn file_name_returns_unicode_name() -> Result<()> {
        let path = std::path::Path::new("/tmp/example.txt");
        assert_eq!(file_name(path)?, "example.txt");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn file_name_rejects_non_unicode_names() {
        let os_string = std::ffi::OsString::from_vec(vec![0x66, 0x6f, 0x80]);
        let path = std::path::PathBuf::from(os_string);

        let error = file_name(&path).expect_err("non-unicode filename should fail");
        assert!(error.to_string().contains("valid unicode"));
    }
}
