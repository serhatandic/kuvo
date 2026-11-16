//! A page the box describes (`kuvo/settings/<section>`, PROTOCOL.md "A box's settings"), as the
//! rows this app draws: one way for each kind of row, in the box's words. Nothing here knows what
//! any setting is; what can be changed, what's asked first and what's refused are the box's.

use kuvo_box::areas::Level;
use kuvo_box::settings_pages::{Action, Can, Kind, Page, Row as Described, Tone as Said};
use serde_json::{Value, json};

use super::rows::{Act, At, Block, Control, FlowDrawn, Meter, Push, Row, Tone};

/// What draws a row whose kind this app has nothing for yet.
const NEWER: &str = "A newer Kuvo changes this";
const NOT_YET: &str = "Not in the desktop app yet";
/// Why what makes sure it's you first (`verified`) isn't done here: this app can't yet.
pub(super) const VERIFIED_ELSEWHERE: &str = "Do this in Kuvo on your iPhone, which makes sure it’s you first: this app can’t yet.";

/// How a page is drawn from here.
pub struct Drawing<'a> {
    pub section: &'a str,
    /// Rows with a change on its way, by key, and what they say meanwhile.
    pub busy: &'a [(String, String)],
    /// Secrets whose field is shown to replace the one set, by row id.
    pub replacing: &'a [String],
    /// Whether the box is on the computer this app runs on.
    pub here: bool,
    pub filters: &'a std::collections::HashMap<String, String>,
}

pub fn key(id: &str) -> String {
    format!("row-{id}")
}

/// The page's blocks: each group a card, a list's items rows of its own, then the page's notes.
pub fn blocks(page: &Page, drawing: &Drawing) -> Vec<Block> {
    let mut blocks = Vec::new();
    for group in &page.groups {
        let mut label = group.title.clone().unwrap_or_default();
        let mut words = group.note.clone().unwrap_or_default();
        let mut rows = Vec::new();
        for described in &group.rows {
            if described.kind != Kind::List {
                rows.push(row(described, drawing));
                continue;
            }
            // A list alone in a group with no heading (or whose heading matches the group) names the group; else it heads its items.
            let mut adds = described.add.as_ref().map(|add| push(described, add, drawing));
            // (An add that makes sure it's you first says why it can't be pressed here, under it.)
            let add_why = described.add.as_ref().filter(|add| add.verified).map(|_| VERIFIED_ELSEWHERE.to_owned());
            // Its add, where the list names its group: a button of its own at the card's end.
            let mut add_last = None;
            if group.rows.len() == 1 && (label.is_empty() || label == described.title) {
                if label.is_empty() {
                    label = described.title.clone();
                }
                if words.is_empty() {
                    words = described.line.clone().unwrap_or_default();
                }
                add_last = adds.take();
            } else {
                rows.push(Row::new(if described.search.is_some() { format!("{}-heading", key(&described.id)) } else { key(&described.id) }, described.title.clone(), described.line.clone().unwrap_or_default(), adds.map_or(Control::None, |add| Control::Buttons(vec![add]))).why(add_why.clone()));
            }
            let query = drawing.filters.get(&key(&described.id)).cloned().unwrap_or_default();
            if let Some(search) = &described.search {
                rows.push(Row::new(key(&described.id), String::new(), String::new(), Control::Field { unit: None, lines: None, masked: false, value: query.clone(), placeholder: search.placeholder.clone(), revision: String::new(), save: Act::Filter, more: Vec::new() }));
            }
            let query_lower = query.to_lowercase();
            let list_matches = described.title.to_lowercase().contains(&query_lower);
            let items: Vec<_> = described.items.iter().filter(|item| query.is_empty() || list_matches || item.title.to_lowercase().contains(&query_lower) || item.line.as_deref().unwrap_or_default().to_lowercase().contains(&query_lower)).collect();
            rows.extend(items.iter().map(|item| row(item, drawing)));
            if items.is_empty() {
                rows.push(Row::new(format!("{}-empty", key(&described.id)), String::new(), if query.is_empty() { described.empty.clone().unwrap_or_else(|| "None yet.".into()) } else { described.search.as_ref().map(|search| search.empty.replace("{query}", &query)).unwrap_or_default() }, Control::None));
            }
            if let Some(add) = add_last {
                rows.push(Row::new(format!("{}-add", key(&described.id)), String::new(), String::new(), Control::Buttons(vec![add])).why(add_why));
            }
        }
        blocks.push(Block::Group(label, words, rows));
    }
    for (ix, note) in page.notes.iter().enumerate() {
        blocks.push(Block::Note { key: format!("note-{}-{ix}", page.id), words: note.words.clone(), busy: false, warning: note.tone == Said::Warning });
    }
    blocks
}

