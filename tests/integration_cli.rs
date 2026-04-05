use std::io::{Read, Write};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use assert_cmd::prelude::*;
use mockito::Server;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serial_test::serial;
use tempfile::tempdir;

fn binary_path() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("transfer-rs")
}

fn run_pty_command<I, S>(args: I, home: &std::path::Path, cwd: &std::path::Path, input: &str) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new(binary_path());
    for arg in args {
        command.arg(arg.as_ref());
    }
    command.env("HOME", home);
    command.cwd(cwd);

    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_output = Arc::clone(&output);
    let mut reader = pair.master.try_clone_reader()?;
    let reader_thread = thread::spawn(move || -> Result<()> {
        let mut buffer = [0_u8; 4096];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }

            reader_output
                .lock()
                .expect("lock PTY output")
                .extend_from_slice(&buffer[..read]);
        }
        Ok(())
    });

    let mut writer = pair.master.take_writer()?;
    let input = input.to_owned();
    let writer_output = Arc::clone(&output);
    let writer_thread = thread::spawn(move || -> Result<()> {
        for _ in 0..120 {
            if !writer_output.lock().expect("lock PTY output").is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    });

    writer_thread.join().expect("writer thread panicked")?;
    let status = child.wait()?;
    drop(pair.master);
    reader_thread.join().expect("reader thread panicked")?;

    let output = String::from_utf8_lossy(&output.lock().expect("lock PTY output")).into_owned();
    anyhow::ensure!(status.success(), "command failed: {output}");
    Ok(output)
}

#[test]
#[serial]
fn plain_upload_download_delete_flow_via_binary() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;
    let source = workdir.path().join("source.txt");
    std::fs::write(&source, "plain payload")?;

    let mut server = Server::new();
    let download_url = format!("{}/source.txt", server.url());
    let delete_url = format!("{}/delete/source.txt", server.url());

    let upload_mock = server
        .mock("PUT", "/source.txt")
        .with_status(200)
        .with_header("x-url-delete", &delete_url)
        .with_body(format!("{download_url}\n"))
        .create();

    let mut upload = Command::new(binary_path());
    upload
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .args(["--server", &server.url(), "upload", source.to_str().context("source path")?]);
    upload
        .assert()
        .success()
        .stdout(contains("Uploaded: source.txt").and(contains(&download_url)).and(contains(&delete_url)));
    upload_mock.assert();

    let download_mock = server.mock("GET", "/source.txt").with_status(200).with_body("plain payload").create();
    std::fs::remove_file(&source)?;

    let mut download = Command::new(binary_path());
    download
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .args(["download", &download_url]);
    download
        .assert()
        .success()
        .stdout(contains("Saved:"));
    download_mock.assert();
    assert_eq!(std::fs::read_to_string(workdir.path().join("source.txt"))?, "plain payload");

    let delete_mock = server.mock("DELETE", "/delete/source.txt").with_status(200).create();
    let mut delete = Command::new(binary_path());
    delete
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .args(["delete", &download_url]);
    delete
        .assert()
        .success()
        .stdout(contains("Deleted remote file: source.txt"));
    delete_mock.assert();

    Ok(())
}

#[test]
#[serial]
fn upload_rejects_non_file_input() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;

    let mut command = Command::new(binary_path());
    command
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .args(["upload", workdir.path().to_str().context("workdir path")?]);
    command
        .assert()
        .failure()
        .stderr(contains("input path is not a file"));
    Ok(())
}

#[test]
#[serial]
fn delete_reports_missing_history_record() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;

    let mut command = Command::new(binary_path());
    command
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .args(["delete", "missing-record"]);
    command
        .assert()
        .failure()
        .stderr(contains("no history record matched 'missing-record'"));
    Ok(())
}

#[test]
#[serial]
fn download_rejects_existing_output_path() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;
    let output = workdir.path().join("existing.txt");
    std::fs::write(&output, "already here")?;

    let mut command = Command::new(binary_path());
    command
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .args([
            "download",
            "https://example.invalid/file.txt",
            "--output",
            output.to_str().context("output path")?,
        ]);
    command
        .assert()
        .failure()
        .stderr(contains("output file already exists"));
    Ok(())
}

#[test]
#[serial]
fn identity_upload_prints_encryption_mode() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;
    let source = workdir.path().join("secret.txt");
    std::fs::write(&source, "identity payload")?;

    let mut server = Server::new();
    let download_url = format!("{}/secret.txt.age", server.url());
    let delete_url = format!("{}/delete/secret.txt.age", server.url());
    let upload_mock = server
        .mock("PUT", "/secret.txt.age")
        .with_status(200)
        .with_header("x-url-delete", &delete_url)
        .with_body(format!("{download_url}\n"))
        .create();

    let mut upload = Command::new(binary_path());
    upload
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .args([
            "--server",
            &server.url(),
            "upload",
            source.to_str().context("source path")?,
            "--identity",
        ]);
    upload
        .assert()
        .success()
        .stdout(contains("Encryption: identity"));
    upload_mock.assert();
    Ok(())
}

