use std::path::Path;

use anyhow::{Context, Result, bail};
use reqwest::header::{CONTENT_LENGTH, HeaderValue};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use url::Url;

pub struct TransferClient {
    http: reqwest::Client,
    base_url: Url,
}

pub struct UploadResponse {
    pub download_url: String,
    pub delete_url: String,
    pub remote_name: String,
}

impl TransferClient {
    pub fn new(base_url: &str) -> Result<Self> {
        let base_url = Url::parse(base_url).context("invalid transfer server URL")?;
        Ok(Self {
            http: reqwest::Client::new(),
            base_url,
        })
    }

    pub async fn upload_file(
        &self,
        local_path: &Path,
        remote_name: &str,
        max_days: Option<u32>,
        max_downloads: Option<u32>,
    ) -> Result<UploadResponse> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("base URL does not support path segments"))?;
            segments.pop_if_empty();
            segments.push(remote_name);
        }

        let file = tokio::fs::File::open(local_path)
            .await
            .with_context(|| format!("failed to open {}", local_path.display()))?;
        let len = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("failed to inspect {}", local_path.display()))?
            .len();
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let mut request = self
            .http
            .put(url)
            .header(CONTENT_LENGTH, HeaderValue::from_str(&len.to_string())?)
            .body(body);

        if let Some(days) = max_days {
            request = request.header("Max-Days", days.to_string());
        }
        if let Some(downloads) = max_downloads {
            request = request.header("Max-Downloads", downloads.to_string());
        }

        let response = request.send().await.context("failed to contact transfer server")?;
        let status = response.status();
        let delete_url = response
            .headers()
            .get("x-url-delete")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let body = response.text().await.context("failed to read upload response")?;

        if !status.is_success() {
            bail!("upload failed with status {status}: {}", body.trim());
        }

        let download_url = body.trim().to_owned();
        let delete_url = delete_url.context("transfer server did not return X-Url-Delete")?;
        Ok(UploadResponse {
            download_url,
            delete_url,
            remote_name: remote_name.to_owned(),
        })
    }

    pub async fn download_to_path(&self, url: &str, output: &Path) -> Result<()> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to request {url}"))?
            .error_for_status()
            .with_context(|| format!("download failed for {url}"))?;

        let mut output_file = tokio::fs::File::create(output)
            .await
            .with_context(|| format!("failed to create {}", output.display()))?;
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("failed to read download stream")?;
            output_file
                .write_all(&chunk)
                .await
                .with_context(|| format!("failed to write {}", output.display()))?;
        }
        output_file.flush().await?;
        Ok(())
    }

    pub async fn delete(&self, delete_url: &str) -> Result<()> {
        self.http
            .delete(delete_url)
            .send()
            .await
            .with_context(|| format!("failed to contact {delete_url}"))?
            .error_for_status()
            .with_context(|| format!("delete failed for {delete_url}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::TransferClient;
    use anyhow::Result;
    use mockito::{Matcher, Server};
    use tempfile::tempdir;

    #[test]
    fn new_rejects_invalid_urls() {
        let result = TransferClient::new("::not-a-url::");
        assert!(result.is_err());
        let error = result.err().expect("invalid URL should fail");
        assert!(error.to_string().contains("invalid transfer server URL"));
    }

    #[tokio::test]
    async fn upload_file_sends_expected_request_and_returns_urls() -> Result<()> {
        let mut server = Server::new_async().await;
        let temp = tempdir()?;
        let file_path = temp.path().join("upload.txt");
        std::fs::write(&file_path, b"payload")?;

        let mock = server
            .mock("PUT", "/remote.txt")
            .match_header("content-length", "7")
            .match_header("max-days", "5")
            .match_header("max-downloads", "2")
            .match_body(Matcher::Exact("payload".to_owned()))
            .with_status(200)
            .with_header("x-url-delete", &format!("{}/delete/remote.txt", server.url()))
            .with_body(format!("{}/remote.txt\n", server.url()))
            .create_async()
            .await;

        let client = TransferClient::new(&server.url())?;
        let response = client
            .upload_file(&file_path, "remote.txt", Some(5), Some(2))
            .await?;

        mock.assert_async().await;
        assert_eq!(response.remote_name, "remote.txt");
        assert_eq!(response.download_url, format!("{}/remote.txt", server.url()));
        assert_eq!(response.delete_url, format!("{}/delete/remote.txt", server.url()));
        Ok(())
    }

    #[tokio::test]
    async fn upload_file_rejects_missing_delete_header() -> Result<()> {
        let mut server = Server::new_async().await;
        let temp = tempdir()?;
        let file_path = temp.path().join("upload.txt");
        std::fs::write(&file_path, b"payload")?;
        let _mock = server
            .mock("PUT", "/remote.txt")
            .with_status(200)
            .with_body(format!("{}/remote.txt", server.url()))
            .create_async()
            .await;

        let client = TransferClient::new(&server.url())?;
        let result = client.upload_file(&file_path, "remote.txt", None, None).await;
        assert!(result.is_err());
        let error = result.err().expect("missing delete header should fail");

        assert!(error.to_string().contains("X-Url-Delete"));
        Ok(())
    }

    #[tokio::test]
    async fn upload_file_surfaces_server_errors() -> Result<()> {
        let mut server = Server::new_async().await;
        let temp = tempdir()?;
        let file_path = temp.path().join("upload.txt");
        std::fs::write(&file_path, b"payload")?;
        let _mock = server
            .mock("PUT", "/remote.txt")
            .with_status(500)
            .with_body("nope")
            .create_async()
            .await;

        let client = TransferClient::new(&server.url())?;
        let result = client.upload_file(&file_path, "remote.txt", None, None).await;
        assert!(result.is_err());
        let error = result.err().expect("server failure should fail upload");

        assert!(error.to_string().contains("upload failed with status 500 Internal Server Error"));
        Ok(())
    }

    #[tokio::test]
    async fn upload_file_rejects_base_urls_without_path_segments() -> Result<()> {
        let client = TransferClient::new("mailto:test@example.com")?;
        let temp = tempdir()?;
        let file_path = temp.path().join("upload.txt");
        std::fs::write(&file_path, b"payload")?;

        let result = client.upload_file(&file_path, "remote.txt", None, None).await;
        assert!(result.is_err());
        let error = result.err().expect("mailto URLs cannot accept path segments");

        assert!(error.to_string().contains("base URL does not support path segments"));
        Ok(())
    }

    #[tokio::test]
    async fn download_to_path_writes_body_to_disk() -> Result<()> {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("GET", "/download.txt")
            .with_status(200)
            .with_body("downloaded")
            .create_async()
            .await;

        let client = TransferClient::new(&server.url())?;
        let temp = tempdir()?;
        let output = temp.path().join("download.txt");
        client.download_to_path(&format!("{}/download.txt", server.url()), &output).await?;

        assert_eq!(std::fs::read_to_string(output)?, "downloaded");
        Ok(())
    }

    #[tokio::test]
    async fn download_to_path_surfaces_http_failures() -> Result<()> {
        let mut server = Server::new_async().await;
        let _mock = server.mock("GET", "/missing.txt").with_status(404).create_async().await;
        let client = TransferClient::new(&server.url())?;
        let temp = tempdir()?;
        let output = temp.path().join("download.txt");

        let error = client
            .download_to_path(&format!("{}/missing.txt", server.url()), &output)
            .await
            .expect_err("404 should fail");

        assert!(error.to_string().contains("download failed for"));
        Ok(())
    }

    #[tokio::test]
    async fn delete_calls_remote_endpoint() -> Result<()> {
        let mut server = Server::new_async().await;
        let mock = server.mock("DELETE", "/delete/file").with_status(200).create_async().await;
        let client = TransferClient::new(&server.url())?;

        client.delete(&format!("{}/delete/file", server.url())).await?;
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn delete_surfaces_http_errors() -> Result<()> {
        let mut server = Server::new_async().await;
        let _mock = server.mock("DELETE", "/delete/file").with_status(500).create_async().await;
        let client = TransferClient::new(&server.url())?;

        let error = client
            .delete(&format!("{}/delete/file", server.url()))
            .await
            .expect_err("delete failure should surface");

        assert!(error.to_string().contains("delete failed for"));
        Ok(())
    }
}