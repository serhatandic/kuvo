use serde_json::{Value, json};

use super::Owner;
use crate::{Error, edits, lease};

impl Owner {
    async fn settle_edit(&mut self, envelope: &edits::Envelope) -> Result<Option<Value>, Error> {
        if self.link.setup()?.position.id != envelope.setup || self.can_write().is_err() {
            return Ok(None);
        }
        let op = &envelope.edit.operation;
        let (next, receipt) = match edits::apply(&self.files, &self.link, self.own()?, envelope) {
            Ok(result) => result,
            Err(Error::Capacity) => {
                return Ok(Some(json!({
                    "operation": op, "status": "capacity", "message": "This edit exceeds the setup’s saved preference capacity.",
                })));
            }
            Err(Error::Conflict) => {
                return Ok(Some(edits::interrupted(
                    op,
                    "That operation id was already used for a different edit.",
                )));
            }
            Err(Error::Invalid(_)) => {
                return Ok(Some(edits::interrupted(
                    op,
                    "That is not an editable setup preference or value.",
                )));
            }
            Err(error) => return Err(error),
        };
        if let Some(next) = next {
            self.commit(next).await?;
            if let Err(error) = self.publish(false).await {
                eprintln!("publishing accepted setup edit: {error}");
            }
        }
        Ok(Some(receipt))
    }

    pub(super) async fn deliver_edit(
        &mut self,
        outgoing: &mut edits::Outgoing,
    ) -> Result<(), Error> {
        let Some(mut envelope) = outgoing.queued_edit.clone() else {
            return Ok(());
        };
        let setup = &self.link.setup()?.position;
        if outgoing.setup != setup.id {
            return Ok(());
        }
        if outgoing.owner != setup.primary {
            outgoing.owner = setup.primary.clone();
            outgoing.keep(&self.files)?;
        }
        if outgoing.owner == self.own()?.public {
            if let Some(receipt) = self.settle_edit(&envelope).await? {
                outgoing.finish(&self.files, receipt)?;
            }
        } else if let Some(peer) = self.host(&outgoing.owner) {
            envelope.replay_only = !outgoing.submitter.current(&self.link);
            self.send(
                &peer,
                json!({"scopedOfflineEdit": envelope, "at": lease::wall()}),
            )
            .await;
        }
        Ok(())
    }

    pub(super) async fn flush_edits(&mut self) -> Result<(), Error> {
        if self.link.setup().is_err() || self.identity.is_none() {
            return Ok(());
        }
        for mut outgoing in edits::rows(&self.files)? {
            self.deliver_edit(&mut outgoing).await?;
        }
        Ok(())
    }

    pub(super) async fn receive_edit(&mut self, from: &str, value: &Value) -> Result<(), Error> {
        let envelope = edits::Envelope::read(value, from)?;
        if let Some(receipt) = self.settle_edit(&envelope).await?
            && let Some(peer) = self.host(from)
        {
            self.send(&peer, json!({"offlineEditReceipt": {"setup": envelope.setup, "receipt": receipt}, "at": lease::wall()})).await;
        }
        Ok(())
    }

    pub(super) fn receive_edit_receipt(&self, from: &str, value: &Value) -> Result<(), Error> {
        if let Some((setup, operation, receipt)) = edits::received_receipt(value)
            && self
                .link
                .setup()
                .is_ok_and(|current| current.position.id == setup)
            && let Some(mut outgoing) = edits::read_outgoing(&self.files, operation)?
            && outgoing.setup == setup
            && outgoing.owner == from
        {
            outgoing.finish(&self.files, receipt)?;
        }
        Ok(())
    }
}
