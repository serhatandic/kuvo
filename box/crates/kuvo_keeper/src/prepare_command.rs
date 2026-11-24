//! The offline prepare command shared by the updater and its standalone caller.

use std::{ffi::OsString, fs, io, path::Path, process::ExitCode};

use kuvo_os::platform::Account;
use kuvo_store::home::Home;
use serde_json::{Value, json};

use crate::{
    Release, SELECTOR,
    manifest::{SignedRelease, UPDATE_KEY, local_target},
    staging::{self, MAX_ENVELOPE, MAX_PACK},
};

pub fn run(args: &[OsString]) -> io::Result<ExitCode> {
    let mut folder = None;
    let mut test_key = None;
    let mut prepare = false;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.to_str() {
            Some("--from") if folder.is_none() => folder = words.next().map(Path::new),
            Some("--key") if test_key.is_none() => {
                test_key = words.next().and_then(|arg| arg.to_str())
            }
            Some("--phase") if !prepare => {
                prepare = words.next().is_some_and(|arg| arg == "prepare")
            }
            _ => return Err(usage()),
        }
    }
    let Some(folder) = folder.filter(|_| prepare) else {
        return Err(usage());
    };
    let home = std::env::var_os("KUVO_HOME")
        .ok_or_else(|| io::Error::other("KUVO_HOME must name the Kuvo installation"))?;
    let home = Home::new(home.into())?;
    let answer = prepare_update(&home, folder, test_key)
        .unwrap_or_else(|error| json!({"result":"failed","message":error.to_string()}));
    println!("{answer}");
    Ok(ExitCode::SUCCESS)
}

fn prepare_update(home: &Home, folder: &Path, test_key: Option<&str>) -> io::Result<Value> {
    if !folder.is_absolute() {
        return Err(io::Error::other(
            "Send the full path to the release folder.",
        ));
    }
    let installed = Release::read(&home.root().join(SELECTOR))?;
    let version = if let Some(release) = &installed {
        Some(release.version().to_owned())
    } else {
        kuvo_store::files::text_of(&home.bin().join("box-version"))
            .map(|value| value.trim().to_owned())
    };
    let Some(installed_version) = version.filter(|version| version_order(version).is_some()) else {
        return Ok(json!({"result":"blocked","message":"This computer’s box version is unknown."}));
    };
    if ["stopped", "paused", "off"]
        .iter()
        .any(|mark| home.run().join(mark).exists())
    {
        return Ok(json!({"result":"blocked","message":"This computer’s box is stopped."}));
    }
    let key = if let Some(key) = test_key
        && is_scratch(home)?
    {
        key
    } else {
        UPDATE_KEY
    };
    let envelope = staging::read_release(&folder.join("boxes.json"), MAX_ENVELOPE)?;
    let signed = SignedRelease::verify(&envelope, &[key])?;
    if version_order(signed.version()) <= version_order(&installed_version) {
        return Ok(json!({"result":"current","version":installed_version}));
    }
    if !signed.spreads() {
        return Err(io::Error::other(
            "This release cannot be shared with your setup.",
        ));
    }
    let target = local_target()?;
    let pack = signed.pack_for(target)?;
    let ref_id = signed.reference(pack);
    let result = staging::read_release(&folder.join(pack.filename()?), MAX_PACK)
        .and_then(|bytes| staging::prepare(home, &signed, target, &bytes, &envelope));
    Ok(match result {
        Ok(release) => json!({"result":"waiting","version":release.version}),
        Err(error) => {
            json!({"result":"failed","message":error.to_string(),"version":ref_id.version})
        }
    })
}

// A fixture key is accepted only against an existing scratch home. Compare the
// filesystem's resolved objects so a second spelling cannot stand in for a real home.
pub fn is_scratch(home: &Home) -> io::Result<bool> {
    let root = fs::canonicalize(home.root())?;
    let real = Account::current()?.home.join(".kuvo-code");
    let real = fs::canonicalize(&real).unwrap_or(real);
    let equal = if cfg!(any(windows, target_os = "macos")) {
        root.to_string_lossy()
            .eq_ignore_ascii_case(&real.to_string_lossy())
    } else {
        root == real
    };
    Ok(!equal
        && !root.parent().and_then(Path::file_name).is_some_and(|part| {
            part.to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("run.kuvo.code")
        }))
}

pub(crate) fn version_order(version: &str) -> Option<([u64; 3], bool, u64)> {
    let (base, beta) = version
        .split_once("-beta.")
        .map_or((version, None), |(base, beta)| (base, Some(beta)));
    let number = |part: &str| {
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse::<u64>().ok())
            .flatten()
    };
    let mut parts = base.split('.');
    let sequence = [
        number(parts.next()?)?,
        number(parts.next()?)?,
        number(parts.next()?)?,
    ];
    if parts.next().is_some() {
        return None;
    }
    Some((
        sequence,
        beta.is_none(),
        if let Some(beta) = beta {
            number(beta)?
        } else {
            0
        },
    ))
}

fn usage() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Usage: kuvo-box update --phase prepare --from <release folder>",
    )
}
