use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::iter;

use age::secrecy::{ExposeSecret, SecretString};
use anyhow::{Context, Result};
use tempfile::NamedTempFile;

use crate::model::EncryptionMode;
use crate::storage::paths::AppPaths;

pub struct PreparedUpload {
    pub remote_name: String,
    pub mode: EncryptionMode,
    upload_path: PathBuf,
    _temp_file: Option<NamedTempFile>,
}

impl PreparedUpload {
    pub fn plain(path: PathBuf, remote_name: String) -> Self {
        Self {
            remote_name,
            mode: EncryptionMode::None,
            upload_path: path,
            _temp_file: None,
        }
    }

    pub fn upload_path(&self) -> &Path {
        &self.upload_path
    }
}

pub fn prompt_passphrase(label: &str) -> Result<String> {
    rpassword::prompt_password(format!("{label}: ")).context("failed to read passphrase")
}

pub fn prepare_passphrase_upload(
    source: &Path,
    remote_name: &str,
    passphrase: String,
) -> Result<PreparedUpload> {
    let temp = NamedTempFile::new().context("failed to create temporary encrypted file")?;
    encrypt_with_passphrase(source, temp.path(), passphrase)?;
    Ok(PreparedUpload {
        remote_name: format!("{remote_name}.age"),
        mode: EncryptionMode::Passphrase,
        upload_path: temp.path().to_path_buf(),
        _temp_file: Some(temp),
    })
}

pub fn prepare_identity_upload(
    source: &Path,
    remote_name: &str,
    paths: &AppPaths,
) -> Result<PreparedUpload> {
    let temp = NamedTempFile::new().context("failed to create temporary encrypted file")?;
    encrypt_with_identity(source, temp.path(), paths)?;
    Ok(PreparedUpload {
        remote_name: format!("{remote_name}.age"),
        mode: EncryptionMode::Identity,
        upload_path: temp.path().to_path_buf(),
        _temp_file: Some(temp),
    })
}

pub fn encrypt_with_passphrase(source: &Path, output: &Path, passphrase: String) -> Result<()> {
    let input = File::open(source).with_context(|| format!("failed to open {}", source.display()))?;
    let output = File::create(output).with_context(|| format!("failed to create {}", output.display()))?;
    let encryptor = age::Encryptor::with_user_passphrase(SecretString::from(passphrase));
    let mut writer = encryptor
        .wrap_output(BufWriter::new(output))
        .context("failed to start passphrase encryption")?;
    std::io::copy(&mut BufReader::new(input), &mut writer).context("failed to encrypt file")?;
    writer.finish().context("failed to finalize encrypted file")?;
    Ok(())
}

pub fn decrypt_passphrase_file(input: &Path, output: &Path, passphrase: String) -> Result<()> {
    let input_file = File::open(input).with_context(|| format!("failed to open {}", input.display()))?;
    let output_file = File::create(output).with_context(|| format!("failed to create {}", output.display()))?;
    let decryptor = age::Decryptor::new(BufReader::new(input_file)).context("failed to read encrypted payload")?;

    if !decryptor.is_scrypt() {
        anyhow::bail!("payload requires an identity, not a passphrase");
    }

    let identity = age::scrypt::Identity::new(SecretString::from(passphrase));
    let mut reader = decryptor
        .decrypt(iter::once(&identity as &dyn age::Identity))
        .context("invalid passphrase or encrypted payload")?;
    let mut writer = BufWriter::new(output_file);
    std::io::copy(&mut reader, &mut writer).context("failed to decrypt file")?;

    Ok(())
}

pub fn encrypt_with_identity(source: &Path, output: &Path, paths: &AppPaths) -> Result<()> {
    let identity = ensure_identity(paths)?;
    let recipient = identity.to_public();
    let encryptor = age::Encryptor::with_recipients(iter::once(&recipient as &dyn age::Recipient))
        .context("failed to create identity encryptor")?;
    let input = File::open(source).with_context(|| format!("failed to open {}", source.display()))?;
    let output = File::create(output).with_context(|| format!("failed to create {}", output.display()))?;
    let mut writer = encryptor
        .wrap_output(BufWriter::new(output))
        .context("failed to start identity encryption")?;
    std::io::copy(&mut BufReader::new(input), &mut writer).context("failed to encrypt file")?;
    writer.finish().context("failed to finalize encrypted file")?;
    Ok(())
}

