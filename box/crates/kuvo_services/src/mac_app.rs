//! The app's local registration. Browser and host consumers read this one record;
//! a desktop registration never grants browser or host authority.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use http::StatusCode;
use kuvo_protocol::{
    json::{Null, Nullable},
    routes::types::{GetKuvoBoxAppStatus200 as Status, GetKuvoBoxAppStatus200Failed as Failed},
};
use kuvo_server::{LocalOrigin, Refusal};
use kuvo_store::files;
use serde::Serialize;
use serde_json::Value;
use url::Url;

pub const ONLY_MAC: &str = "Only Kuvo on a Mac in its own folder has its app register with it.";
const SAY_TOOLS: &str = "Say where its tools answer (tools), or that it quits (tools: null).";
const APP_UPDATING: i64 = 120_000;
const OPENING: i64 = 55_000;
const FAILED_REST: i64 = 300_000;

#[derive(Clone, Serialize)]
pub struct Tools {
    pub url: String,
    pub token: String,
}

#[derive(Clone, Serialize)]
pub struct Host {
    pub url: String,
    pub password: String,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub bundle: Option<String>,
    pub desktop: Option<String>,
    pub tools: Option<Tools>,
    pub host: Option<Host>,
    pub at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desktop_version: Option<String>,
}

fn bundle(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    if text.encode_utf16().count() > 1024 || text.contains('\0') {
        return None;
    }
    let text = text.trim_end_matches('/');
    let path = Path::new(text);
    if !path.is_absolute()
        || !text.ends_with(".app")
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || path.components().collect::<PathBuf>().to_str() != Some(text)
    {
        return None;
    }
    Some(text.into())
}

fn secret(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    (16..=512)
        .contains(&text.encode_utf16().count())
        .then(|| text.into())
}

fn local_url(value: &Value) -> Option<Url> {
    let url = Url::parse(value.as_str()?).ok()?;
    (url.scheme() == "http" && url.username().is_empty() && url.password().is_none()).then_some(url)
}

fn tools(value: &Value) -> Option<Tools> {
    let url = local_url(&value["url"])?;
    matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost")).then_some(Tools {
        url: url.to_string(),
        token: secret(&value["token"])?,
    })
}

fn host(value: &Value) -> Option<Host> {
    let url = local_url(&value["url"])?;
    if url.host_str() != Some("127.0.0.1") || url.path() != "/" || url.query().is_some() {
        return None;
    }
    Some(Host {
        url: format!("http://127.0.0.1:{}", url.port()?),
        password: secret(&value["password"])?,
    })
}

fn version(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    kuvo_store::release_version::valid(text).then(|| text.into())
}

fn read_value(path: &Path) -> Option<Value> {
    serde_json::from_slice(&files::read(path).ok()??).ok()
}

pub fn read(run: &Path) -> Option<Record> {
    let value = read_value(&run.join("app.json"))?;
    value.as_object()?;
    Some(Record {
        bundle: bundle(&value["bundle"]),
        desktop: bundle(&value["desktop"]),
        tools: tools(&value["tools"]),
        host: host(&value["host"]),
        at: value["at"].as_i64().unwrap_or_default(),
        desktop_version: version(&value["desktopVersion"]),
    })
}