fn row(described: &Described, drawing: &Drawing) -> Row {
    let key = key(&described.id);
    let why = described.why_not().filter(|why| !why.is_empty()).map(str::to_owned);
    // What can be changed from here says so (`can: true`); one that doesn't has nothing to change.
    let can = described.can == Some(Can::Yes);
    let at = At::of(drawing.section, described);
    let change = |value: Value| Act::Change { at: at.clone(), value, asked: false };
    let mut why = why;
    let control = if let Some(words) = described.busy.clone().or_else(|| drawing.busy.iter().find(|(busy, _)| *busy == key).map(|(_, words)| words.clone())) {
        Control::Busy(words)
    } else {
        match described.kind {
            Kind::Switch if !described.actions.is_empty() => {
                // A thing in a list (a server): what it offers now as buttons, what asks first
                // (a form, a confirm) in its "…" menu, then its switch.
                let (asks, now): (Vec<&Action>, Vec<&Action>) = described.actions.iter().partition(|action| action.form.is_some() || action.confirm.is_some());
                Control::Thing {
                    buttons: now.into_iter().map(|action| push(described, action, drawing)).collect(),
                    menu: asks.into_iter().map(|action| push(described, action, drawing)).collect(),
                    switch: Some((described.is_on(), can.then(|| change(json!(!described.is_on()))))),
                }
            }
            Kind::Switch => Control::Switch { on: described.is_on(), act: can.then(|| change(json!(!described.is_on()))) },
            Kind::Choice => {
                let shown = described.chosen().map(|choice| choice.title.clone()).or_else(|| described.words().map(str::to_owned)).unwrap_or_default();
                if can {
                    Control::Menu { shown: Some(shown), choices: described.choices.iter().map(|choice| (choice.title.clone(), choice.line.clone(), Some(change(json!(choice.id))), Some(choice.id.as_str()) == described.words())).collect() }
                } else {
                    Control::Text(shown)
                }
            }
            Kind::Status => Control::Text(match (&described.unit, described.value.as_ref().and_then(Value::as_f64)) {
                (Some(unit), Some(value)) => unit.display(value),
                _ => shown_value(described),
            }),
            Kind::Text | Kind::Number | Kind::Folder => {
                let value = shown_value(described);
                if can {
                    let save = Act::Save { at: at.clone(), number: described.kind == Kind::Number };
                    Control::Field { unit: described.unit.as_ref().map(|unit| unit.name.clone()), lines: described.lines.as_ref().map(|lines| (lines.min, lines.max)), masked: false, value, placeholder: described.placeholder.clone().unwrap_or_default(), revision: described.revision.clone(), save: save.clone(), more: { let mut picks = vec![Push::new("Save", save)]; picks.extend(described.suggestions.iter().map(|pick| Push::new(pick.title.clone(), change(json!(pick.value))))); if let Some(folder) = &described.folder { picks.push(Push::new(folder.choose.clone(), Act::Folder { at: at.clone(), words: folder.clone() })); picks.push(Push::new(folder.clear.clone(), change(json!("")))); } picks } }
                } else {
                    Control::Text(match (&described.unit, described.value.as_ref().and_then(Value::as_f64)) {
                        (Some(unit), Some(value)) => unit.display(value),
                        _ => value,
                    })
                }
            }
            Kind::Secret => {
                let replacing = drawing.replacing.contains(&described.id);
                let save = Act::Save { at: at.clone(), number: false };
                let field = |more: Vec<Push>| Control::Field { unit: None, lines: None, masked: true, value: String::new(), placeholder: described.placeholder.clone().unwrap_or_default(), revision: described.revision.clone(), save: save.clone(), more };
                match (can, described.set, replacing) {
                    (false, set, _) => Control::Text(if set { "Set".into() } else { "Not set".into() }),
                    (true, false, _) => field(vec![Push::new("Save", save.clone())]),
                    (true, true, true) => field(vec![Push::new("Save", save.clone()), Push::new("Cancel", Act::Replacing { row: described.id.clone(), on: false })]),
                    (true, true, false) => Control::Buttons(vec![Push::new("Replace", Act::Replacing { row: described.id.clone(), on: true }), Push::new("Remove", change(Value::Null))]),
                }
            }
            Kind::Actions => {
                // One that can't be done from here says why under the row, in the box's words.
                if why.is_none() {
                    why = described.actions.iter().find_map(|action| action.why_not().filter(|why| !why.is_empty()).map(str::to_owned));
                }
                let pushes: Vec<Push> = described.actions.iter().map(|action| push(described, action, drawing)).collect();
                // (Three answers to one question, none asking for a form: each its own button.)
                let answers = pushes.len() == 3 && described.actions.iter().all(|action| action.form.is_none());
                match pushes.len() {
                    0 => Control::Text(described.words().unwrap_or_default().to_owned()),
                    // Past two, a menu, in the box's words as they are; one that can't be done
                    // from here is still listed, and can't be picked.
                    1 | 2 => Control::Buttons(pushes),
                    3 if answers => Control::Buttons(pushes),
                    _ => Control::Menu { shown: None, choices: pushes.into_iter().map(|push| (push.label, None, push.act, false)).collect() },
                }
            }
            Kind::Meter => Control::Windows(
                described
                    .windows
                    .iter()
                    .map(|window| Meter { title: window.title.clone(), said: format!("{}% used", (window.used * 100.).round() as u32), share: window.used.clamp(0., 1.) as f32, level: Level::Calm, resets_at: window.resets })
                    .collect(),
            ),
            Kind::Open => match (&described.section, &described.address) {
                (Some(section), _) => Control::Opens(Act::Section(section.clone())),
                (None, Some(address)) if opens(address, drawing.here) => Control::Opens(Act::Open(address.clone())),
                _ => Control::None,
            },
            // A flow the box leads (signing in, pairing): to come with Accounts (#741).
            // BOX-NEEDED: `steps` isn't answered by a box yet (PROTOCOL.md: "to come, with Accounts").
            Kind::Steps => Control::Text(NOT_YET.into()),
            // What only an installed app draws, by name: none is drawn here yet, so its why, or that.
            Kind::Custom => Control::Text(if why.is_some() { String::new() } else { NOT_YET.into() }),
            Kind::List => Control::None,
            Kind::Unknown => Control::Text(NEWER.into()),
        }
    };
    // One that makes sure it's you first is drawn, and can't be pressed here: its row says why.
    if why.is_none() && described.actions.iter().chain(described.add.iter()).any(|action| action.verified) {
        why = Some(VERIFIED_ELSEWHERE.into());
    }
    let mut drawn = Row::new(key, described.title.clone(), described.line.clone().unwrap_or_default(), control).warning(described.tone == Said::Warning).why(why);
    drawn.badge = described.badge.as_ref().map(|badge| (badge.words.clone(), badge.tone));
    drawn.notice = described.notice.as_ref().map(|notice| (notice.words.clone(), if notice.tone == Said::Warning { Tone::Warning } else { Tone::Quiet }));
    drawn.flow = described.flow.as_ref().map(|flow| {
        let at = At::of(drawing.section, described);
        // (The action that takes a pasted address is drawn beside its field, not as a button.)
        let taken = flow.code.as_ref().map(|code| code.action.as_str());
        FlowDrawn {
            state: flow.state,
            title: flow.title.clone(),
            line: flow.line.clone(),
            buttons: flow.actions.iter().filter(|action| Some(action.id.as_str()) != taken).map(|action| push(described, action, drawing)).collect(),
            code: flow.code.as_ref().map(|code| {
                let title = flow.actions.iter().find(|action| action.id == code.action).map_or_else(|| "Send".to_owned(), |action| action.title.clone());
                (code.placeholder.clone(), title, Act::Code { at, action: code.action.clone() })
            }),
        }
    });
    drawn
}

