//! Vault-owned session cards and replies. The engine owns session visibility.
use std::sync::Arc;

use http::{Method, StatusCode};
use kuvo_protocol::{
    json::{Null, Nullable},
    routes::types::{
        GetKuvoInfoStatus200MacFeaturesItem258 as Kind,
        GetKuvoPeerSessionComputerSessionAsksStatus200AsksItemPermissionChoicesItem as Choice,
        GetKuvoPeerSessionComputerSessionAsksStatus200AsksItemPermissionChoicesItemAnswer4 as ChoiceAnswer,
        GetKuvoPeerSessionComputerSessionAsksStatus200AsksItemVaultNeed3 as Need,
    },
};
use kuvo_server::{DispatchRequest, Refusal};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::{
    Error, Item, Resident,
    asks::{Answer, AskKind, Decision, Question},
    store::missing,
};

pub use kuvo_protocol::routes::types::GetKuvoPeerSessionComputerSessionAsksStatus200AsksItemVault as VaultCard;

pub struct VaultAsk {
    pub at: u64,
    pub card: VaultCard,
    pub caller: VaultAskCaller,
}

/// The card's session is the conversation that owns the ask. The caller can
/// be a child of that conversation, a bot, or a seat belonging to a chat.
pub struct VaultAskCaller {
    pub session: Option<String>,
    pub bot: Option<String>,
    pub group: Option<String>,
}

#[derive(Clone)]
pub struct VaultWaiting {
    pub id: String,
    pub session: String,
    pub at: u64,
}

#[derive(Clone, Copy)]
pub enum VaultDecision {
    Once,
    Always,
    Reject,
}

pub struct VaultAskReply<'a> {
    pub session: &'a str,
    pub id: &'a str,
    pub decision: VaultDecision,
    pub item: Option<&'a str>,
    pub request: &'a DispatchRequest,
    pub cancel: &'a CancellationToken,
}

const NOT_A_CLIENT: &str =
    "Answer it on your iPhone: only a device this computer’s vault knows can answer it.";
const EMPTY: &str = "There’s nothing in this computer’s vault to give it yet.";
const LOCKED: &str = "This computer’s vault is locked: unlock it to give it an item.";

impl Resident {
    pub fn session_asks(
        &self,
        sessions: &[String],
        viewer: &DispatchRequest,
    ) -> Result<Vec<VaultAsk>, Refusal> {
        let asks = self
            .broker
            .asks
            .list()
            .into_iter()
            .filter(|ask| {
                ask.session
                    .as_ref()
                    .is_some_and(|session| sessions.contains(session))
            })
            .collect::<Vec<_>>();
        if asks.is_empty() {
            return Ok(Vec::new());
        }
        self.block(|local| {
            // Answer routes first render the live offered choices. The decision
            // consumes their request proof after checking the card.
            let client = local
                .clients
                .check_client(&mut local.vault, viewer, viewer.method != Method::POST)
                .is_some();
            let locked = local.vault.locked();
            let items =
                if client && !locked && asks.iter().any(|ask| matches!(ask.kind, AskKind::Item)) {
                    local.vault.items()?
                } else {
                    Vec::new()
                };
            Ok(asks
                .iter()
                .filter_map(|ask| {
                    Some(VaultAsk {
                        at: ask.at,
                        card: card(ask, ask.session.clone()?, client, locked, &items),
                        caller: VaultAskCaller {
                            session: ask.caller_session.clone(),
                            bot: ask.bot.clone(),
                            group: ask.group.clone(),
                        },
                    })
                })
                .collect())
        })
        .map_err(Into::into)
    }