pub fn decrypt_identity_file(input: &Path, output: &Path, paths: &AppPaths) -> Result<()> {
    let identity = ensure_identity(paths)?;
    let input_file = File::open(input).with_context(|| format!("failed to open {}", input.display()))?;
    let output_file = File::create(output).with_context(|| format!("failed to create {}", output.display()))?;
    let decryptor = age::Decryptor::new(BufReader::new(input_file)).context("failed to read encrypted payload")?;

    if decryptor.is_scrypt() {
        anyhow::bail!("payload requires a passphrase, not the local identity");
    }

    let mut reader = decryptor
        .decrypt(iter::once(&identity as &dyn age::Identity))
        .context("failed to decrypt payload with the local identity")?;
    let mut writer = BufWriter::new(output_file);
    std::io::copy(&mut reader, &mut writer).context("failed to decrypt file")?;

    Ok(())
}

fn ensure_identity(paths: &AppPaths) -> Result<age::x25519::Identity> {
    if !paths.identity_path.exists() {
        let identity = age::x25519::Identity::generate();
        fs::write(&paths.identity_path, identity.to_string().expose_secret())
            .with_context(|| format!("failed to write {}", paths.identity_path.display()))?;
        return Ok(identity);
    }

    let contents = fs::read_to_string(&paths.identity_path)
        .with_context(|| format!("failed to read {}", paths.identity_path.display()))?;
    age::x25519::Identity::from_str(contents.trim()).map_err(|error| anyhow::anyhow!(error))
}

#[cfg(test)]
mod tests {
    use super::{
        PreparedUpload, decrypt_identity_file, decrypt_passphrase_file, encrypt_with_identity,
        encrypt_with_passphrase, ensure_identity, prepare_identity_upload, prepare_passphrase_upload,
    };
    use age::secrecy::ExposeSecret;
    use crate::model::EncryptionMode;
    use crate::storage::paths::AppPaths;
    use anyhow::Result;
    use tempfile::{TempDir, tempdir};

    fn test_paths() -> Result<(TempDir, AppPaths)> {
        let root = tempdir()?;
        let config_dir = root.path().join("config");
        let data_dir = root.path().join("data");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        Ok((root, AppPaths::from_dirs(config_dir, data_dir)))
    }

    fn write_source(root: &TempDir, name: &str, contents: &str) -> Result<std::path::PathBuf> {
        let path = root.path().join(name);
        std::fs::write(&path, contents)?;
        Ok(path)
    }

    #[test]
    fn plain_prepared_upload_keeps_original_path() -> Result<()> {
        let root = tempdir()?;
        let source = write_source(&root, "plain.txt", "plain data")?;

        let prepared = PreparedUpload::plain(source.clone(), "remote.txt".to_owned());

        assert_eq!(prepared.remote_name, "remote.txt");
        assert_eq!(prepared.mode, EncryptionMode::None);
        assert_eq!(prepared.upload_path(), source.as_path());
        Ok(())
    }

    #[test]
    fn passphrase_encryption_round_trips_file_contents() -> Result<()> {
        let root = tempdir()?;
        let source = write_source(&root, "plain.txt", "super secret")?;
        let encrypted = root.path().join("plain.txt.age");
        let decrypted = root.path().join("round-trip.txt");

        encrypt_with_passphrase(&source, &encrypted, "passphrase".to_owned())?;
        decrypt_passphrase_file(&encrypted, &decrypted, "passphrase".to_owned())?;

        assert_eq!(std::fs::read_to_string(decrypted)?, "super secret");
        Ok(())
    }