#[test]
#[serial]
fn passphrase_download_flow_via_pty() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;
    let source = workdir.path().join("secret.txt");
    let encrypted = workdir.path().join("secret.txt.age");
    let output = workdir.path().join("downloaded.txt");
    std::fs::write(&source, "very secret payload")?;
    transfer_rs::client::crypto::encrypt_with_passphrase(&source, &encrypted, "secret-passphrase".to_owned())?;

    let mut server = Server::new();
    let _download_mock = server
        .mock("GET", "/secret.txt.age")
        .with_status(200)
        .with_body(std::fs::read(&encrypted)?)
        .create();

    let output_text = run_pty_command(
        [
            "download",
            &format!("{}/secret.txt.age", server.url()),
            "--passphrase",
            "--output",
            output.to_str().context("output path")?,
        ],
        home.path(),
        workdir.path(),
        "secret-passphrase\n",
    )?;

    assert!(output_text.contains("Saved:"));
    assert_eq!(std::fs::read_to_string(output)?, "very secret payload");
    Ok(())
}

#[test]
#[serial]
fn identity_download_flow_via_binary() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;
    let saved_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let paths = transfer_rs::storage::paths::AppPaths::discover()?;
    let source = workdir.path().join("identity.txt");
    let encrypted = workdir.path().join("identity.txt.age");
    let output = workdir.path().join("identity.out");
    std::fs::write(&source, "identity payload")?;
    transfer_rs::client::crypto::encrypt_with_identity(&source, &encrypted, &paths)?;
    match saved_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }

    let mut server = Server::new();
    let _download_mock = server
        .mock("GET", "/identity.txt.age")
        .with_status(200)
        .with_body(std::fs::read(&encrypted)?)
        .create();

    let mut command = Command::new(binary_path());
    command
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .args([
            "download",
            &format!("{}/identity.txt.age", server.url()),
            "--identity",
            "--output",
            output.to_str().context("output path")?,
        ]);
    command
        .assert()
        .success()
        .stdout(contains("Saved:"));
    assert_eq!(std::fs::read_to_string(output)?, "identity payload");
    Ok(())
}

#[test]
#[serial]
fn history_command_handles_real_key_presses() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;
    let saved_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let paths = transfer_rs::storage::paths::AppPaths::discover()?;
    let store = transfer_rs::storage::db::HistoryStore::new(&paths)?;
    let mut server = Server::new();
    let now = chrono::Utc::now();
    store.insert_record(&transfer_rs::model::UploadRecord {
        id: "one".to_owned(),
        original_name: "one.txt".to_owned(),
        remote_name: "one.txt".to_owned(),
        source_path: Some("/tmp/one.txt".to_owned()),
        download_url: "https://example.invalid/one.txt".to_owned(),
        delete_url: format!("{}/delete/one", server.url()),
        uploaded_at: now,
        size_bytes: 1,
        encryption_mode: transfer_rs::model::EncryptionMode::None,
        is_deleted: false,
        deleted_at: None,
    })?;
    store.insert_record(&transfer_rs::model::UploadRecord {
        id: "two".to_owned(),
        original_name: "two.txt".to_owned(),
        remote_name: "two.txt".to_owned(),
        source_path: Some("/tmp/two.txt".to_owned()),
        download_url: "https://example.invalid/two.txt".to_owned(),
        delete_url: format!("{}/delete/two", server.url()),
        uploaded_at: now + chrono::Duration::seconds(1),
        size_bytes: 2,
        encryption_mode: transfer_rs::model::EncryptionMode::None,
        is_deleted: false,
        deleted_at: None,
    })?;
    match saved_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }

    let delete_two = server.mock("DELETE", "/delete/two").with_status(200).create();
    run_pty_command(
        ["--server", &server.url(), "history", "--show-deleted"],
        home.path(),
        workdir.path(),
        "dq",
    )?;

    delete_two.assert();
    assert!(store.find_by_id_or_url("two")?.context("missing deleted record")?.is_deleted);
    assert!(!store.find_by_id_or_url("one")?.context("missing remaining record")?.is_deleted);
    Ok(())
}

#[test]
fn binary_help_smoke_test() -> Result<()> {
    let home = tempdir()?;
    let workdir = tempdir()?;

    let mut command = Command::new(binary_path());
    command
        .env("HOME", home.path())
        .current_dir(workdir.path())
        .arg("--help");
    command
        .assert()
        .success()
        .stdout(contains("CLI client for transfer.sh-compatible instances").and(contains("upload")).and(contains("history")));
    Ok(())
}

#[test]
fn binary_version_reports_package_version() -> Result<()> {
    let mut command = Command::new(binary_path());
    command.arg("--version");
    command
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
    Ok(())
}