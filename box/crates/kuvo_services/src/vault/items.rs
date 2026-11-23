use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use super::{Error, Result, Secret, Values, crypto};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Login,
    Key,
    File,
    Note,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub sites: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sign_in_only: Option<bool>,
    pub created: u64,
    pub changed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<u64>,
}

impl Item {
    pub(super) fn new(given: &Map<String, Value>, existing: &[Self], now: u64) -> Result<Self> {
        let kind = given
            .get("kind")
            .and_then(|kind| serde_json::from_value(kind.clone()).ok())
            .ok_or_else(|| Error::refused(400, "An item is one of: login, key, file, note."))?;
        let item = Self {
            id: new_id("v_")?,
            kind,
            name: String::new(),
            sites: Vec::new(),
            username: None,
            fields: Vec::new(),
            file_name: None,
            readable: None,
            sign_in_only: None,
            created: now,
            changed: now,
            used: None,
        };
        if !given.contains_key("name") {
            return Err(Error::refused(400, "Give it a name."));
        }
        item.apply(given, existing, now)
    }

    pub(super) fn apply(
        mut self,
        given: &Map<String, Value>,
        existing: &[Self],
        now: u64,
    ) -> Result<Self> {
        self.changed = now;
        if let Some(name) = given.get("name") {
            self.name =
                nonempty(name, 120).ok_or_else(|| Error::refused(400, "Give it a name."))?;
            let folded = self.name.to_lowercase();
            if existing
                .iter()
                .any(|item| item.id != self.id && item.name.to_lowercase() == folded)
            {
                return Err(Error::refused(
                    409,
                    format!("There's an item called “{}” already.", self.name),
                ));
            }
        }
        if let Some(given) = given.get("sites") {
            self.sites = sites(given)?;
        }
        if self.kind == Kind::Login && self.sites.is_empty() {
            return Err(Error::refused(400, "A sign-in needs the site it's for."));
        }
        if let Some(username) = given.get("username") {
            self.username = nonempty(username, 200);
        }
        if let Some(name) = given.get("fileName").filter(|_| self.kind == Kind::File) {
            self.file_name = nonempty(name, 200).and_then(|name| {
                let trimmed = name.trim_end_matches(std::path::is_separator);
                Path::new(trimmed)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                    .or_else(|| {
                        trimmed
                            .rsplit(std::path::is_separator)
                            .next()
                            .map(str::to_owned)
                    })
            });
        }
        if let Some(value) = given.get("readable").filter(|_| self.kind == Kind::Note) {
            self.readable = (value.as_bool() == Some(true)).then_some(true);
        }
        if let Some(value) = given.get("signInOnly").filter(|_| self.kind == Kind::Login) {
            self.sign_in_only = (value.as_bool() == Some(true)).then_some(true);
        }
        Ok(self)
    }

    pub fn usable_on(&self, host: &str) -> bool {
        self.sites.iter().any(|site| {
            host == site
                || (site.contains('.')
                    && host
                        .strip_suffix(site)
                        .is_some_and(|prefix| prefix.ends_with('.')))
        })
    }
}

fn nonempty(given: &Value, limit: usize) -> Option<String> {
    let text = one_line(given.as_str()?, limit);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

pub(crate) fn one_line(text: &str, limit: usize) -> String {
    text.chars()
        .take(limit)
        .map(|c| match c {
            '\t' | '\r' | '\n' => ' ',
            _ => c,
        })
        .collect()
}

fn sites(given: &Value) -> Result<Vec<String>> {
    if given.is_null() {
        return Ok(Vec::new());
    }
    let list = given
        .as_array()
        .ok_or_else(|| Error::refused(400, "Send its sites as a list."))?;
    let mut unique = Vec::new();
    for given in list {
        let text = given.as_str().unwrap_or_default();
        let site = site_of(text).ok_or_else(|| {
            Error::refused(400, format!("“{}” isn't a site.", one_line(text, 80)))
        })?;
        if !unique.contains(&site) {
            unique.push(site);
        }
    }
    Ok(unique)
}

pub fn site_of(given: &str) -> Option<String> {
    let given = given.trim().to_lowercase();
    let has_scheme = given.split_once("://").is_some_and(|(scheme, _)| {
        scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    });
    let url = if has_scheme {
        given
    } else {
        format!("https://{given}")
    };
    let url = Url::parse(&url).ok()?;
    let host = url.host_str()?;
    let host = host.strip_suffix('.').unwrap_or(host);
    (host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
        && host.bytes().any(|b| b.is_ascii_alphanumeric()))
    .then(|| host.to_owned())
}

pub(super) fn changes(
    given: Option<Value>,
    changing: bool,
) -> Result<Vec<(String, Option<Secret>)>> {
    let Some(given) = given.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let Value::Object(values) = given else {
        return Err(Error::refused(
            400,
            "Send the values as {\"field\": \"value\"}.",
        ));
    };
    values.into_iter().map(|(field, value)| {
        if !field.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            || field.len() > 40
            || !field.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(Error::refused(400, format!("“{}” isn't a field's name: use lowercase letters, digits and _.", one_line(&field, 40))));
        }
        let secret = match value {
            Value::Null if changing => None,
            Value::String(value) => {
                let value = Secret::new(value);
                match value.expose().encode_utf16().count() {
                    0..=3 => return Err(Error::refused(400, "A value needs at least 4 characters, so it can be hidden from agents.")),
                    4..=1_000_000 => Some(value),
                    _ => return Err(value_too_large()),
                }
            }
            _ => return Err(value_too_large()),
        };
        Ok((field, secret))
    }).collect()
}

fn value_too_large() -> Error {
    Error::refused(400, "A value is text, at most a megabyte.")
}

pub(super) fn merge(values: &mut Values, changes: Vec<(String, Option<Secret>)>) {
    for (field, value) in changes {
        if let Some(value) = value {
            values.insert(field, value);
        } else {
            values.remove(&field);
        }
    }
}

pub(crate) fn new_id(prefix: &str) -> Result<String> {
    let random = crypto::random::<10>()?;
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789";
    Ok(prefix
        .chars()
        .chain(
            random
                .iter()
                .map(|byte| char::from(alphabet[(byte % 36) as usize])),
        )
        .collect())
}
