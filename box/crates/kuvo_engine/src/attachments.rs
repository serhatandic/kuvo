//! Keep inline attachments in the conversation's folder and pass what its model can read.

use std::{path::Path, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;

use crate::{Engine, lifetime::Demand};

const MODEL_BYTES: usize = 20 * 1024 * 1024;
const MODEL_WAIT: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(crate) struct Attachments {
    pub files: Vec<Value>,
    pub included: Vec<String>,
    pub paths: Vec<String>,
}

impl Attachments {
    pub async fn keep(engine: &Engine, session: &str, files: Vec<Value>) -> Self {
        let mut kept = Self::default();
        let mut input = None;
        let folder = engine.home.join("attachments").join(session);
        for mut file in files {
            let Some((mime, bytes)) = inline(&file) else {
                kept.files.push(file);
                continue;
            };
            if input.is_none() {
                input = Some(model_input(engine, session).await);
            }
            let formats = input.as_ref().and_then(Option::as_deref);
            let text = text_mime(&mime);
            let accepts = bytes.len() <= MODEL_BYTES
                && (text
                    || (mime == "application/pdf" && accepts_input(formats, "pdf"))
                    || (mime.starts_with("image/") && accepts_input(formats, "image")));
            let saved = save(&folder, file["name"].as_str().unwrap_or_default(), &bytes)
                .await
                .map(|path| path.to_string_lossy().into_owned());
            match (accepts, saved) {
                (true, saved) => {
                    if text && mime != "text/plain" {
                        file["uri"] = Value::String(format!(
                            "data:text/plain;base64,{}",
                            STANDARD.encode(bytes)
                        ));
                    }
                    kept.files.push(file);
                    kept.included.extend(saved);
                }
                (false, Some(path)) => kept.paths.push(path),
                (false, None) => kept.files.push(file),
            }
        }
        kept
    }
}

fn inline(file: &Value) -> Option<(String, Vec<u8>)> {
    let (label, data) = file
        .get("uri")?
        .as_str()?
        .strip_prefix("data:")?
        .split_once(',')?;
    let label = label.strip_suffix(";base64")?;
    let mime = label
        .split(';')
        .next()
        .filter(|mime| !mime.is_empty())
        .unwrap_or("application/octet-stream")
        .to_ascii_lowercase();
    // The existing clients also send unpadded and URL-safe base64 inside data URIs.
    let mut encoded = data
        .bytes()
        .take_while(|&byte| byte != b'=')
        .filter_map(|byte| match byte {
            b'-' => Some(b'+'),
            b'_' => Some(b'/'),
            b'+' | b'/' => Some(byte),
            byte if byte.is_ascii_alphanumeric() => Some(byte),
            _ => None,
        })
        .collect::<Vec<_>>();
    if encoded.len() % 4 == 1 {
        encoded.pop();
    }
    encoded.resize(encoded.len().next_multiple_of(4), b'=');
    use base64::engine::{GeneralPurpose, general_purpose::GeneralPurposeConfig};
    let decoder = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    );
    Some((mime, decoder.decode(encoded).ok()?))
}

fn text_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/javascript"
                | "application/xml"
                | "application/x-yaml"
                | "application/yaml"
                | "application/toml"
                | "application/x-sh"
                | "application/sql"
        )
}

fn accepts_input(input: Option<&[String]>, kind: &str) -> bool {
    input.is_none_or(|formats| formats.iter().any(|format| format == kind))
}

async fn save(folder: &Path, name: &str, bytes: &[u8]) -> Option<std::path::PathBuf> {
    let basename = name.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let basename = if Path::new(basename).file_name() != Some(std::ffi::OsStr::new(basename)) {
        "attachment"
    } else {
        basename
    };
    fs::create_dir_all(folder).await.ok()?;
    for _ in 0..10 {
        let mut random = [0; 4];
        getrandom::fill(&mut random).ok()?;
        let directory = folder.join(hex::encode(random));
        match fs::create_dir(&directory).await {
            Ok(()) => {
                let path = directory.join(basename);
                use tokio::io::AsyncWriteExt;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .await
                    .ok()?;
                file.write_all(bytes).await.ok()?;
                file.sync_all().await.ok()?;
                return Some(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
    }
    None
}

#[derive(Deserialize)]
struct Model {
    #[serde(rename = "providerID")]
    provider: String,
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    capabilities: Option<Capabilities>,
}

#[derive(Deserialize)]
struct Capabilities {
    input: Option<Vec<String>>,
}

async fn model_input(engine: &Engine, session: &str) -> Option<Vec<String>> {
    let lease = engine.acquire(Demand::Passive).await?;
    let read = async {
        let route = crate::sessions::path(session);
        let headers = http::HeaderMap::new();
        let record = &lease.record;
        let client = &engine.upstream;
        let session = client
            .get_data::<Value>(record, &route, &headers, MODEL_WAIT)
            .await
            .ok()?;
        let model = if session["model"].is_null() {
            client
                .get_data::<Model>(record, "api/model/default", &headers, MODEL_WAIT)
                .await
                .ok()?
        } else {
            serde_json::from_value(session["model"].clone()).ok()?
        };
        if let Some(input) = model.capabilities.and_then(|caps| caps.input) {
            return Some(input);
        }
        client
            .get_data::<Vec<Value>>(record, "api/model", &headers, MODEL_WAIT)
            .await
            .ok()?
            .into_iter()
            .filter_map(|value| serde_json::from_value::<Model>(value).ok())
            .find(|entry| {
                entry.provider == model.provider
                    && (entry.id == model.id || entry.aliases.contains(&model.id))
            })?
            .capabilities?
            .input
    };
    tokio::time::timeout(MODEL_WAIT, read).await.ok().flatten()
}
