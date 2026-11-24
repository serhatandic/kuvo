use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    Encrypted, Error, File, Generation, Item, Result, Secret, Signed, TAMPERED, Values, check_id,
    crypto::Key, items, signed::Writer,
};

pub struct Vault {
    home: PathBuf,
    signed: Signed,
    backend: Backend,
}

enum Backend {
    Encrypted(Encrypted),
    Keychain(super::helper::Keychain),
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Who {
    Bot { bot: String },
    Project { project: String },
    Everyone { everyone: bool },
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum How {
    Own,
    Ask,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Grant {
    pub item: String,
    pub who: Who,
    pub how: How,
    pub at: u64,
}

#[derive(Default, Deserialize, Serialize)]
struct Pause {
    #[serde(skip_serializing_if = "Option::is_none")]
    all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bots: Option<BTreeMap<String, String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub made: bool,
    pub locked: bool,
    pub paused: bool,
    pub paused_bots: BTreeMap<String, String>,
    pub store: &'static str,
    pub kept_days: u32,
    pub agents: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub agents_waiting: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tampered: bool,
    pub generations: BTreeMap<String, Generation>,
}

impl Vault {
    pub fn open(home: &Path, mac: bool, input: super::helper::Launch) -> Self {
        let backend = if mac {
            Backend::Keychain(super::helper::Keychain::new(home, input))
        } else {
            Backend::Encrypted(Encrypted::new(home))
        };
        Self {
            home: home.to_owned(),
            signed: Signed::new(home),
            backend,
        }
    }

    pub fn on_host(&self) -> bool {
        matches!(self.backend, Backend::Keychain(_))
    }

    pub fn made(&self) -> bool {
        match &self.backend {
            Backend::Encrypted(store) => store.made(),
            Backend::Keychain(_) => true,
        }
    }

    pub fn locked(&self) -> bool {
        matches!(&self.backend, Backend::Encrypted(store) if store.locked())
    }

    pub fn make(&mut self) -> Result<Secret> {
        let _writer = Writer::acquire(&self.home)?;
        match &mut self.backend {
            Backend::Encrypted(store) => store.make(),
            Backend::Keychain(_) => Err(Error::refused(
                409,
                "This Mac's vault is its Keychain: there's nothing to make.",
            )),
        }
    }

    pub fn unlock(&mut self, key: &str) -> bool {
        match &mut self.backend {
            Backend::Encrypted(store) => store.unlock(key),
            Backend::Keychain(_) => false,
        }
    }

    pub fn unlock_key(&self) -> Option<Secret> {
        match &self.backend {
            Backend::Encrypted(store) => store.unlock_key(),
            Backend::Keychain(_) => None,
        }
    }

    pub(super) fn unlock_proof(&self, tag: &str, nonce: &str) -> Option<String> {
        match &self.backend {
            Backend::Encrypted(store) => store.unlock_proof(tag, nonce),
            Backend::Keychain(_) => None,
        }
    }

    pub(super) fn proves_unlock(&self, tag: &str, nonce: &str, proof: &str) -> bool {
        matches!(&self.backend, Backend::Encrypted(store) if store.proves_unlock(tag, nonce, proof))
    }

    pub fn prime(&mut self) {
        if let Backend::Keychain(store) = &mut self.backend {
            store.prime(&mut self.signed);
        }
    }

    fn file_key(&self) -> Option<Key> {
        match &self.backend {
            Backend::Encrypted(store) => store.file_key(),
            Backend::Keychain(store) => store.file_key(),
        }
    }

    pub fn values(&self, id: &str) -> Result<Values> {
        match &self.backend {
            Backend::Encrypted(store) => store.values(id),
            Backend::Keychain(store) => store.values(id),
        }
    }

    fn put_values(&self, id: &str, values: &Values) -> Result<()> {
        match &self.backend {
            Backend::Encrypted(store) => store.put_values(id, values),
            Backend::Keychain(store) => store.put_values(id, values),
        }
    }

    fn delete_values(&self, id: &str) -> Result<()> {
        match &self.backend {
            Backend::Encrypted(store) => store.delete_values(id),
            Backend::Keychain(store) => store.delete_values(id),
        }
    }

    fn read<T: serde::de::DeserializeOwned>(&mut self, file: File) -> Result<Option<T>> {
        let key = self.file_key();
        self.signed.read(file, key.as_ref())
    }

    fn write(&mut self, file: File, value: &impl Serialize) -> Result<()> {
        let key = self.file_key().ok_or_else(|| if self.on_host() {
            Error::refused(503, "Kuvo Code on this Mac didn't give its vault its key. Quit Kuvo Code and open it again.")
        } else {
            Error::refused(423, super::LOCKED)
        })?;
        let generation = self.signed.write(file, &key, value)?;
        if let Backend::Keychain(store) = &mut self.backend {
            store.remember(file, generation);
        }
        Ok(())
    }

    pub fn items(&mut self) -> Result<Vec<Item>> {
        Ok(self.read(File::Items)?.unwrap_or_default())
    }

    pub fn item(&mut self, id: &str) -> Result<Option<Item>> {
        Ok(self.items()?.into_iter().find(|item| item.id == id))
    }

    pub fn named(&mut self, named: &str) -> Result<Option<Item>> {
        let name = named.trim();
        if name.is_empty() {
            return Ok(None);
        }
        let items = self.items()?;
        let lower = name.to_lowercase();
        let index = items.iter().position(|item| item.id == name).or_else(|| {
            items
                .iter()
                .position(|item| item.name.to_lowercase() == lower)
        });
        Ok(index.and_then(|index| items.into_iter().nth(index)))
    }

    pub fn add(&mut self, mut given: Map<String, Value>, now: u64) -> Result<Item> {
        if !self.signed.exists(File::Paused) {
            let _writer = Writer::acquire(&self.home)?;
            if !self.signed.exists(File::Paused) {
                self.write(File::Paused, &Pause::default())?;
            }
        }
        let mut item = Item::new(&given, &self.items()?, now)?;
        let mut values = Values::new();
        items::merge(&mut values, items::changes(given.remove("values"), false)?);
        item.fields = values.keys().cloned().collect();
        self.put_values(&item.id, &values)?;
        let _writer = Writer::acquire(&self.home)?;
        let mut current = self.items()?;
        current.push(item.clone());
        self.write(File::Items, &current)?;
        Ok(item)
    }

    pub fn change(&mut self, id: &str, mut given: Map<String, Value>, now: u64) -> Result<Item> {
        check_id(id)?;
        let current = self.items()?;
        let previous = current
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(missing)?;
        let mut item = previous.clone().apply(&given, &current, now)?;
        let changes = items::changes(given.remove("values"), true)?;
        if !changes.is_empty() {
            let mut values = self.values(id)?;
            items::merge(&mut values, changes);
            self.put_values(id, &values)?;
            item.fields = values.keys().cloned().collect();
        }
        let _writer = Writer::acquire(&self.home)?;
        let mut current = self.items()?;
        if let Some(kept) = current.iter_mut().find(|item| item.id == id) {
            let used = kept.used;
            *kept = item.clone();
            kept.used = used;
        }
        self.write(File::Items, &current)?;
        Ok(item)
    }

    pub fn remove(&mut self, id: &str) -> Result<Item> {
        check_id(id)?;
        let item = self.item(id)?.ok_or_else(missing)?;
        self.delete_values(id)?;
        let _writer = Writer::acquire(&self.home)?;
        let mut items = self.items()?;
        items.retain(|item| item.id != id);
        self.write(File::Items, &items)?;
        let mut grants = self.grants()?;
        grants.retain(|grant| grant.item != id);
        self.write(File::Grants, &grants)?;
        Ok(item)
    }

    pub fn note_used(&mut self, id: &str, now: u64) -> Result<()> {
        let _writer = Writer::acquire(&self.home)?;
        let mut items = self.items()?;
        if let Some(item) = items.iter_mut().find(|item| item.id == id) {
            item.used = Some(now);
        }
        self.write(File::Items, &items)
    }

    pub fn grants(&mut self) -> Result<Vec<Grant>> {
        Ok(self.read(File::Grants)?.unwrap_or_default())
    }

    pub fn set_grant(
        &mut self,
        id: &str,
        who: Who,
        how: Option<How>,
        now: u64,
    ) -> Result<Vec<Grant>> {
        check_id(id)?;
        let _writer = Writer::acquire(&self.home)?;
        self.item(id)?.ok_or_else(missing)?;
        let mut grants = self.grants()?;
        grants.retain(|grant| grant.item != id || grant.who != who);
        if let Some(how) = how {
            grants.push(Grant {
                item: id.into(),
                who,
                how,
                at: now,
            });
        }
        self.write(File::Grants, &grants)?;
        Ok(grants)
    }

    fn pause(&mut self) -> Result<Pause> {
        let all = || Pause {
            all: Some(true),
            bots: None,
        };
        if !self.signed.exists(File::Paused) {
            return if self.signed.newest(File::Paused).is_some() || self.signed.exists(File::Items)
            {
                self.signed.mark_tampered(File::Paused);
                Ok(all())
            } else {
                Ok(Pause::default())
            };
        }
        Ok(self.read(File::Paused)?.unwrap_or_else(all))
    }

    pub fn set_paused(&mut self, paused: bool, who: Option<&str>, why: &str) -> Result<()> {
        let _writer = Writer::acquire(&self.home)?;
        let mut now: Pause = self.read(File::Paused)?.unwrap_or_default();
        if let Some(who) = who.filter(|who| !who.is_empty()) {
            let bots = now.bots.get_or_insert_with(BTreeMap::new);
            if paused {
                bots.insert(who.into(), items::one_line(why, 300));
            } else {
                bots.remove(who);
            }
        } else {
            now.all = Some(paused);
        }
        self.write(File::Paused, &now)
    }

    pub fn paused_for(&mut self, bot: Option<&str>, root: Option<&str>) -> Result<Option<String>> {
        if let Backend::Keychain(store) = &self.backend
            && let Some(why) = store.waiting()
        {
            return Ok(Some(why.to_owned()));
        }
        let now = self.pause()?;
        if now.all == Some(true) {
            return Ok(Some(if self.signed.was_tampered(File::Paused) {
                TAMPERED.into()
            } else {
                "The user paused the vault on this computer.".into()
            }));
        }
        let bots = now.bots.unwrap_or_default();
        Ok(bot
            .and_then(|bot| bots.get(bot))
            .filter(|why| !why.is_empty())
            .map(|why| format!("The user paused this bot's use of the vault: {why}"))
            .or_else(|| {
                root.and_then(|root| bots.get(root))
                    .filter(|why| !why.is_empty())
                    .map(|why| {
                        format!("The user paused this conversation's use of the vault: {why}")
                    })
            }))
    }

    pub fn state(&mut self, agents: bool, agents_waiting: bool) -> Result<State> {
        let pause = self.pause()?;
        Ok(State {
            made: self.made(),
            locked: self.locked(),
            paused: pause.all.unwrap_or_default(),
            paused_bots: pause.bots.unwrap_or_default(),
            store: if self.on_host() {
                "keychain"
            } else {
                "encrypted"
            },
            kept_days: 365,
            agents,
            agents_waiting,
            tampered: self.signed.tampered(),
            generations: self.signed.generations(),
        })
    }
}

pub(crate) fn missing() -> Error {
    Error::refused(404, "That item isn't in this vault.")
}
