//! What bots take from this computer's setup: the personalities you made, your time zone, and the
//! computer's id; and the zone files a bot's wake-ups go by (`timezone`, `zoneinfo`).

use std::path::Path;

use serde_json::Value;

/// This computer's setup, as far as its bots read it.
pub trait Setup: Send + Sync + 'static {
    /// Your personalities as the library has them now (each its `id`, `name` and `description`).
    fn personalities(&self) -> Vec<Value>;
    /// Your time zone, with its rules (TZif) where they came with it; none where you set none.
    fn time_zone(&self) -> Option<(String, Option<Vec<u8>>)>;
    /// This computer's id in its link, where it has one.
    fn link_id(&self) -> Option<String>;
}

/// No setup: no personalities of yours, no zone, no id (a test's).
pub struct NoSetup;

impl Setup for NoSetup {
    fn personalities(&self) -> Vec<Value> {
        Vec::new()
    }

    fn time_zone(&self) -> Option<(String, Option<Vec<u8>>)> {
        None
    }

    fn link_id(&self) -> Option<String> {
        None
    }
}

/// This computer's own clock's zone.
pub fn own_clock() -> String {
    jiff::tz::TimeZone::system()
        .iana_name()
        .filter(|name| !name.is_empty())
        .unwrap_or("UTC")
        .to_owned()
}

/// What a folder of wake-ups says of its zone: your zone where there's one; with none, on a
/// machine its own clock, on a Mac nothing (it goes by its clock as it changes).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ZoneFiles {
    pub timezone: Option<String>,
    pub zoneinfo: Option<Vec<u8>>,
}

impl ZoneFiles {
    pub fn of(setting: Option<(String, Option<Vec<u8>>)>, is_mac: bool) -> Self {
        match setting {
            Some((zone, rules)) => Self {
                timezone: Some(zone),
                zoneinfo: rules,
            },
            None if !is_mac => Self {
                timezone: Some(own_clock()),
                zoneinfo: None,
            },
            None => Self::default(),
        }
    }

    /// Written in `folder` where they differ: the rules first, then the name that reads them;
    /// rules no longer wanted go last.
    pub fn write(&self, folder: &Path) -> std::io::Result<()> {
        let put = |name: &str, data: Option<&[u8]>| -> std::io::Result<()> {
            let file = folder.join(name);
            let Some(data) = data else {
                return match std::fs::remove_file(&file) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                };
            };
            if std::fs::read(&file).ok().as_deref() == Some(data) {
                return Ok(());
            }
            kuvo_store::files::write_atomic(&file, data)
        };
        if let Some(rules) = &self.zoneinfo {
            put("zoneinfo", Some(rules))?;
        }
        put("timezone", self.timezone.as_deref().map(str::as_bytes))?;
        if self.zoneinfo.is_none() {
            put("zoneinfo", None)?;
        }
        Ok(())
    }
}
