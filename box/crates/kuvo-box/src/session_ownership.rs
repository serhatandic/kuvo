//! Adapt authenticated peer mail to the engine's own receipt and send handlers.

use std::time::Duration;

use http::StatusCode;
use kuvo_engine::{
    ComputerKey, DeviceAccess, Engine, LocalSessions, OwnedReply, OwnedRequest, OwnerScope,
    PeerReply, SessionOwner, SessionOwnership,
};
use kuvo_peers::ownership::{
    Admission, Error, Handle, IncomingRequests, PairingState, PeerOwner, Reply,
};
use kuvo_server::Refusal;
use serde_json::json;

pub(crate) struct OwnerPort(pub Handle);

fn refusal(error: Error) -> Refusal {
    let status = match error {
        Error::UnknownPeer => StatusCode::NOT_FOUND,
        Error::SetupChanged => StatusCode::CONFLICT,
        Error::Invalid => StatusCode::BAD_REQUEST,
        Error::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    Refusal::new(status, error.to_string())
}

fn owner(row: PeerOwner) -> SessionOwner {
    SessionOwner {
        key: row.key,
        name: row.name,
        local: row.local,
        reachable: row.reachable,
        features: row.features,
    }
}

impl SessionOwnership for OwnerPort {
    fn scope(&self, session: &str, issuer: Option<ComputerKey>) -> Result<OwnerScope, Refusal> {
        let scope = self.0.scope(session, issuer).map_err(refusal)?;
        Ok(OwnerScope {
            setup: scope.setup,
            here: scope.here,
            owner: scope.owner.map(owner),
            receipt_issuer: scope.receipt_issuer,
            held: scope.held,
        })
    }

    fn members(&self) -> Vec<SessionOwner> {
        self.0.members().into_iter().map(owner).collect()
    }

    fn current_pairing(&self, key: &ComputerKey, pairing: &str) -> DeviceAccess {
        match self.0.current_pairing(key, pairing) {
            PairingState::Allowed => DeviceAccess::Allowed,
            PairingState::Revoked => DeviceAccess::Revoked,
            PairingState::Checking => DeviceAccess::Checking,
        }
    }

    fn request(&self, target: ComputerKey, request: OwnedRequest, wait: Duration) -> PeerReply<'_> {
        Box::pin(async move {
            let request = serde_json::to_value(request)
                .and_then(serde_json::from_value)
                .map_err(|_| refusal(Error::Invalid))?;
            let answer = self
                .0
                .request(target, request, wait)
                .await
                .map_err(refusal)?;
            Ok(OwnedReply {
                status: answer.status,
                body: answer.body,
            })
        })
    }
}

pub(crate) async fn serve(
    handle: Handle,
    mut requests: IncomingRequests,
    engine: Engine,
    sessions: LocalSessions,
) {
    let mut active = tokio::task::JoinSet::new();
    loop {
        let incoming = tokio::select! {
            request = requests.recv(), if active.len() < 16 => match request {
                Ok(request) => request,
                Err(_) => break,
            },
            _ = active.join_next(), if !active.is_empty() => continue,
        };
        let handle = handle.clone();
        let engine = engine.clone();
        let sessions = sessions.clone();
        active.spawn(async move {
            let refused = |status, message| Reply { status, body: json!({"message": message}) };
            let answer = match handle.admit(&incoming) {
                Ok(Admission::Allowed) => match serde_json::to_value(incoming.request()).and_then(serde_json::from_value) {
                    Ok(request) => {
                        let result = engine.answer_owner_request(&sessions, incoming.peer(), request).await;
                        Reply { status: result.status, body: result.body }
                    }
                    Err(_) => refused(400, "The owner request is invalid."),
                },
                Ok(Admission::OwnerChanged) => refused(409, "The conversation's owner changed. Check the conversation before sending again."),
                Ok(Admission::Revoked) => refused(403, "The submitting device no longer has access. Check the record before sending again."),
                Ok(Admission::Checking) => refused(503, "Checking the submitting device's access."),
                Err(_) => refused(409, "The accepted setup changed."),
            };
            let _ = handle.reply(&incoming, answer).await;
        });
    }
}