    #[test]
    fn passphrase_decryption_rejects_identity_payloads() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let source = write_source(&root, "plain.txt", "identity only")?;
        let encrypted = root.path().join("plain.txt.age");
        let output = root.path().join("out.txt");

        encrypt_with_identity(&source, &encrypted, &paths)?;
        let error = decrypt_passphrase_file(&encrypted, &output, "wrong".to_owned())
            .expect_err("identity payload should reject passphrase decryption");

        assert!(error.to_string().contains("payload requires an identity"));
        Ok(())
    }

    #[test]
    fn identity_encryption_round_trips_file_contents() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let source = write_source(&root, "plain.txt", "local identity")?;
        let encrypted = root.path().join("plain.txt.age");
        let decrypted = root.path().join("round-trip.txt");

        encrypt_with_identity(&source, &encrypted, &paths)?;
        decrypt_identity_file(&encrypted, &decrypted, &paths)?;

        assert_eq!(std::fs::read_to_string(decrypted)?, "local identity");
        Ok(())
    }

    #[test]
    fn identity_decryption_rejects_passphrase_payloads() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let source = write_source(&root, "plain.txt", "passphrase only")?;
        let encrypted = root.path().join("plain.txt.age");
        let output = root.path().join("out.txt");

        encrypt_with_passphrase(&source, &encrypted, "passphrase".to_owned())?;
        let error = decrypt_identity_file(&encrypted, &output, &paths)
            .expect_err("passphrase payload should reject identity decryption");

        assert!(error.to_string().contains("payload requires a passphrase"));
        Ok(())
    }

    #[test]
    fn prepare_passphrase_upload_encrypts_to_temporary_file() -> Result<()> {
        let root = tempdir()?;
        let source = write_source(&root, "plain.txt", "encrypted upload")?;

        let prepared = prepare_passphrase_upload(&source, "plain.txt", "upload-secret".to_owned())?;
        let decrypted = root.path().join("decrypted.txt");
        decrypt_passphrase_file(prepared.upload_path(), &decrypted, "upload-secret".to_owned())?;

        assert_eq!(prepared.remote_name, "plain.txt.age");
        assert_eq!(prepared.mode, EncryptionMode::Passphrase);
        assert_eq!(std::fs::read_to_string(decrypted)?, "encrypted upload");
        Ok(())
    }

    #[test]
    fn prepare_identity_upload_encrypts_to_temporary_file() -> Result<()> {
        let root = tempdir()?;
        let (_paths_root, paths) = test_paths()?;
        let source = write_source(&root, "plain.txt", "identity upload")?;

        let prepared = prepare_identity_upload(&source, "plain.txt", &paths)?;
        let decrypted = root.path().join("decrypted.txt");
        decrypt_identity_file(prepared.upload_path(), &decrypted, &paths)?;

        assert_eq!(prepared.remote_name, "plain.txt.age");
        assert_eq!(prepared.mode, EncryptionMode::Identity);
        assert_eq!(std::fs::read_to_string(decrypted)?, "identity upload");
        Ok(())
    }

    #[test]
    fn ensure_identity_generates_and_reuses_identity_key() -> Result<()> {
        let (_root, paths) = test_paths()?;

        let generated = ensure_identity(&paths)?;
        let stored = std::fs::read_to_string(&paths.identity_path)?;
        let loaded = ensure_identity(&paths)?;

        assert!(paths.identity_path.exists());
        assert_eq!(stored.trim(), generated.to_string().expose_secret());
        assert_eq!(loaded.to_string().expose_secret(), generated.to_string().expose_secret());
        Ok(())
    }

    #[test]
    fn ensure_identity_rejects_invalid_key_material() -> Result<()> {
        let (_root, paths) = test_paths()?;
        std::fs::write(&paths.identity_path, "not-a-valid-age-key")?;

        let result = ensure_identity(&paths);
        assert!(result.is_err());
        let error = result.err().expect("invalid identity should fail");

        assert!(!error.to_string().is_empty());
        Ok(())
    }
}