/// A web page is opened at any box's word; anything else only at the word of the box on this
/// computer (its own Software Update).
fn opens(address: &str, here: bool) -> bool {
    here || address.starts_with("https://") || address.starts_with("http://")
}

/// One of a row's actions as a button: it opens its address itself, or asks the box.
fn push(described: &Described, action: &Action, drawing: &Drawing) -> Push {
    let act = match (&action.opens, action.why_not()) {
        _ if action.verified => None,
        (_, Some(_)) => None,
        (Some(address), None) => opens(address, drawing.here).then(|| Act::Open(address.clone())),
        (None, None) => Some(match &action.form {
            Some(form) => Act::Form { at: At::of(drawing.section, described), action: action.id.clone(), form: Box::new(form.clone()) },
            None => Act::Do { at: At::of(drawing.section, described), action: action.id.clone(), confirm: action.confirm.clone(), asked: false },
        }),
    };
    Push { label: action.title.clone(), act }
}

fn shown_value(row: &Described) -> String {
    match row.value.as_ref() {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) if row.unit.is_some() => row.unit.as_ref().unwrap().number(number.as_f64().unwrap_or_default()),
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn(actions: Value) -> Control {
        let page: Page = serde_json::from_value(json!({ "id": "asks", "title": "Asks", "groups": [{ "id": "now", "rows": [
            { "id": "ask", "title": "May it read your passwords?", "kind": "actions", "revision": "r1", "actions": actions }
        ] }] }))
        .unwrap();
        let filters = std::collections::HashMap::new();
        let drawing = Drawing { section: "asks", busy: &[], replacing: &[], here: true, filters: &filters };
        match blocks(&page, &drawing).into_iter().next() {
            Some(Block::Group(_, _, mut rows)) => rows.remove(0).control,
            _ => panic!("no group"),
        }
    }

    #[test]
    fn three_answers_to_a_question_are_three_buttons_and_more_or_a_form_a_menu() {
        let answers = json!([
            { "id": "deny", "title": "Don't Allow" },
            { "id": "once", "title": "Allow Once" },
            { "id": "always", "title": "Always Allow…", "confirm": { "title": "Always allow it?", "button": "Always Allow" } }
        ]);
        match drawn(answers) {
            Control::Buttons(pushes) => assert_eq!(pushes.iter().map(|push| push.label.as_str()).collect::<Vec<_>>(), ["Don't Allow", "Allow Once", "Always Allow…"]),
            _ => panic!("three answers aren't three buttons"),
        }
        let with_form = json!([
            { "id": "a", "title": "One" }, { "id": "b", "title": "Two" },
            { "id": "c", "title": "Three…", "form": { "title": "Three", "submit": "Go", "fields": [] } }
        ]);
        assert!(matches!(drawn(with_form), Control::Menu { .. }));
        let four = json!([{ "id": "a", "title": "A" }, { "id": "b", "title": "B" }, { "id": "c", "title": "C" }, { "id": "d", "title": "D" }]);
        assert!(matches!(drawn(four), Control::Menu { .. }));
    }

    #[test]
    fn what_makes_sure_its_you_first_is_drawn_and_cant_be_pressed_here_and_says_why() {
        let page: Page = serde_json::from_value(json!({ "id": "things", "title": "Things", "groups": [{ "id": "one", "rows": [
            { "id": "thing", "title": "A thing", "kind": "actions", "revision": "r1", "actions": [{ "id": "grant", "title": "Let It Use…", "verified": true }, { "id": "remove", "title": "Remove" }] }
        ] }] }))
        .unwrap();
        let filters = std::collections::HashMap::new();
        let drawing = Drawing { section: "things", busy: &[], replacing: &[], here: true, filters: &filters };
        let Some(Block::Group(_, _, rows)) = blocks(&page, &drawing).into_iter().next() else { panic!("no group") };
        let Control::Buttons(pushes) = &rows[0].control else { panic!("not buttons") };
        assert!(pushes[0].act.is_none() && pushes[1].act.is_some());
        assert_eq!(rows[0].why.as_deref(), Some(VERIFIED_ELSEWHERE));
    }
}
