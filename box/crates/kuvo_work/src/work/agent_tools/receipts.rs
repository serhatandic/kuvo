//! A tool's call that changes something, with its operation id (`operation`, or the
//! `x-kuvo-operation` header), done once however many times it's sent: its answer kept and given
//! again, and what it got to kept as it goes (`run/tool-calls/<key>.json`), so one cut short goes on
//! from there.

use std::path::PathBuf;

use http::{HeaderMap, StatusCode};
use kuvo_server::Refusal;
use serde_json::{Value, json};

/// The tools a call of which, cut short, is gone on with rather than refused.
const RECOVERS: &[&str] = &[
    "send_message",
    "start_session",
    "schedule_wake_up",
    "watch_for_change",
    "cancel_wake_up",
];

pub struct Operation {
    pub key: String,
    digest: String,
    folder: PathBuf,
    recovers: bool,
}

fn refusal(status: StatusCode, message: &str) -> Refusal {
    Refusal::json(status, json!({ "message": message }))
}

fn digest(value: &Value) -> String {
    kuvo_wire::keys::encode(&kuvo_wire::json::digest(value))
}

impl Operation {
    /// The call's operation, if it names one: its key, from the conversation calling and the id.
    pub fn of(
        run: &std::path::Path,
        headers: &HeaderMap,
        given: &Value,
        name: &str,
    ) -> Result<Option<Self>, Refusal> {
        let invalid = || refusal(StatusCode::BAD_REQUEST, "Invalid tool operation id.");
        let header = headers
            .get("x-kuvo-operation")
            .map(|value| value.to_str().map(str::to_owned).map_err(|_| invalid()))
            .transpose()?;
        let field = given
            .get("operation")
            .map(|value| value.as_str().map(str::to_owned).ok_or_else(invalid))
            .transpose()?;
        if header.is_some() && field.is_some() && header != field {
            return Err(invalid());
        }
        let Some(nonce) = header.or(field) else {
            return Ok(None);
        };
        if !(1..=80).contains(&nonce.len())
            || !nonce
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return Err(invalid());
        }
        let scope = json!(["tool", "kuvo/agent-tools", given["session"]]).to_string();
        Ok(Some(Self {
            key: digest(&json!([scope, nonce])),
            digest: digest(given),
            folder: run.join("tool-calls"),
            recovers: RECOVERS.contains(&name),
        }))
    }

    fn receipt(&self) -> PathBuf {
        self.folder.join(format!("{}.receipt.json", self.key))
    }

    fn progress_file(&self) -> PathBuf {
        self.folder.join(format!("{}.json", self.key))
    }

    fn read(file: &std::path::Path) -> Option<Value> {
        serde_json::from_slice(&std::fs::read(file).ok()?).ok()
    }

    fn write(file: &std::path::Path, value: &Value) {
        if let Some(folder) = file.parent() {
            let _ = kuvo_store::files::create_private_dirs(folder);
        }
        let _ =
            serde_json::to_vec(value).map(|bytes| kuvo_store::files::write_secret(file, &bytes));
    }

    /// Claimed for this call: its answer where it was answered already; refused where the id is
    /// another call's, or one cut short that can't be gone on with.
    pub fn claim(&self) -> Result<Option<Value>, Refusal> {
        match Self::read(&self.receipt()) {
            Some(kept) if kept["digest"] != self.digest.as_str() => Err(refusal(
                StatusCode::CONFLICT,
                "That operation id is already in use.",
            )),
            Some(kept) if kept.get("answer").is_some() => Ok(Some(kept["answer"].clone())),
            Some(_) if !self.recovers => Err(refusal(
                StatusCode::CONFLICT,
                "The owner has claimed this operation. Check the record before trying again.",
            )),
            Some(_) => Ok(None),
            None => {
                Self::write(&self.receipt(), &json!({ "digest": self.digest }));
                Ok(None)
            }
        }
    }

    /// Answered: kept to give again, and what it got to let go.
    pub fn finish(&self, answer: &Value) {
        let at = jiff::Timestamp::now().as_millisecond();
        Self::write(
            &self.receipt(),
            &json!({ "digest": self.digest, "answer": answer, "at": at }),
        );
        let _ = std::fs::remove_file(self.progress_file());
    }

    /// What the call got to, as it was kept.
    pub fn progress(&self) -> Value {
        Self::read(&self.progress_file()).unwrap_or_else(|| json!({}))
    }

    pub fn keep(&self, progress: &Value) {
        Self::write(&self.progress_file(), progress);
    }
}
