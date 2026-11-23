//! Ownership, conflict and backup policies for the frozen home layout.

mod inventory;
pub use inventory::DATA_DECLARATIONS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Owner {
    SubmissionIssuer,
    SessionComputer,
    GroupHost,
    BotHome,
    Primary,
    LocalComputer,
    Client,
    BackupSource,
    BackupReceiver,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Authority {
    Fixed(Owner),
    WorkQueue,
}

impl Authority {
    pub fn owner(self, queue: Option<&str>) -> Option<Owner> {
        match self {
            Self::Fixed(owner) => Some(owner),
            Self::WorkQueue => queue.and_then(queue_owner),
        }
    }
}

fn queue_owner(queue: &str) -> Option<Owner> {
    if queue == "usr_you" {
        return Some(Owner::LocalComputer);
    }
    let (kind, id) = queue.split_once('_')?;
    if !(1..=60).contains(&id.len()) {
        return None;
    }
    match kind {
        "ses" if id.bytes().all(|b| b.is_ascii_alphanumeric()) => Some(Owner::SessionComputer),
        "grp"
            if id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) =>
        {
            Some(Owner::GroupHost)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Conflict {
    Fields,
    KeepBothNotes,
    OwnerOrder,
    NativeFormat,
    Regenerate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Restore {
    Portable,
    MachineLocal,
    NeedsFreshApproval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Snapshot {
    Bytes,
    EngineDatabase,
    Sqlite,
    AgentTranscript,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PayloadInventory {
    Session,
    Queue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Location {
    pub pattern: &'static str,
    pub restore: Option<Restore>,
    pub snapshot: Snapshot,
    pub inventory: Option<PayloadInventory>,
}

impl Location {
    pub const fn bytes(pattern: &'static str) -> Self {
        Self {
            pattern,
            restore: None,
            snapshot: Snapshot::Bytes,
            inventory: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupVia {
    AcceptedSetup,
    QueueItems,
    OwnerMessageReceipts,
    MutationEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backup {
    Disabled,
    Storage(&'static [Location]),
    Via {
        policy: BackupVia,
        storage: &'static [Location],
    },
    Excluded(&'static str),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SharedPolicy {
    pub seed_from_member_copies: bool,
    pub publish: bool,
    pub save_usage_locally: bool,
    pub checkpoint_only: bool,
    pub local_effects: &'static [&'static str],
}

impl SharedPolicy {
    const DEFAULT: Self = Self {
        seed_from_member_copies: false,
        publish: false,
        save_usage_locally: false,
        checkpoint_only: false,
        local_effects: &[],
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalStorage {
    pub root: &'static str,
    pub version: u32,
    pub archive_suffix: Option<&'static str>,
    pub journal: Option<&'static str>,
    pub partial: Option<&'static str>,
}

pub const BACKUP_ARCHIVE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataDeclaration {
    pub kind: &'static str,
    pub owner: Authority,
    pub conflict: Conflict,
    pub backup: Backup,
    pub restore: Restore,
    pub families: &'static [&'static str],
    pub shared: Option<SharedPolicy>,
    pub external: Option<ExternalStorage>,
}

pub fn data(kind: &str) -> Option<&'static DataDeclaration> {
    DATA_DECLARATIONS.iter().find(|entry| entry.kind == kind)
}

pub fn shared() -> impl Iterator<Item = (&'static str, SharedPolicy)> {
    DATA_DECLARATIONS.iter().filter_map(|entry| {
        if entry.owner != Authority::Fixed(Owner::Primary) {
            return None;
        }
        Some((entry.kind, entry.shared?))
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationLane {
    OwnerWrite,
    QueuedSessionWork,
    ReceiptLookup,
    ReceiptMove,
    SharedSave,
    Command,
    Consent,
    SandboxCommand,
    MacProtectedAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Actor {
    CurrentSubmitter,
    DeclaredOwner,
    ExecutingComputer,
    NativeAdapter,
    AuthenticatedMoveSource,
    AuthenticatedMoveDestination,
    MoveRecordScope,
    CurrentPrimary,
    LiveClientOrAdmittedDraft,
    FreshRequest,
    LiveClient,
    CurrentSessionTool,
    LocallyGrantedHelper,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mutation {
    pub propose: Actor,
    pub accept: Actor,
    pub tighten: Option<Actor>,
    pub perform: Option<Actor>,
    pub recover: Actor,
}

impl MutationLane {
    pub const fn policy(self) -> Mutation {
        use Actor::*;
        use MutationLane::*;
        let (propose, accept, tighten, perform, recover) = match self {
            OwnerWrite | ReceiptLookup => {
                (CurrentSubmitter, DeclaredOwner, None, None, DeclaredOwner)
            }
            QueuedSessionWork => (
                CurrentSubmitter,
                DeclaredOwner,
                Some(ExecutingComputer),
                Some(NativeAdapter),
                DeclaredOwner,
            ),
            ReceiptMove => (
                AuthenticatedMoveSource,
                AuthenticatedMoveDestination,
                None,
                None,
                MoveRecordScope,
            ),
            SharedSave => (CurrentPrimary, CurrentPrimary, None, None, DeclaredOwner),
            Command => (
                LiveClientOrAdmittedDraft,
                DeclaredOwner,
                Some(ExecutingComputer),
                Some(NativeAdapter),
                FreshRequest,
            ),
            Consent => (
                LiveClient,
                DeclaredOwner,
                Some(ExecutingComputer),
                Some(NativeAdapter),
                FreshRequest,
            ),
            SandboxCommand => (
                CurrentSessionTool,
                DeclaredOwner,
                Some(ExecutingComputer),
                Some(NativeAdapter),
                FreshRequest,
            ),
            MacProtectedAction => (
                ExecutingComputer,
                ExecutingComputer,
                Some(ExecutingComputer),
                Some(LocallyGrantedHelper),
                FreshRequest,
            ),
        };
        Mutation {
            propose,
            accept,
            tighten,
            perform,
            recover,
        }
    }
}