fn modified(path: &Path) -> Option<i64> {
    let elapsed = fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

fn opening(run: &Path, now: i64) -> bool {
    let path = run.join("app-opening.json");
    let note = read_value(&path).and_then(|value| {
        Some((
            value["at"].as_i64()?,
            u32::try_from(value["by"].as_u64()?).ok()?,
        ))
    });
    if let Some((at, by)) = note {
        let live = by == std::process::id()
            || !matches!(
                kuvo_os::process::ProcessIdentity::read(by),
                Ok(None) | Err(_)
            );
        return live && now.saturating_sub(at) <= OPENING;
    }
    fs::metadata(&path).is_ok_and(|entry| entry.is_file())
        && modified(&path).is_some_and(|at| now.saturating_sub(at) <= OPENING)
}

fn nullable<T>(value: Option<T>) -> Nullable<T> {
    value.map_or(Nullable::Null(Null), Nullable::Value)
}

pub fn status(run: &Path, now: i64) -> Status {
    let record = read(run).unwrap_or_default();
    let failed = read_value(&run.join("app-open-failed.json")).and_then(|value| {
        let at = value["at"].as_i64()?;
        let message = value["message"].as_str()?;
        (now.saturating_sub(at) < FAILED_REST).then(|| Failed {
            at: at.into(),
            message: message.into(),
        })
    });
    Status {
        bundle: nullable(record.bundle),
        desktop: Some(nullable(record.desktop)),
        open: record.tools.is_some(),
        updating: modified(&run.join("app-updating"))
            .is_some_and(|at| now.abs_diff(at) < APP_UPDATING as u64),
        opening: opening(run, now),
        failed: nullable(failed),
    }
}

/// The registered desktop may be behind the selected Kuvo release. The app
/// checks its own release; this comparison never starts an app update.
pub fn app_check(run: &Path, selected: &str) -> Option<String> {
    let app = read(run)?.desktop_version?;
    kuvo_store::release_version::newer(selected, &app).then(|| selected.to_owned())
}

/// The old Mac app's advertised version was kept in its link configuration.
/// A launch version takes precedence, as it did before the own-home move.
fn legacy_config(home: &Path) -> Option<Value> {
    let saved = read_value(&home.join("run/mac-link.json"))?;
    saved["phones"].as_bool()?;
    let relay = Url::parse(saved["relay"].as_str()?).ok()?;
    (matches!(relay.scheme(), "ws" | "wss") && saved["own"] != true).then_some(saved)
}

pub fn legacy_version(home: &Path) -> Option<String> {
    if let Some(version) = crate::config::box_env(home, "KUVO_APP_VERSION") {
        return Some(version);
    }
    legacy_config(home)?["app"]
        .as_str()
        .filter(|app| !app.is_empty())
        .map(str::to_owned)
}

/// The name supplied by the earlier Mac app, if it managed this Mac's link settings.
pub fn legacy_name(home: &Path) -> Option<String> {
    legacy_config(home)?["name"].as_str().map(str::to_owned)
}

enum Registration {
    Updating,
    Desktop {
        desktop: String,
        version: Option<String>,
    },
    Browser {
        bundle: Option<String>,
        tools: Option<Tools>,
        host: Option<Host>,
    },
}

fn parse(value: &Value, origin: LocalOrigin) -> Result<Registration, Refusal> {
    let bad = |message: &str| Refusal::new(StatusCode::BAD_REQUEST, message);
    let forbidden = |message: &str| Refusal::new(StatusCode::FORBIDDEN, message);
    if !matches!(origin, LocalOrigin::App | LocalOrigin::Mac) {
        return Err(forbidden("Only Kuvo Code on this Mac registers its app."));
    }
    let fields = value.as_object().ok_or_else(|| bad(SAY_TOOLS))?;
    if value["updating"] == true {
        return Ok(Registration::Updating);
    }
    if fields.contains_key("desktop") && !fields.contains_key("tools") {
        if origin != LocalOrigin::App {
            return Err(forbidden("Only Kuvo itself says where its desktop app is."));
        }
        return Ok(Registration::Desktop {
            desktop: bundle(&value["desktop"]).ok_or_else(|| {
                bad("Say where the desktop app is: a whole path to it (…/Kuvo.app).")
            })?,
            version: version(&value["version"]),
        });
    }
    let tool_value = fields.get("tools").ok_or_else(|| bad(SAY_TOOLS))?;
    let tools = if tool_value.is_null() {
        None
    } else {
        Some(tools(tool_value).ok_or_else(|| {
            bad("Its tools answer on this Mac, at an http address, with a token.")
        })?)
    };
    let bundle = fields
        .get("bundle")
        .map(|value| {
            bundle(value)
                .ok_or_else(|| bad("Its bundle is a whole path to the app (…/Kuvo Code.app)."))
        })
        .transpose()?;
    let host = if value["host"].is_null() {
        None
    } else {
        if origin != LocalOrigin::App {
            return Err(forbidden(
                "Only Kuvo Code itself says where its own host answers.",
            ));
        }
        Some(host(&value["host"]).ok_or_else(|| {
            bad("Its host answers on this Mac (http://127.0.0.1:<port>), with a password.")
        })?)
    };
    Ok(Registration::Browser {
        bundle,
        tools,
        host,
    })
}

pub fn register(run: &Path, value: &Value, origin: LocalOrigin, now: i64) -> Result<(), Refusal> {
    let given = parse(value, origin)?;
    save(run, given, now)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn save(run: &Path, given: Registration, now: i64) -> io::Result<()> {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(run.join(".app-registration.lock"))?;
    lock.lock()?;
    let mut record = read(run).unwrap_or(Record {
        at: now,
        ..Record::default()
    });
    let updating = matches!(given, Registration::Updating);
    let opened = matches!(&given, Registration::Browser { tools: Some(_), .. });
    match given {
        Registration::Updating => {
            record.tools = None;
            record.host = None;
            record.at = now;
        }
        Registration::Desktop { desktop, version } => {
            record.desktop = Some(desktop);
            record.desktop_version = version;
        }
        Registration::Browser {
            bundle,
            tools,
            host,
        } => {
            if let Some(bundle) = bundle {
                record.bundle = Some(bundle);
            }
            record.host = tools.as_ref().and(host);
            record.tools = tools;
            record.at = now;
        }
    }
    files::write_secret(&run.join("app.json"), &serde_json::to_vec(&record)?)?;
    if updating {
        files::write_secret(&run.join("app-updating"), now.to_string().as_bytes())?;
    } else if opened {
        files::remove_file_synced(&run.join("app-open-failed.json"))?;
        files::remove_file_synced(&run.join("app-updating"))?;
    }
    Ok(())
}
