use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Status {
    pub id: &'static str,
    pub name: &'static str,
    pub closed: bool,
}

impl Status {
    const fn open(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            closed: false,
        }
    }

    const fn closed(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            closed: true,
        }
    }
}

pub(super) const STATUSES: [Status; 8] = [
    Status::open("needs-you", "Needs you"),
    Status::open("in-review", "In review"),
    Status::open("in-progress", "In progress"),
    Status::open("waiting", "Waiting"),
    Status::open("todo", "Todo"),
    Status::open("backlog", "Backlog"),
    Status::closed("done", "Done"),
    Status::closed("canceled", "Canceled"),
];

#[derive(Serialize)]
pub(super) struct Priority {
    pub id: &'static str,
    pub name: &'static str,
}

pub(super) const PRIORITIES: [Priority; 4] = [
    Priority {
        id: "urgent",
        name: "Urgent",
    },
    Priority {
        id: "high",
        name: "High",
    },
    Priority {
        id: "normal",
        name: "Normal",
    },
    Priority {
        id: "low",
        name: "Low",
    },
];

pub(super) fn closed(id: &str) -> bool {
    STATUSES
        .iter()
        .any(|status| status.id == id && status.closed)
}

pub(super) fn category<'a>(
    id: Option<&'a str>,
    name: Option<&str>,
) -> (&'static str, Option<&'a str>) {
    match id {
        Some("later") => ("backlog", None),
        Some("done") => ("done", None),
        Some("parked") => ("backlog", Some("lbl_parked")),
        Some(id) if id.starts_with("cat_") && super::store::label_id(id) => {
            let name = name.unwrap_or(&id[4..]);
            let words = name
                .split(|ch: char| ch.is_whitespace() || ch == '-' || ch == '_')
                .filter(|word| !word.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            match words.as_str() {
                "waiting on you" => ("needs-you", None),
                "correctness sweep" | "edge cases" => ("backlog", Some(id)),
                _ => ("todo", Some(id)),
            }
        }
        _ => ("todo", None),
    }
}
