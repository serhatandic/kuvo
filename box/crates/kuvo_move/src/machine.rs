use crate::{Disk, Error, Step, entries, json_at, metadata, save_json};
use base64::{Engine, engine::general_purpose::STANDARD};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{io, path::Path};

pub(crate) mod settings;

const LINK: ObjectFormat = ObjectFormat::additive("Link state");
const EXISTING_MACHINE: &[&str] = &[
    "run/link.json",
    "run/engine.json",
    "run/launch.env",
    "bin/kuvo-start",
    "config/opencode/opencode.json",
    "box/opencode.json",
    "keys/settings.json",
    "data/opencode/opencode.db",
];

pub(super) fn recognized(disk: &Disk) -> Result<bool, Error> {
    let kind = files::read(&disk.path("box/kind")?)?;
    if let Some(kind) = kind {
        return Ok(String::from_utf8_lossy(&kind).trim() == "machine");
    }
    for marker in EXISTING_MACHINE {
        if metadata(&disk.path(marker)?)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

// Each step can derive its result from the source bytes or from what the prior
// attempt placed. The layout record is the only completion checkpoint.
pub(super) fn run(
    disk: &Disk,
    account_home: Option<&Path>,
    completed: &mut impl FnMut(Step),
) -> Result<(), Error> {
    // Refuse unknown link formats before changing any state.
    let link_path = disk.path("run/link.json")?;
    let link = LINK.read(&link_path)?;
    settings::take(disk, account_home)?;
    completed(Step::Settings);
    kit(disk)?;
    completed(Step::Kit);
    if let Some(link) = link
        && link.version == 0
        && let Some(projected) = legacy_link(link.fields)
    {
        save_json(&link_path, &LINK.encode(projected))?;
    }
    completed(Step::Link);
    outbox(disk)?;
    completed(Step::Outbox);
    pages(disk)?;
    completed(Step::Pages);
    agents(disk)?;
    completed(Step::Agents);
    files::remove_file_synced(&disk.path("run/mac-agents.json")?)?;
    for name in ["run/connected", "run/present"] {
        files::remove_tree_synced(&disk.path(name)?)?;
    }
    completed(Step::Presence);
    Ok(())
}

// Project a stored beta.1 list. Accepting later mail and deciding where that
// Mac's peers are reachable belongs to the served compatibility path (7.3).
fn legacy_link(mut link: Fields) -> Option<Fields> {
    if ["peersFrom", "vouched", "macAts"]
        .iter()
        .all(|field| !link.contains_key(*field))
    {
        return None;
    }
    let offered = link.remove("peersFrom");
    link.remove("vouched");
    link.remove("macAts");
    if let Some(Value::Object(offered)) = offered {
        let signs = link.get("macSigns").and_then(Value::as_object);
        let projected = offered
            .into_iter()
            .filter(|(mac, _)| {
                !signs
                    .and_then(|signs| signs.get(mac))
                    .and_then(Value::as_str)
                    .is_some_and(base64url_key)
            })
            .filter_map(|(mac, record)| {
                let given = record.as_object()?;
                Some((
                    mac,
                    json!({
                        "at": given.get("at").filter(|v| !v.is_null()).cloned().unwrap_or(json!(0)),
                        "peers": given.get("peers").filter(|v| v.is_array()).cloned().unwrap_or(json!([])),
                        "moved": given.get("moved").filter(|v| !v.is_null()).cloned().unwrap_or(json!([])),
                    }),
                ))
            })
            .collect::<Fields>();
        if !projected.is_empty() {
            let legacy = link.entry("legacy").or_insert_with(|| json!({}));
            if legacy.is_null() {
                *legacy = json!({});
            }
            if let Some(legacy) = legacy.as_object_mut() {
                for (mac, record) in projected {
                    let kept = legacy.entry(mac).or_insert(Value::Null);
                    if kept.is_null() {
                        *kept = record;
                    }
                }
            }
        }
    }
    Some(link)
}

fn base64url_key(word: &str) -> bool {
    word.len() == 43
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn outbox(disk: &Disk) -> Result<(), Error> {
    let directory = disk.path("run/outbox")?;
    if !metadata(&directory)?.is_some_and(|meta| meta.is_dir()) {
        return Ok(());
    }
    for entry in entries(&directory)? {
        let name = entry.file_name();
        let wire_name = name.to_string_lossy();
        if !wire_name.starts_with('.')
            && wire_name.ends_with(".json")
            && entry.file_type()?.is_file()
        {
            let target = disk.path(Path::new("run/outbox/sending").join(name))?;
            files::create_private_dirs(target.parent().expect("outbox file has a parent"))?;
            files::replace_synced(&entry.path(), &target)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct Page<'a> {
    url: &'a str,
    session: Option<&'a str>,
}

fn pages(disk: &Disk) -> Result<(), Error> {
    let file = disk.path("run/pages.json")?;
    let Some(Value::Array(entries)) = json_at(&file)? else {
        return Ok(());
    };
    if !entries.iter().any(Value::is_string) {
        return Ok(());
    }
    let pages: Vec<_> = entries
        .iter()
        .filter_map(|value| {
            let url = value.as_str().or_else(|| value.get("url")?.as_str())?;
            ["https:", "http:", "file:"]
                .iter()
                .any(|prefix| url.starts_with(prefix))
                .then(|| Page {
                    url,
                    session: value.get("session").and_then(Value::as_str),
                })
        })
        .take(8)
        .collect();
    save_json(&file, &pages)
}

fn agents(disk: &Disk) -> Result<(), Error> {
    for retired in [
        "agents/runtime/codex",
        "agents/runtime/grok",
        "agents/cache/install",
    ] {
        files::remove_tree_synced(&disk.path(retired)?)?;
    }
    let scope = "agents/runtime/claude/node_modules/@anthropic-ai";
    let directory = disk.path(scope)?;
    if metadata(&directory)?.is_some_and(|meta| meta.is_dir()) {
        for entry in entries(&directory)? {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("claude-agent-sdk-linux-")
            {
                files::remove_tree_synced(&entry.path())?;
            }
        }
    }
    let root = disk.path("agents")?;
    if metadata(&root)?.is_some_and(|meta| meta.is_dir()) {
        retire_carried_sign_ins(&root, 0)?;
    }
    Ok(())
}

fn retire_carried_sign_ins(folder: &Path, depth: u8) -> io::Result<()> {
    for entry in entries(folder)? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(sign_in) = name
            .strip_suffix(".from-mac")
            .filter(|name| !name.is_empty())
        {
            files::remove_file_synced(&folder.join(sign_in))?;
            files::remove_file_synced(&entry.path())?;
        } else if depth < 5
            && !matches!(name.as_ref(), "runtime" | "sessions" | "snapshots")
            && entry.file_type()?.is_dir()
        {
            retire_carried_sign_ins(&entry.path(), depth + 1)?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct Kit {
    kuvo: u32,
    time_zone: Option<String>,
    zoneinfo: Option<String>,
    projects: Vec<Value>,
    personalities: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CarriedZone<'a> {
    kuvo: u8,
    zone: &'a str,
    n: u64,
    carried: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    kit_seen: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    zoneinfo: Option<String>,
}

fn kit(disk: &Disk) -> Result<(), Error> {
    let Some(kit) = json_at::<Value>(&disk.path("bots/kit.json")?)? else {
        return carry_zone(disk, Kit::default());
    };
    if kit.get("kuvo").and_then(Value::as_u64) != Some(1) {
        return carry_zone(disk, Kit::default());
    }
    let kit = Kit {
        kuvo: 1,
        time_zone: kit
            .get("timeZone")
            .and_then(Value::as_str)
            .map(str::to_owned),
        zoneinfo: kit
            .get("zoneinfo")
            .and_then(Value::as_str)
            .map(str::to_owned),
        projects: kit
            .get("projects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        personalities: kit
            .get("personalities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    carry_projects(disk, &kit.projects)?;
    carry_personalities(disk, &kit.personalities)?;
    carry_zone(disk, kit)
}

#[derive(Serialize)]
struct Project<'a> {
    directory: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    folders: [&'a str; 0],
}

fn carry_projects(disk: &Disk, offered: &[Value]) -> Result<(), Error> {
    let file = disk.path("box/projects.json")?;
    if metadata(&file)?.is_some() {
        return Ok(());
    }
    let projects: Vec<_> = offered
        .iter()
        .filter_map(|project| {
            let directory = project.get("directory")?.as_str()?;
            let path = Path::new(directory);
            if !path.is_absolute() {
                return None;
            }
            let name = project.get("name").and_then(Value::as_str).filter(|name| {
                !name.is_empty() && path.file_name().and_then(|part| part.to_str()) != Some(name)
            });
            Some(Project {
                directory,
                name,
                folders: [],
            })
        })
        .collect();
    if !projects.is_empty() {
        save_json(&file, &json!({ "version": 1, "projects": projects }))?;
    }
    Ok(())
}

fn carry_personalities(disk: &Disk, offered: &[Value]) -> Result<(), Error> {
    if offered.is_empty() {
        return Ok(());
    }
    let path = disk.path("bots/personalities.json")?;
    let kept: Value = json_at(&path)?.unwrap_or(Value::Null);
    kuvo_store::format::check_version("Personality library", kept.get("kuvo"), 0, 1)?;
    if let Some(projected) = kuvo_setup::project_kit_personalities(&kept, offered) {
        save_json(&path, &projected)?;
    }
    Ok(())
}

fn carry_zone(disk: &Disk, kit: Kit) -> Result<(), Error> {
    let output = disk.path("box/time-zone.json")?;
    if metadata(&output)?.is_some() {
        return Ok(());
    }
    let known = |value: &str| jiff::tz::TimeZone::get(value).is_ok();
    let wakes = files::read(&disk.path("wakes/timezone")?)?.unwrap_or_default();
    let wakes = String::from_utf8_lossy(&wakes);
    let launch = files::read(&disk.path("run/launch.env")?)?.unwrap_or_default();
    let launch = String::from_utf8_lossy(&launch);
    let from_kit = kit
        .time_zone
        .as_deref()
        .filter(|name| kit.kuvo == 1 && known(name));
    let (zone, rules) = if let Some(name) = from_kit {
        let rules = kit.zoneinfo.and_then(|rules| {
            if rules.len() > 22_000 {
                return None;
            }
            let bytes = STANDARD.decode(rules).ok()?;
            (bytes.starts_with(b"TZif") && (5..16_384).contains(&bytes.len()))
                .then(|| STANDARD.encode(bytes))
        });
        (name, rules)
    } else if known(wakes.trim()) {
        let rules = files::read(&disk.path("wakes/zoneinfo")?)?
            .filter(|rules| !rules.is_empty())
            .map(|rules| STANDARD.encode(rules));
        (wakes.trim(), rules)
    } else if let Some(name) = launch
        .lines()
        .find_map(|line| line.strip_prefix("KUVO_TZ="))
        .map(str::trim)
        .filter(|name| known(name))
    {
        (name, None)
    } else {
        return Ok(());
    };
    save_json(
        &output,
        &CarriedZone {
            kuvo: 1,
            zone,
            n: 0,
            carried: true,
            kit_seen: from_kit,
            zoneinfo: rules,
        },
    )
}
