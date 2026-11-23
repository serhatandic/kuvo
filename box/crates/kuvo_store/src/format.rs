use crate::files;
use serde_json::{Map, Value};
use std::path::Path;
use thiserror::Error;

pub type Fields = Map<String, Value>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{family} format version {found} is unsupported (this build writes {current}).")]
    Unsupported {
        family: String,
        found: String,
        current: u32,
    },

    #[error("{family} isn't a JSON {shape}.")]
    Shape { family: String, shape: &'static str },

    #[error("Kuvo cannot read {}; its bytes were kept.", path.display())]
    Unreadable {
        path: std::path::PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// TypeScript's Number.isSafeInteger also accepts 1.0 and 1e0 from JSON.
fn safe_integer(value: &Value) -> Option<i64> {
    let number = value.as_f64()?;
    (number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_991.0).then_some(number as i64)
}

pub(crate) fn unsupported(family: &str, found: Option<&Value>, current: u32) -> Error {
    Error::Unsupported {
        family: family.into(),
        found: found
            .and_then(safe_integer)
            .map_or_else(|| "invalid".into(), |n| n.to_string()),
        current,
    }
}

pub fn check_version(
    family: &str,
    found: Option<&Value>,
    legacy: u32,
    current: u32,
) -> Result<u32, Error> {
    match found {
        None => Ok(legacy),
        Some(value) => safe_integer(value)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|&version| version <= current)
            .ok_or_else(|| unsupported(family, found, current)),
    }
}

type Upgrade<T, E> = fn(T) -> Result<T, E>;

pub fn upgrade<T, E: From<Error>>(
    family: &str,
    from: Option<&Value>,
    mut value: T,
    steps: &[Upgrade<T, E>],
) -> Result<T, E> {
    let version = check_version(family, from, 0, steps.len() as u32)?;
    for step in &steps[version as usize..] {
        value = step(value)?;
    }
    Ok(value)
}

#[derive(Debug, PartialEq)]
pub struct Object {
    pub version: u32,
    pub fields: Fields,
}

#[derive(Clone, Copy, Debug)]
pub struct ObjectFormat {
    pub family: &'static str,
    pub current: u32,
}

impl ObjectFormat {
    pub const fn additive(family: &'static str) -> Self {
        Self { family, current: 1 }
    }

    pub fn decode(self, value: Value) -> Result<Object, Error> {
        let Value::Object(mut fields) = value else {
            return Err(Error::Shape {
                family: self.family.into(),
                shape: "object",
            });
        };
        let found = fields.remove("formatVersion");
        let version = check_version(self.family, found.as_ref(), 0, self.current)?;
        Ok(Object { version, fields })
    }

    pub fn encode(self, mut fields: Fields) -> Value {
        fields.insert("formatVersion".into(), self.current.into());
        Value::Object(fields)
    }

    pub fn read(self, path: &Path) -> Result<Option<Object>, Error> {
        let unreadable = |source| Error::Unreadable {
            path: path.into(),
            source,
        };
        let Some(bytes) = files::read(path).map_err(|err| unreadable(Box::new(err)))? else {
            return Ok(None);
        };
        let value = serde_json::from_str(&String::from_utf8_lossy(&bytes))
            .map_err(|err| unreadable(Box::new(err)))?;
        match self.decode(value) {
            Ok(record) => Ok(Some(record)),
            Err(error @ Error::Unsupported { .. }) => Err(error),
            Err(error) => Err(unreadable(Box::new(error))),
        }
    }
}

pub fn read_additive_list(family: &str, value: Value) -> Result<Vec<Value>, Error> {
    let Value::Array(mut items) = value else {
        return Err(Error::Shape {
            family: family.into(),
            shape: "array",
        });
    };
    if let Some(found) = items
        .first()
        .and_then(Value::as_object)
        .and_then(|marker| marker.get("formatVersion"))
    {
        check_version(family, Some(found), 0, 1)?;
        items.remove(0);
    }
    Ok(items)
}

pub fn write_additive_list(items: impl IntoIterator<Item = Value>) -> Vec<Value> {
    std::iter::once(serde_json::json!({"formatVersion": 1}))
        .chain(items)
        .collect()
}

pub fn token_payload<'a>(
    input: &'a str,
    family: &str,
    version: u32,
) -> Result<Option<&'a str>, Error> {
    let Some((tag, payload)) = input
        .strip_prefix(family)
        .and_then(|rest| rest.split_once('.'))
    else {
        return Ok(None);
    };
    if tag != version.to_string() {
        let found = (tag.len() <= 15 && tag.bytes().all(|b| b.is_ascii_digit()))
            .then(|| tag.parse::<u64>().ok())
            .flatten()
            .map(Value::from);
        return Err(unsupported(
            family
                .char_indices()
                .next_back()
                .map_or("", |(end, _)| &family[..end]),
            found.as_ref(),
            version,
        ));
    }
    Ok(Some(payload))
}

pub fn supports_legacy_object_version(fields: &Fields) -> bool {
    check_version("record", fields.get("formatVersion"), 0, 1).is_ok()
}

pub const BOX_FILES: ObjectFormat = ObjectFormat::additive("Box pack file list");
pub const INSTALLED: ObjectFormat = ObjectFormat::additive("Installed Box receipt");

pub fn read_installed(uploads: &Path) -> Result<Option<Object>, Error> {
    let root = uploads.parent().unwrap_or_else(|| {
        if uploads.has_root() {
            uploads
        } else {
            Path::new("..")
        }
    });
    match INSTALLED.read(&root.join("bin/.installed.json"))? {
        Some(record) => Ok(Some(record)),
        None => INSTALLED.read(&uploads.join("installed.json")),
    }
}

/// Check an existing discriminator such as kuvo:1 or v:2, with no legacy fallback.
pub fn require_stamp(family: &str, found: Option<&Value>, expected: u32) -> Result<(), Error> {
    if found.and_then(safe_integer) == Some(i64::from(expected)) {
        Ok(())
    } else {
        Err(unsupported(family, found, expected))
    }
}