    pub fn decide_session_ask(&self, reply: VaultAskReply<'_>) -> Result<(), Refusal> {
        self.block(|local| {
            if reply.cancel.is_cancelled() {
                return Err(stopped());
            }
            let by = local.client(reply.request)?;
            let ask = self
                .broker
                .asks
                .list()
                .into_iter()
                .find(|ask| ask.id == reply.id && ask.session.as_deref() == Some(reply.session))
                .ok_or_else(gone)?;
            let decision = match reply.decision {
                VaultDecision::Once => Decision::Once,
                VaultDecision::Always if ask.always && ask.group.is_none() => Decision::Always,
                VaultDecision::Always => {
                    return Err(Error::refused(400, "This ask can be given once."));
                }
                VaultDecision::Reject => Decision::No,
            };
            if decision != Decision::No && matches!(ask.kind, AskKind::Item) && reply.item.is_none()
            {
                return Err(Error::refused(400, "Say which item to give it."));
            }
            if reply.item.is_some()
                && (matches!(ask.kind, AskKind::Use) || decision == Decision::No)
            {
                return Err(Error::refused(400, "This answer doesn't choose an item."));
            }
            if let Some(item) = reply.item {
                local.vault.item(item)?.ok_or_else(missing)?;
            }
            if reply.cancel.is_cancelled() {
                return Err(stopped());
            }
            if !self.broker.asks.answer_for(
                reply.session,
                reply.id,
                Answer {
                    decision,
                    item: reply.item.map(str::to_owned),
                    by: Some(by),
                },
            ) {
                return Err(gone());
            }
            Ok(())
        })
        .map_err(Into::into)
    }

    /// Use when the owning session ends, in addition to the originating
    /// tool's ordinary cancellation. An ask from another session stays open.
    pub fn cancel_session_ask(&self, session: &str, id: &str) -> bool {
        self.broker.asks.cancel_for(session, id)
    }

    /// A watch snapshot lets consumers compute additions and removals even if
    /// several asks changed before they could read the latest update.
    pub fn session_waiting(&self) -> watch::Receiver<Arc<Vec<VaultWaiting>>> {
        self.broker.asks.waiting()
    }
}

fn stopped() -> Error {
    Error::refused(409, "The vault's answer was cancelled.")
}

fn gone() -> Error {
    Error::refused(
        StatusCode::NOT_FOUND.as_u16(),
        "That vault ask is no longer waiting.",
    )
}

fn card(ask: &Question, session: String, client: bool, locked: bool, items: &[Item]) -> VaultCard {
    let asker = ask
        .bot_name
        .as_deref()
        .or(ask.session_name.as_deref())
        .unwrap_or("An agent");
    let who = ask.bot_name.as_deref().unwrap_or("this project");
    let always = ask.always && ask.group.is_none();
    let (need, title) = match ask.kind {
        AskKind::Use => (
            Need::Use,
            format!(
                "{asker} wants to use {}",
                ask.item_name.as_deref().unwrap_or("an item")
            ),
        ),
        AskKind::Item => (
            Need::Item,
            format!(
                "{asker} needs {}",
                ask.need.as_deref().unwrap_or("something from your vault")
            ),
        ),
    };
    let mut choices = Vec::new();
    let note = if !client {
        Some(NOT_A_CLIENT.into())
    } else {
        let note = match ask.kind {
            AskKind::Use => {
                let item = ask
                    .item_name
                    .as_ref()
                    .map_or_else(|| "it".into(), |name| format!("“{name}”"));
                choices.push(choice(
                    ChoiceAnswer::Once,
                    "Allow Once",
                    format!("Lets it use {item} this once"),
                    Some(format!("Allow once: let it use {item}")),
                    None,
                ));
                if always {
                    choices.push(choice(
                        ChoiceAnswer::Always,
                        "Always Allow",
                        format!("Lets {who} use {item} from now on, without asking"),
                        Some(format!("Always allow {who} to use {item}")),
                        None,
                    ));
                }
                None
            }
            AskKind::Item if locked => Some(LOCKED.into()),
            AskKind::Item if items.is_empty() => Some(EMPTY.into()),
            AskKind::Item => {
                let mut by_site: Vec<_> = items.iter().collect();
                by_site.sort_by_cached_key(|item| {
                    let matches = ask.site.as_deref().is_some_and(|site| {
                        item.sites.iter().any(|each| {
                            each == site
                                || site.ends_with(&format!(".{each}"))
                                || each.ends_with(&format!(".{site}"))
                        })
                    });
                    (!matches, item.name.to_lowercase())
                });
                for item in by_site {
                    let name = &item.name;
                    choices.push(choice(
                        ChoiceAnswer::Once,
                        "Give",
                        format!("Gives it “{name}” this once"),
                        Some(format!("Give {name}, this once")),
                        Some(item),
                    ));
                    if always {
                        choices.push(choice(ChoiceAnswer::Always, "Always Give",
                            format!("Gives it “{name}”, and lets {who} use it from now on, without asking"),
                            Some(format!("Always give {name}")), Some(item)));
                    }
                }
                None
            }
        };
        choices.push(choice(
            ChoiceAnswer::Reject,
            "Don’t Allow",
            "It goes on without it".into(),
            None,
            None,
        ));
        note
    };
    VaultCard {
        id: ask.id.clone(),
        kind: Kind::Vault,
        session,
        need,
        title,
        message: nullable(ask.why.clone()),
        site: nullable(ask.site.clone()),
        note: nullable(note),
        choices,
    }
}

