use std::{collections::HashMap, time::Duration};

use kuvo_peers::{
    relay,
    session::{BoxHello, ComputerKind as PeerKind},
};
use kuvo_store::{home::Home, layout, locks::OwnedLock};
use kuvo_wire::{
    documents::{self, Document, Envelope, Kind, Part, Parts},
    keys::{Host, address_of, encode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Applicant, CLAIM_LIFETIME, Code, hash};
use crate::{
    Error,
    actor::{LINK, new_setup, project_membership, remember_link_key, validate_disk, write_link},
    document, lease,
    model::{
        ComputerKind, Entry, Identity, Link, Membership, Position, Role, SharedKind, safe_token,
        validate_position,
    },
    random,
    storage::Files,
};

const KIND: ComputerKind = ComputerKind::Machine;
const NO_CODE: &str = "Enter the installer code shown on your device.";
const RETRY: &str = "Your main computer didn't finish adding this computer. Check that it’s online and run the installer again with the same code.";
const OFFLINE: &str =
    "Couldn't connect to your setup's relay. Run the installer again with the same code.";
const IN_USE: &str = "This computer already belongs to a setup or has paired devices. Unlink it before adding it to another setup.";
const HAS_WORK: &str = "This computer has setup data. Use a setup merge to bring that data along.";

#[derive(Debug, Serialize)]
pub struct Joined {
    pub setup: String,
    pub primary: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalClaim {
    setup: String,
    invite: String,
    until: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery: Option<Recovery>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    adopted: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    activation_pending: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    portable_activation: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    cancel_pending: bool,
    #[serde(default, flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Recovery {
    key: String,
    secret: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    nonce: String,
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    recovery_capable: bool,
    #[serde(default)]
    activation_receipt: bool,
    #[serde(default)]
    activated: bool,
    #[serde(default)]
    portable_activation: bool,
    #[serde(default)]
    copy_credentials: bool,
    message: Option<String>,
    docs: Option<Docs>,
}

#[derive(Deserialize)]
struct Docs {
    #[serde(default)]
    give: Vec<Envelope>,
    part: Option<Part>,
}

enum Exchange {
    Enroll,
    Activate { portable: bool },
}

struct Accepted {
    doc: Document,
    activation: bool,
    portable: bool,
}

pub async fn join(home: &Home, name: String, input: &str) -> Result<Joined, Error> {
    let text = input.trim();
    if text.len() <= 4096
        && let Some((version, _)) = text
            .strip_prefix("kuvo-add-")
            .and_then(|token| token.split_once('.'))
        && version != "1"
    {
        let shown = if !version.is_empty()
            && version.len() <= 15
            && version.bytes().all(|b| b.is_ascii_digit())
        {
            version
                .parse::<u64>()
                .map_or_else(|_| "invalid".into(), |n| n.to_string())
        } else {
            "invalid".into()
        };
        return Err(Error::Installer(format!(
            "kuvo-add format version {shown} is unsupported (this build writes 1)."
        )));
    }
    let (code, primary) = Code::read(text).ok_or_else(|| refused(NO_CODE))?;
    if code.merge.is_some() {
        return Err(refused(
            "Use Merge Setups on the setup this code was made for.",
        ));
    }
    // Opening for enrollment never starts a move. A fresh applicant can
    // initialize its layout here, after the code is validated.
    layout::open(home)?;
    let files = Files::new(home.root().to_owned());
    // The applicant's installer is the only Rust writer while it replaces the local identity.
    let owner = files.path("run/.client-owner");
    let _owner = OwnedLock::acquire(&owner, Duration::ZERO).map_err(|source| Error::File {
        action: "claiming the installer home at",
        path: owner,
        source,
    })?;
    validate_disk(&files)?;
    if std::fs::read_to_string(files.path("box/kind")).is_ok_and(|kind| kind.trim() == "mac") {
        return Err(refused(
            "This Mac cannot connect to the relay in that installer code.",
        ));
    }
    let mut saved_link = SavedLink {
        files: &files,
        previous: files.read(LINK)?,
    };
    let mut link: Link = saved_link
        .previous
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| Error::Invalid("installer link"))?
        .unwrap_or_default();
    let id = hash(&code.token);
    let mut claim = link
        .extra
        .get("installerClaim")
        .cloned()
        .map(serde_json::from_value::<LocalClaim>)
        .transpose()
        .map_err(|_| Error::Invalid("installer claim"))?;
    let already_joined = link.setup.as_ref().is_some_and(|setup| {
        setup.role == Role::Member
            && setup.position.primary == code.primary
            && setup.position.id == code.setup
    }) && claim
        .as_ref()
        .is_some_and(|claim| claim.invite == id && claim.adopted);
    let blocked = [
        "localUnlink",
        "localWipe",
        "removalSource",
        "setupMerge",
        "macRotation",
    ]
    .iter()
    .any(|field| link.extra.get(*field).is_some_and(|value| !value.is_null()));
    if blocked {
        return Err(refused(IN_USE));
    }
    if !already_joined {
        check_join(&files, &link, &code, &id, claim.as_ref())?;
        if link.extra.get("codeIdentity").and_then(Value::as_str) != Some(&id)
            && !claim.as_ref().is_some_and(|claim| claim.invite == id)
        {
            link.key = Some(encode(&random::<32>()?));
            link.sign = Some(encode(&random::<32>()?));
            link.secret = Some(encode(&random::<32>()?));
            // The old bootstrap certificate belongs to the old signer.
            link.setup = None;
            link.leadership.clear();
            link.shared_setup = None;
            link.shared.clear();
            link.shared_outcomes.clear();
            for field in ["own", "docs", "setupReplica", "installerClaim"] {
                link.extra.shift_remove(field);
            }
            claim = None;
            link.extra.insert("codeIdentity".into(), json!(id));
        }
        link.name
            .get_or_insert_with(|| crate::preferences::truncate(&name, 100));
        link.extra.entry("phones").or_insert_with(|| json!([]));
    }
    if already_joined && let Some(claim) = &claim {
        if ["credentialsPending", "credentialsAckPending"]
            .iter()
            .any(|field| claim.extra.get(*field) == Some(&Value::Bool(true)))
        {
            return Err(refused(
                "Update this computer’s installer to copy the sign-ins and keys selected for this code.",
            ));
        }
        if !claim.activation_pending {
            return Ok(Joined {
                setup: code.setup,
                primary: code.primary,
            });
        }
    }
    let identity = Identity::read(&link)?;
    if !already_joined {
        remember_link_key(&mut link, &identity, KIND);
        saved_link.write(&link)?;
    }
    let name = link.name.clone().unwrap_or(name);
    let mut relay = relay::Relay::start(relay::Config {
        url: code.relay.clone(),
        secret: random::<32>()?,
        key: identity.key,
        hello: BoxHello {
            v: 1,
            kind: PeerKind::Machine,
            name: name.clone(),
            app: env!("CARGO_PKG_VERSION").into(),
            engine: None,
        },
        grants: HashMap::new(),
        peers: Vec::new(),
    });
    let local = identity.self_peer(KIND, &name);
    let mut given = Applicant {
        token: code.token.clone(),
        nonce: String::new(),
        address: local.address.clone(),
        sign: identity.sign.clone(),
        name,
        kind: KIND,
        confirmed: false,
        activation_ack: false,
        activation_ack_capable: true,
        recovery_commitment: None,
        credential_receipt: false,
        cancel: false,
        merging: false,
        credential_ack: false,
    };
    if !already_joined {
        let accepted = exchange(
            &mut relay,
            &primary,
            &mut given,
            Exchange::Enroll,
            |recovery_capable| {
                check_join(&files, &link, &code, &id, claim.as_ref())?;
                if !claim.as_ref().is_some_and(|claim| claim.invite == id) {
                    let recovery = if recovery_capable {
                        Some(Recovery {
                            key: identity.public.clone(),
                            secret: encode(&random::<24>()?),
                        })
                    } else {
                        None
                    };
                    claim = Some(LocalClaim {
                        setup: code.setup.clone(),
                        invite: id.clone(),
                        until: lease::wall() + CLAIM_LIFETIME,
                        recovery,
                        adopted: false,
                        activation_pending: false,
                        portable_activation: false,
                        cancel_pending: false,
                        extra: Default::default(),
                    });
                }
                let reserved = claim.as_ref().ok_or(Error::Conflict)?;
                saved_link.keep_claim(&mut link, reserved)?;
                Ok(reserved
                    .recovery
                    .as_ref()
                    .map(|recovery| hash(&recovery.secret)))
            },
        )
        .await?
        .ok_or(Error::Invalid("installer membership"))?;
        check_join(&files, &link, &code, &id, claim.as_ref())?;
        adopt(&mut link, &identity, &code, accepted.doc)?;
        let saved = claim.as_mut().ok_or(Error::Conflict)?;
        saved.adopted = true;
        saved.activation_pending = accepted.activation;
        saved.portable_activation = accepted.portable && accepted.activation;
        saved.recovery = None;
        for field in [
            "pairing",
            "appPairing",
            "releasedSetup",
            "releasedWork",
            "releasedDatabaseWork",
            "releasedScopedWork",
            "codeIdentity",
        ] {
            link.extra.shift_remove(field);
        }
        saved_link.keep_claim(&mut link, saved)?;
    }
    if let Some(claim) = &mut claim
        && claim.activation_pending
        && exchange(
            &mut relay,
            &primary,
            &mut given,
            Exchange::Activate {
                portable: claim.portable_activation,
            },
            |_| Ok(None),
        )
        .await
        .is_ok()
    {
        claim.activation_pending = false;
        claim.portable_activation = false;
        saved_link.keep_claim(&mut link, claim)?;
    }
    Ok(Joined {
        setup: code.setup,
        primary: code.primary,
    })
}

struct SavedLink<'a> {
    files: &'a Files,
    previous: Option<Value>,
}

