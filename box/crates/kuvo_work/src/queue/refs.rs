use super::{
    mentions::Mentions,
    store::{Item, array, item_id},
    view::number_aliases,
};
use regex::Regex;
use serde_json::{Value, json};

pub(super) struct References {
    pub mentions: Mentions,
    stored: Regex,
    page: Regex,
}

impl References {
    pub fn new() -> Self {
        let owner = r"([A-Za-z0-9][A-Za-z0-9_-]{0,38})";
        let repo = r"([A-Za-z0-9_.-]{1,100})";
        Self {
            mentions: Mentions::new(),
            stored: Regex::new(&format!(r"^gh:{owner}/{repo}#([0-9]{{1,10}})$")).expect("stored issue reference"),
            page: Regex::new(&format!(r"(?i)^https?://(?:www\.)?github\.com/{owner}/{repo}/(?:issues|pull)/([0-9]{{1,10}})(?:[/?#][^\s]*)?$")).expect("GitHub issue page"),
        }
    }

    pub fn issue<'a>(&self, text: &'a str) -> Option<Issue<'a>> {
        let fields = self
            .stored
            .captures(text.trim())
            .or_else(|| self.page.captures(text.trim()))?;
        let owner = fields.get(1)?.as_str();
        let repo = fields.get(2)?.as_str();
        let number = fields
            .get(3)?
            .as_str()
            .parse::<i32>()
            .ok()
            .filter(|number| *number > 0)?;
        (!matches!(repo, "." | "..")).then_some(Issue {
            owner,
            repo,
            number,
        })
    }

    pub fn same_source(&self, a: &Value, b: &Value) -> bool {
        let key = |source: &Value| {
            let reference = source["ref"].as_str()?;
            Some(
                self.issue(reference)
                    .map(|issue| issue.key())
                    .unwrap_or_else(|| {
                        format!(
                            "{}:{reference}",
                            source["kind"].as_str().unwrap_or_default()
                        )
                        .to_lowercase()
                    }),
            )
        };
        a["id"].as_str().is_some_and(|id| b["id"] == id)
            || key(a).is_some_and(|name| key(b).as_ref() == Some(&name))
    }

    pub fn parse<'a>(&self, text: &'a str) -> Option<Reference<'a>> {
        let text = text.trim();
        if item_id(text) {
            return Some(Reference::Id(text));
        }
        if let Some((setup, digits)) = text.split_once(':')
            && setup.strip_prefix("set_").is_some_and(|id| {
                id.len() == 22
                    && id
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
            })
            && let Some(number) = number_of(digits)
            && number > 0
            && !number_part(digits).starts_with('0')
        {
            return Some(Reference::Number {
                number,
                setup: Some(setup),
            });
        }
        if let Some(number) = number_of(text) {
            return Some(Reference::Number {
                number,
                setup: None,
            });
        }
        self.issue(text).map(|issue| Reference::Source(issue.key()))
    }

    pub fn find<'a>(&self, items: &'a [Item], reference: &Reference<'_>) -> Option<&'a Item> {
        self.named(items, reference)
            .min_by_key(|item| item.removed.is_some())
    }

    pub fn named<'a, 'b>(
        &'b self,
        items: &'a [Item],
        reference: &'b Reference<'_>,
    ) -> impl Iterator<Item = &'a Item> + use<'a, 'b> {
        items.iter().filter(move |item| match reference {
            Reference::Id(id) => item.id == *id,
            Reference::Number {
                number,
                setup: None,
            } => item.number == Some(f64::from(*number)),
            Reference::Number {
                number,
                setup: Some(setup),
            } => number_aliases(item).iter().any(|alias| {
                alias["setup"].as_str() == Some(*setup)
                    && alias["number"].as_f64() == Some(f64::from(*number))
            }),
            Reference::Source(key) => item
                .source()
                .and_then(|source| self.issue(source["ref"].as_str()?))
                .is_some_and(|source| source.key() == *key),
        })
    }

    pub fn names_summary(&self, item: &Value, reference: &Reference<'_>) -> bool {
        match reference {
            Reference::Id(id) => item["id"] == *id,
            Reference::Number {
                number,
                setup: None,
            } => item["number"].as_f64() == Some(f64::from(*number)),
            Reference::Number {
                number,
                setup: Some(setup),
            } => array(item.get("numberAliases")).iter().any(|alias| {
                alias["setup"] == *setup && alias["number"].as_f64() == Some(f64::from(*number))
            }),
            Reference::Source(key) => item["source"]["ref"]
                .as_str()
                .and_then(|reference| self.issue(reference))
                .is_some_and(|issue| issue.key() == *key),
        }
    }
}

