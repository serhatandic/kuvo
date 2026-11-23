use super::Run;
use crate::{Error, metadata};
use kuvo_store::format;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    process::Command,
};

const LEGACY_PROGRAMS: &[&str] = &[
    "bin/bot-words.json",
    "bin/integrations-mac.json",
    "bin/integrations.json",
    "bin/kuvo-agents.js",
    "bin/kuvo-browser.js",
    "bin/kuvo-launch",
    "bin/kuvo-link.js",
    "bin/kuvo-npx",
    "bin/kuvo-setsid",
    "bin/kuvo-start",
    "bin/kuvo-wake",
    "bin/mcp-catalog.json",
    "bin/runtime/node",
    "config/opencode/plugins/kuvo-code.js",
    "bin/.kuvo-boot",
    "bin/box-version",
];
const DAMAGED: &str = "This copy of Kuvo is damaged. Download it again, then try again.";

pub(super) fn check(run: &Run<'_>, real: bool) -> Result<(), Error> {
    let root = run.home.root().canonicalize()?;
    let release = run.release.canonicalize()?;
    let engine = run.engine.canonicalize()?;
    let installer = root.join("updates/install");
    if !release.starts_with(&installer) || !engine.starts_with(&installer) {
        return Err(super::invalid(
            "The release and engine must be in the installer's staging folder.",
        ));
    }
    let manifest = format::BOX_FILES
        .read(&release.join("box.json"))?
        .ok_or_else(|| {
            super::invalid("Kuvo's own files weren't unpacked. Open Kuvo again to try again.")
        })?;
    let listed = manifest
        .fields
        .get("files")
        .and_then(Value::as_object)
        .filter(|_| manifest.fields.get("version").is_some_and(Value::is_string))
        .ok_or_else(|| super::invalid(DAMAGED))?;
    let needed: &[&str] = if listed.contains_key("bin/kuvo-box") {
        &[
            "bin/kuvo-box",
            "bin/kuvo-start",
            "config/opencode/plugins/kuvo-code.js",
        ]
    } else {
        if real {
            return Err(super::invalid(DAMAGED));
        }
        LEGACY_PROGRAMS
    };
    if needed.iter().any(|name| !listed.contains_key(*name)) {
        return Err(super::invalid(DAMAGED));
    }
    for (name, digest) in listed {
        let relative = Path::new(name);
        if !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(super::invalid(DAMAGED));
        }
        let file = release.join(relative);
        if !metadata(&file)?.is_some_and(|meta| meta.is_file())
            || !file.canonicalize()?.starts_with(&release)
        {
            return Err(super::invalid(DAMAGED));
        }
        let mut reader = fs::File::open(file)?;
        let mut sha256 = Sha256::new();
        io::copy(&mut reader, &mut sha256)?;
        if digest.as_str() != Some(hex::encode(sha256.finalize()).as_str()) {
            return Err(super::invalid(DAMAGED));
        }
    }
    super::super::checked_engine(&release, &engine)?;
    if real
        && !metadata(&run.bundle.join("Contents/MacOS/kuvo-keys"))?
            .is_some_and(|meta| meta.is_file())
    {
        return Err(super::invalid(DAMAGED));
    }
    let id = super::super::bundle_id(run.bundle)?;
    let source = run.account.join("Library/Application Support").join(id);
    if fs::metadata(&source)?.dev() != fs::metadata(&root)?.dev() {
        return Err(super::invalid(
            "Kuvo can't move your sessions on this disk: your home folder must be on one APFS disk.",
        ));
    }
    let output = Command::new("/bin/df")
        .env_clear()
        .args(["-Pk"])
        .arg(&root)
        .output()?;
    let rows = String::from_utf8_lossy(&output.stdout);
    let capacity = rows
        .lines()
        .last()
        .and_then(|line| line.split_ascii_whitespace().nth(3))
        .and_then(|value| value.parse::<u64>().ok());
    let database = source.join("opencode/data/opencode");
    let db_bytes: u64 = ["opencode.db", "opencode.db-wal"]
        .into_iter()
        .map(|name| fs::metadata(database.join(name)).map_or(0, |meta| meta.len()))
        .sum();
    let needed = 200_000_000u64.saturating_add(db_bytes.saturating_mul(11) / 10);
    if capacity.is_some_and(|blocks| blocks.saturating_mul(1024) < needed) {
        return Err(super::invalid(&format!(
            "There isn't enough room on this Mac to move your sessions. Free up {} MB, then try again.",
            needed.div_ceil(1_000_000)
        )));
    }
    Ok(())
}
