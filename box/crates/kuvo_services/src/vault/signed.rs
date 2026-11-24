use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kuvo_store::files;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use subtle::ConstantTimeEq;

use super::{
    Error, Result,
    crypto::{self, Key},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum File {
    Items,
    Grants,
    Paused,
}

impl File {
    pub const ALL: [Self; 3] = [Self::Items, Self::Grants, Self::Paused];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Items => "items",
            Self::Grants => "grants",
            Self::Paused => "paused",
        }
    }

    const fn filename(self) -> &'static str {
        match self {
            Self::Items => "items.json",
            Self::Grants => "grants.json",
            Self::Paused => "paused.json",
        }
    }
}

pub type Generation = u64;

pub struct Signed {
    folder: PathBuf,
    newest: BTreeMap<File, Generation>,
    tampered: BTreeSet<File>,
}

#[derive(Deserialize, Serialize)]
struct Envelope {
    r#gen: Generation,
    data: Value,
    sig: String,
}

impl Signed {
    pub fn new(home: &Path) -> Self {
        Self {
            folder: home.join("box/vault"),
            newest: BTreeMap::new(),
            tampered: BTreeSet::new(),
        }
    }

    pub fn exists(&self, file: File) -> bool {
        self.folder.join(file.filename()).exists()
    }

    pub fn tampered(&self) -> bool {
        !self.tampered.is_empty()
    }

    pub fn was_tampered(&self, file: File) -> bool {
        self.tampered.contains(&file)
    }

    pub fn mark_tampered(&mut self, file: File) {
        self.tampered.insert(file);
    }

    pub fn newest(&self, file: File) -> Option<Generation> {
        self.newest.get(&file).copied()
    }

    pub fn remember(&mut self, file: File, generation: Generation) {
        let newest = self.newest.entry(file).or_default();
        *newest = (*newest).max(generation);
    }

