use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use tokio::time::Instant;

#[derive(Default)]
pub(super) struct Reasoning(BTreeMap<String, Phase>);

#[derive(Clone, Copy)]
enum Phase {
    Starting(Instant),
    Visible,
    Ending(Instant),
}

impl Phase {
    fn visible(self) -> bool {
        matches!(self, Self::Visible | Self::Ending(_))
    }
}

impl Reasoning {
    pub(super) fn hear(&mut self, session: &str, event: &str) -> bool {
        let phase = self.0.get(session).copied();
        match event {
            "session.reasoning.started" => match phase {
                None => {
                    self.0.insert(
                        session.to_owned(),
                        Phase::Starting(Instant::now() + Duration::from_secs(2)),
                    );
                }
                Some(Phase::Ending(_)) => {
                    self.0.insert(session.to_owned(), Phase::Visible);
                }
                _ => {}
            },
            "session.reasoning.ended"
            | "session.text.started"
            | "session.tool.input.started"
            | "session.tool.called" => match phase {
                Some(Phase::Starting(_)) => {
                    self.0.remove(session);
                }
                Some(Phase::Visible) => {
                    self.0.insert(
                        session.to_owned(),
                        Phase::Ending(Instant::now() + Duration::from_millis(1500)),
                    );
                }
                _ => {}
            },
            "session.deleted"
            | "session.execution.started"
            | "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted" => {
                return self.0.remove(session).is_some_and(Phase::visible);
            }
            _ => {}
        }
        false
    }

    pub(super) fn advance(&mut self) {
        let now = Instant::now();
        self.0.retain(|_, phase| match *phase {
            Phase::Starting(at) if at <= now => {
                *phase = Phase::Visible;
                true
            }
            Phase::Ending(at) if at <= now => false,
            _ => true,
        });
    }

    pub(super) fn visible(&self) -> BTreeSet<String> {
        self.0
            .iter()
            .filter(|(_, phase)| phase.visible())
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(super) fn next(&self) -> Option<Instant> {
        self.0
            .values()
            .filter_map(|phase| match *phase {
                Phase::Starting(at) | Phase::Ending(at) => Some(at),
                Phase::Visible => None,
            })
            .min()
    }
}