impl SavedLink<'_> {
    fn write(&mut self, link: &Link) -> Result<(), Error> {
        // A TypeScript one-shot writer may still use the link lock while the
        // resident is stopped. Check the exact record under that same lock.
        let lock = self.files.path("run/.link.lock");
        let _guard = OwnedLock::acquire(&lock, Duration::ZERO).map_err(|source| Error::File {
            action: "claiming",
            path: lock,
            source,
        })?;
        if self.files.read::<Value>(LINK)? != self.previous {
            return Err(refused(
                "This computer’s identity changed. Run the installer again.",
            ));
        }
        write_link(self.files, link, KIND)?;
        self.previous = Some(json!(link));
        Ok(())
    }

    fn keep_claim(&mut self, link: &mut Link, claim: &LocalClaim) -> Result<(), Error> {
        link.format_version = 1;
        link.extra.insert("installerClaim".into(), json!(claim));
        self.write(link)
    }
}

fn check_join(
    files: &Files,
    link: &Link,
    code: &Code,
    id: &str,
    claim: Option<&LocalClaim>,
) -> Result<(), Error> {
    if link
        .setup
        .as_ref()
        .is_some_and(|setup| setup.role == Role::Member || setup.aside)
        || ["phones", "macs", "macPeers", "preJoinPairings"]
            .iter()
            .any(|field| {
                link.extra
                    .get(*field)
                    .and_then(Value::as_array)
                    .is_some_and(|rows| !rows.is_empty())
            })
        || link.extra.get("mac").and_then(Value::as_str).is_some()
    {
        return Err(refused(IN_USE));
    }
    if link
        .setup
        .as_ref()
        .is_some_and(|setup| setup.position.primary == code.primary)
        || Identity::read(link).is_ok_and(|own| own.public == code.primary)
    {
        return Err(refused("This computer is already your main computer."));
    }
    if has_work(files, link).unwrap_or(true) {
        return Err(refused(HAS_WORK));
    }
    let adding = files.path("../.kuvo-code-adding");
    match adding.symlink_metadata() {
        Ok(_) => {
            return Err(refused(
                "An SSH setup is already in progress on this computer.",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(source) => {
            return Err(Error::File {
                action: "reading",
                path: adding,
                source,
            });
        }
    }
    let now = lease::wall();
    if let Some(claim) = claim {
        if !claim.adopted && claim.until > now && claim.invite != id {
            return Err(refused(
                "An installer enrollment is already in progress. Retry with its original code.",
            ));
        }
        if claim.invite == id && (claim.cancel_pending || claim.until <= now) {
            return Err(refused(
                "The installer claim expired or was cancelled. Finish recovering it before enrolling again.",
            ));
        }
    }
    if code.expires <= now && !claim.is_some_and(|claim| claim.invite == id) {
        return Err(refused(
            "This installer code has expired. Get a new code from your setup.",
        ));
    }
    Ok(())
}

// Until the move reader is available, conservatively keep homes with existing
// work in place. The installer only replaces an empty setup's identity.
fn has_work(files: &Files, link: &Link) -> Result<bool, Error> {
    let now = lease::wall();
    if link.installer_invites.values().any(|entry| entry.live(now))
        || (!link.shared.is_empty() && !bootstrap_only(link))
        || link
            .extra
            .get("untold")
            .and_then(Value::as_array)
            .is_some_and(|rows| !rows.is_empty())
        || link
            .extra
            .get("appPairing")
            .and_then(|offer| offer["expires"].as_u64())
            .is_some_and(|end| end > now)
        || link.extra.get("pairing").is_some_and(|offer| {
            offer.get("issuer").is_some() && offer["expires"].as_u64().is_some_and(|end| end > now)
        })
    {
        return Ok(true);
    }
    if link.extra.contains_key("releasedWork") {
        return super::released::unchanged(files, link).map(|unchanged| !unchanged);
    }
    for directory in [
        "agents/sessions",
        "chats",
        "groups",
        "artifacts",
        "attachments",
        "wakes",
        "run/work",
        "run/work-queue",
        "run/owned-data/outbox",
        "run/owned-data/edit-outbox",
        "run/outbox",
        "data/opencode/storage/session",
    ] {
        if files
            .names(directory)?
            .iter()
            .any(|name| name != ".DS_Store")
        {
            return Ok(true);
        }
    }
    for mark in [
        "bots/personalities.json",
        "box/time-zone.json",
        "agents/usage/settings.json",
        "run/session-settings.json",
        "run/named.json",
        "run/archived.json",
    ] {
        if files
            .read::<Value>(mark)?
            .is_some_and(|value| value != json!({}))
        {
            return Ok(true);
        }
    }
    for bot in files.names("bots")? {
        let path = files.path("bots").join(bot);
        if path.is_symlink() || path.join("bot.json").exists() {
            return Ok(true);
        }
    }
    Ok(files
        .names("data/opencode")?
        .iter()
        .any(|name| name.starts_with("opencode") && name.ends_with(".db")))
}

fn bootstrap_only(link: &Link) -> bool {
    let Ok(setup) = link.setup() else {
        return false;
    };
    let Ok(roster) = link.member_list() else {
        return false;
    };
    setup.role == Role::Primary
        && link.shared_outcomes.is_empty()
        && link.shared.len() == 1
        && link
            .shared
            .get(&SharedKind::MembershipAndAccess)
            .is_some_and(|entry| entry.sequence == 1)
        && roster.boxes.len() == 1
        && roster.left.is_empty()
        && roster.devices().next().is_none()
        && [
            "moved",
            "removalRevisions",
            "removals",
            "wipePreparations",
            "mergedComputers",
            "installerRecoveries",
        ]
        .iter()
        .all(|field| roster.extra.get(*field).is_none_or(empty_collection))
        && roster
            .extra
            .get("devices")
            .and_then(|devices| devices.get("removed"))
            .is_none_or(empty_collection)
        && !roster.extra.contains_key("computerHistory")
}

fn empty_collection(value: &Value) -> bool {
    value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
}

async fn exchange(
    relay: &mut relay::Relay,
    to: &Host,
    given: &mut Applicant,
    mode: Exchange,
    mut reserve: impl FnMut(bool) -> Result<Option<String>, Error>,
) -> Result<Option<Accepted>, Error> {
    given.nonce = encode(&random::<24>()?);
    given.activation_ack = matches!(mode, Exchange::Activate { .. });
    given.confirmed = false;
    // Only one socket exists for this temporary relay address, including the activation round.
    // Subscribing after connect is safe: the timer retries sends until the socket is ready.
    let mut observations = relay.handle().subscribe().ok_or_else(|| refused(OFFLINE))?;
    let max = Duration::from_secs(if given.activation_ack { 5 } else { 60 });
    let mut resend = tokio::time::interval(Duration::from_millis(1800));
    resend.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut parts = Parts::default();
    let deadline = tokio::time::sleep(max);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => return Err(refused(RETRY)),
            event = relay.events.recv() => match event {
                Some(relay::Event::Connected(_)) => resend.reset_immediately(),
                Some(relay::Event::Disconnected(_)) | None => return Err(refused(OFFLINE)),
                _ => {}
            },
            _ = resend.tick() => {
                let payload = serde_json::to_vec(given).expect("installer claim serializes");
                let _ = relay.sender.installer(to.clone(), payload).await;
            }
            event = observations.recv() => {
                let Ok(event) = event else { continue };
                let relay::RelayEvent::Installer { from, key, payload } = &*event else { continue };
                if *from != to.address || *key != to.key {
                    continue;
                }
                let Ok(reply) = serde_json::from_slice::<Reply>(payload) else { continue };
                if reply.nonce != given.nonce {
                    continue;
                }
                if let Exchange::Activate { portable } = mode {
                    if reply.activated && (!portable || reply.portable_activation) {
                        return Ok(None);
                    }
                    continue;
                }
                if let Some(message) = reply.message {
                    return Err(Error::Installer(message));
                }
                if reply.ready && !given.confirmed {
                    given.recovery_commitment = reserve(reply.recovery_capable)?;
                    given.confirmed = true;
                    resend.reset_immediately();
                    continue;
                }
                if given.confirmed && let Some(docs) = reply.docs {
                    let doc = if let Some(part) = docs.part {
                        parts.receive(*key, part).ok().flatten().filter(|doc| doc.kind == Kind::Peers)
                    } else {
                        docs.give.into_iter().filter_map(|env| documents::verify(env).ok()).find(|doc| doc.kind == Kind::Peers)
                    };
                    if let Some(doc) = doc {
                        if reply.copy_credentials {
                            return Err(refused("Update this computer’s installer to copy the sign-ins and keys selected for this code."));
                        }
                        return Ok(Some(Accepted { doc, activation: reply.activation_receipt, portable: reply.portable_activation }));
                    }
                }
            }
        }
    }
}

fn adopt(link: &mut Link, identity: &Identity, code: &Code, doc: Document) -> Result<(), Error> {
    let invalid = || refused("The main computer didn’t send this computer's accepted membership.");
    let position: Position =
        serde_json::from_value(doc.body.get("setup").cloned().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    let leadership = doc
        .body
        .get("leadershipHash")
        .and_then(Value::as_str)
        .filter(|hash| safe_token(hash, 11, 11));
    if validate_position(&position).is_err()
        || position.id != code.setup
        || position.primary != code.primary
        || doc.body.get("managed") != Some(&Value::Bool(true))
        || doc.body.get("checkpointed") != Some(&Value::Bool(true))
        || leadership.is_none()
    {
        return Err(invalid());
    }
    let sign = encode(&doc.origin);
    let value = Value::Object(doc.body.clone());
    let roster = Membership::parse(&value, &position, &sign).map_err(|_| invalid())?;
    if !roster.boxes.iter().any(|each| {
        each.key == identity.public
            && each.sign.as_ref() == Some(&identity.sign)
            && each.address == encode(&address_of(&identity.secret))
    }) {
        return Err(invalid());
    }
    let mut setup = new_setup(position.clone(), sign, Role::Member);
    setup
        .extra
        .insert("leadershipHash".into(), json!(leadership));
    link.setup = Some(setup);
    link.relay = Some(code.relay.clone());
    link.shared.clear();
    link.leadership.clear();
    link.shared_outcomes.clear();
    link.shared_setup = Some(position.id);
    // The signed roster is sufficient to connect. The main sends the full signed
    // replica and checkpoint when the new resident announces availability.
    let membership = Membership {
        extra: Default::default(),
        ..roster
    };
    link.shared.insert(
        SharedKind::MembershipAndAccess,
        Entry {
            sequence: 1,
            value: json!(membership),
            offline_edits: None,
        },
    );
    project_membership(link, &membership, identity)?;
    *link = document::take(link, &doc)?.ok_or_else(invalid)?;
    Ok(())
}

fn refused(message: &str) -> Error {
    Error::Installer(message.into())
}
