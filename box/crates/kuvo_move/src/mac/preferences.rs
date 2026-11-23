use super::invalid;
use crate::{Disk, Error, json_at, metadata, save_json};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat, check_version},
};
use serde_json::{Value, json};
use std::{collections::HashSet, io::Cursor, path::Path};

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
pub(super) fn take(source: &Path, stage: &Disk) -> Result<(), Error> {
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
    git(stage, &given)?;
    box_preferences(stage, &given)
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

fn git(stage: &Disk, prefs: &Fields) -> Result<(), Error> {
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
        return Ok(());
    }
    let record = stage.path("box/git.json")?;
    let mut kept = GIT
        .read(&record)?
        .map(|record| record.fields)
        .unwrap_or_default();
    if kept.get("moved").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let settings = stage.path("config/opencode/opencode.json")?;
    let before = files::read(&settings)?.unwrap_or_default();
    let Some(mut fields) = (if before.iter().all(u8::is_ascii_whitespace) {
        Some(Fields::new())
    } else {
        serde_json::from_slice(&before).ok()
    }) else {
        // An uneditable engine file stays as the user kept it.
        return Ok(());
    };
    let original = fields.len();
    for (field, value) in offered {
        fields.entry(field).or_insert_with(|| value.into());
    }
    if fields.len() != original {
        pretty(&settings, &Value::Object(fields), true)?;
    }
    kept.insert("moved".into(), true.into());
    save_json(&record, &GIT.encode(kept))
}

fn box_preferences(stage: &Disk, prefs: &Fields) -> Result<(), Error> {
    if prefs.get("keepsAwake").and_then(Value::as_bool) == Some(false) {
        let file = stage.path("box/work.json")?;
        let mut fields: Fields = json_at(&file)?.unwrap_or_default();
        fields.insert("keepsAwake".into(), false.into());
        pretty(&file, &fields.into(), false)?;
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
