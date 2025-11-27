use anyhow::Context;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use self::zip::ZipArchive;

mod zip;

#[derive(Debug, Default)]
pub struct Runtime;

#[cfg(feature = "async-std-runtime")]
impl Runtime {
    pub async fn exists(folder_path: &Path) -> bool {
        async_std::fs::metadata(folder_path).await.is_ok()
    }

    pub async fn download_json<T>(url: &str) -> anyhow::Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        use surf::http;

        let url = url.parse::<surf::Url>().context("Invalid metadata url")?;
        let mut res = surf::RequestBuilder::new(http::Method::Get, url)
            .await
            .map_err(|e| e.into_inner())
            .context("Failed to send request to host")?;
        if res.status() != surf::StatusCode::Ok {
            anyhow::bail!("Invalid metadata url");
        }
        let body = res
            .body_json::<T>()
            .await
            .map_err(|e| e.into_inner().context("Failed to read response body"))?;
        Ok(body)
    }

    pub async fn download_text(url: &str) -> anyhow::Result<String> {
        use surf::http;
        let url = url.parse::<surf::Url>().context("Invalid metadata url")?;
        let mut res = surf::RequestBuilder::new(http::Method::Get, url)
            .await
            .map_err(|e| e.into_inner())
            .context("Failed to send request to host")?;
        if res.status() != surf::StatusCode::Ok {
            anyhow::bail!("Invalid metadata url");
        }
        let body = res
            .body_string()
            .await
            .map_err(|e| e.into_inner().context("Failed to read response body"))?;
        Ok(body)
    }

    pub async fn download_file(url: &str, archive_path: &Path) -> anyhow::Result<()> {
        use async_std::io::WriteExt;
        use surf::http;

        // Open file
        let file = async_std::fs::File::create(&archive_path)
            .await
            .context("Failed to create archive file")?;
        let mut file = async_std::io::BufWriter::new(file);

        // Download
        let url = url.parse::<surf::Url>().context("Invalid archive url")?;
        let res = surf::RequestBuilder::new(http::Method::Get, url)
            .await
            .map_err(|e| e.into_inner())
            .context("Failed to send request to host")?;
        if res.status() != surf::StatusCode::Ok {
            anyhow::bail!("Invalid archive url");
        }
        async_std::io::copy(res, &mut file)
            .await
            .context("Failed to write to archive file")?;

        // Flush to disk
        file.flush().await.context("Failed to flush to disk")?;
        Ok(())
    }

    pub async fn unzip(archive_path: PathBuf, folder_path: PathBuf) -> anyhow::Result<()> {
        async_std::task::spawn_blocking(move || do_unzip(&archive_path, &folder_path)).await?;
        Ok(())
    }
}

#[cfg(feature = "tokio-runtime")]
impl Runtime {
    pub async fn exists(folder_path: &Path) -> bool {
        tokio::fs::metadata(folder_path).await.is_ok()
    }

    pub async fn download_json<T>(url: &str) -> anyhow::Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let url = url
            .parse::<reqwest::Url>()
            .context("Invalid metadata url")?;
        let res = reqwest::get(url)
            .await
            .context("Failed to send request to host")?;
        if res.status() != reqwest::StatusCode::OK {
            anyhow::bail!("Invalid metadata url");
        }
        let body = res
            .json::<T>()
            .await
            .context("Failed to read response body")?;
        Ok(body)
    }

    pub async fn download_text(url: &str) -> anyhow::Result<String> {
        let url = url
            .parse::<reqwest::Url>()
            .context("Invalid metadata url")?;
        let res = reqwest::get(url)
            .await
            .context("Failed to send request to host")?;
        if res.status() != reqwest::StatusCode::OK {
            anyhow::bail!("Invalid metadata url");
        }
        let body = res.text().await.context("Failed to read response body")?;
        Ok(body)
    }

    pub async fn download(url: &str, archive_path: &Path) -> anyhow::Result<()> {
        use tokio::io::AsyncWriteExt;

        // Open file
        let file = tokio::fs::File::create(&archive_path)
            .await
            .context("Failed to create archive file")?;
        let mut file = tokio::io::BufWriter::new(file);

        // Download
        let url = url.parse::<reqwest::Url>().context("Invalid archive url")?;
        let mut res = reqwest::get(url)
            .await
            .context("Failed to send request to host")?;
        if res.status() != reqwest::StatusCode::OK {
            anyhow::bail!("Invalid archive url");
        }
        while let Some(chunk) = res.chunk().await.context("Failed to read response chunk")? {
            file.write(&chunk)
                .await
                .context("Failed to write to archive file")?;
        }

        // Flush to disk
        file.flush().await.context("Failed to flush to disk")?;

        Ok(())
    }

    pub async fn unzip(archive_path: PathBuf, folder_path: PathBuf) -> anyhow::Result<()> {
        tokio::task::spawn_blocking(move || do_unzip(&archive_path, &folder_path)).await?
    }
}

fn do_unzip(archive_path: &Path, folder_path: &Path) -> anyhow::Result<()> {
    use std::fs;

    // Prepare
    fs::create_dir_all(folder_path).context("Failed to create folder")?;
    let file = fs::File::open(archive_path).context("Failed to open archive")?;

    // Unzip
    let mut archive = ZipArchive::new(file).context("Failed to unzip archive")?;
    archive.extract(folder_path)?;

    // Clean (if possible)
    let _ = fs::remove_file(archive_path);
    Ok(())
}
