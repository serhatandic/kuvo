//! Bind the search route to authenticated read-only peer work.

use std::collections::BTreeMap;

use kuvo_engine::{
    ComputerKey, Engine, PeerSearch, SearchAvailability, SearchComputer, SearchPeer, SearchPeers,
    SearchResult,
};
use kuvo_peers::{
    memory_read::{Answer, Handle, Kind, Read, SEARCH_FEATURE, Standing},
    setup_lists::Reader,
};
use serde_json::{Value, json};

pub(crate) struct SetupSearch {
    pub peers: Reader,
    pub reads: Handle,
}

fn availability(standing: Standing) -> SearchAvailability {
    match standing {
        Standing::Ready => SearchAvailability::Ready,
        Standing::Unavailable => SearchAvailability::Unavailable,
        Standing::Away => SearchAvailability::Away,
    }
}

impl SearchPeers for SetupSearch {
    fn local(&self) -> SearchComputer {
        let local = self.peers.view().roster.local();
        SearchComputer {
            id: local.id,
            name: local.name,
        }
    }

    fn peers(&self) -> Vec<SearchPeer> {
        self.peers
            .view()
            .roster
            .members
            .iter()
            .map(|member| {
                let computer = member.computer();
                SearchPeer {
                    key: member.key,
                    computer: SearchComputer {
                        id: computer.id,
                        name: computer.name,
                    },
                    availability: availability(self.reads.standing(member.key, SEARCH_FEATURE)),
                }
            })
            .collect()
    }

    fn moved(&self, sessions: &[&str]) -> BTreeMap<String, ComputerKey> {
        self.reads.moved_sessions(sessions)
    }

    fn search(&self, key: ComputerKey, words: String, limit: usize) -> PeerSearch<'_> {
        Box::pin(async move {
            let read = Read {
                area: Some("search".to_owned()),
                item: "conversations".to_owned(),
                kind: Kind::Record,
                body: Some(
                    [
                        ("words".to_owned(), json!(words)),
                        ("limit".to_owned(), json!(limit)),
                    ]
                    .into_iter()
                    .collect(),
                ),
            };
            if let Ok(mut answer) = self.reads.request(key, read).await
                && answer.status == 200
                && let Some(found) = answer.value.get_mut("results")
                && let Ok(results) = serde_json::from_value::<Vec<SearchResult>>(found.take())
            {
                return Ok(results);
            }
            Err(availability(self.reads.missed(key)))
        })
    }
}

pub(crate) async fn answer(engine: Engine, read: Read) -> Answer {
    if read.area.as_deref() != Some("search")
        || read.item != "conversations"
        || read.kind != Kind::Record
    {
        return Answer {
            status: 404,
            value: Value::Null,
        };
    }
    let body = read.body.unwrap_or_default();
    let words = body
        .get("words")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if words.is_empty() || words.encode_utf16().count() > 512 {
        return Answer {
            status: 400,
            value: json!({"message": "Say what to search for (q)."}),
        };
    }
    let limit = body
        .get("limit")
        .and_then(Value::as_f64)
        .filter(|limit| *limit != 0.)
        .unwrap_or(20.)
        .clamp(1., 50.) as usize;
    match engine.search_conversations(words, limit).await {
        Ok(results) => Answer {
            status: 200,
            value: json!({"results": results}),
        },
        Err(refusal) => Answer {
            status: refusal.reply().status.as_u16(),
            value: json!({"message": "This computer's engine didn't answer. Search again in a moment."}),
        },
    }
}
