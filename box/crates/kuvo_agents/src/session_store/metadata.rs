//! Metadata updates preserve fields introduced by other box versions.

use super::{FORMAT, SessionStore, TRANSCRIPT, read_header};
use kuvo_store::{files, sqlite};
use serde_json::{Map, Value};
use std::{
    io::{self, ErrorKind},
    time::{Duration, SystemTime},
};

impl SessionStore {
    pub fn make_id(prefix: &str) -> io::Result<String> {
        let millis = SystemTime::UNIX_EPOCH
            .elapsed()
            .map_err(io::Error::other)?
            .as_millis();
        let mut random = [0; 9];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        Ok(format!("{prefix}_{millis:012x}{}", hex::encode(random)))
    }

    pub async fn create(&self, header: Map<String, Value>) -> io::Result<()> {
        let id = header
            .get("id")
            .and_then(Value::as_str)
            .ok_or(ErrorKind::InvalidInput)?;
        let folder = self.folder(id).ok_or(ErrorKind::InvalidInput)?;
        tokio::task::spawn_blocking(move || {
            files::write_new(
                &folder.join("session.json"),
                &serde_json::to_vec(&FORMAT.encode(header))?,
            )
        })
        .await?
    }

    pub(crate) async fn change_header(
        &self,
        id: &str,
        edit: impl FnOnce(&mut Map<String, Value>) + Send + 'static,
    ) -> io::Result<bool> {
        let session = id.to_owned();
        self.mutate_existing(id, move |folder| {
            let mut header = read_header(folder, &session)?
                .ok_or_else(|| {
                    io::Error::new(ErrorKind::NotFound, "The coding-agent session is gone.")
                })?
                .into_fields();
            edit(&mut header);
            files::write_atomic(
                &folder.join("session.json"),
                &serde_json::to_vec(&FORMAT.encode(header))?,
            )
        })
        .await
    }

    pub async fn has_message(&self, id: &str, message: &str) -> io::Result<bool> {
        let Some(folder) = self.folder(id) else {
            return Ok(false);
        };
        let message = message.to_owned();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            if read_header(&folder, &id)?.is_none() {
                return Ok(false);
            }
            let transcript = folder.join(TRANSCRIPT);
            if transcript.try_exists()? {
                let db = sqlite::open_index(&transcript, Duration::from_secs(1))
                    .map_err(io::Error::other)?;
                return db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM message WHERE id = ?1)",
                        [&message],
                        |row| row.get(0),
                    )
                    .map_err(io::Error::other);
            }
            let Some(raw) = files::read(&folder.join("messages.json"))? else {
                return Ok(false);
            };
            let rows: Vec<Value> = serde_json::from_slice(&raw)?;
            Ok(rows.iter().any(|row| row["id"] == message))
        })
        .await?
    }

    pub(crate) async fn turn_messages(&self, id: &str, prefix: String) -> io::Result<Vec<Value>> {
        let folder = self.folder(id).ok_or(ErrorKind::InvalidInput)?;
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            if read_header(&folder, &id)?.is_none() {
                return Err(ErrorKind::NotFound.into());
            }
            let file = folder.join(TRANSCRIPT);
            if !file.try_exists()? {
                return Ok(Vec::new());
            }
            let db = sqlite::open_index(&file, Duration::from_secs(1)).map_err(io::Error::other)?;
            let mut statement = db.prepare(
                "SELECT body FROM message WHERE substr(id, 1, length(?1)) = ?1 ORDER BY position",
            ).map_err(io::Error::other)?;
            let rows = statement
                .query_map([prefix], |row| row.get::<_, String>(0))
                .map_err(io::Error::other)?;
            rows.map(|body| Ok(serde_json::from_str(&body.map_err(io::Error::other)?)?))
                .collect()
        })
        .await?
    }
}
