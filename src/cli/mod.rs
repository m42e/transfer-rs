use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use crate::commands;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "CLI client for transfer.sh-compatible instances"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub server: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Upload(UploadArgs),
    Download(DownloadArgs),
    History(HistoryArgs),
    Delete(DeleteArgs),
}

#[derive(Debug, Args)]
pub struct UploadArgs {
    pub file: PathBuf,
    #[arg(long)]
    pub remote_name: Option<String>,
    #[arg(long)]
    pub max_days: Option<u32>,
    #[arg(long)]
    pub max_downloads: Option<u32>,
    #[arg(long, conflicts_with = "identity")]
    pub passphrase: bool,
    #[arg(long, conflicts_with = "passphrase")]
    pub identity: bool,
}

#[derive(Debug, Args)]
pub struct DownloadArgs {
    pub url: String,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, conflicts_with = "identity")]
    pub passphrase: bool,
    #[arg(long, conflicts_with = "passphrase")]
    pub identity: bool,
}

#[derive(Debug, Args)]
pub struct HistoryArgs {
    #[arg(long)]
    pub show_deleted: bool,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    pub id_or_url: String,
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Upload(args) => commands::upload::run(cli.server, args).await,
        Command::Download(args) => commands::download::run(cli.server, args).await,
        Command::History(args) => commands::history::run(cli.server, args).await,
        Command::Delete(args) => commands::delete::run(cli.server, args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn parses_upload_command_with_options() {
        let cli = Cli::try_parse_from([
            "transfer-rs",
            "--server",
            "https://example.invalid",
            "upload",
            "file.txt",
            "--remote-name",
            "remote.txt",
            "--max-days",
            "7",
            "--max-downloads",
            "3",
            "--identity",
        ])
        .expect("upload args parse");

        assert_eq!(cli.server.as_deref(), Some("https://example.invalid"));
        assert!(matches!(&cli.command, Command::Upload(_)));
        if let Command::Upload(args) = cli.command {
            assert_eq!(args.file, std::path::PathBuf::from("file.txt"));
            assert_eq!(args.remote_name.as_deref(), Some("remote.txt"));
            assert_eq!(args.max_days, Some(7));
            assert_eq!(args.max_downloads, Some(3));
            assert!(args.identity);
            assert!(!args.passphrase);
        }
    }

    #[test]
    fn parses_download_history_and_delete_commands() {
        let download = Cli::try_parse_from([
            "transfer-rs",
            "download",
            "https://example.invalid/file.age",
            "--passphrase",
        ])
        .expect("download args parse");
        assert!(matches!(download.command, Command::Download(_)));

        let history = Cli::try_parse_from(["transfer-rs", "history", "--show-deleted"])
            .expect("history args parse");
        assert!(matches!(history.command, Command::History(_)));

        let delete =
            Cli::try_parse_from(["transfer-rs", "delete", "record-id"]).expect("delete args parse");
        assert!(matches!(delete.command, Command::Delete(_)));
    }

    #[test]
    fn rejects_conflicting_encryption_flags() {
        let error = Cli::try_parse_from([
            "transfer-rs",
            "upload",
            "file.txt",
            "--passphrase",
            "--identity",
        ])
        .expect_err("conflicting upload flags should fail");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);

        let error = Cli::try_parse_from([
            "transfer-rs",
            "download",
            "https://example.invalid/file.age",
            "--passphrase",
            "--identity",
        ])
        .expect_err("conflicting download flags should fail");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}
