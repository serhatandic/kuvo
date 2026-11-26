//! Verified downloads. A partial file is named by the expected digest, never
//! unpacked, and can be continued after a stopped transfer.
use reqwest::{Client, StatusCode, header};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{io, path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Deserialize)]
pub(super) struct Expected {
    pub sha256: String,
    pub size: u64,
}

pub(super) fn allowed(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.as_str() == address
        && [
            "https://storage.googleapis.com/chrome-for-testing-public/",
            "https://dl.rockylinux.org/pub/rocky/8/",
            "https://dl.rockylinux.org/vault/rocky/8.10/",
        ]
        .iter()
        .any(|prefix| address.starts_with(prefix))
}
fn failure(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
async fn verify(file: &Path, expected: &Expected) -> io::Result<bool> {
    let mut file = tokio::fs::File::open(file).await?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut count = 0u64;
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > expected.size {
            return Ok(false);
        }
        hash.update(&buffer[..n]);
    }
    Ok(count == expected.size && hex::encode(hash.finalize()) == expected.sha256)
}

/// True when complete and verified; false when the source doesn't have it (404).
pub(super) async fn download(
    client: &Client,
    address: &str,
    source: Option<&str>,
    expected: &Expected,
    partial: &Path,
    mut progress: impl FnMut(u64),
) -> io::Result<bool> {
    if !allowed(address) {
        return Err(failure(format!(
            "Kuvo Code doesn't download the browser from {address}."
        )));
    }
    if expected.sha256.len() != 64
        || !expected
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || b"abcdef".contains(&b))
        || expected.size == 0
    {
        return Err(failure("The browser has an invalid pin."));
    }
    let original = url::Url::parse(address).map_err(io::Error::other)?;
    let host = original.host_str().unwrap_or_default();
    let address = source.map_or_else(
        || address.to_owned(),
        |source| {
            format!(
                "{}/{}",
                source.trim_end_matches('/'),
                address.trim_start_matches("https://")
            )
        },
    );
    let mut offset = tokio::fs::metadata(partial).await.map_or(0, |m| m.len());
    if offset >= expected.size {
        if offset == expected.size && verify(partial, expected).await? {
            progress(offset);
            return Ok(true);
        }
        tokio::fs::remove_file(partial).await?;
        offset = 0;
    }
    let mut request = client
        .get(&address)
        .timeout(Duration::from_secs((expected.size / 50_000).max(600)));
    if offset > 0 {
        request = request.header(header::RANGE, format!("bytes={offset}-"));
    }
    let mut response=request.send().await.map_err(|e|failure(format!("Couldn't reach {host} to download the browser ({e}): the machine needs to reach storage.googleapis.com and dl.rockylinux.org over HTTPS.")))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !response.status().is_success() {
        return Err(failure(format!(
            "{host} answered {} for the browser's download.",
            response.status().as_u16()
        )));
    }
    if response.status() == StatusCode::PARTIAL_CONTENT {
        let wanted = format!("bytes {offset}-{}/{}", expected.size - 1, expected.size);
        if response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            != Some(wanted.as_str())
        {
            return Err(failure(format!(
                "The browser's download from {host} stopped partway."
            )));
        }
    } else {
        offset = 0;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(offset == 0)
        .append(offset > 0)
        .open(partial)
        .await?;
    let mut count = offset;
    progress(count);
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        failure(format!(
            "The browser's download from {host} stopped partway ({e})."
        ))
    })? {
        count += chunk.len() as u64;
        if count > expected.size {
            drop(file);
            tokio::fs::remove_file(partial).await?;
            return Err(failure(format!(
                "The browser's download from {host} is larger than it should be, so it wasn't used."
            )));
        }
        file.write_all(&chunk).await?;
        progress(count);
    }
    file.sync_all().await?;
    drop(file);
    if count != expected.size {
        return Err(failure(format!(
            "The browser's download from {host} stopped partway."
        )));
    }
    if !verify(partial, expected).await? {
        tokio::fs::remove_file(partial).await?;
        return Err(failure(format!(
            "The browser's download from {host} isn't what Kuvo Code expects (its checksum doesn't match), so it wasn't used."
        )));
    }
    Ok(true)
}
