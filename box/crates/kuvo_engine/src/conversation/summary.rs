//! Manual compaction on its own preserves the read status of the previous turn.

use http::Method;
use serde::Deserialize;
use serde_json::json;

use crate::{Engine, api, lifetime::Lease, sessions};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum Row {
    Idle {
        time: Option<Time>,
    },
    Compaction {
        reason: Option<Reason>,
        status: Option<Status>,
    },
    User,
    Synthetic,
    System,
    ModelSwitched,
    AgentSwitched,
    Shell,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Time {
    created: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Reason {
    Manual,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Completed,
    #[serde(other)]
    Other,
}

impl Row {
    fn between_turns(&self) -> bool {
        matches!(
            self,
            Self::Synthetic
                | Self::System
                | Self::ModelSwitched
                | Self::AgentSwitched
                | Self::Shell
        )
    }
}

fn prior_end(rows: &[Row]) -> Option<u64> {
    let standalone = |at: usize| {
        rows[at + 1..]
            .iter()
            .find(|row| !row.between_turns())
            .is_none_or(|row| matches!(row, Row::Idle { .. }))
    };
    let (at, last) = rows
        .iter()
        .enumerate()
        .skip_while(|(_, row)| matches!(row, Row::User) || row.between_turns())
        .skip_while(|(_, row)| matches!(row, Row::Idle { .. }))
        .find(|(_, row)| !row.between_turns())?;
    if !matches!(
        last,
        Row::Compaction {
            reason: Some(Reason::Manual),
            status: Some(Status::Completed)
        }
    ) || !standalone(at)
    {
        return None;
    }
    let mut work = Vec::<&Row>::new();
    for (at, row) in rows.iter().enumerate() {
        if matches!(
            row,
            Row::Compaction {
                reason: Some(Reason::Manual),
                ..
            }
        ) && standalone(at)
        {
            if let Some(end) = work.iter().rposition(|row| !row.between_turns())
                && matches!(work[end], Row::Idle { .. })
            {
                work.remove(end);
            }
        } else {
            work.push(row);
        }
    }
    let Row::Idle { time } = work
        .into_iter()
        .find(|row| matches!(row, Row::Idle { .. }))?
    else {
        return None;
    };
    time.as_ref()?.created
}

pub(super) async fn keep_read(
    engine: &Engine,
    lease: &Lease,
    session: &str,
) -> Result<(), api::Error> {
    let route = sessions::path(session);
    let page: Vec<Row> = engine
        .call(
            lease,
            Method::GET,
            &format!("{route}/message?limit=6&order=desc"),
            None,
        )
        .await?;
    let Some(before) = prior_end(&page) else {
        return Ok(());
    };
    let listed: sessions::Session = engine.call(lease, Method::GET, &route, None).await?;
    let Some(idle) = listed
        .time
        .idle
        .filter(|_| listed.time.viewed.unwrap_or_default() >= before)
    else {
        return Ok(());
    };
    if let Ok(_admitted) = crate::session_move::admit(engine, session, None).await {
        engine
            .call::<serde_json::Value>(
                lease,
                Method::POST,
                &format!("{route}/view"),
                Some(&json!({ "idle": idle })),
            )
            .await?;
    }
    Ok(())
}
