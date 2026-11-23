use super::invalid;
use crate::{Disk, Error, entries, json_at, metadata, save_json};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat, check_version},
};
use serde_json::{Value, json};
use std::{collections::HashSet, io::Cursor, path::Path, process::Command};

const GIT: ObjectFormat = ObjectFormat::additive("Git preferences migration record");
const WORKTREES: ObjectFormat = ObjectFormat::additive("Worktree records");

pub(super) fn preference_path(source: &Path) -> Result<std::path::PathBuf, Error> {
    let id = source
        .file_name()
        .ok_or_else(|| invalid("The app has no identifier."))?;
    let library = source
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| invalid("The app has no Library folder."))?;
    let mut name = id.to_os_string();
    name.push(".plist");
    Ok(library.join("Preferences").join(name))
}

/// Project app-owned preferences into the same files the box opens at startup.
pub(super) fn take(source: &Path, stage: &Disk, account: &Path, home: &Path) -> Result<(), Error> {
    let path = preference_path(source)?;
    let given = files::read(&path)?
        .map(|bytes| {
            let pref =
                plist::Value::from_reader(Cursor::new(bytes)).map_err(std::io::Error::other)?;
            as_json(pref)?
                .as_object()
                .cloned()
                .ok_or_else(|| invalid("The app's property list must hold a dictionary."))
        })
        .transpose()?
        .unwrap_or_default();
    projects(source, stage, &given)?;
    worktrees(stage, &given)?;
    machines(stage, &given)?;
    engine_settings(stage, &given, account, home)?;
    box_preferences(stage, &given)?;
    mac_link(stage, &given, home)?;
    personalities(source, stage)
}

fn mac_link(stage: &Disk, prefs: &Fields, home: &Path) -> Result<(), Error> {
    const DEFAULT_RELAY: &str = "wss://kuvolink.porigam.com.tr";
    let phones = prefs.get("phoneLink").and_then(Value::as_bool);
    let relay = prefs
        .get("phoneLink.relay")
        .and_then(Value::as_str)
        .map(|text| text.trim_end_matches('/'))
        .filter(|text| url::Url::parse(text).is_ok_and(|url| matches!(url.scheme(), "ws" | "wss")));
    if phones.is_none() && relay.is_none() {
        return Ok(());
    }
    let file = stage.path("run/mac-link.json")?;
    if metadata(&file)?.is_some() {
        return Ok(());
    }
    if relay.is_none() {
        let real = kuvo_os::platform::Account::current()?
            .home
            .join(".kuvo-code");
        if real.canonicalize().ok().as_deref() != Some(home.canonicalize()?.as_path()) {
            return Ok(());
        }
    }
    let output = Command::new("/usr/sbin/scutil")
        .env_clear()
        .args(["--get", "ComputerName"])
        .output()?;
    let name = String::from_utf8_lossy(&output.stdout);
    let name: String = name.trim().chars().take(100).collect();
    save_json(
        &file,
        &json!({"phones": phones.unwrap_or(true), "relay": relay.unwrap_or(DEFAULT_RELAY),
        "name": if output.status.success() && !name.is_empty() { &name } else { "Mac" }, "app": "" }),
    )
}

fn personalities(source: &Path, stage: &Disk) -> Result<(), Error> {
    let read = |name| -> Result<Option<Vec<Value>>, Error> {
        match json_at::<Value>(&source.join(name))? {
            Some(value @ Value::Array(_)) => Ok(Some(kuvo_store::format::read_additive_list(
                "Legacy Mac import list",
                value,
            )?)),
            _ => Ok(None),
        }
    };
    let (made, removed) = (
        read("personalities.json")?,
        read("personalities-removed.json")?,
    );
    if made.is_none() && removed.is_none() {
        return Ok(());
    }
    let file = stage.path("bots/personalities.json")?;
    let kept: Value = json_at(&file)?.unwrap_or_default();
    check_version("Personality library", kept.get("kuvo"), 0, 1)?;
    if let Some(next) = kuvo_setup::project_app_personalities(
        &kept,
        made.as_deref().unwrap_or_default(),
        removed.as_deref().unwrap_or_default(),
    ) {
        pretty(&file, &next, true)?;
    }
    Ok(())
}

