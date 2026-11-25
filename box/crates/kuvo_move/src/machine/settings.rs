use crate::{Disk, Error, entries, metadata, save_json};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
};
use serde_json::{Value, json};
use std::path::Path;

const GIT: ObjectFormat = ObjectFormat::additive("Git preferences migration record");

pub(super) fn take(disk: &Disk, account_home: Option<&Path>) -> Result<(), Error> {
    let git_path = disk.path("box/git.json")?;
    let git = GIT.read(&git_path)?;
    let saved = disk.path("box/opencode.json")?;
    let global = disk.path("config/opencode/opencode.json")?;
    let legacy = disk.path("keys/settings.json")?;
    let keys = disk.path("keys")?;
    let shown = account_home
        .and_then(|home| disk.0.strip_prefix(home).ok())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| disk.0.display().to_string());
    let launch = disk.path("run/launch.env")?;
    let eligible = metadata(&global)?.is_some()
        || metadata(&keys)?.is_some()
        || files::text_of(&launch).is_some_and(|launch| launch.lines().any(settings_line));
    let mut current = settings_at(&saved)?;
    if matches!(current, Settings::Missing) && eligible {
        current = Settings::Plain(adopt(disk, &saved, &global, &legacy, &shown, None)?.0);
    }
    if eligible && let Settings::Plain(settings) = &mut current {
        default_permissions(disk, &saved, settings)?;
        clear_launch_settings(&launch)?;
    }
    if let Some(mut git) = git
        && let Some(prefix) = git.fields.remove("branchPrefix")
        && let Settings::Plain(mut settings) = current
    {
        if !settings.contains_key("branch_prefix")
            && let Value::String(prefix) = prefix
        {
            settings.insert("branch_prefix".into(), prefix.into());
            save_settings(&saved, &settings)?;
        }
        save_json(&git_path, &GIT.encode(git.fields))?;
    }
    Ok(())
}

enum Settings {
    Missing,
    Plain(Fields),
    Unreadable,
}

impl Settings {
    fn into_fields(self) -> Fields {
        match self {
            Self::Plain(fields) => fields,
            _ => Fields::new(),
        }
    }
}

fn settings_at(file: &Path) -> Result<Settings, Error> {
    let Some(bytes) = files::read(file)? else {
        return Ok(if metadata(file)?.is_some() {
            Settings::Unreadable
        } else {
            Settings::Missing
        });
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Settings::Plain(Fields::new()));
    }
    Ok(match serde_json::from_slice(&bytes) {
        Ok(Value::Object(fields)) => Settings::Plain(fields),
        _ => Settings::Unreadable,
    })
}

fn adopt(
    disk: &Disk,
    saved: &Path,
    global: &Path,
    legacy: &Path,
    shown: &str,
    seed: Option<&Value>,
) -> Result<(Fields, bool), Error> {
    let global_settings = settings_at(global)?;
    let global_readable = !matches!(global_settings, Settings::Unreadable);
    let mut merged = global_settings.into_fields();
    merge(&mut merged, settings_at(legacy)?.into_fields());
    let seeded = merged.is_empty()
        && seed
            .and_then(|value| value.get("settings"))
            .is_some_and(Value::is_object);
    if seeded {
        merged = seed
            .and_then(|value| value["settings"].as_object())
            .cloned()
            .unwrap_or_default();
    }
    let root = disk.0.to_string_lossy();
    for value in merged.values_mut() {
        key_paths(value, &root, shown);
    }

    files::create_private_dirs(&disk.path("box/keys")?)?;
    let keys = disk.path("keys")?;
    if seeded {
        if let Some(keys) = seed.and_then(|value| value["keys"].as_object()) {
            for (name, value) in keys {
                if valid_key(name)
                    && let Some(value) = value.as_str()
                {
                    files::write_secret(
                        &disk.path(Path::new("box/keys").join(name))?,
                        value.as_bytes(),
                    )?;
                }
            }
        }
    } else if metadata(&keys)?.is_some_and(|meta| meta.is_dir()) {
        for entry in entries(&keys)? {
            let name = entry.file_name();
            let kept = name.to_string_lossy();
            if valid_key(&kept)
                && !kept.contains("__")
                && entry.file_type()?.is_file()
                && let Some(bytes) = files::read(&entry.path())?
            {
                files::write_secret(&disk.path(Path::new("box/keys").join(name))?, &bytes)?;
            }
        }
    }
    for (source, target) in [
        (global, "box/adopted-from/opencode.json"),
        (legacy, "box/adopted-from/settings.json"),
    ] {
        if let Some(bytes) = files::read(source)? {
            files::write_secret(&disk.path(target)?, &bytes)?;
        }
    }
    if global_readable {
        files::write_atomic(&disk.path("box/global-adopted")?, b"")?;
    }
    // Publish the settings before retiring the old launch overrides. The
    // caller also retires them on replay once the settings exist.
    save_settings(saved, &merged)?;
    Ok((merged, seeded))
}