fn choice(
    answer: ChoiceAnswer,
    title: &str,
    help: String,
    spoken: Option<String>,
    item: Option<&Item>,
) -> Choice {
    Choice {
        after: None,
        answer,
        confirm: (answer == ChoiceAnswer::Always).then_some(true),
        help: Nullable::Value(help),
        item: item.map(|item| item.id.clone()),
        named: nullable(item.map(|item| item.name.clone())),
        placeholder: Nullable::Null(Null),
        spoken: nullable(spoken),
        title: title.into(),
    }
}

fn nullable(value: Option<String>) -> Nullable<String> {
    value.map_or(Nullable::Null(Null), Nullable::Value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuvo_server::{Caller, LocalOrigin, Transport};

    #[tokio::test(flavor = "multi_thread")]
    async fn cards_and_waiting_follow_the_live_session_ask() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("box");
        let account = root.path().join("account");
        let resident = Resident::new(
            &home,
            false,
            Default::default(),
            account.clone(),
            &account,
            None,
        );
        let mut waiting = resident.session_waiting();
        let cancel = CancellationToken::new();
        let book = resident.broker.asks.clone();
        let from_tool = cancel.clone();
        let answer = tokio::spawn(async move {
            let question = Question {
                session: Some("ses_root".into()),
                caller_session: Some("ses_child".into()),
                bot: Some("bot_fixture".into()),
                group: Some("group_fixture".into()),
                item_name: Some("Fixture item".into()),
                always: true,
                ..Question::default()
            };
            book.ask(question, &from_tool).await
        });
        waiting.changed().await.unwrap();
        let id = waiting.borrow_and_update()[0].id.clone();
        let mut request = DispatchRequest {
            method: Method::GET,
            path: "kuvo/sessions/ses_root/asks".into(),
            headers: Default::default(),
            body: Default::default(),
            caller: Caller::Local(Transport::Http {
                origin: LocalOrigin::Direct,
                internal: false,
            }),
        };
        let sessions = ["ses_root".to_owned()];
        let denied = resident
            .session_asks(&sessions, &request)
            .ok()
            .expect("pending cards");
        assert!(denied[0].card.choices.is_empty());
        request.caller = Caller::Local(Transport::Http {
            origin: LocalOrigin::App,
            internal: false,
        });
        let list = resident
            .session_asks(&sessions, &request)
            .ok()
            .expect("pending cards");
        assert_eq!(list[0].caller.session.as_deref(), Some("ses_child"));
        assert_eq!(list[0].caller.bot.as_deref(), Some("bot_fixture"));
        assert_eq!(list[0].card.session, "ses_root");
        assert_eq!(list[0].card.choices.len(), 2);
        assert!(!resident.cancel_session_ask("ses_child", &id));

        let reply = |decision| VaultAskReply {
            session: "ses_root",
            id: &id,
            decision,
            item: None,
            request: &request,
            cancel: &cancel,
        };
        assert!(
            resident
                .decide_session_ask(reply(VaultDecision::Always))
                .is_err()
        );
        assert!(
            resident
                .decide_session_ask(reply(VaultDecision::Once))
                .is_ok()
        );
        assert!(answer.await.unwrap().unwrap().decision == Decision::Once);
        waiting.changed().await.unwrap();
        assert!(waiting.borrow_and_update().is_empty());

        let book = resident.broker.asks.clone();
        let answer = tokio::spawn(async move {
            book.ask(
                Question {
                    session: Some("ses_root".into()),
                    ..Question::default()
                },
                &CancellationToken::new(),
            )
            .await
        });
        waiting.changed().await.unwrap();
        let id = waiting.borrow_and_update()[0].id.clone();
        assert!(resident.cancel_session_ask("ses_root", &id));
        waiting.changed().await.unwrap();
        assert!(waiting.borrow().is_empty());
        assert!(answer.await.unwrap().unwrap().decision == Decision::No);
    }
}
