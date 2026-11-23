//! Keep inline attachments in the conversation's folder and pass what its model can read.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http::{HeaderMap, Method};
use kuvo_server::{DispatchRequest, LocalOrigin, Transport};
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;

use crate::{Engine, lifetime::Demand};

const MODEL_BYTES: usize = 20 * 1024 * 1024;
const MODEL_WAIT: Duration = Duration::from_secs(1);
const KEEP_FOR: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const SWEEP_EVERY_MS: u64 = 60 * 60 * 1000;

#[derive(Default)]
pub(crate) struct Attachments {
    pub files: Vec<Value>,
    pub included: Vec<String>,
    pub paths: Vec<String>,
}

impl Attachments {
    pub async fn keep(engine: &Engine, session: &str, files: Vec<Value>) -> Self {
        Self::keep_in(engine, session, files, &HeaderMap::new()).await
    }

    async fn keep_in(
        engine: &Engine,
        session: &str,
        files: Vec<Value>,
        headers: &HeaderMap,
    ) -> Self {
        let mut kept = Self::default();
        let mut input = None;
        let root = engine.home.join("attachments");
        let mut folder = root.clone();
        if session_folder(session) {
            folder.push(session);
        }
        for mut file in files {
            let Some((mime, bytes)) = inline(&file) else {
                kept.files.push(file);
                continue;
            };
            if input.is_none() {
                input = Some(model_input(engine, session, headers).await);
                engine.attachment_expiry.schedule(root.clone());
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

pub(crate) fn session_folder(name: &str) -> bool {
    name.starts_with("ses_") && (5..=124).contains(&name.len()) && crate::sessions::valid_id(name)
}

/// Files kept beside the transcript are shown after the files carried in its prompt.
pub(crate) async fn for_edit(home: &Path, removed: &Value, mut files: Vec<Value>) -> Vec<Value> {
    let metadata = removed.pointer("/metadata/kuvo/parts");
    let paths = match metadata.filter(|parts| parts["words"].is_string()) {
        Some(parts) => parts["paths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect(),
        None => crate::parts::read_parts(removed["text"].as_str().unwrap_or_default()).paths,
    };
    let root = home.join("attachments");
    let legacy = std::env::var_os("KUVO_MAC_ATTACHMENTS")
        .filter(|root| !root.is_empty())
        .map(PathBuf::from);
    for path in paths.into_iter().map(Path::new) {
        if !(path.starts_with(&root) || legacy.as_ref().is_some_and(|root| path.starts_with(root)))
            || !fs::metadata(path).await.is_ok_and(|file| file.is_file())
        {
            continue;
        }
        let (Ok(uri), Some(name)) = (url::Url::from_file_path(path), path.file_name()) else {
            continue;
        };
        if !files.iter().any(|file| file["uri"] == uri.as_str()) {
            files.push(serde_json::json!({ "uri": uri.as_str(), "name": name.to_string_lossy() }));
        }
    }
    files
}

#[derive(Clone, Default)]
pub(crate) struct Expiry(Arc<AtomicU64>);

impl Expiry {
    fn schedule(&self, root: PathBuf) {
        let at = crate::disk::now();
        if self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                (at >= next).then_some(u64::MAX)
            })
            .is_err()
        {
            return;
        }
        let next = self.0.clone();
        tokio::spawn(async move {
            sweep_expired(&root).await;
            next.store(
                crate::disk::now().saturating_add(SWEEP_EVERY_MS),
                Ordering::Relaxed,
            );
        });
    }
}

async fn sweep_expired(root: &Path) {
    let Some(before) = SystemTime::now().checked_sub(KEEP_FOR) else {
        return;
    };
    let Ok(mut entries) = fs::read_dir(root).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let folder = entry.path();
        if entry.file_name().to_str().is_some_and(session_folder)
            && entry.file_type().await.is_ok_and(|kind| kind.is_dir())
        {
            if let Ok(mut files) = fs::read_dir(&folder).await {
                while let Ok(Some(file)) = files.next_entry().await {
                    let name = file.file_name();
                    let name = name.to_string_lossy();
                    if name == "transcript-pictures"
                        || name.strip_prefix("move-mov_").is_some_and(|id| {
                            (1..=40).contains(&id.len())
                                && id.bytes().all(|ch| ch.is_ascii_alphanumeric())
                        })
                    {
                        continue;
                    }
                    expire(&file.path(), before).await;
                }
            }
            let _ = fs::remove_dir(&folder).await;
        } else {
            expire(&folder, before).await;
        }
    }
}

async fn expire(file: &Path, before: SystemTime) {
    if let Ok(metadata) = fs::symlink_metadata(file).await
        && metadata.modified().is_ok_and(|modified| modified < before)
    {
        if metadata.is_dir() {
            let _ = fs::remove_dir_all(file).await;
        } else {
            let _ = fs::remove_file(file).await;
        }
    }
}

/// Keep inline payloads on legacy prompt and command routes, using the caller's model folder.
pub(crate) async fn raw(engine: &Engine, request: &DispatchRequest) -> Option<Vec<u8>> {
    if request.method != Method::POST {
        return None;
    }
    let path = request
        .path
        .split('?')
        .next()?
        .strip_prefix("api/session/")?;
    let (session, action) = path.split_once('/')?;
    if !matches!(action, "prompt" | "command") {
        return None;
    }
    let session = percent_encoding::percent_decode_str(session)
        .decode_utf8()
        .ok()?;
    let mut given = serde_json::from_slice::<serde_json::Map<String, Value>>(&request.body).ok()?;
    let files = given.get_mut("files")?.as_array_mut()?;
    if !files.iter().any(|file| inline_data(file).is_some()) {
        return None;
    }
    let kept =
        Attachments::keep_in(engine, &session, std::mem::take(files), &request.headers).await;
    let from_mac = matches!(
        request.caller.transport(),
        Transport::Http {
            origin: LocalOrigin::Mac,
            ..
        }
    );
    let phone = !from_mac
        && !matches!(request.caller.transport(), Transport::Ssh { .. })
        && !request
            .caller
            .pairing()
            .is_some_and(|device| device.desktop);
    let text = given
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut text = crate::parts::with_attachment_note(text, &kept, from_mac);
    if action == "prompt" {
        crate::parts::raw_prompt_parts(&mut given, &mut text, phone);
    }
    given.insert("text".into(), Value::String(text));
    if kept.files.is_empty() {
        given.remove("files");
    } else {
        given.insert("files".into(), Value::Array(kept.files));
    }
    serde_json::to_vec(&given).ok()
}

fn inline_data(file: &Value) -> Option<(&str, &str)> {
    let (label, data) = file
        .get("uri")?
        .as_str()?
        .strip_prefix("data:")?
        .split_once(',')?;
    Some((label.strip_suffix(";base64")?, data))
}

fn inline(file: &Value) -> Option<(String, Vec<u8>)> {
    let (label, data) = inline_data(file)?;
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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // The expiry worker can remove an empty session folder between the two mkdirs.
                fs::create_dir_all(folder).await.ok()?;
            }
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

async fn model_input(engine: &Engine, session: &str, headers: &HeaderMap) -> Option<Vec<String>> {
    let lease = engine.acquire(Demand::Passive).await?;
    let read = async {
        let route = crate::sessions::path(session);
        let record = &lease.record;
        let client = &engine.upstream;
        let session = client
            .get_data::<Value>(record, &route, headers, MODEL_WAIT)
            .await
            .ok()?;
        let model = if session["model"].is_null() {
            client
                .get_data::<Model>(record, "api/model/default", headers, MODEL_WAIT)
                .await
                .ok()?
        } else {
            serde_json::from_value(session["model"].clone()).ok()?
        };
        if let Some(input) = model.capabilities.and_then(|caps| caps.input) {
            return Some(input);
        }
        client
            .get_data::<Vec<Value>>(record, "api/model", headers, MODEL_WAIT)
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