fn valid_key(name: &str) -> bool {
    name.strip_prefix("KUVO_KEY_").is_some_and(|part| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

/// The released Mac can seed settings once, even after the machine layout has opened.
pub fn adopt_legacy_settings(home: &Path, shown: &str, seed: &Value) -> Result<Value, Error> {
    let disk = Disk(home.to_owned());
    #[cfg(unix)]
    let _launch = kuvo_store::locks::ReleasedLaunch::acquire(&disk.path("run")?)?;
    #[cfg(windows)]
    let _launch = kuvo_store::locks::OwnedLock::acquire(
        &disk.path("run/.launching")?,
        std::time::Duration::from_secs(15),
    )?;
    let _settings = kuvo_store::locks::OwnedLock::acquire(
        &disk.path("run/.config.lock")?,
        std::time::Duration::from_secs(15),
    )?;
    let saved = disk.path("box/opencode.json")?;
    if metadata(&saved)?.is_some() {
        return Ok(json!({"status":"own","settings":settings_at(&saved)?.into_fields()}));
    }
    let (mut settings, seeded) = adopt(
        &disk,
        &saved,
        &disk.path("config/opencode/opencode.json")?,
        &disk.path("keys/settings.json")?,
        shown,
        Some(seed),
    )?;
    default_permissions(&disk, &saved, &mut settings)?;
    clear_launch_settings(&disk.path("run/launch.env")?)?;
    Ok(json!({"status":"adopted","seeded":seeded,"settings":settings}))
}

fn settings_line(line: &str) -> bool {
    line.starts_with("KUVO_SETTINGS=") || line.starts_with("KUVO_PRIVATE_SETTINGS=")
}

fn clear_launch_settings(path: &Path) -> Result<(), Error> {
    if let Some(before) = files::read(path)? {
        let text = String::from_utf8_lossy(&before);
        let next = text
            .split('\n')
            .filter(|line| !settings_line(line))
            .collect::<Vec<_>>()
            .join("\n");
        if next.as_bytes() != before {
            files::write_atomic(path, next.as_bytes())?;
        }
    }
    Ok(())
}

fn merge(under: &mut Fields, over: Fields) {
    for (name, value) in over {
        match (under.get_mut(&name), value) {
            (Some(Value::Object(left)), Value::Object(right)) => merge(left, right),
            (_, other) => {
                under.insert(name, other);
            }
        }
    }
}

fn key_paths(value: &mut Value, root: &str, shown: &str) {
    match value {
        Value::String(text) => {
            let to = format!("{{file:{shown}/box/keys/KUVO_KEY_");
            for from in [shown, root] {
                *text = text.replace(&format!("{{file:{from}/keys/KUVO_KEY_"), &to);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|value| key_paths(value, root, shown)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|value| key_paths(value, root, shown)),
        _ => {}
    }
}

fn default_permissions(disk: &Disk, saved: &Path, settings: &mut Fields) -> Result<(), Error> {
    let mark = disk.path("box/defaults-added")?;
    let before = files::read(&mark)?.unwrap_or_default();
    if before
        .split(|b| *b == b'\n')
        .any(|line| line == b"bot-memory")
    {
        return Ok(());
    }
    let rule = json!({ "action": "edit", "resource": "*/bots/bot_*/MEMORY.md", "effect": "allow" });
    let mut rules = settings
        .get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !rules.contains(&rule) {
        rules.push(rule);
        settings.insert("permissions".into(), Value::Array(rules));
        save_settings(saved, settings)?;
    }
    let mut after = before;
    if !after.is_empty() && !after.ends_with(b"\n") {
        after.push(b'\n');
    }
    after.extend_from_slice(b"bot-memory\n");
    files::write_atomic(&mark, &after)?;
    Ok(())
}

fn save_settings(path: &Path, fields: &Fields) -> Result<(), Error> {
    if metadata(path)?.is_some_and(|meta| !meta.is_file()) {
        return Err(Error::Busy(path.into()));
    }
    let content = format!(
        "{}\n",
        serde_json::to_string_pretty(fields).map_err(|source| Error::Input {
            path: path.into(),
            source
        })?
    );
    Ok(files::write_atomic(path, content.as_bytes())?)
}
