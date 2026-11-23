use super::{LINK, new_setup, write_link};
use crate::{
    Error, checkpoint,
    lease::{Action, Client},
    mac_host::Overlay,
    model::{ComputerKind, Identity, Link, Position, Role, SharedKind, validate_position},
    storage::Files,
};
use kuvo_wire::{documents, keys::encode};

/// A moved Mac gets its old identity from the authenticated Helper. If that
/// identity led a checkpointed setup under the older authority model, keep
/// the same setup and mint its next checkpoint only under its relay lease.
pub(super) async fn convert(
    files: &Files,
    overlay: &Overlay,
    test_clock: bool,
) -> Result<(), Error> {
    let Some(mut link) = files.read::<Link>(LINK)? else {
        return Ok(());
    };
    let Some(prior) = link.setup.as_ref() else {
        return Ok(());
    };
    if prior.aside || prior.role != Role::Primary || !prior.managed || !prior.checkpointed {
        return Ok(());
    }
    let Some(tip) = link.leadership.get(&prior.position.epoch) else {
        return Ok(());
    };
    let tip = documents::verify(tip.clone())?;
    if tip.kind != documents::Kind::Leadership
        || (tip.body.get("relayTerm").is_some()
            && tip.body.get("standby").is_none()
            && tip.body.get("successorChain").is_none())
    {
        return Ok(());
    }

    let previous = prior.position.clone();
    let sign = prior.sign.clone();
    let signed_position = tip
        .body
        .get("setup")
        .cloned()
        .and_then(|value| serde_json::from_value::<Position>(value).ok());
    if tip.n != previous.epoch
        || encode(&tip.origin) != sign
        || signed_position.as_ref() != Some(&previous)
        || link.extra.contains_key("setupMerge")
        || link.value(SharedKind::SetupDeparture).is_some()
    {
        return Err(Error::Invalid("predecessor setup authority"));
    }
    overlay.apply(&mut link);
    let own = Identity::read(&link)?;
    if own.public != previous.primary || own.sign != sign {
        return Err(Error::Invalid("predecessor Mac identity"));
    }
    validate_position(&previous)?;
    link.member_list()?;
    let current_members = link
        .shared
        .get_mut(&SharedKind::MembershipAndAccess)
        .and_then(|membership| membership.value.as_object_mut())
        .ok_or(Error::Invalid("predecessor setup roster"))?;
    for field in [
        "standby",
        "successorOrder",
        "successorPin",
        "successorChain",
        "ranks",
    ] {
        current_members.shift_remove(field);
    }

    let mut lease = Client::new(files, &link, test_clock)?;
    let old = lease
        .request(files, &link, &own, Action::Read, 0, None)
        .await?;
    let (action, term) = match old {
        None => (Action::Claim, 0),
        Some((observed, _)) if observed.holder == own.public && observed.sign == own.sign => {
            (Action::Renew, observed.term)
        }
        Some(_) => return Err(Error::Conflict),
    };
    let observed = lease
        .request(files, &link, &own, action, term, None)
        .await?;
    let Some((observed, _)) = observed.filter(|_| lease.can_promote(&own)) else {
        return Err(Error::Conflict);
    };
    let next = Position {
        epoch: previous.epoch + 1,
        ..previous.clone()
    };
    let certificate = checkpoint::build(
        files,
        &own,
        next.clone(),
        Some(previous.primary),
        observed.term,
        &link,
    )?;
    link.leadership.insert(next.epoch, certificate.envelope);
    link.setup = Some(new_setup(next, own.sign, Role::Primary));
    write_link(files, &link, ComputerKind::Mac)
}