    pub fn generations(&self) -> BTreeMap<String, Generation> {
        #[derive(Deserialize)]
        struct Kept {
            r#gen: Generation,
        }
        File::ALL
            .into_iter()
            .map(|file| {
                let generation = self
                    .bytes(file)
                    .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
                    .map_or(0, |kept| kept.r#gen);
                (file.name().into(), generation)
            })
            .collect()
    }

    pub(crate) fn read<T: DeserializeOwned>(
        &mut self,
        file: File,
        key: Option<&Key>,
    ) -> Result<Option<T>> {
        let Some(bytes) = self.bytes(file) else {
            return Ok(None);
        };
        let Some(value) = envelope(file, &bytes)? else {
            return Ok(None);
        };
        let Some(key) = key else { return Ok(None) };
        let Ok(kept) = serde_json::from_value::<Envelope>(value) else {
            self.mark_tampered(file);
            return Ok(None);
        };
        let signed = signature(key, file, kept.r#gen, &stringify(&kept.data));
        if !bool::from(signed.as_bytes().ct_eq(kept.sig.as_bytes()))
            || kept.r#gen < self.newest(file).unwrap_or_default()
        {
            self.mark_tampered(file);
            return Ok(None);
        }
        match serde_json::from_value(kept.data) {
            Ok(data) => {
                self.remember(file, kept.r#gen);
                Ok(Some(data))
            }
            Err(_) => {
                self.mark_tampered(file);
                Ok(None)
            }
        }
    }

    pub(crate) fn write(
        &mut self,
        file: File,
        key: &Key,
        data: &impl Serialize,
    ) -> Result<Generation> {
        let on_disk = match self.bytes(file) {
            Some(bytes) => envelope(file, &bytes)?
                .and_then(|value| value.get("gen").and_then(Value::as_u64))
                .unwrap_or_default(),
            None => 0,
        };
        let generation = on_disk
            .max(self.newest(file).unwrap_or_default())
            .checked_add(1)
            .ok_or_else(|| Error::refused(409, "The vault's generation is out of range."))?;
        let data = serde_json::to_value(data)
            .map_err(|_| Error::refused(503, "Couldn't encode the vault's metadata."))?;
        let sig = signature(key, file, generation, &stringify(&data));
        let kept = serde_json::json!({"v": 2, "gen": generation, "data": data, "sig": sig});
        let mut bytes = serde_json::to_vec_pretty(&kept).expect("vault envelopes serialize");
        bytes.push(b'\n');
        files::write_secret(&self.folder.join(file.filename()), &bytes)
            .map_err(|error| Error::disk("Couldn't write the vault's metadata.", error))?;
        self.remember(file, generation);
        self.tampered.remove(&file);
        Ok(generation)
    }

    fn bytes(&self, file: File) -> Option<Vec<u8>> {
        fs::read(self.folder.join(file.filename())).ok()
    }
}

fn envelope(file: File, bytes: &[u8]) -> Result<Option<Value>> {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    validate_version(
        value.get("v"),
        2,
        &format!("Vault {} envelope", file.name()),
    )?;
    Ok(Some(value))
}

pub(super) fn validate_version(
    marker: Option<&Value>,
    supported: u64,
    document: &str,
) -> Result<()> {
    let version = marker
        .and_then(Value::as_f64)
        .filter(|version| version.fract() == 0.0 && version.abs() <= 9_007_199_254_740_991.0)
        .map(|version| version as i64);
    if marker.is_none() || version.is_some_and(|version| (0..=supported as i64).contains(&version))
    {
        return Ok(());
    }
    let version = version.map_or_else(|| "invalid".into(), |version| version.to_string());
    Err(Error::refused(
        503,
        format!(
            "{document} format version {version} is unsupported (this build writes {supported})."
        ),
    ))
}

fn signature(key: &Key, file: File, generation: Generation, data: &str) -> String {
    URL_SAFE_NO_PAD.encode(&*crypto::hmac(
        &**key,
        &[
            file.filename().as_bytes(),
            b"\0",
            generation.to_string().as_bytes(),
            b"\0",
            data.as_bytes(),
        ],
    ))
}

/// JavaScript enumerates array-index keys before other object properties.
/// Keep the insertion order of all remaining keys, as the TS signer does.
pub(super) fn stringify(data: &Value) -> String {
    fn ordered(data: &Value) -> Value {
        match data {
            Value::Object(object) => {
                let mut keys: Vec<_> = object.keys().collect();
                keys.sort_by_key(|key| {
                    key.parse::<u32>()
                        .ok()
                        .filter(|index| *index != u32::MAX && index.to_string() == **key)
                        .unwrap_or(u32::MAX)
                });
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), ordered(&object[key])))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(ordered).collect()),
            other => other.clone(),
        }
    }
    struct Js;
    impl serde_json::ser::Formatter for Js {
        fn write_f64<W: io::Write + ?Sized>(
            &mut self,
            writer: &mut W,
            value: f64,
        ) -> io::Result<()> {
            writer.write_all(ryu_js::Buffer::new().format(value).as_bytes())
        }
    }
    let mut bytes = Vec::new();
    ordered(data)
        .serialize(&mut serde_json::Serializer::with_formatter(&mut bytes, Js))
        .expect("vault metadata serializes");
    String::from_utf8(bytes).expect("JSON is UTF-8")
}

/// The directory lock used by TypeScript vault writers.
pub(crate) struct Writer(PathBuf);

impl Writer {
    pub(crate) fn acquire(home: &Path) -> Result<Self> {
        use std::time::{Duration, Instant};
        let run = home.join("run");
        files::create_private_dirs(&run)
            .map_err(|error| Error::disk("Couldn't prepare the vault's writer lock.", error))?;
        let path = run.join(".vault.lock");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => {
                    return Err(Error::disk("Couldn't lock the vault for writing.", error));
                }
            }
            if fs::metadata(&path)
                .and_then(|entry| entry.modified())
                .ok()
                .and_then(|stamp| stamp.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(10))
            {
                let _ = fs::remove_dir(&path);
            }
            if Instant::now() >= deadline {
                return Err(Error::refused(503, "Couldn't lock the vault for writing."));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
