//! Renaming a machine outside any setup: the released Mac that enrolled this
//! computer keeps the machines' records, so the change is made there.

use super::*;
use crate::{actor::LINK, model::Link};
use kuvo_peers::{
    devices::{RecordWriter, record_writer},
    session::ComputerKind,
};

const MOST_NAME: usize = 100;

/// A name as given for a computer, cleaned; `None` when empty or too long.
pub(crate) fn given_name(name: &str) -> Option<String> {
    let name = crate::preferences::short_name(name, MOST_NAME + 1);
    (!name.is_empty() && name.encode_utf16().count() <= MOST_NAME).then_some(name)
}

pub(super) fn bad_name() -> Refusal {
    Refusal::new(
        StatusCode::BAD_REQUEST,
        format!("Give it a name of up to {MOST_NAME} characters."),
    )
}

fn conflict(message: String) -> Refusal {
    Refusal::new(StatusCode::CONFLICT, message)
}

impl SetupHandle {
    /// The rename answered through a released Mac, or `None` where this
    /// computer's setup keeps the names.
    pub(super) async fn released_rename(
        &self,
        key: &str,
        given: &str,
        caller: &Caller,
        records: Option<&Arc<dyn RecordEdits>>,
    ) -> Result<Option<Response>, Refusal> {
        let files = self.files.clone();
        let link = tokio::task::spawn_blocking(move || files.read::<Link>(LINK))
            .await
            .map_err(|_| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, "The rename stopped."))?
            .map_err(refusal)?;
        let Some(link) = link.filter(|link| link.setup.is_none()) else {
            return Ok(None);
        };
        let RecordWriter::Mac(mac) = record_writer(&link.extra, link.key.as_deref()) else {
            return Ok(None);
        };
        let Some(records) = records else {
            return Ok(None);
        };
        let here = link.name.clone().unwrap_or_default();
        let own = crate::model::Identity::read(&link)
            .ok()
            .and_then(|identity| kuvo_wire::keys::decode_exact::<32>(&identity.public));
        let target = kuvo_wire::keys::decode_exact::<32>(key);
        let (named, kind) = match target {
            Some(target) if Some(target) == own => (here.clone(), ComputerKind::Machine),
            Some(target) => mac
                .listed
                .iter()
                .find(|member| member.key == target)
                .map(|member| (member.name.clone(), member.kind))
                .ok_or_else(|| {
                    Refusal::new(StatusCode::NOT_FOUND, "That isn't one of your computers.")
                })?,
            None => {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't one of your computers.",
                ));
            }
        };
        let name = given_name(given).ok_or_else(bad_name)?;
        let shown = if named.is_empty() { "This computer" } else { named.as_str() };
        if kind == ComputerKind::Mac {
            return Err(conflict(format!(
                "{shown} is a Mac: it goes by the name it has there."
            )));
        }
        if !mac.listed.iter().any(|member| member.key == mac.key) {
            return Err(conflict(format!(
                "{here} can't reach {}, which keeps your machines' records: rename {shown} from there.",
                mac.name
            )));
        }
        if !target.is_some_and(|target| mac.machines.contains(&target)) {
            return Err(conflict(format!(
                "{shown} isn't one of {}’s machines, so it can't be renamed from here.",
                mac.name
            )));
        }
        let computer = json!({"key":kuvo_wire::keys::encode(&mac.key),"name":mac.name,"kind":"mac"});
        let call = RecordEditCall::ReleasedRename {
            mac: mac.key,
            computer,
            named,
            key: key.to_owned(),
            name,
        };
        Ok(records
            .perform(caller.clone(), call)
            .await?
            .map(|answer| Response {
                status: answer.status,
                value: answer.value,
            }))
    }
}
