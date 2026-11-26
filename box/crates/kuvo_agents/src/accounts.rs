//! The account index used by all external agent adapters.

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use kuvo_store::{files, format, home::Home};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const FAMILY: &str = "Managed accounts";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Cursor,
    Grok,
    Antigravity,
    OpenCode,
}

impl AgentKind {
    pub const ALL: [Self; 6] = [
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::Grok,
        Self::Antigravity,
        Self::OpenCode,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::Antigravity => "antigravity",
            Self::OpenCode => "opencode",
        }
    }

    /// The agent's name, as people read it.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Grok => "Grok",
            Self::Antigravity => "Antigravity",
            Self::OpenCode => "OpenCode",
        }
    }

    /// Its accounts are Settings › Accounts' rows. OpenCode's aren't: its accounts are its own
    /// providers, listed as OpenCode lists them.
    pub fn in_accounts(self) -> bool {
        self != Self::OpenCode
    }

    pub const fn provider(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Codex => "codex-cli",
            Self::Cursor => "cursor",
            Self::Grok => "grok-cli",
            Self::Antigravity => "antigravity",
            // Not `opencode`, which is one of OpenCode's own providers (Zen).
            Self::OpenCode => "opencode-agent",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Provider {
    pub kind: AgentKind,
    pub account: String,
}

impl Provider {
    pub fn parse(provider: &str) -> Option<Self> {
        let (base, account) = provider.split_once('@').unwrap_or((provider, "default"));
        let kind = AgentKind::ALL
            .into_iter()
            .find(|kind| kind.provider() == base)?;
        valid_id(account).then(|| Self {
            kind,
            account: account.into(),
        })
    }
}

impl Provider {
    /// The agent a model's provider runs on: a coding agent's own, else OpenCode's (its providers
    /// are OpenCode's own, `opencode-go`, `anthropic`…: it has one account).
    pub fn of_model(provider: &str) -> Option<Self> {
        Self::parse(provider).or_else(|| {
            let opencode = !provider.is_empty()
                && provider
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
            opencode.then(|| Self {
                kind: AgentKind::OpenCode,
                account: "default".into(),
            })
        })
    }

    /// Where a model's conversation keeps its agent's state: a coding agent's provider as it is,
    /// OpenCode's account for each of OpenCode's providers.
    pub fn slot(provider: &str) -> String {
        match Self::parse(provider) {
            None => Self::of_model(provider).map(|parsed| parsed.to_string()),
            Some(_) => None,
        }
        .unwrap_or_else(|| provider.to_owned())
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.kind.provider())?;
        if self.account != "default" {
            write!(f, "@{}", self.account)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Account {
    pub kind: AgentKind,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

impl Account {
    pub fn provider(&self) -> Provider {
        Provider {
            kind: self.kind,
            account: self.id.clone(),
        }
    }

    fn default_for(kind: AgentKind) -> Self {
        Self {
            kind,
            id: "default".into(),
            label: None,
            folder: None,
            other: Map::new(),
        }
    }

    fn from_saved(value: Value) -> Option<Self> {
        let account: Self = serde_json::from_value(value).ok()?;
        (account.id != "default"
            && valid_id(&account.id)
            && account
                .folder
                .as_deref()
                .is_none_or(|folder| valid_folder(account.kind, folder)))
        .then_some(account)
    }
}

#[derive(Clone)]
pub struct Accounts {
    root: PathBuf,
}

impl Accounts {
    pub fn for_home(home: &Home) -> Self {
        Self {
            root: home.root().join("agents"),
        }
    }

    pub async fn all(&self) -> io::Result<Vec<Account>> {
        let accounts = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut roster = AgentKind::ALL.map(Account::default_for).to_vec();
            roster.extend(accounts.read()?);
            Ok(roster)
        })
        .await?
    }

    /// The default adapter resolves its own sign-in directory.
    pub fn home(&self, account: &Account) -> Option<PathBuf> {
        if account.id == "default" || !valid_id(&account.id) {
            return None;
        }
        if let Some(folder) = &account.folder {
            return Some(
                folder
                    .split(['/', '\\'])
                    .fold(self.root.clone(), |parent, name| parent.join(name)),
            );
        }
        Some(
            self.root
                .join(account.kind.name())
                .join("accounts")
                .join(&account.id),
        )
    }

    pub async fn add(&self, kind: AgentKind, label: Option<String>) -> io::Result<Account> {
        let accounts = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = accounts.lock()?;
            let mut records = accounts.read()?;
            let mut bytes = [0; 4];
            getrandom::fill(&mut bytes).map_err(io::Error::other)?;
            let id = hex::encode(bytes);
            let account = Account {
                kind,
                id,
                label: clean_label(label),
                folder: None,
                other: Map::new(),
            };
            let home = accounts.home(&account).expect("generated account ID");
            let parent = home.parent().expect("account parent");
            accounts.check_containment(parent)?;
            files::create_private_dirs(parent)?;
            fs::create_dir(&home)?;
            if let Err(error) = files::private_folder(&home).and_then(|()| {
                records.push(account.clone());
                accounts.save(&records)
            }) {
                // A durable write can report an error after publishing its record.
                if accounts.read().is_ok_and(|records| {
                    !records
                        .iter()
                        .any(|row| row.provider() == account.provider())
                }) {
                    let _ = fs::remove_dir(&home);
                }
                return Err(error);
            }
            Ok(account)
        })
        .await?
    }

    /// Register an existing managed folder in place, keeping native credentials bound to its path.
    pub async fn adopt_home(
        &self,
        kind: AgentKind,
        folder: String,
        label: Option<String>,
    ) -> io::Result<Account> {
        if !valid_folder(kind, &folder) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid account folder.",
            ));
        }
        let accounts = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = accounts.lock()?;
            let mut records = accounts.read()?;
            let folder = folder.replace('\\', "/");
            let home = accounts.root.join(&folder);
            accounts.check_containment(&home)?;
            if !fs::metadata(&home)?.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "Invalid account folder.",
                ));
            }
            if let Some(account) = records
                .iter()
                .find(|row| row.kind == kind && accounts.home(row).as_ref() == Some(&home))
            {
                return Ok(account.clone());
            }
            let mut bytes = [0; 4];
            getrandom::fill(&mut bytes).map_err(io::Error::other)?;
            let id = hex::encode(bytes);
            if records.iter().any(|row| row.kind == kind && row.id == id) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "An account with this ID already exists.",
                ));
            }
            let account = Account {
                kind,
                id,
                label: clean_label(label),
                folder: Some(folder),
                other: Map::new(),
            };
            records.push(account.clone());
            accounts.save(&records)?;
            Ok(account)
        })
        .await?
    }

    pub async fn rename(&self, provider: Provider, label: Option<String>) -> io::Result<()> {
        let accounts = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = accounts.lock()?;
            let mut records = accounts.read()?;
            if let Some(account) = records.iter_mut().find(|row| row.provider() == provider) {
                account.label = clean_label(label);
                accounts.save(&records)?;
            }
            Ok(())
        })
        .await?
    }

    /// Call after the adapter has stopped using this added account's files.
    pub async fn remove(&self, provider: Provider) -> io::Result<Option<Account>> {
        let accounts = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = accounts.lock()?;
            let mut records = accounts.read()?;
            let Some(index) = records.iter().position(|row| row.provider() == provider) else {
                return Ok(None);
            };
            let account = records.remove(index);
            let home = accounts.home(&account).expect("valid stored account");
            accounts.check_containment(&home)?;
            // A legacy folder can be shared by index entries. Keep it until
            // the last account pointing at it is removed.
            let shared = records
                .iter()
                .any(|row| accounts.home(row).as_ref() == Some(&home));
            accounts.save(&records)?;
            if !shared {
                match fs::remove_dir_all(home) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(Some(account))
        })
        .await?
    }

    fn lock(&self) -> io::Result<File> {
        self.check_containment(&self.root)?;
        files::create_private_dirs(&self.root)?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(".accounts.lock"))?;
        lock.lock()?;
        Ok(lock)
    }

    fn check_containment(&self, path: &Path) -> io::Result<()> {
        for current in path.ancestors() {
            match fs::symlink_metadata(current) {
                Ok(metadata) if linked(&metadata) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "An account folder points outside its own directory",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            if current == self.root {
                break;
            }
        }
        Ok(())
    }

    fn read(&self) -> io::Result<Vec<Account>> {
        let Some(bytes) = files::read(&self.root.join("accounts.json"))? else {
            return Ok(Vec::new());
        };
        let Ok(value) = serde_json::from_slice(&bytes) else {
            return Ok(Vec::new());
        };
        match format::read_additive_list(FAMILY, value) {
            Ok(rows) => Ok(rows.into_iter().filter_map(Account::from_saved).collect()),
            Err(format::Error::Shape { .. }) => Ok(Vec::new()),
            Err(error) => Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        }
    }

    fn save(&self, records: &[Account]) -> io::Result<()> {
        let rows = records
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        let value = format::write_additive_list(rows);
        files::write_atomic(
            &self.root.join("accounts.json"),
            &serde_json::to_vec_pretty(&value)?,
        )
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_folder(kind: AgentKind, folder: &str) -> bool {
    let mut parts = folder.split(['/', '\\']);
    if parts.next() != Some(kind.name()) || parts.clone().next().is_none() {
        return false;
    }
    parts.all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.contains([':', '\0'])
            && !part.ends_with(['.', ' '])
    })
}

fn clean_label(label: Option<String>) -> Option<String> {
    label
        .map(|label| label.trim().to_owned())
        .filter(|label| !label.is_empty())
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(unix)]
    metadata.file_type().is_symlink()
}