pub(super) struct Issue<'a> {
    pub owner: &'a str,
    pub repo: &'a str,
    pub number: i32,
}

impl Issue<'_> {
    pub fn key(&self) -> String {
        format!("github.com/{}/{}#{}", self.owner, self.repo, self.number).to_ascii_lowercase()
    }
}

pub(super) enum Reference<'a> {
    Id(&'a str),
    Number { number: u32, setup: Option<&'a str> },
    Source(String),
}

fn number_part(text: &str) -> &str {
    text.strip_prefix('#').unwrap_or(text)
}

fn number_of(text: &str) -> Option<u32> {
    let digits = number_part(text);
    if !(1..=6).contains(&digits.len()) || !digits.bytes().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub(super) fn relations(item: &Item, items: &[Item], far: &[Value]) -> Vec<Value> {
    if item.removed.is_some() {
        return Vec::new();
    }
    let live: Vec<_> = items.iter().filter(|item| item.removed.is_none()).collect();
    let mut links: Vec<_> = direct(item).collect();
    links.extend(targets(item).map(|target| (target, "mentions")));
    for other in live.iter().filter(|other| other.id != item.id) {
        links.extend(
            direct(other)
                .filter(|(target, _)| *target == item.id)
                .filter_map(|(_, kind)| Some((other.id.as_str(), inverse(kind)?))),
        );
        if targets(other).any(|target| target == item.id) {
            links.push((&other.id, "mentioned-in"));
        }
    }
    for other in far {
        if let Some(id) = other["id"].as_str().filter(|id| *id != item.id)
            && array(other.get("mentionTargets"))
                .iter()
                .any(|target| target.as_str() == Some(item.id.as_str()))
        {
            links.push((id, "mentioned-in"));
        }
    }
    links
        .into_iter()
        .filter_map(|(target, kind)| {
            if let Some(other) = live.iter().find(|other| other.id == target) {
                let mut row = json!({"id":other.id,"title":other.title,"kind":kind});
                if let Some(number) = other.number {
                    row["number"] = json!(number);
                }
                return Some(row);
            }
            if !matches!(kind, "mentions" | "mentioned-in") {
                return None;
            }
            let other = far.iter().find(|other| other["id"] == target)?;
            let mut row = json!({"id":target,"title":other["title"],"kind":kind});
            if let Some(number) = other.get("number") {
                row["number"] = number.clone();
            }
            Some(row)
        })
        .collect()
}

pub(super) fn inverse(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "related" => "related",
        "blocks" => "blocked-by",
        "blocked-by" => "blocks",
        "duplicate-of" => "duplicated-by",
        "duplicated-by" => "duplicate-of",
        _ => return None,
    })
}

fn direct(item: &Item) -> impl Iterator<Item = (&str, &str)> {
    array(item.saved.get("relations"))
        .iter()
        .filter_map(|link| {
            let (id, kind) = (link["item"].as_str()?, link["kind"].as_str()?);
            (item_id(id) && inverse(kind).is_some()).then_some((id, kind))
        })
}

fn targets(item: &Item) -> impl Iterator<Item = &str> {
    array(
        item.saved
            .get("mentionIndex")
            .and_then(|index| index.get("targets")),
    )
    .iter()
    .filter_map(Value::as_str)
}