fn as_json(value: plist::Value) -> Result<Value, Error> {
    Ok(match value {
        plist::Value::Dictionary(dict) => Value::Object(
            dict.into_iter()
                .map(|(key, value)| Ok((key, as_json(value)?)))
                .collect::<Result<_, Error>>()?,
        ),
        plist::Value::Array(array) => {
            Value::Array(array.into_iter().map(as_json).collect::<Result<_, _>>()?)
        }
        plist::Value::Boolean(value) => value.into(),
        plist::Value::Integer(value) => value
            .as_signed()
            .map(Value::from)
            .or_else(|| value.as_unsigned().map(Value::from))
            .unwrap_or(Value::Null),
        plist::Value::Real(value) => value.into(),
        plist::Value::String(value) => value.into(),
        plist::Value::Data(value) => serde_json::from_slice(&value)
            .unwrap_or_else(|_| String::from_utf8_lossy(&value).into_owned().into()),
        _ => Value::Null,
    })
}

fn objects(value: Option<&Value>) -> impl Iterator<Item = (&String, &Value)> {
    value.and_then(Value::as_object).into_iter().flatten()
}

fn list(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn path(value: &Value) -> Option<&str> {
    value.as_str().filter(|path| Path::new(path).is_absolute())
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value
        .get(field)
        .and_then(Value::as_str)
        .map_or("", str::trim)
}

fn projects(source: &Path, stage: &Disk, prefs: &Fields) -> Result<(), Error> {
    let order: Vec<_> = list(prefs.get("projectFolders"))
        .iter()
        .filter_map(path)
        .collect();
    let settings = prefs.get("projectSettings");
    let mut seen = HashSet::new();
    let offered: Vec<_> = order
        .iter()
        .copied()
        .chain(objects(settings).map(|(directory, _)| directory.as_str()))
        .filter(|directory| {
            Path::new(directory).is_absolute()
                && !Path::new(directory).starts_with(source.join("bots"))
                && !directory.contains("/.kuvo-code/bots/")
                && seen.insert(*directory)
        })
        .collect();
    if offered.is_empty() {
        return Ok(());
    }
    let file = stage.path("box/projects.json")?;
    let kept: Value = json_at(&file)?.unwrap_or_default();
    check_version("Project list", kept.get("version"), 0, 1)?;
    let mut projects = list(kept.get("projects")).to_vec();
    for directory in offered {
        if projects
            .iter()
            .any(|entry| entry.get("directory").and_then(Value::as_str) == Some(directory))
        {
            continue;
        }
        let given = settings
            .and_then(|settings| settings.get(directory))
            .unwrap_or(&Value::Null);
        let mut record = Fields::new();
        record.insert("directory".into(), directory.into());
        let name = text(given, "name");
        if !name.is_empty()
            && Path::new(directory)
                .file_name()
                .and_then(|part| part.to_str())
                != Some(name)
        {
            record.insert("name".into(), name.into());
        }
        let mut added = HashSet::new();
        record.insert(
            "folders".into(),
            json!(
                list(given.get("folders"))
                    .iter()
                    .filter_map(path)
                    .filter(|folder| *folder != directory && added.insert(*folder))
                    .collect::<Vec<_>>()
            ),
        );
        let run = text(given, "run");
        if !run.is_empty() {
            record.insert("run".into(), run.into());
        }
        if !order.contains(&directory) {
            record.insert("removed".into(), true.into());
        }
        projects.push(Value::Object(record));
    }
    pretty(&file, &json!({"version":1,"projects":projects}), false)
}

fn worktrees(stage: &Disk, prefs: &Fields) -> Result<(), Error> {
    let offered = prefs.get("worktrees");
    let removed = prefs.get("removedWorktrees");
    if objects(offered).next().is_none() && objects(removed).next().is_none() {
        return Ok(());
    }
    let file = stage.path("box/worktrees.json")?;
    let kept = WORKTREES
        .read(&file)?
        .map(|record| record.fields)
        .unwrap_or_default();
    let mut records = list(kept.get("worktrees")).to_vec();
    let gone = kept
        .get("removed")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut handed_over = kept
        .get("handedOver")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut ordered: Vec<_> = objects(offered).collect();
    ordered.sort_by_key(|(folder, _)| *folder);
    for (folder, record) in ordered {
        let Some(project) = record.get("project").and_then(path) else {
            continue;
        };
        let Some(root) = record.get("root").and_then(path) else {
            continue;
        };
        let Some(branch) = record
            .get("branch")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        if !Path::new(folder).is_absolute()
            || !Path::new(root).is_dir()
            || records
                .iter()
                .any(|entry| entry.get("root").and_then(Value::as_str) == Some(root))
        {
            continue;
        }
        let mut next =
            json!({ "project": project, "root": root, "folder": folder, "branch": branch });
        if let Some(base) = record
            .get("base")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            next["base"] = base.into();
        }
        records.push(next);
    }
    let mut ordered: Vec<_> = objects(removed).collect();
    ordered.sort_by_key(|(folder, _)| *folder);
    for (folder, project) in ordered {
        let Some(project) = path(project) else {
            continue;
        };
        if !Path::new(folder).is_absolute()
            || folder == project
            || gone.contains_key(folder)
            || records.iter().any(|entry| {
                ["root", "folder"]
                    .iter()
                    .any(|field| entry.get(field).and_then(Value::as_str) == Some(folder))
            })
        {
            continue;
        }
        handed_over
            .entry(folder.clone())
            .or_insert_with(|| project.into());
    }
    let mut fields = json!({
        "worktrees": records, "removed": gone, "restorable": list(kept.get("restorable")),
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    if !handed_over.is_empty() {
        fields.insert("handedOver".into(), handed_over.into());
    }
    pretty(&file, &WORKTREES.encode(fields), false)
}

fn machines(stage: &Disk, prefs: &Fields) -> Result<(), Error> {
    let file = stage.path("machines.json")?;
    let existing: Option<Vec<Value>> = json_at(&file)?;
    let mut changed = existing.is_none();
    let mut machines = existing.unwrap_or_default();
    for input in ["machines.unsaved", "machines"] {
        for record in list(prefs.get(input)) {
            let Some(destination) = record
                .get("destination")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            if machines.iter().any(|machine| {
                machine.get("destination").and_then(Value::as_str) == Some(destination)
            }) {
                continue;
            }
            let mut machine = json!({ "destination": destination });
            let other: Vec<_> = list(record.get("otherAddresses"))
                .iter()
                .filter_map(Value::as_str)
                .collect();
            if !other.is_empty() {
                machine["otherAddresses"] = json!(other);
            }
            for name in ["identity", "reachedAt"] {
                if let Some(value) = record
                    .get(name)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    machine[name] = value.into();
                }
            }
            machines.push(machine);
            changed = true;
        }
    }
    if changed {
        save_json(&file, &machines)?;
    }
    Ok(())
}

fn engine_settings(stage: &Disk, prefs: &Fields, account: &Path, home: &Path) -> Result<(), Error> {
    let settings = stage.path("config/opencode/opencode.json")?;
    let before = files::read(&settings)?.unwrap_or_default();
    let Some(fields) = (if before.iter().all(u8::is_ascii_whitespace) {
        Some(Fields::new())
    } else {
        serde_json::from_slice::<Fields>(&before).ok()
    }) else {
        // An uneditable engine file stays as the user kept it.
        return Ok(());
    };
    let shown = home
        .strip_prefix(account)
        .ok()
        .filter(|rest| !rest.as_os_str().is_empty())
        .map_or_else(|| home.to_path_buf(), |rest| Path::new("~").join(rest));
    let keys = shown.join("box/keys");
    let mut value = Value::Object(fields);
    let mut changed = name_keys(&mut value, &keys);
    if let Value::Object(fields) = &mut value {
        changed |= take_git(stage, prefs, fields)?;
    }
    if changed {
        pretty(&settings, &value, true)?;
    }
    for bot in entries(&stage.path("bots")?)? {
        if !bot.file_type()?.is_dir() {
            continue;
        }
        let relative = Path::new("bots")
            .join(bot.file_name())
            .join("workspace/.opencode/opencode.json");
        let file = stage.path(&relative)?;
        if let Some(before) = files::read(&file)?
            && let Ok(Value::Object(fields)) = serde_json::from_slice(&before)
        {
            let mut value = Value::Object(fields);
            if name_keys(&mut value, &keys) {
                pretty(&file, &value, true)?;
            }
        }
    }
    Ok(())
}

fn name_keys(value: &mut Value, keys: &Path) -> bool {
    match value {
        Value::Object(fields) => fields
            .values_mut()
            .fold(false, |changed, field| name_keys(field, keys) | changed),
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, field| name_keys(field, keys) | changed),
        Value::String(text) => {
            let mut output = String::with_capacity(text.len());
            for segment in text.split_inclusive('}') {
                let marker = segment
                    .strip_suffix('}')
                    .and_then(|text| text.rsplit_once('{'));
                let Some((before, key)) = marker.and_then(|(before, marker)| {
                    let value = marker
                        .strip_prefix("env:")
                        .or_else(|| marker.strip_prefix("file:"))?;
                    let key = value.match_indices("KUVO_KEY_").find_map(|(at, label)| {
                        let suffix = &value[at + label.len()..];
                        (!suffix.is_empty()
                            && suffix
                                .bytes()
                                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_'))
                        .then_some(&value[at..])
                    })?;
                    Some((before, key))
                }) else {
                    output.push_str(segment);
                    continue;
                };
                output.push_str(before);
                output.push_str("{file:");
                output.push_str(&keys.join(key).to_string_lossy());
                output.push('}');
            }
            if output == *text {
                return false;
            }
            *text = output;
            true
        }
        _ => false,
    }
}

fn take_git(stage: &Disk, prefs: &Fields, fields: &mut Fields) -> Result<bool, Error> {
    let offered: Vec<_> = [
        ("git.branchPrefix", "branch_prefix"),
        ("git.commitInstructions", "commit_instructions"),
        ("git.pullRequestInstructions", "pull_request_instructions"),
    ]
    .into_iter()
    .filter_map(|(old, field)| {
        let value = prefs.get(old)?.as_str()?;
        let value = if field == "branch_prefix" {
            value
        } else {
            value.trim()
        };
        (field == "branch_prefix" || !value.is_empty()).then_some((field, value))
    })
    .collect();
    if offered.is_empty() {
        return Ok(false);
    }
    let record = stage.path("box/git.json")?;
    let mut kept = GIT
        .read(&record)?
        .map(|record| record.fields)
        .unwrap_or_default();
    if kept.get("moved").and_then(Value::as_bool) == Some(true) {
        return Ok(false);
    }
    let original = fields.len();
    for (field, value) in offered {
        fields.entry(field).or_insert_with(|| value.into());
    }
    kept.insert("moved".into(), true.into());
    save_json(&record, &GIT.encode(kept))?;
    Ok(fields.len() != original)
}

fn box_preferences(stage: &Disk, prefs: &Fields) -> Result<(), Error> {
    if prefs.get("keepsAwake").and_then(Value::as_bool) == Some(false) {
        let file = stage.path("box/work.json")?;
        let mut fields: Fields = json_at(&file)?.unwrap_or_default();
        fields.insert("keepsAwake".into(), false.into());
        pretty(&file, &fields.into(), false)?;
    }
    if let Some(keys) = prefs.get("favoriteModels").and_then(Value::as_str) {
        let mut seen = HashSet::new();
        let stars: Vec<_> = keys
            .split(',')
            .filter_map(|key| key.split_once('/'))
            .filter(|(provider, id)| !provider.is_empty() && !id.is_empty())
            .filter(|pair| seen.insert(*pair))
            .map(|(provider, id)| json!({"providerID":provider,"id":id}))
            .collect();
        if !stars.is_empty() {
            let file = stage.path("box/models.json")?;
            let mut fields: Fields = json_at(&file)?.unwrap_or_default();
            if list(fields.get("starred")).is_empty() {
                let last = stars.len().saturating_sub(200);
                fields.insert("starred".into(), json!(&stars[last..]));
                pretty(&file, &fields.into(), false)?;
            }
        }
    }
    if prefs.get("toolApproval").and_then(Value::as_str) == Some("askFirst") {
        let file = stage.path("box/approval.json")?;
        let mut fields: Fields = json_at(&file)?.unwrap_or_default();
        if !fields.get("last").is_some_and(Value::is_string) {
            fields.insert("last".into(), "askFirst".into());
            pretty(&file, &fields.into(), false)?;
        }
    }
    let marks: Vec<_> = [
        ("engine.defaultsAdded", "websearch"),
        ("engine.searchMovedToExa", "search-exa"),
        ("engine.botMemoryRuleAdded", "bot-memory"),
    ]
    .into_iter()
    .filter(|(pref, _)| prefs.get(*pref).and_then(Value::as_bool) == Some(true))
    .map(|(_, mark)| mark)
    .collect();
    if !marks.is_empty() {
        let file = stage.path("box/defaults-added")?;
        let before = files::read(&file)?.unwrap_or_default();
        let text = String::from_utf8_lossy(&before);
        let mut added = HashSet::new();
        let next = text
            .lines()
            .chain(marks)
            .filter(|mark| !mark.is_empty() && added.insert(*mark))
            .collect::<Vec<_>>()
            .join("\n");
        files::write_atomic(&file, format!("{next}\n").as_bytes())?;
    }
    Ok(())
}

fn pretty(file: &Path, value: &Value, newline: bool) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    if newline {
        bytes.push(b'\n');
    }
    if metadata(file)?.is_some_and(|meta| !meta.is_file()) {
        return Err(Error::Busy(file.into()));
    }
    files::write_atomic(file, &bytes)?;
    Ok(())
}
