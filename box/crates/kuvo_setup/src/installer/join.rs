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

use super::{Applicant, CLAIM_LIFETIME, Cancellation, Code, credentials, hash};
use crate::{
    Error,
    actor::{
        LINK, bootstrap, new_setup, project_membership, remember_link_key, validate_disk,
        write_link,
    },
    document, lease,
    model::{
        ComputerKind, Entry, Identity, Link, Membership, Position, Role, SharedKind, safe_token,
        validate_position,
    },
    random,
    storage::Files,
    wipe,
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    credentials_pending: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    credentials_ack_pending: bool,
    #[serde(default, flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Recovery {
    key: String,
    secret: String,
}

impl LocalClaim {
    fn cancellation(&self) -> Option<Cancellation> {
        let recovery = self.recovery.as_ref().filter(|_| !self.adopted)?;
        Some(Cancellation {
            setup: self.setup.clone(),
            invite: self.invite.clone(),
            key: recovery.key.clone(),
            secret: recovery.secret.clone(),
        })
    }
}

pub fn cancellation_receipts(home: &Home) -> Result<Vec<String>, Error> {
    let files = Files::new(home.root().to_owned());
    let previous: Option<Value> = files.read(LINK)?;
    let Some(value) = previous.clone() else {
        return Err(refused(
            "This computer has no installer cancellation receipt.",
        ));
    };
    let mut link: Link =
        serde_json::from_value(value).map_err(|_| Error::Invalid("installer link"))?;
    let mut archived = link
        .extra
        .get("installerCancellations")
        .cloned()
        .map(serde_json::from_value::<Vec<Cancellation>>)
        .transpose()
        .map_err(|_| Error::Invalid("installer cancellation receipts"))?
        .unwrap_or_default();
    if link
        .setup
        .as_ref()
        .is_none_or(|setup| setup.role != Role::Member)
        && !link.extra.contains_key("macRotation")
        && let Some(mut claim) = local_claim(&link)?
        && let Some(receipt) = claim.cancellation()
        && (claim.cancel_pending
            || claim.until <= lease::wall()
            || has_work(&files, &link).unwrap_or(true))
    {
        if !claim.cancel_pending {
            claim.cancel_pending = true;
            SavedLink {
                files: &files,
                previous,
            }
            .keep_claim(&mut link, &claim)?;
        }
        archived.push(receipt);
    }
    if archived.is_empty() {
        return Err(refused(
            "This computer has no installer cancellation receipt.",
        ));
    }
    Ok(archived.iter().map(Cancellation::encode).collect())
}

fn local_claim(link: &Link) -> Result<Option<LocalClaim>, Error> {
    link.extra
        .get("installerClaim")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| Error::Invalid("installer claim"))
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
    #[serde(default)]
    credential_complete: bool,
    #[serde(default)]
    cancelled: bool,
    credential_until: Option<u64>,
    credential_problem: Option<String>,
    seed: Option<Value>,
    seed_part: Option<Part>,
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
    Cancel,
    Activate { portable: bool },
    AcknowledgeCredentials,
}

struct Accepted {
    doc: Document,
    activation: bool,
    portable: bool,
    credentials: Option<CredentialCopy>,
}

struct CredentialCopy {
    seed: Result<Value, String>,
    until: Option<u64>,
}

pub async fn join(home: &Home, name: String, input: &str) -> Result<Joined, Error> {
    join_and_reinstall(home, name, input, false).await
}

pub async fn join_ssh(home: &Home, name: String, input: &str) -> Result<Joined, Error> {
    join_and_reinstall(home, name, input, true).await
}

async fn join_and_reinstall(
    home: &Home,
    name: String,
    input: &str,
    staged: bool,
) -> Result<Joined, Error> {
    let joined = join_at(home, name, input, staged).await?;
    if let Err(error) = wipe::finish_reinstall(home.root()) {
        eprintln!("clearing the former installation’s wipe receipt: {error}");
    }
    Ok(joined)
}

pub fn validate_join_code(input: &str) -> Result<(), Error> {
    read_join_code(input).map(|_| ())
}

fn read_join_code(input: &str) -> Result<(Code, Host), Error> {
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
    Ok((code, primary))
}

async fn join_at(home: &Home, name: String, input: &str, staged: bool) -> Result<Joined, Error> {
    let (code, primary) = read_join_code(input)?;
    let prior = wipe::install_state(home.root()).map_err(|source| Error::File {
        action: "checking the earlier wipe at",
        path: home.root().to_owned(),
        source,
    })?;
    if let Some(message) = prior.as_ref().and_then(wipe::InstallState::pending_message) {
        return Err(refused(message));
    }
    let ssh_account = if staged {
        SshAccount::inherited(home.root())?
    } else {
        None
    };
    // Opening for enrollment never starts a move. A fresh applicant can
    // initialize its layout here, after the code is validated.
    if staged {
        open_staged_home_at(home, ssh_account.as_ref())?;
    } else {
        layout::open(home)?;
    }
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
    let mut claim = local_claim(&link)?;
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
    let mut cancellation = None;
    if !already_joined {
        link.name
            .get_or_insert_with(|| crate::preferences::truncate(&name, 100));
        let pending_claim = claim.as_ref().filter(|held| {
            held.invite == id
                && !held.adopted
                && link
                    .setup
                    .as_ref()
                    .is_none_or(|setup| setup.role != Role::Member)
        });
        if pending_claim.is_some_and(|held| {
            held.cancel_pending || (held.recovery.is_some() && held.until <= lease::wall())
        }) {
            cancellation = Some(refused("This computer could not join."));
        } else if let Err(problem) = check_join(
            &files,
            &link,
            &code,
            &id,
            claim.as_ref(),
            ssh_account.as_ref(),
        ) {
            if pending_claim.is_none() {
                return Err(problem);
            }
            cancellation = Some(problem);
        }
        if cancellation.is_none()
            && link.extra.get("codeIdentity").and_then(Value::as_str) != Some(&id)
            && !claim.as_ref().is_some_and(|claim| claim.invite == id)
        {
            let standalone = link.setup.take();
            link.key = Some(encode(&random::<32>()?));
            link.sign = Some(encode(&random::<32>()?));
            link.secret = Some(encode(&random::<32>()?));
            // The old bootstrap certificate belongs to the old signer.
            link.leadership.clear();
            link.shared_setup = None;
            link.shared.clear();
            link.shared_outcomes.clear();
            if let Some(receipt) = claim.as_ref().and_then(LocalClaim::cancellation) {
                link.extra
                    .entry("installerCancellations")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .ok_or(Error::Invalid("installer cancellation receipts"))?
                    .push(json!(receipt));
            }
            for field in ["own", "docs", "setupReplica", "installerClaim"] {
                link.extra.shift_remove(field);
            }
            claim = None;
            link.extra.insert("codeIdentity".into(), json!(id));
            if let Some(standalone) = standalone {
                let released = link.extra.get("releasedSetup").and_then(Value::as_str)
                    == Some(&standalone.position.id);
                let own = Identity::read(&link)?;
                bootstrap(&files, &mut link, &own, KIND)?;
                if released {
                    let setup = link.setup()?;
                    link.extra
                        .insert("releasedSetup".into(), json!(setup.position.id));
                }
            }
        }
        link.extra.entry("phones").or_insert_with(|| json!([]));
    }
    if already_joined
        && claim.as_ref().is_some_and(|claim| {
            !claim.activation_pending
                && !claim.credentials_pending
                && !claim.credentials_ack_pending
        })
    {
        return Ok(Joined {
            setup: code.setup,
            primary: code.primary,
        });
    }
    let identity = Identity::read(&link)?;
    if !already_joined && cancellation.is_none() {
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
        credential_receipt: true,
        cancel: false,
        merging: false,
        credential_ack: false,
    };
    if let Some(problem) = cancellation {
        let claim = claim.as_mut().ok_or(Error::Conflict)?;
        return cancel_join(
            &mut saved_link,
            &mut link,
            claim,
            &mut relay,
            &primary,
            &mut given,
            problem,
        )
        .await;
    }
    let mut copy = None;
    if !already_joined {
        let accepted = exchange(
            &mut relay,
            &primary,
            &mut given,
            Exchange::Enroll,
            |recovery_capable| {
                check_join(
                    &files,
                    &link,
                    &code,
                    &id,
                    claim.as_ref(),
                    ssh_account.as_ref(),
                )?;
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
                        credentials_pending: false,
                        credentials_ack_pending: false,
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
        .await;
        if let Some(held) = claim.as_mut()
            && let Err(problem) =
                check_join(&files, &link, &code, &id, Some(held), ssh_account.as_ref())
        {
            return cancel_join(
                &mut saved_link,
                &mut link,
                held,
                &mut relay,
                &primary,
                &mut given,
                problem,
            )
            .await;
        }
        let accepted = accepted?.ok_or(Error::Invalid("installer membership"))?;
        adopt(&mut link, &identity, &code, accepted.doc)?;
        let saved = claim.as_mut().ok_or(Error::Conflict)?;
        saved.adopted = true;
        saved.activation_pending = accepted.activation;
        saved.portable_activation = accepted.portable && accepted.activation;
        saved.credentials_pending = accepted.credentials.is_some();
        if let Some(until) = accepted.credentials.as_ref().and_then(|copy| copy.until) {
            saved.until = until;
        }
        copy = accepted.credentials;
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
    if already_joined
        && claim
            .as_ref()
            .is_some_and(|claim| claim.credentials_pending)
    {
        copy = exchange(&mut relay, &primary, &mut given, Exchange::Enroll, |_| {
            Ok(None)
        })
        .await?
        .and_then(|accepted| accepted.credentials);
        if copy.is_none() {
            return Err(refused(
                "This computer joined. The selected sign-ins and keys are no longer available.",
            ));
        }
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
    if let Some(choice) = copy {
        let seed = choice
            .seed
            .map_err(|problem| Error::Installer(format!("This computer joined. {problem}")))?;
        let _copy_lock = crate::seed::lock(&files).map_err(|_| refused(
            "This computer joined. Run the installer again with the same code to complete the selected sign-ins and keys."
        ))?;
        let expected = saved_link.previous.as_ref().ok_or(Error::Conflict)?;
        if !crate::seed::apply_installer(&files, &seed, expected).await {
            return Err(refused(
                "This computer joined. Run the installer again with the same code to complete the selected sign-ins and keys.",
            ));
        }
        let claim = claim.as_mut().ok_or(Error::Conflict)?;
        claim.credentials_pending = false;
        claim.credentials_ack_pending = true;
        saved_link.keep_claim(&mut link, claim)?;
    }
    if let Some(claim) = &mut claim
        && !claim.activation_pending
        && claim.credentials_ack_pending
        && exchange(
            &mut relay,
            &primary,
            &mut given,
            Exchange::AcknowledgeCredentials,
            |_| Ok(None),
        )
        .await
        .is_ok()
    {
        claim.credentials_ack_pending = false;
        saved_link.keep_claim(&mut link, claim)?;
    }
    Ok(Joined {
        setup: code.setup,
        primary: code.primary,
    })
}

async fn cancel_join(
    saved: &mut SavedLink<'_>,
    link: &mut Link,
    claim: &mut LocalClaim,
    relay: &mut relay::Relay,
    primary: &Host,
    given: &mut Applicant,
    problem: Error,
) -> Result<Joined, Error> {
    const RECOVERY: &str = "Run installer-recovery issue on this computer and installer-recovery apply on the setup’s current main computer.";
    if !claim.cancel_pending {
        claim.cancel_pending = true;
        saved.keep_claim(link, claim)?;
    }
    if claim.recovery.is_some() && claim.until <= lease::wall() {
        return Err(Error::Installer(format!(
            "{problem} The installer claim expired. {RECOVERY}"
        )));
    }
    if let Err(error) = exchange(relay, primary, given, Exchange::Cancel, |_| Ok(None)).await {
        let detail = if claim.recovery.is_some() {
            RECOVERY.to_owned()
        } else {
            format!(
                "{error} Run the installer again with the original code to finish cancelling the enrollment."
            )
        };
        return Err(Error::Installer(format!("{problem} {detail}")));
    }
    let key = Identity::read(link)?.public;
    link.extra.insert("departedKey".into(), json!(key));
    link.extra.shift_remove("installerClaim");
    saved.write(link)?;
    Err(Error::Installer(format!(
        "{problem} The enrollment was cancelled. This computer kept its own setup."
    )))
}

pub fn open_staged_home(home: &Home) -> Result<(), Error> {
    open_staged_home_at(home, None)
}

fn open_staged_home_at(home: &Home, ssh_account: Option<&SshAccount>) -> Result<(), Error> {
    use std::fs;

    if layout::inspect(home)? != layout::Layout::Unmarked {
        return Ok(layout::open(home)?);
    }
    let _guard = layout::lock_for_move(home, Duration::from_secs(3)).map_err(layout::Error::Io)?;
    if layout::inspect(home)? != layout::Layout::Unmarked {
        return Ok(layout::open(home)?);
    }
    let ordinary = || {
        let entries = fs::read_dir(home.root()).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            match entry.file_name().to_str()? {
                ".folder-upgrade.lock" => (),
                "run"
                    if fs::read_dir(entry.path()).ok()?.all(|file| {
                        file.is_ok_and(|file| {
                            ssh_account.is_some_and(|account| {
                                let path = file.path();
                                path == account.marker || path == account.lock
                            })
                        })
                    }) =>
                {
                    ()
                }
                "bin"
                    if fs::read_dir(entry.path()).ok()?.all(|file| {
                        file.is_ok_and(|file| {
                            matches!(
                                file.file_name().to_str(),
                                Some("kuvo-box" | "kuvo-box.exe" | "Kuvo.exe" | "libunwind.dll")
                            ) && file.file_type().is_ok_and(|kind| kind.is_file())
                        })
                    }) =>
                {
                    ()
                }
                _ => return None,
            }
        }
        Some(())
    };
    if ordinary().is_none() {
        return Err(Error::Installer(
            "An older installation needs the move before joining.".into(),
        ));
    }
    Ok(layout::publish(home)?)
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

struct SshAccount {
    marker: std::path::PathBuf,
    lock: std::path::PathBuf,
    token: String,
}

impl SshAccount {
    #[cfg(unix)]
    fn inherited(root: &std::path::Path) -> Result<Option<Self>, Error> {
        use std::os::unix::fs::MetadataExt as _;

        let Some(token) = std::env::var_os("KUVO_SSH_ACCOUNT_TOKEN") else {
            return Ok(None);
        };
        let token = token
            .into_string()
            .ok()
            .filter(|value| crate::ssh::is_reservation_token(value))
            .ok_or(Error::Invalid("SSH account reservation"))?;
        let (marker, lock) = if root.file_name().is_some_and(|name| name == ".kuvo-code") {
            (
                root.with_file_name(".kuvo-code-adding"),
                root.with_file_name(".kuvo-code-adding.lock"),
            )
        } else {
            (
                root.join("run/ssh-adding"),
                root.join("run/ssh-adding.lock"),
            )
        };
        let meta = std::fs::symlink_metadata(&lock).map_err(|source| Error::File {
            action: "checking the SSH account lock at",
            path: lock.clone(),
            source,
        })?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // The reservation shell passes FD 9. Verify its inode before marking
        // it close-on-exec so a detached resident can't keep the writer lock.
        let matches_lock = meta.is_file()
            && unsafe {
                if libc::fstat(9, stat.as_mut_ptr()) == 0 {
                    let stat = stat.assume_init();
                    stat.st_dev as u64 == meta.dev() && stat.st_ino as u64 == meta.ino()
                } else {
                    false
                }
            };
        if !matches_lock {
            return Err(Error::Invalid("SSH account reservation lock"));
        }
        if unsafe { libc::fcntl(9, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(Error::File {
                action: "keeping the SSH account lock private at",
                path: marker,
                source: std::io::Error::last_os_error(),
            });
        }
        let account = Self {
            marker,
            lock,
            token,
        };
        account.check()?;
        Ok(Some(account))
    }

    #[cfg(not(unix))]
    fn inherited(_: &std::path::Path) -> Result<Option<Self>, Error> {
        Ok(None)
    }

    fn check(&self) -> Result<(), Error> {
        if std::fs::read_link(&self.marker)
            .is_ok_and(|token| token.as_os_str() == self.token.as_str())
        {
            Ok(())
        } else {
            Err(refused(
                "The SSH account was cleared for another attempt. Try again.",
            ))
        }
    }
}

fn check_join(
    files: &Files,
    link: &Link,
    code: &Code,
    id: &str,
    claim: Option<&LocalClaim>,
    ssh_account: Option<&SshAccount>,
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
    if let Some(ssh_account) = ssh_account {
        ssh_account.check()?;
    } else {
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
    }
    let now = lease::wall();
    if let Some(claim) = claim {
        if !claim.adopted && claim.until > now && claim.invite != id {
            return Err(refused(
                "An installer enrollment is already in progress. Retry with its original code.",
            ));
        }
        if claim.invite == id && claim.cancel_pending {
            return Err(refused("This computer could not join."));
        }
        if claim.invite == id && claim.until <= now && claim.recovery.is_some() {
            return Err(refused(
                "The installer did not finish before the claim expired.",
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
    // Starting the letter worker creates these folders and a writer lock.
    // Only queued records keep an otherwise unused setup from joining.
    for (directory, failed) in [("run/outbox", false), ("run/outbox/sending", true)] {
        if files
            .names(directory)?
            .iter()
            .any(|name| name.ends_with(".json") || (failed && name.ends_with(".failed")))
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
    given.credential_ack = matches!(mode, Exchange::AcknowledgeCredentials);
    given.cancel = matches!(mode, Exchange::Cancel);
    given.confirmed = false;
    given.recovery_commitment = None;
    // Only one socket exists for this temporary relay address, including the activation round.
    // Subscribing after connect is safe: the timer retries sends until the socket is ready.
    let mut observations = relay.handle().subscribe().ok_or_else(|| refused(OFFLINE))?;
    let max = Duration::from_secs(if given.activation_ack || given.credential_ack {
        5
    } else {
        60
    });
    let mut resend = tokio::time::interval(Duration::from_millis(1800));
    resend.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut parts = Parts::default();
    let mut seed_parts = credentials::Parts::default();
    let mut seed = None;
    let mut membership = None;

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
                if matches!(mode, Exchange::Cancel) {
                    if reply.cancelled { return Ok(None); }
                    if let Some(message) = reply.message { return Err(Error::Installer(message)); }
                    continue;
                }
                if let Exchange::Activate { portable } = mode {
                    if reply.activated && (!portable || reply.portable_activation) {
                        return Ok(None);
                    }
                    continue;
                }
                if matches!(mode, Exchange::AcknowledgeCredentials) {
                    if reply.credential_complete { return Ok(None); }
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
                if !given.confirmed { continue; }
                if let Some(docs) = reply.docs {
                    let doc = if let Some(part) = docs.part {
                        parts.receive(*key, part).ok().flatten().filter(|doc| doc.kind == Kind::Peers)
                    } else {
                        docs.give.into_iter().filter_map(|env| documents::verify(env).ok()).find(|doc| doc.kind == Kind::Peers)
                    };
                    if let Some(doc) = doc {
                        let accepted = Accepted { doc, activation: reply.activation_receipt, portable: reply.portable_activation, credentials: None };
                        if !reply.copy_credentials {
                            return Ok(Some(accepted));
                        }
                        membership = Some((accepted, reply.credential_until));
                    }
                }
                if let Some(seed_value) = reply.seed.or_else(|| reply.seed_part.and_then(|part| seed_parts.receive(part))) {
                    seed = Some(Ok(seed_value));
                } else if let Some(problem) = reply.credential_problem {
                    seed = Some(Err(problem));
                }
                if membership.is_some() && seed.is_some() {
                    let (mut accepted, until) = membership.take().expect("received membership");
                    accepted.credentials = Some(CredentialCopy { seed: seed.take().expect("received seed"), until });
                    return Ok(Some(accepted));
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
