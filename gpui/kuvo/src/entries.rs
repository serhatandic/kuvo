//! A conversation's rows: which of the box's messages show, and how each is drawn. Your messages
//! by their parts (what you wrote, the keyword it went with, who it's from, files it names); one
//! another agent wrote, and its sessions' news, as a card from that agent; the agent's words
//! (Markdown); what it did between its words (tool steps and thinking, across its steps) as one
//! group, folded to a line that sums it up; the end of each turn as how long it worked. What's only
//! for the agent (the box's other notes, a switch of mode) shows nothing.

use kuvo_ui::motion::{MotionExt as _};
use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, Window, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div,
    prelude::FluentBuilder as _, px, relative,
};
use kuvo_box::messages::{Content, Message, News, Step};
use kuvo_box::{Conversation, Parts, pictures};
use kuvo_box::pictures::Picture;

use crate::pictures::Looks;
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    markdown::{self, Markdown, Selectable},
    output::{DiffView, OutputWell, diff_units, parse_patch, well_units},
    selection::{Lead, SelectableText, SelectionState, Unit},
    small::{Chip, ChipTone, spinner},
    theme::{radius, space},
};
use serde_json::Value;

use crate::time;

/// One row of the conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// The way back to earlier messages: they load as it comes into view.
    Earlier,
    /// One of its messages (boxed: a message is large, and most rows are one): yours, a note, a
    /// command you ran. (The agent's steps are drawn as their words and their groups.)
    Message(Box<Message>),
    /// Some of a step of the agent's: its words (`parts`, where each is in the step) and, `error`,
    /// why it stopped.
    Words { key: String, step: Rc<Step>, parts: Vec<usize>, error: bool },
    /// What the agent did between its words, one after another across its steps: tool steps and
    /// thinking, each by its step and where it is in it. Drawn as one group.
    Steps { key: String, items: Vec<(Rc<Step>, usize)> },
    /// How long a turn worked, or how it ended, at its end.
    TurnEnd { id: String, outcome: String, worked: Option<f64> },
    /// A run under way, after what it has said so far.
    Working { title: String },
    /// Yours, sent from here and on its way (`Composer::outgoing`): drawn at once as your message,
    /// until the box's copy (by the same id) takes its place. `failed`: why it couldn't go; `waits`:
    /// its box took it while the agent works, and it's read at the agent's next step.
    /// `files`: the names of those that aren't pictures; `pictures`: those that are, read from here.
    Outgoing { id: String, words: String, files: Vec<String>, pictures: Vec<Picture>, failed: Option<String>, waits: bool },
}

impl Entry {
    /// What it's known by from one read to the next.
    pub fn key(&self) -> &str {
        match self {
            Entry::Earlier => "earlier",
            Entry::Working { .. } => "working",
            Entry::Message(message) => message.id(),
            Entry::TurnEnd { id, .. } | Entry::Outgoing { id, .. } => id,
            Entry::Words { key, .. } | Entry::Steps { key, .. } => key,
        }
    }

    /// Whether `fold` is in this row, which is drawn again as it opens or closes.
    pub fn holds(&self, fold: &Fold) -> bool {
        match (self, fold) {
            (Entry::Steps { key, .. }, Fold::Group(group)) => key == group,
            (Entry::Steps { items, .. }, Fold::Thinking(step, n) | Fold::Step(step, n) | Fold::Whole(step, n, _)) => items.iter().any(|(each, at)| each.id == *step && at == n),
            (Entry::Steps { .. } | Entry::Words { .. }, _) => false,
            (entry, fold) => entry.key() == fold.step(),
        }
    }

    /// Yours on its way, as its conversation draws it, with the revision it's drawn by: above every
    /// one the box gives, so the box's copy taking its place is always drawn again.
    pub fn outgoing(outgoing: &crate::composer::Outgoing, running: bool) -> (Entry, u64) {
        let files = outgoing.files.iter().filter(|path| !pictures::is_picture_path(&path.to_string_lossy())).map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()).collect();
        let shown = pictures::of_sending(&outgoing.id, &outgoing.files);
        // (One for after the run goes to the queue over the composer, not the agent's next step.)
        let waits = outgoing.taken && running && outgoing.failed.is_none() && !outgoing.is_queued();
        let revision = if outgoing.failed.is_some() { u64::MAX - 1 } else if waits { u64::MAX - 2 } else { u64::MAX };
        (Entry::Outgoing { id: outgoing.id.clone(), words: outgoing.words.clone(), files, pictures: shown, failed: outgoing.failed.clone(), waits }, revision)
    }
}

/// The rows of `conversation`, oldest first, each with its revision (it goes up as its message
/// changes, so only what changed is drawn again). Its messages are moved into them, not copied.
pub fn entries(mut conversation: Conversation) -> (Vec<Entry>, Vec<u64>) {
    let mut rows = Vec::new();
    let mut revisions = Vec::new();
    if conversation.earlier.is_some() {
        rows.push(Entry::Earlier);
        revisions.push(0);
    }
    // A turn's length counts from its first message, as the Mac's does.
    let mut asked_at: Option<f64> = None;
    let mut summarizing: Option<String> = None;
    // The group the agent's steps go on in: until anything else shows.
    let mut group: Option<usize> = None;
    for message in std::mem::take(&mut conversation.messages) {
        let revision = conversation.revision(message.id());
        if let Message::Assistant(step) = message {
            if shows(&step) {
                agent_rows(Rc::new(step), revision, &mut group, &mut rows, &mut revisions);
            }
            continue;
        }
        let row = match &message {
            // Its sessions' news shows, as messages from them; the box's other notes don't.
            Message::User(user) if !user.news().is_empty() || user.unread_news().is_some() => Some(Entry::Message(Box::new(message))),
            Message::User(user) if user.is_box_note() => None,
            Message::User(user) => {
                asked_at.get_or_insert(user.time.created);
                Some(Entry::Message(Box::new(message)))
            }
            Message::System(note) if !note.text.trim().is_empty() => Some(Entry::Message(Box::new(message))),
            Message::Shell(_) => Some(Entry::Message(Box::new(message))),
            Message::Other { kind, raw, .. } if kind == "compaction" => {
                summarizing = (raw["status"].as_str() == Some("running")).then(|| raw["workingTitle"].as_str().unwrap_or_default().to_owned());
                Some(Entry::Message(Box::new(message)))
            }
            Message::Idle { id, time, outcome } => {
                summarizing = None;
                let worked = asked_at.take().map(|asked| time.created - asked).filter(|worked| *worked > 0.0);
                Some(Entry::TurnEnd { id: id.clone(), outcome: outcome.clone(), worked })
            }
            _ => None,
        };
        if let Some(row) = row {
            group = None;
            rows.push(row);
            revisions.push(revision);
        }
    }
    if conversation.running {
        // BOX-NEEDED: the working row's words for a run that isn't a summary ("Working…"); only a
        // summary's come from the box so far (`workingTitle`, `GET api/session/<id>/message`).
        rows.push(Entry::Working { title: summarizing.unwrap_or_else(|| "Working…".to_owned()) });
        revisions.push(0);
    }
    (rows, revisions)
}

/// The rows of one of the agent's steps: its words, each run of them a row, and what it did between
/// them (tool steps, thinking) going on in `group`, the one the steps before it left open, or a new
/// one. Each row is known by where it starts (the step's id at its start), so a group stays the same
/// row as what it did grows; a group drawn from several steps changes with any of them.
fn agent_rows(step: Rc<Step>, revision: u64, group: &mut Option<usize>, rows: &mut Vec<Entry>, revisions: &mut Vec<u64>) {
    let key = |n: usize| if n == 0 { step.id.clone() } else { format!("{}#{n}", step.id) };
    let mut words: Vec<usize> = Vec::new();
    let flush = |words: &mut Vec<usize>, error: bool, at: usize, rows: &mut Vec<Entry>, revisions: &mut Vec<u64>| {
        if words.is_empty() && !error {
            return;
        }
        let parts = std::mem::take(words);
        rows.push(Entry::Words { key: key(parts.first().copied().unwrap_or(at)), step: step.clone(), parts, error });
        revisions.push(revision);
    };
    for (n, content) in step.content.iter().enumerate() {
        match content {
            Content::Text(text) if !text.trim().is_empty() => {
                *group = None;
                words.push(n);
            }
            Content::Reasoning { .. } | Content::Tool { .. } => {
                flush(&mut words, false, n, rows, revisions);
                match *group {
                    Some(ix) => {
                        if let Some(Entry::Steps { items, .. }) = rows.get_mut(ix) {
                            items.push((step.clone(), n));
                        }
                        revisions[ix] = revisions[ix].max(revision);
                    }
                    None => {
                        *group = Some(rows.len());
                        rows.push(Entry::Steps { key: key(n), items: vec![(step.clone(), n)] });
                        revisions.push(revision);
                    }
                }
            }
            _ => {}
        }
    }
    // How it failed ends what it did, and shows outside any group.
    let error = step.error.is_some();
    if error {
        *group = None;
    }
    flush(&mut words, error, step.content.len(), rows, revisions);
}

/// Whether a step has anything to show: words, thinking, a tool step, or an error.
fn shows(step: &Step) -> bool {
    step.error.is_some()
        || step.content.iter().any(|content| match content {
            Content::Text(text) => !text.trim().is_empty(),
            Content::Reasoning { .. } | Content::Tool { .. } => true,
            Content::Other { .. } => false,
        })
}

/// The widest a conversation's column gets: it stays readable in a wide window.
pub const COLUMN: f32 = 720.;

/// How large pictures are drawn: one a step brought (one alone, several), as 0.7 drew them, fitted
/// to the column; and yours, as thumbnails on your message (one alone, several), 240 points on
/// their longest side at most (the user, 11-16: 0.7's 360 × 220 looked out of place).
const MADE: ((f32, f32), (f32, f32)) = ((560., 420.), (300., 200.));
const YOURS: ((f32, f32), (f32, f32)) = ((240., 240.), (160., 160.));

/// Your pictures, as thumbnails on your side, over your words: in a row that wraps, the first
/// nearest the bubble.
fn yours_pictures(list: &[Picture], looks: &Looks, cx: &App) -> AnyElement {
    let (most_w, most_h) = if list.len() == 1 { YOURS.0 } else { YOURS.1 };
    div()
        .flex()
        .flex_wrap()
        .justify_end()
        .gap(space::S1_5)
        .children(list.iter().map(|each| crate::pictures::picture(each, most_w, most_h, looks, cx)))
        .into_any_element()
}

/// What's opened in a conversation, each by its step's id and which of the step's blocks it is:
/// a thinking block, a tool step showing what it printed or changed, or that output (or one of its
/// files' changes) shown whole rather than folded; a group of what the agent did, by its row, shown
/// step by step; a message from another agent, by its id and which of them, shown whole.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Fold {
    Thinking(String, usize),
    Step(String, usize),
    Whole(String, usize, usize),
    Group(String),
    Card(String, usize),
}

impl Fold {
    /// The step (or row, or message) it's in, whose row is drawn again as it changes.
    pub fn step(&self) -> &str {
        match self {
            Fold::Thinking(step, _) | Fold::Step(step, _) | Fold::Whole(step, _, _) | Fold::Card(step, _) => step,
            Fold::Group(row) => row,
        }
    }
}

/// What a conversation's rows do beyond opening and closing what's in them: open another of your
/// conversations (one an agent's message is from), and whether it waits on you now (a step under
/// way then is what it waits on, and shows outside its group).
#[derive(Clone)]
pub struct Around {
    pub open_session: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    pub waiting: bool,
}

impl Around {
    /// Rows drawn on their own (a group chat's): nothing to open, and nothing waits.
    pub fn none() -> Around {
        Around { open_session: Rc::new(|_, _, _| {}), waiting: false }
    }
}

/// Draws row `ix`. `open`: what's opened; `toggle` opens or closes one. Its text is selectable in
/// `selection`, as [`units`] says. `own`: what its view draws of it (the way back to earlier
/// messages; Retry and Edit under one of yours that couldn't go).
#[allow(clippy::too_many_arguments)]
pub fn render(
    ix: usize,
    entry: &Entry,
    open: &HashSet<Fold>,
    toggle: impl Fn(Fold, &mut App) + Clone + 'static,
    own: AnyElement,
    around: &Around,
    selection: &SelectionState,
    looks: &Looks,
    cx: &App,
) -> AnyElement {
    let body: AnyElement = match entry {
        Entry::Earlier => own,
        Entry::Outgoing { words, files, pictures, failed, waits, .. } => {
            let sel = Selectable::new(selection.clone(), SharedString::from(entry.key().to_owned()), &[]);
            let shown = (!pictures.is_empty()).then(|| yours_pictures(pictures, looks, cx));
            outgoing(ix, words, files, shown, failed.as_deref(), *waits, sel, own, cx)
        }
        Entry::Message(message) => match &**message {
            Message::User(user) if user.is_letter() => {
                let sel = |base: &[u32]| Selectable::new(selection.clone(), SharedString::from(entry.key().to_owned()), base);
                from_agents(&user.id, &letters(user), user.time.created, open, toggle, around, &sel, cx)
            }
            // (What its view draws under it: the files sent with yours, while its copy has none.)
            message => div().child(message_body(ix, entry.key(), message, open, toggle, selection, looks, cx)).child(own).into_any_element(),
        },
        Entry::Words { key, step, parts, error } => {
            let sel = |base: &[u32]| Selectable::new(selection.clone(), SharedString::from(key.clone()), base);
            words(step, parts, *error, &sel, looks, cx)
        }
        Entry::Steps { key, items } => {
            let sel = |base: &[u32]| Selectable::new(selection.clone(), SharedString::from(key.clone()), base);
            steps(key, items, open, toggle, around.waiting, &sel, looks, cx)
        }
        Entry::TurnEnd { outcome, worked, .. } => turn_end(outcome, *worked, cx),
        Entry::Working { title } => working(title, cx),
    };
    frame(ix, entry.key(), body, matches!(entry, Entry::Working { .. }))
}

/// A message as a conversation draws it, its text selectable in `selection` under row `key` (as
/// [`units`] says): what you wrote in your bubble, the agent's steps and words, a note's line.
pub fn message_body(
    ix: usize,
    key: &str,
    message: &Message,
    open: &HashSet<Fold>,
    toggle: impl Fn(Fold, &mut App) + Clone + 'static,
    selection: &SelectionState,
    looks: &Looks,
    cx: &App,
) -> AnyElement {
    let sel = |base: &[u32]| Selectable::new(selection.clone(), SharedString::from(key.to_owned()), base);
    match message {
        // The pictures it carried, as thumbnails on it, over its words (as 0.7 put them).
        Message::User(user) => {
            let shown: Vec<Picture> = looks.shown.iter().filter(|picture| picture.id.starts_with(&format!("{}#", user.id)) || picture.id.starts_with(&format!("{}@", user.id))).cloned().collect();
            let words = user_message(ix, &user.parts(), sel(&[]), cx);
            if shown.is_empty() {
                words
            } else {
                div().w_full().flex().flex_col().items_end().gap(space::S1_5).child(yours_pictures(&shown, looks, cx)).child(words).into_any_element()
            }
        }
        Message::Assistant(step) => agent_step(step, open, toggle, &sel, looks, cx),
        Message::System(note) => quiet_line(note.text.lines().next().unwrap_or("").to_owned(), cx),
        Message::Shell(run) => shell_run(run, open, toggle, &sel, cx),
        Message::Other { kind, raw, .. } if kind == "compaction" => quiet_line(raw["title"].as_str().unwrap_or_default().to_owned(), cx),
        _ => div().into_any_element(),
    }
}

/// Row `ix` of a conversation, `key`: centred in a readable column and spaced from the next
/// (`last`: the run under way, at the end, with more room under it).
pub fn frame(ix: usize, key: &str, body: AnyElement, last: bool) -> AnyElement {
    let last_gap = if last { space::S8 } else { space::S5 };
    let selector = format!("entry-{key}");
    div()
        .debug_selector(move || selector)
        .w_full()
        .flex()
        .justify_center()
        .px(space::S6)
        .pb(last_gap)
        .when(ix == 0, |d| d.pt(space::S6))
        .child(div().w_full().max_w(px(COLUMN)).child(body))
        .into_any_element()
}

/// The selectable text of a row, as [`render`] draws it, in reading order: what you wrote; the
/// agent's words (`[part, block…]`), its thinking (`[part, 0]`) and what an opened step printed
/// or changed (`[part, 0 and 1]`, `[part, file, line]`). For copying, without laying anything out.
pub fn units(entry: &Entry, open: &HashSet<Fold>) -> Vec<Unit> {
    match entry {
        Entry::Message(message) => match &**message {
            Message::User(user) if user.is_letter() => letters(user)
                .iter()
                .enumerate()
                .filter(|(n, letter)| letter.opens(open, &user.id, *n))
                .flat_map(|(n, letter)| markdown::units(&letter.said, &[n as u32]))
                .collect(),
            message => message_units(message, open),
        },
        Entry::Words { step, parts, .. } => parts
            .iter()
            .filter_map(|n| match &step.content[*n] {
                Content::Text(text) => Some(markdown::units(text, &[*n as u32])),
                _ => None,
            })
            .flatten()
            .collect(),
        Entry::Steps { key, items } => {
            let mut units = Vec::new();
            for (at, (step, n)) in items.iter().enumerate() {
                if !shown_in_group(key, items, at, open, false) {
                    continue;
                }
                units.extend(content_units(step, *n, at as u32, open));
            }
            units
        }
        // (As the box's copy will be: what you wrote.)
        Entry::Outgoing { words, .. } if !quoted(words).1.is_empty() => vec![Unit::new(&[0], quoted(words).1, Lead::Paragraph)],
        _ => Vec::new(),
    }
}

/// The selectable text of a message, as [`message_body`] draws it.
pub fn message_units(message: &Message, open: &HashSet<Fold>) -> Vec<Unit> {
    match message {
        Message::User(user) => {
            let words = quoted(&user.parts().words).1;
            if words.is_empty() { Vec::new() } else { vec![Unit::new(&[0], words, Lead::Paragraph)] }
        }
        Message::Shell(run) => run.output.as_ref().map(|output| well_units(&output.output, Some(&run.command), open.contains(&Fold::Whole(run.id.clone(), 0, 0)), &[0])).unwrap_or_default(),
        Message::Assistant(step) => {
            let mut units = Vec::new();
            for (n, content) in step.content.iter().enumerate() {
                match content {
                    Content::Text(text) if !text.trim().is_empty() => units.extend(markdown::units(text, &[n as u32])),
                    _ => units.extend(content_units(step, n, n as u32, open)),
                }
            }
            units
        }
        _ => Vec::new(),
    }
}

/// The selectable text of a step's thinking or tool step `n`, opened, drawn at `at`: its thinking
/// (`[at, 0]`), or what it printed or changed (`[at, 0 and 1]`, `[at, file, line]`).
fn content_units(step: &Step, n: usize, at: u32, open: &HashSet<Fold>) -> Vec<Unit> {
    let mut units = Vec::new();
    match &step.content[n] {
        Content::Reasoning { text, .. } if open.contains(&Fold::Thinking(step.id.clone(), n)) && !text.trim().is_empty() => {
            units.push(Unit::new(&[at, 0], text.trim(), Lead::Paragraph));
        }
        Content::Tool { name, state, .. } if open.contains(&Fold::Step(step.id.clone(), n)) => match details(name, state) {
            Some(Details::Output { command, text }) => units.extend(well_units(&text, command.as_deref(), open.contains(&Fold::Whole(step.id.clone(), n, 0)), &[at])),
            Some(Details::Changes(files)) => {
                for (p, file) in files.iter().enumerate() {
                    units.extend(diff_units(&parse_patch(&file.patch), open.contains(&Fold::Whole(step.id.clone(), n, p)), &[at, p as u32]));
                }
            }
            None => {}
        },
        _ => {}
    }
    units
}

/// Yours on its way: your message as it will show, its files' names under it, and under them
/// "Sending…", or why it couldn't go with `actions` (Retry, Edit) beside it.
#[allow(clippy::too_many_arguments)]
fn outgoing(ix: usize, words: &str, files: &[String], shown: Option<AnyElement>, failed: Option<&str>, waits: bool, sel: Selectable, actions: AnyElement, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let parts = Parts::words(words.to_owned());
    let names = files.iter().enumerate().map(|(n, name)| Chip::new(("sending-file", ix * 1000 + n), name.clone()).icon(IconName::File).into_any_element());
    let status = match failed {
        // Its box has it; the agent reads it at its next step (0.7's words), and it can be taken back.
        None if waits => div()
            .debug_selector(|| "at-next-step".into())
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap(space::S2)
            .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("At its next step"))
            .child(actions)
            .into_any_element(),
        None => div().debug_selector(|| "sending".into()).text_ui(TypeSize::Caption).text_color(c.text_faint).child("Sending…").into_any_element(),
        Some(why) => div()
            .debug_selector(|| "not-sent".into())
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap(space::S2)
            .child(div().text_ui(TypeSize::Caption).text_color(c.danger).child(SharedString::from(format!("Not sent. {why}"))))
            .child(actions)
            .into_any_element(),
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(space::S1_5)
        .children(shown)
        // The bubble across the row's width, as a sent one's is: its width (and where its words
        // wrap) the same Sending as Sent, its state on its own line under it.
        .child(div().w_full().child(user_message(ix, &parts, sel, cx)))
        .when(!files.is_empty(), |d| d.child(div().flex().flex_wrap().justify_end().gap(space::S1).children(names)))
        .child(status)
        .into_any_element()
}

/// A message's words as what it quotes (each quote, in order) and what it says itself.
fn quoted(words: &str) -> (Vec<String>, String) {
    let segments = kuvo_box::quoting::segments(words);
    let quotes = segments.iter().filter_map(|(quote, _)| quote.clone()).collect();
    let own = segments.into_iter().map(|(_, text)| text).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n\n");
    (quotes, own)
}

fn user_message(ix: usize, parts: &Parts, sel: Selectable, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    // What went with it, above it: the keyword it was sent with as typed, who wrote it.
    let mut above: Vec<AnyElement> = Vec::new();
    if let Some(mail) = &parts.mail {
        above.push(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(format!("From &{}", mail.name)).into_any_element());
    }
    if parts.phone {
        above.push(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("From iPhone").into_any_element());
    }
    if let Some(widget) = &parts.widget {
        let said = if widget.is_empty() { "Answers a widget".to_owned() } else { format!("Answers “{widget}”") };
        above.push(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(said).into_any_element());
    }
    let keyword = parts.keyword.clone().map(|keyword| Chip::new(("keyword", ix), keyword).icon(IconName::Sparkle).tone(ChipTone::Accent));
    let mut below: Vec<AnyElement> = Vec::new();
    // (Pictures it sent as paths show as pictures.)
    for (n, path) in parts.paths.iter().enumerate().filter(|(_, path)| !pictures::is_picture_path(path)) {
        below.push(Chip::new(("path", ix * 1000 + n), short_path(path)).icon(IconName::File).into_any_element());
    }
    if !parts.comments.is_empty() {
        let count = parts.comments.len();
        below.push(Chip::new(("comments", ix), if count == 1 { "1 comment on the changes".to_owned() } else { format!("{count} comments on the changes") }).icon(IconName::Message).into_any_element());
    }
    for (n, agent) in parts.agents.iter().enumerate() {
        below.push(Chip::new(("named", ix * 1000 + n), format!("&{}", agent.name)).icon(IconName::Bot).into_any_element());
    }
    // What it replies to, in its bubble above its own words.
    let (quotes, words) = quoted(parts.words.trim());
    let quote_blocks: Vec<AnyElement> = quotes
        .into_iter()
        .enumerate()
        .map(|(n, quote)| {
            div()
                .debug_selector(move || format!("quoted-{ix}-{n}"))
                .pl(space::S2)
                .border_l_2()
                .border_color(c.border_strong)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(quote)
                .into_any_element()
        })
        .collect();
    let has_quotes = !quote_blocks.is_empty();
    // (The whole column's width, so its bubble is as wide as words need, with pictures over it or not.)
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(space::S1_5)
        .children(above)
        .child(
            div()
                .max_w(relative(0.78))
                .flex()
                .flex_col()
                .items_end()
                .gap(space::S1_5)
                .children(keyword)
                .when(!words.is_empty() || has_quotes, |d| {
                    d.child(
                        div()
                            .debug_selector(move || format!("bubble-{ix}"))
                            .px(space::S4)
                            .py(px(10.))
                            .rounded(radius::CARD)
                            .bg(c.sunken)
                            .flex()
                            .flex_col()
                            .gap(space::S2)
                            .text_ui(TypeSize::Body)
                            .text_color(c.text)
                            .children(quote_blocks)
                            .when(!words.is_empty(), |d| d.child(SelectableText::new(("words", ix), words).selectable(Some(sel.unit(&[0]))))),
                    )
                }),
        )
        .when(!below.is_empty(), |d| d.child(div().flex().flex_wrap().justify_end().gap(space::S1).children(below)))
        .into_any_element()
}

fn agent_step(step: &Step, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks, cx: &App) -> AnyElement {
    let mut blocks: Vec<AnyElement> = Vec::new();
    for (n, content) in step.content.iter().enumerate() {
        match content {
            Content::Text(text) if !text.trim().is_empty() => {
                blocks.push(words_block(step, n, text, sel, looks));
            }
            _ => blocks.extend(content_blocks(step, n, n as u32, open, toggle.clone(), false, sel, looks, cx)),
        }
    }
    if let Some(error) = &step.error {
        blocks.push(callout(error_words(error), cx));
    }
    div().flex().flex_col().gap(GAP).children(blocks).into_any_element()
}

/// The one gap between the lines of what the agent did (a tool step, thinking, what one opened to,
/// why one failed), and between its words' blocks: lines that belong together sit this close, and
/// rows (a message, its words, a group) [`frame`]'s gap apart.
const GAP: gpui::Pixels = space::S2;

/// Where a step's line puts what goes under it (why it failed, what it opened to): under its words,
/// past its icon.
const UNDER: gpui::Pixels = px(22.);

/// Some of the agent's step: its words, and why it stopped.
fn words(step: &Step, parts: &[usize], error: bool, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks, cx: &App) -> AnyElement {
    let blocks = parts.iter().filter_map(|n| match &step.content[*n] {
        Content::Text(text) => Some(words_block(step, *n, text, sel, looks)),
        _ => None,
    });
    div()
        .flex()
        .flex_col()
        .gap(GAP)
        .children(blocks)
        .when_some(step.error.as_ref().filter(|_| error), |d, error| d.child(callout(error_words(error), cx)))
        .into_any_element()
}

/// Block `n` of a step's words, as Markdown, with the pictures written in it.
fn words_block(step: &Step, n: usize, text: &str, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks) -> AnyElement {
    let mut words = Markdown::new(SharedString::from(format!("{}:{n}", step.id)), text).selectable(sel(&[n as u32]));
    // (Pictures written in its words, drawn as the conversation reads them.)
    if text.contains("![") {
        words = words.pictures(crate::pictures::words_pictures(&step.id, n, text, MADE.0, looks));
    }
    div().child(words).into_any_element()
}

/// What tool step `n` of a step made, showed you, or took of a page to show you: large, in the flow,
/// shown whether or not its group is opened.
fn made_pictures(step: &Step, n: usize, looks: &Looks, cx: &App) -> Option<AnyElement> {
    let Content::Tool { name, state, .. } = &step.content[n] else { return None };
    if !pictures::is_made(name, state) {
        return None;
    }
    let prefix = format!("{}:{n}-", step.id);
    let made: Vec<Picture> = looks.shown.iter().filter(|picture| picture.id.starts_with(&prefix)).cloned().collect();
    (!made.is_empty()).then(|| crate::pictures::pictures(&made, MADE.0, MADE.1, looks, cx))
}

/// A step's thinking or tool step `n`, drawn as item `at` of its row: its line, why it failed and
/// what it's opened to. `waits`: it's what the conversation waits on you for.
#[allow(clippy::too_many_arguments)]
fn content_blocks(step: &Step, n: usize, at: u32, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, waits: bool, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks, cx: &App) -> Vec<AnyElement> {
    let c = &cx.theme().colors;
    let chevron = |opened: bool| Icon::new(if opened { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Nav).color(c.text_faint);
    let key = format!("{}:{n}", step.id);
    let mut blocks: Vec<AnyElement> = Vec::new();
    match &step.content[n] {
        Content::Reasoning { text, done } => {
            let fold = Fold::Thinking(step.id.clone(), n);
            let opened = open.contains(&fold);
            let label = if *done { "Thought" } else { "Thinking" };
            let row = step_row(SharedString::from(format!("think-{key}")), IconName::Sparkle, label.into(), None, !*done, false, cx)
                .cursor_pointer()
                .hover(|s| s.opacity(0.8))
                .on_click(move |_, _, cx| toggle(fold.clone(), cx))
                .child(chevron(opened));
            blocks.push(row.into_any_element());
            if opened && !text.trim().is_empty() {
                blocks.push(
                    div()
                        .ml(px(7.))
                        .pl(space::S3)
                        .border_l_1()
                        .border_color(c.border_strong)
                        .text_ui(TypeSize::Small)
                        .text_color(c.text_muted)
                        .child(SelectableText::new(SharedString::from(format!("thought-{key}")), text.trim().to_owned()).selectable(Some(sel(&[at]).unit(&[0]))))
                        .into_any_element(),
                );
            }
        }
        Content::Tool { name, state, title, doing, .. } => {
            let (busy, failed) = call_status(state);
            let (icon, label, target) = call_label(name, title.as_deref(), doing.as_deref(), state, busy);
            let shown = details(name, state);
            let fold = Fold::Step(step.id.clone(), n);
            // (What it brought back for itself shows once it's opened.)
            let opens = shown.is_some() || pictures::has_own(name, state);
            let opened = opens && open.contains(&fold);
            let mut row = step_row(SharedString::from(format!("tool-{key}")), icon, label, target, busy, failed, cx);
            // One with something to show opens to it.
            if opens {
                let toggle = toggle.clone();
                let fold = fold.clone();
                row = row
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .on_click(move |_, _, cx| toggle(fold.clone(), cx))
                    .children(shown.as_ref().and_then(Details::counts).map(|(added, removed)| counts(added, removed, cx)))
                    .child(chevron(opened));
            }
            blocks.push(row.into_any_element());
            if busy && waits {
                blocks.push(
                    div()
                        .debug_selector(|| "waits-for-you".into())
                        .ml(UNDER)
                        .text_ui(TypeSize::Caption)
                        .text_color(c.warning)
                        .child("Waiting for you")
                        .into_any_element(),
                );
            }
            if failed && let Some(error) = tool_error(state) {
                blocks.push(div().ml(UNDER).text_ui(TypeSize::Caption).text_color(c.danger).child(first_line(&error)).into_any_element());
            }
            if let (true, Some(shown)) = (opened, shown) {
                blocks.push(div().ml(UNDER).child(details_view(&key, step, n, at, shown, open, toggle, sel)).into_any_element());
            }
            if opened && !pictures::is_made(name, state) {
                let prefix = format!("{}:{n}-", step.id);
                let own: Vec<Picture> = looks.shown.iter().filter(|picture| picture.id.starts_with(&prefix)).cloned().collect();
                if !own.is_empty() {
                    blocks.push(div().ml(UNDER).child(crate::pictures::pictures(&own, MADE.1, MADE.1, looks, cx)).into_any_element());
                }
            }
            blocks.extend(made_pictures(step, n, looks, cx));
        }
        _ => {}
    }
    blocks
}

/// Whether a tool step is under way, and whether it failed, as the engine says.
fn call_status(state: &Value) -> (bool, bool) {
    let status = state.get("status").and_then(Value::as_str).unwrap_or("");
    (matches!(status, "running" | "streaming" | "pending"), status == "failed" || status == "error")
}

/// Whether item `at` of a group shows: all of them once it's opened (or when it holds only one
/// tool step, which needs no summing up); folded, only a step that failed, or the one it waits on
/// you for (`waiting`), so what needs you is never folded away.
fn shown_in_group(key: &str, items: &[(Rc<Step>, usize)], at: usize, open: &HashSet<Fold>, waiting: bool) -> bool {
    if !folds(items) || open.contains(&Fold::Group(key.to_owned())) {
        return true;
    }
    let (step, n) = &items[at];
    match &step.content[*n] {
        Content::Tool { state, .. } => call_status(state).1 || (waiting && waiting_at(items) == Some(at)),
        _ => false,
    }
}

/// The step of a group the conversation waits on you for, while it does: its last one under way.
fn waiting_at(items: &[(Rc<Step>, usize)]) -> Option<usize> {
    items.iter().rposition(|(step, n)| matches!(&step.content[*n], Content::Tool { state, .. } if call_status(state).0))
}

/// Whether a group folds to a line: it holds more than one tool step.
fn folds(items: &[(Rc<Step>, usize)]) -> bool {
    items.iter().filter(|(step, n)| matches!(step.content[*n], Content::Tool { .. })).count() > 1
}

/// What the agent did between its words, as one group: a line summing it up ("Read 4 files, ran 2
/// commands") that opens to each step, with the step under way on it as it runs. A step that
/// failed, or that waits on you, shows under it even while it's folded.
#[allow(clippy::too_many_arguments)]
fn steps(key: &str, items: &[(Rc<Step>, usize)], open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, waiting: bool, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    // What it waits on you for: its last step under way.
    let waits_at = waiting.then(|| waiting_at(items)).flatten();
    let lines = |only: &dyn Fn(usize) -> bool| -> Vec<AnyElement> {
        items
            .iter()
            .enumerate()
            .filter(|(at, _)| only(*at))
            .flat_map(|(at, (step, n))| content_blocks(step, *n, at as u32, open, toggle.clone(), waits_at == Some(at), sel, looks, cx))
            .collect()
    };
    if !folds(items) {
        return div().flex().flex_col().gap(GAP).children(lines(&|_| true)).into_any_element();
    }
    let fold = Fold::Group(key.to_owned());
    let opened = open.contains(&fold);
    let calls: Vec<Call> = items
        .iter()
        .filter_map(|(step, n)| match &step.content[*n] {
            Content::Tool { name, state, title, doing, .. } => Some(Call { name, title: title.as_deref(), doing: doing.as_deref(), state }),
            _ => None,
        })
        .collect();
    // (While it waits on you, that step shows under the line, and the line sums up the rest.)
    let under_way = calls.iter().rev().find(|call| call_status(call.state).0).filter(|_| waits_at.is_none());
    let thinking = items.iter().any(|(step, n)| matches!(step.content[*n], Content::Reasoning { done: false, .. }));
    let id = SharedString::from(format!("group-{key}"));
    let leading: AnyElement = if under_way.is_some() || thinking {
        spinner(SharedString::from(format!("{id}-spin")), IconSize::Nav, c.text_muted).into_any_element()
    } else {
        Icon::new(IconName::Check).size(IconSize::Nav).color(c.text_faint).into_any_element()
    };
    // Under way: what it's doing now, then what it did before it.
    let (label, target, rest) = match under_way {
        Some(call) => {
            let (_, label, target) = call_label(call.name, call.title, call.doing, call.state, true);
            let done: Vec<Call> = calls.iter().filter(|each| !call_status(each.state).0).cloned().collect();
            (label, target, (!done.is_empty()).then(|| summary(&done)))
        }
        None => (SharedString::from(summary(&calls)), None, None),
    };
    let selector = id.to_string();
    let toggle_group = toggle.clone();
    let line = div()
        .id(id)
        .debug_selector(move || selector)
        .flex()
        .items_center()
        .gap(space::S1_5)
        .min_w_0()
        .text_ui(TypeSize::Small)
        .text_color(c.text_muted)
        .cursor_pointer()
        .hover(|s| s.opacity(0.8))
        .on_click(move |_, _, cx| toggle_group(fold.clone(), cx))
        .child(div().flex().flex_none().w(px(16.)).justify_center().child(leading))
        .child(div().flex_none().child(label))
        .children(target.map(|target| div().min_w_0().truncate().text_mono(TypeSize::Small).text_color(c.text_faint).child(target)))
        .children(rest.map(|rest| div().min_w_0().truncate().text_color(c.text_faint).child(format!("· {}", lowered(&rest)))))
        .child(Icon::new(if opened { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Nav).color(c.text_faint));
    let body = if opened {
        // Each step, along a line from the group's.
        Some(div().ml(px(7.)).pl(space::S3).border_l_1().border_color(c.border).flex().flex_col().gap(GAP).children(lines(&|_| true)).into_any_element())
    } else {
        // What needs you, and what it showed you, in its place.
        let mut needs_you = Vec::new();
        for (at, (step, n)) in items.iter().enumerate() {
            if shown_in_group(key, items, at, open, waiting) {
                needs_you.extend(content_blocks(step, *n, at as u32, open, toggle.clone(), waits_at == Some(at), sel, looks, cx));
            } else {
                needs_you.extend(made_pictures(step, *n, looks, cx));
            }
        }
        (!needs_you.is_empty()).then(|| div().flex().flex_col().gap(GAP).children(needs_you).into_any_element())
    };
    div().flex().flex_col().gap(GAP).child(line).children(body).into_any_element()
}

/// A tool step, as a group's line sums it up.
#[derive(Clone)]
struct Call<'a> {
    name: &'a str,
    title: Option<&'a str>,
    doing: Option<&'a str>,
    state: &'a Value,
}

/// What a group of tool steps did, in a line: each kind once, in the order it first came, counted
/// ("Read 4 files, ran 2 commands, searched twice"); files counted once however often they were
/// read or edited. Past four kinds, how many more steps there were.
fn summary(calls: &[Call]) -> String {
    #[derive(PartialEq)]
    enum Kind<'a> {
        Read,
        Edited,
        Ran,
        Searched,
        Fetched,
        Asked,
        Planned,
        Titled(&'a str),
        Other,
    }
    let mut kinds: Vec<(Kind, usize, HashSet<String>)> = Vec::new();
    for call in calls {
        let kind = match call.name {
            "read" => Kind::Read,
            "edit" | "multiedit" | "patch" | "apply_patch" | "write" => Kind::Edited,
            "shell" => Kind::Ran,
            "grep" | "glob" | "list" | "ls" | "websearch" => Kind::Searched,
            "webfetch" => Kind::Fetched,
            "task" | "subagent" => Kind::Asked,
            "todowrite" | "todoread" => Kind::Planned,
            _ => call.title.map_or(Kind::Other, Kind::Titled),
        };
        let (_, _, target) = tool_label(call.name, call.state, false);
        let at = match kinds.iter().position(|(each, _, _)| *each == kind) {
            Some(at) => at,
            None => {
                kinds.push((kind, 0, HashSet::new()));
                kinds.len() - 1
            }
        };
        let (_, count, targets) = &mut kinds[at];
        *count += 1;
        if let Some(target) = target {
            targets.insert(target.to_string());
        }
    }
    let plural = |count: usize, one: &str, many: &str| if count == 1 { format!("1 {one}") } else { format!("{count} {many}") };
    let mut said: Vec<String> = kinds
        .iter()
        .take(4)
        .map(|(kind, count, targets)| {
            // Files once each, however often they were read or edited.
            let files = if targets.is_empty() { *count } else { targets.len() };
            match kind {
                Kind::Read => format!("read {}", plural(files, "file", "files")),
                Kind::Edited => format!("edited {}", plural(files, "file", "files")),
                Kind::Ran => format!("ran {}", plural(*count, "command", "commands")),
                Kind::Searched => match count {
                    1 => "searched once".to_owned(),
                    2 => "searched twice".to_owned(),
                    n => format!("searched {n} times"),
                },
                Kind::Fetched => format!("read {}", plural(*count, "web page", "web pages")),
                Kind::Asked => format!("asked {}", plural(*count, "subagent", "subagents")),
                Kind::Planned => "updated its plan".to_owned(),
                Kind::Titled(title) if *count == 1 => lowered(title),
                Kind::Titled(title) => format!("{} ×{count}", lowered(title)),
                Kind::Other => format!("used {}", plural(*count, "tool", "tools")),
            }
        })
        .collect();
    let more: usize = kinds.iter().skip(4).map(|(_, count, _)| count).sum();
    if more > 0 {
        said.push(format!("and {}", plural(more, "more step", "more steps")));
    }
    let line = said.join(", ");
    let mut letters = line.chars();
    letters.next().map(|first| first.to_uppercase().chain(letters).collect()).unwrap_or_default()
}

/// Words that go on from others: their first letter small, unless the first word is a name (a
/// service's, "Linear · Create issue"; a word in capitals).
fn lowered(words: &str) -> String {
    let first = words.split_whitespace().next().unwrap_or("");
    let a_name = words.contains(" · ") || first.chars().skip(1).any(char::is_uppercase);
    if a_name {
        return words.to_owned();
    }
    let mut letters = words.chars();
    letters.next().map(|first| first.to_lowercase().chain(letters).collect()).unwrap_or_default()
}

/// What a card in a conversation is: a message from another agent, one of its sessions' news, or a
/// wake-up (or watch) of its own. Each has its own mark and title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mail,
    News,
    Wake,
}

/// A message the user didn't write, as its card shows it: what it is, who it's from (a wake-up:
/// "Woke up", as the phone says it), what happened ("finished", "asks"), what it said, the line it shows folded where it
/// isn't its first (a wake-up's summary), and the conversation it's from (none where it can't be
/// opened).
#[derive(Clone, Debug, PartialEq)]
pub struct Letter {
    pub kind: Kind,
    pub from: String,
    pub status: Option<String>,
    pub said: String,
    pub preview: Option<String>,
    pub session: Option<String>,
}

impl Letter {
    /// Whether it's shown whole: one that's short is, and a longer one once it's opened.
    fn opens(&self, open: &HashSet<Fold>, message: &str, n: usize) -> bool {
        !self.folds() || open.contains(&Fold::Card(message.to_owned(), n))
    }

    /// Whether there's more to it than its folded line shows.
    fn folds(&self) -> bool {
        let said = self.said.trim();
        if let Some(preview) = &self.preview {
            return said != preview.trim();
        }
        said.lines().count() > 1 || said.chars().count() > 140
    }

    /// The line it shows folded: its own (a wake-up's summary), or its first as it reads, without
    /// the marks that shape it whole.
    fn folded_line(&self) -> String {
        if let Some(preview) = &self.preview {
            return preview.trim().to_owned();
        }
        self.said.trim().lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim().trim_start_matches(['#', '>', '-', '*', ' ']).replace("**", "").replace('`', "")
    }

    /// Its mark, and the colour it's drawn in: a send mark for a message, an alarm clock for a
    /// wake-up, and for a session's news what happened (finished, asks, stopped).
    fn mark(&self, c: &kuvo_ui::theme::Colors) -> (IconName, gpui::Hsla) {
        match (self.kind, self.status.as_deref()) {
            (Kind::Mail, _) => (IconName::Send, c.accent),
            (Kind::Wake, _) => (IconName::Alarm, c.accent),
            (Kind::News, Some("finished")) => (IconName::Check, c.success),
            (Kind::News, Some("asks")) => (IconName::Question, c.accent),
            (Kind::News, Some("stopped" | "not summarized")) => (IconName::Warning, c.warning),
            (Kind::News, Some("stopped by you")) => (IconName::Close, c.text_muted),
            (Kind::News, _) => (IconName::Send, c.accent),
        }
    }
}

/// The letters a message of another agent's, or the box's note of its sessions' news, is: one for
/// each session it tells of, or the one from the agent that wrote it.
pub fn letters(user: &kuvo_box::messages::UserMessage) -> Vec<Letter> {
    // Its sessions' news its box couldn't read: whole, from them, rather than lost.
    if let Some(text) = user.unread_news() {
        return vec![Letter { kind: Kind::News, from: "Its sessions".into(), status: None, said: text, preview: None, session: None }];
    }
    // A wake-up of its own: its summary, opening to its note.
    if let Some(wake) = user.wake() {
        return vec![Letter { kind: Kind::Wake, from: "Woke up".into(), status: None, said: wake.note, preview: Some(wake.summary), session: None }];
    }
    let news: Vec<News> = user.news();
    if !news.is_empty() {
        return news
            .into_iter()
            .map(|news| Letter { kind: Kind::News, from: news.title, status: news.status, said: news.said, preview: None, session: Some(news.session).filter(|session| session.starts_with("ses")) })
            .collect();
    }
    let parts = user.parts();
    let Some(mail) = parts.mail else { return Vec::new() };
    vec![Letter { kind: Kind::Mail, from: mail.name, status: None, said: parts.words.trim().to_owned(), preview: None, session: Some(mail.session).filter(|session| session != "unknown" && !session.is_empty()) }]
}

/// Messages from other agents (or a lead's sessions' news), each as a card of its own: a send mark,
/// "From <name>" and what happened, the time and Open (its conversation), and its first line,
/// opening to all of it.
#[allow(clippy::too_many_arguments)]
fn from_agents(message: &str, letters: &[Letter], at: f64, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, around: &Around, sel: &dyn Fn(&[u32]) -> Selectable, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let cards = letters.iter().enumerate().map(|(n, letter)| {
        let whole = letter.opens(open, message, n);
        let fold = Fold::Card(message.to_owned(), n);
        let toggle = toggle.clone();
        let id = SharedString::from(format!("letter-{message}-{n}"));
        let selector = id.to_string();
        let preview = letter.folded_line();
        let open_it = letter.session.clone().map(|session| {
            let open_session = around.open_session.clone();
            let selector = format!("open-{message}-{n}");
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .flex_none()
                .px(space::S1_5)
                .rounded(radius::NAV)
                .text_ui(TypeSize::Caption)
                .text_color(c.accent)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    open_session(&session, window, cx)
                })
                .child("Open")
        });
        // One line: "From" and its name (cut short where it's long) for a message; a session's name
        // and what happened ("Sign-in finished"); "Woke up".
        let from = div()
            .flex()
            .items_center()
            .gap(space::S1)
            .min_w_0()
            .text_color(c.text_muted)
            .when(letter.kind == Kind::Mail, |d| d.child(div().flex_none().child("From")))
            .child(div().min_w_0().truncate().text_color(c.text).font_weight(FontWeight::SEMIBOLD).child(letter.from.clone()))
            .children(letter.status.clone().map(|status| div().flex_none().child(status)));
        let (mark, tint) = letter.mark(c);
        let head = div()
            .flex()
            .items_center()
            .gap(space::S1_5)
            .min_w_0()
            .text_ui(TypeSize::Small)
            .child(div().flex().flex_none().w(px(16.)).justify_center().child(Icon::new(mark).size(IconSize::Nav).color(tint)))
            .child(div().flex().flex_1().min_w_0().child(from))
            .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(time::when(at, time::now_ms())))
            .children(open_it)
            .when(letter.folds(), |d| d.child(Icon::new(if whole { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Nav).color(c.text_faint)));
        let said: AnyElement = if whole {
            div().ml(UNDER).text_ui(TypeSize::Small).child(Markdown::new(SharedString::from(format!("{id}-said")), &letter.said).selectable(sel(&[n as u32]))).into_any_element()
        } else {
            div().ml(UNDER).min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(preview).into_any_element()
        };
        div()
            .id(id)
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .gap(space::S1)
            .px(space::S3)
            .py(space::S2)
            .rounded(radius::CARD)
            .border_1()
            .border_color(c.border)
            .bg(c.card)
            .when(letter.folds(), |d| d.cursor_pointer().on_click(move |_, _, cx| toggle(fold.clone(), cx)))
            .child(head)
            .when(!letter.said.trim().is_empty(), |d| d.child(said))
    });
    div().flex().flex_col().gap(GAP).children(cards).into_any_element()
}

/// "+12 −3", beside a step that changed files.
fn counts(added: usize, removed: usize, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_none()
        .gap(space::S1)
        .text_ui(TypeSize::Caption)
        .child(div().text_color(c.diff_added).child(format!("+{added}")))
        .child(div().text_color(c.diff_removed).child(format!("−{removed}")))
        .into_any_element()
}

/// What a tool step shows when it's opened.
#[derive(Clone, Debug, PartialEq)]
pub enum Details {
    /// What it printed or found: a command's output (with the command), a search's results.
    Output { command: Option<String>, text: String },
    /// The files it changed, each as a diff.
    Changes(Vec<Changed>),
}

/// One file a step changed, as the engine says it (`metadata.files`).
#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub file: String,
    /// Added or deleted (a modified file says nothing).
    pub status: Option<String>,
    pub added: usize,
    pub removed: usize,
    /// Its unified patch, read into lines only once it's shown.
    pub patch: Rc<str>,
}

impl Details {
    /// Lines added and removed, for a step that changed files.
    pub fn counts(&self) -> Option<(usize, usize)> {
        match self {
            Details::Changes(files) => Some(files.iter().fold((0, 0), |(a, r), file| (a + file.added, r + file.removed))),
            Details::Output { .. } => None,
        }
    }
}

/// What a step printed, as the engine keeps it: its text blocks (`state.content`), one after another.
fn printed(state: &Value) -> String {
    let blocks = state.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
    let texts: Vec<&str> = blocks.iter().filter(|block| block.get("type").and_then(Value::as_str) == Some("text")).filter_map(|block| block.get("text").and_then(Value::as_str)).collect();
    texts.join("\n")
}

/// Why a step failed, as the engine says it.
fn tool_error(state: &Value) -> Option<String> {
    let error = state.get("error")?;
    error.as_str().map(str::to_owned).or_else(|| error.get("message").and_then(Value::as_str).map(str::to_owned))
}

/// What a tool step shows when it's opened (none: it has nothing to show, and doesn't open): the
/// files it changed, as diffs; a command's output; a search's results. As the Mac's step rows show
/// them; a file read, or a step that's still running, shows nothing more.
pub fn details(name: &str, state: &Value) -> Option<Details> {
    let status = state.get("status").and_then(Value::as_str).unwrap_or("");
    if matches!(status, "running" | "streaming" | "pending") {
        return None;
    }
    let files = state.pointer("/metadata/files").and_then(Value::as_array).cloned().unwrap_or_default();
    let changed: Vec<Changed> = files
        .iter()
        .filter_map(|file| {
            let patch = file.get("patch").and_then(Value::as_str)?;
            Some(Changed {
                file: file.get("file").and_then(Value::as_str).unwrap_or("").to_owned(),
                status: file.get("status").and_then(Value::as_str).filter(|status| *status != "modified").map(str::to_owned),
                added: file.get("additions").and_then(Value::as_u64).unwrap_or(0) as usize,
                removed: file.get("deletions").and_then(Value::as_u64).unwrap_or(0) as usize,
                patch: patch.into(),
            })
        })
        .filter(|file| file.patch.lines().any(|line| line.starts_with("@@")))
        .collect();
    if !changed.is_empty() {
        return Some(Details::Changes(changed));
    }
    let text = printed(state);
    let command = state.pointer("/input/command").and_then(Value::as_str).map(|command| command.trim().to_owned());
    match name {
        // A command shows what it printed, or that it printed nothing.
        "shell" if status == "completed" || !text.trim().is_empty() => Some(Details::Output { command, text }),
        "grep" | "glob" | "list" | "ls" | "websearch" if !text.trim().is_empty() => Some(Details::Output { command: None, text }),
        _ => None,
    }
}

/// An opened step's output or changes, each folded until it's shown whole.
#[allow(clippy::too_many_arguments)]
fn details_view(key: &str, step: &Step, n: usize, at: u32, shown: Details, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, sel: &dyn Fn(&[u32]) -> Selectable) -> AnyElement {
    let whole = |part: usize| Fold::Whole(step.id.clone(), n, part);
    match shown {
        Details::Output { command, text } => {
            let fold = whole(0);
            let mut well = OutputWell::new(SharedString::from(format!("output-{key}")), text).whole(open.contains(&fold)).on_toggle(move |_, cx| toggle(fold.clone(), cx)).selectable(sel(&[at]));
            if let Some(command) = command {
                well = well.command(command);
            }
            well.into_any_element()
        }
        Details::Changes(files) => {
            // The step names one file already; several get a title each, and so does one that was
            // added or deleted, which only its title says.
            let titled = files.len() > 1 || files.iter().any(|file| file.status.is_some());
            let views = files.into_iter().enumerate().map(|(part, file)| {
                let fold = whole(part);
                let toggle = toggle.clone();
                let mut view = DiffView::new(SharedString::from(format!("diff-{key}-{part}")), Rc::new(parse_patch(&file.patch)))
                    .whole(open.contains(&fold))
                    .on_toggle(move |_, cx| toggle(fold.clone(), cx))
                    .selectable(sel(&[at, part as u32]));
                if titled {
                    let title = match file.status.as_deref() {
                        Some("added") => format!("{} (new)", short_path(&file.file)),
                        Some("deleted") => format!("{} (deleted)", short_path(&file.file)),
                        _ => short_path(&file.file),
                    };
                    view = view.title(title).counts(file.added, file.removed);
                }
                view.into_any_element()
            });
            div().flex().flex_col().gap(space::S2).children(views).into_any_element()
        }
    }
}

/// A tool step or thinking, as one quiet line: its icon (or a spinner while it runs), what it did,
/// and what it did it to.
fn step_row(id: SharedString, icon: IconName, label: SharedString, target: Option<SharedString>, busy: bool, failed: bool, cx: &App) -> gpui::Stateful<gpui::Div> {
    let c = &cx.theme().colors;
    let leading: AnyElement = if busy {
        spinner(SharedString::from(format!("{id}-spin")), IconSize::Nav, c.text_muted).into_any_element()
    } else if failed {
        Icon::new(IconName::Warning).size(IconSize::Nav).color(c.danger).into_any_element()
    } else {
        Icon::new(icon).size(IconSize::Nav).color(c.text_faint).into_any_element()
    };
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector)
        .flex()
        .items_center()
        .gap(space::S1_5)
        .min_w_0()
        .text_ui(TypeSize::Small)
        .text_color(c.text_muted)
        .child(div().flex().flex_none().w(px(16.)).justify_center().child(leading))
        .child(div().flex_none().child(label))
        .children(target.map(|target| div().min_w_0().truncate().text_mono(TypeSize::Small).text_color(c.text_faint).child(target)))
}

/// What a tool step did, in words: its icon, what it did (or does, while it runs), and to what.
pub fn tool_label(name: &str, state: &Value, busy: bool) -> (IconName, SharedString, Option<SharedString>) {
    let input = state.get("input").cloned().unwrap_or(Value::Null);
    let text = |field: &str| input.get(field).and_then(Value::as_str).map(str::to_owned);
    let file = || text("filePath").or_else(|| text("path")).map(|path| short_path(&path));
    let (icon, done, doing, target) = match name {
        "shell" => (IconName::Terminal, "Ran", "Running", text("command").map(|command| first_line(&command))),
        "read" => (IconName::File, "Read", "Reading", file()),
        "edit" | "multiedit" | "patch" | "apply_patch" => (IconName::Compose, "Edited", "Editing", file()),
        "write" => (IconName::Compose, "Wrote", "Writing", file()),
        "grep" => (IconName::Search, "Searched for", "Searching for", text("pattern")),
        "glob" | "list" | "ls" => (IconName::Folder, "Listed", "Listing", text("pattern").or_else(|| text("path"))),
        "webfetch" => (IconName::Globe, "Read", "Reading", text("url")),
        "websearch" => (IconName::Globe, "Searched the web for", "Searching the web for", text("query")),
        "task" | "subagent" => (IconName::Bot, "Asked a subagent", "Asking a subagent", text("description")),
        "execute" => (IconName::Terminal, "Ran code", "Running code", None),
        "skill" => (IconName::Sparkle, "Used the skill", "Using the skill", text("name")),
        "question" => (IconName::Question, "Asked you", "Asking you", None),
        "codesearch" => (IconName::Search, "Searched code for", "Searching code for", text("query")),
        "lsp" => (IconName::Search, "Looked up code", "Looking up code", file()),
        "batch" => (IconName::Sliders, "Ran steps together", "Running steps together", None),
        "plan_enter" => (IconName::Check, "Started planning", "Starting to plan", None),
        "plan_exit" => (IconName::Check, "Finished planning", "Finishing planning", None),
        "context" => (IconName::File, "Read its context", "Reading its context", None),
        "invalid" => (IconName::Warning, "Tried a tool it doesn’t have", "Trying a tool it doesn’t have", None),
        "todowrite" | "todoread" => (IconName::Check, "Updated its plan", "Updating its plan", None),
        // (Named by its box, as `call_label` says; never by its id, which reads as code.)
        _ => (IconName::Sliders, "Used a tool", "Using a tool", None),
    };
    (icon, if busy { doing } else { done }.into(), target.map(SharedString::from))
}

/// What a tool step did, in words, with what its box says of it: the engine's own tools by what
/// they did and to what ([`tool_label`]); any other (Kuvo Code's own, an MCP server's) by the
/// plain title its box gives it (`title`, and `doing` while it runs: PROTOCOL.md, "Tool steps").
pub fn call_label(name: &str, title: Option<&str>, doing: Option<&str>, state: &Value, busy: bool) -> (IconName, SharedString, Option<SharedString>) {
    let said = if busy { doing.or(title) } else { title };
    match said {
        Some(said) => (IconName::Sliders, SharedString::from(said.to_owned()), None),
        None => tool_label(name, state, busy),
    }
}

/// A path as a row shows it: its last two parts.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.trim_end_matches('/').rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if text.trim().lines().count() > 1 { format!("{line} …") } else { line.to_owned() }
}

fn error_words(error: &Value) -> String {
    error
        .pointer("/data/message")
        .or_else(|| error.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| "The model stopped with an error.".into())
}

fn callout(words: String, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .items_start()
        .gap(space::S2)
        .px(space::S3)
        .py(space::S2)
        .rounded(radius::NAV)
        .bg(c.danger.opacity(0.08))
        .border_1()
        .border_color(c.danger.opacity(0.25))
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .child(div().pt(px(2.)).child(Icon::new(IconName::Warning).size(IconSize::Nav).color(c.danger)))
        .child(div().flex_1().child(words))
        .into_any_element()
}

fn shell_run(run: &kuvo_box::messages::ShellRun, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, sel: &dyn Fn(&[u32]) -> Selectable, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let busy = run.status == "running";
    let failed = run.status == "failed" || run.exit.is_some_and(|exit| exit != 0);
    let exit = run.exit.filter(|exit| *exit != 0).map(|exit| format!("exit {exit}"));
    let fold = Fold::Whole(run.id.clone(), 0, 0);
    let output = run.output.as_ref().map(|output| OutputWell::new(SharedString::from(format!("shell-output-{}", run.id)), output.output.clone()).command(run.command.clone()).whole(open.contains(&fold)).on_toggle(move |_, cx| toggle(fold.clone(), cx)).selectable(sel(&[0])));
    div().flex().flex_col().gap(space::S2).child(step_row(SharedString::from(format!("shell-{}", run.id)), IconName::Terminal, if busy { "Running".into() } else { "You ran".into() }, Some(first_line(&run.command).into()), busy, failed, cx)
        .children(exit.map(|exit| div().text_ui(TypeSize::Caption).text_color(c.danger).child(exit))))
        .children(output)
        .when(run.output.as_ref().is_some_and(|output| output.truncated), |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Output cut short")))
        .into_any_element()
}

fn turn_end(outcome: &str, worked: Option<f64>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let (words, color) = match outcome {
        "failed" => ("The turn didn’t finish".to_owned(), c.danger),
        "interrupted" => ("Stopped".to_owned(), c.text_faint),
        _ => (worked.map(time::worked_for).unwrap_or_default(), c.text_faint),
    };
    div().flex().justify_center().text_ui(TypeSize::Caption).text_color(color).child(words).into_any_element()
}

fn quiet_line(words: String, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div().flex().justify_center().text_ui(TypeSize::Caption).text_color(c.text_faint).child(words).into_any_element()
}

/// The words supplied for a running compaction, or the ordinary working row.
fn working(title: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .items_center()
        .gap(space::S1_5)
        .text_ui(TypeSize::Small)
        .text_color(c.text_faint)
        .child(div().flex().w(px(16.)).justify_center().child(spinner("working", IconSize::Nav, c.text_muted)))
        .child(div().id("working-label").child(title.to_owned()).pulsing())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuvo_box::Where;
    use serde_json::json;

    fn conversation(messages: Value, running: bool, earlier: Option<&str>) -> Conversation {
        let mut conversation = Conversation::new("ses_1", Where::Engine);
        let mut newest_first: Vec<Message> = serde_json::from_value(messages).unwrap();
        newest_first.reverse();
        conversation.catch_up(newest_first, earlier.map(str::to_owned), &[], Some(running));
        conversation
    }

    #[test]
    fn compactions_stay_in_the_conversation_for_every_status() {
        for status in ["running", "completed", "failed"] {
            let (rows, _) = entries(conversation(json!([
                { "type": "compaction", "id": "summary", "time": { "created": 1000 }, "status": status, "reason": "manual" }
            ]), false, None));
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].key(), "summary");
        }
    }

    #[test]
    fn active_compaction_since_last_idle_marks_working_as_summarizing() {
        let (rows, _) = entries(conversation(json!([
            { "type": "user", "id": "u1", "time": { "created": 100 }, "text": "Hi" },
            { "type": "idle", "id": "i1", "time": { "created": 200 }, "outcome": "succeeded" },
            { "type": "compaction", "id": "c1", "time": { "created": 300 }, "status": "running", "workingTitle": "Summarizing the conversation…", "reason": "manual" }
        ]), true, None));
        assert_eq!(rows.last(), Some(&Entry::Working { title: "Summarizing the conversation…".into() }));


        // Once compaction finishes or idle arrives, Working returns to normal.
        let (rows, _) = entries(conversation(json!([
            { "type": "compaction", "id": "c1", "time": { "created": 300 }, "status": "completed", "reason": "manual" }
        ]), true, None));
        assert_eq!(rows.last(), Some(&Entry::Working { title: "Working…".into() }));

        let (rows, _) = entries(conversation(json!([
            { "type": "compaction", "id": "c1", "time": { "created": 300 }, "status": "running", "workingTitle": "Summarizing the conversation…", "reason": "auto" },
            { "type": "idle", "id": "i1", "time": { "created": 400 }, "outcome": "interrupted" },
            { "type": "user", "id": "u2", "time": { "created": 500 }, "text": "Next" }
        ]), true, None));
        assert_eq!(rows.last(), Some(&Entry::Working { title: "Working…".into() }));

    }

    #[test]
    fn a_turn_shows_your_message_the_agents_step_and_how_long_it_worked() {
        let (rows, _) = entries(conversation(
            json!([
                { "type": "user", "id": "m1", "time": { "created": 1000 }, "text": "Fix it" },
                { "type": "user", "id": "m0", "time": { "created": 1500 }, "text": "for the agent", "metadata": { "kuvo": { "note": true } } },
                { "type": "user", "id": "m1b", "time": { "created": 1800 }, "text": "and the tests" },
                { "type": "assistant", "id": "m2", "time": { "created": 2000 }, "content": [] },
                { "type": "assistant", "id": "m3", "time": { "created": 3000 }, "content": [{ "type": "text", "text": "Done." }] },
                { "type": "agent-switched", "id": "m4", "time": { "created": 3500 } },
                { "type": "idle", "id": "m5", "time": { "created": 26000 }, "outcome": "succeeded" }
            ]),
            false,
            None,
        ));
        let keys: Vec<&str> = rows.iter().map(Entry::key).collect();
        // The box's note to the agent, an empty step and a switch of mode show nothing.
        assert_eq!(keys, ["m1", "m1b", "m3", "m5"]);
        // From the turn's first message.
        assert!(matches!(&rows[3], Entry::TurnEnd { worked: Some(worked), .. } if *worked == 25000.0));
    }

    #[test]
    fn a_run_under_way_ends_with_working_and_earlier_ones_start_with_the_way_back() {
        let (rows, _) = entries(conversation(json!([{ "type": "user", "id": "m1", "time": { "created": 1 }, "text": "Go" }]), true, Some("100")));
        let keys: Vec<&str> = rows.iter().map(Entry::key).collect();
        assert_eq!(keys, ["earlier", "m1", "working"]);
    }

    #[test]
    fn tool_steps_say_what_they_did_and_to_what() {
        let (_, label, target) = tool_label("read", &json!({ "status": "completed", "input": { "filePath": "/Users/sam/web/src/signup/Form.tsx" } }), false);
        assert_eq!((label.as_ref(), target.as_deref()), ("Read", Some("signup/Form.tsx")));
        let (_, label, target) = tool_label("shell", &json!({ "status": "running", "input": { "command": "npm test\nnpm run lint" } }), true);
        assert_eq!((label.as_ref(), target.as_deref()), ("Running", Some("npm test …")));
    }

    /// Every client's sample steps (`protocol/vectors/tool-steps.json`, PROTOCOL.md's "Tool
    /// steps"): each one's target, and what it opens to.
    #[test]
    fn every_sample_step_shows_its_target_and_opens_to_what_the_vectors_say() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/vectors/tool-steps.json");
        let vectors: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for case in vectors["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let Content::Tool { name: tool, state, .. } = serde_json::from_value(case["step"].clone()).unwrap() else { panic!("{name}: not a tool step") };
            let busy = matches!(state["status"].as_str(), Some("running" | "streaming" | "pending"));
            let (_, _, target) = tool_label(&tool, &state, busy);
            assert_eq!(target.as_deref(), case["target"].as_str(), "{name}: its target");
            let opened = details(&tool, &state);
            match &case["opens"] {
                Value::String(nothing) if nothing == "nothing" => assert_eq!(opened, None, "{name}"),
                opens if opens.get("output").is_some() => {
                    let output = &opens["output"];
                    assert_eq!(opened, Some(Details::Output { command: output["command"].as_str().map(str::to_owned), text: output["text"].as_str().unwrap().to_owned() }), "{name}");
                }
                opens => {
                    let Some(Details::Changes(files)) = opened else { panic!("{name}: no changes") };
                    let shown: Vec<Value> = files.iter().map(|file| json!({ "file": file.file, "added": file.added, "removed": file.removed })).collect();
                    assert_eq!(Value::Array(shown), opens["changes"], "{name}");
                }
            }
        }
    }

    fn tool_part(id: &str, name: &str, status: &str, input: Value) -> Value {
        json!({ "type": "tool", "id": id, "name": name, "state": { "status": status, "input": input } })
    }

    fn step(id: &str, at: f64, content: Value) -> Value {
        json!({ "type": "assistant", "id": id, "time": { "created": at }, "content": content })
    }

    fn keys(rows: &[Entry]) -> Vec<&str> {
        rows.iter().map(Entry::key).collect()
    }

    /// The tool steps (and thinking) between the agent's words are one row, however many steps they
    /// came in; its words, an error and your messages end it.
    #[test]
    fn what_the_agent_did_between_its_words_is_one_group_across_its_steps() {
        let read = |id: &str, file: &str| tool_part(id, "read", "completed", json!({ "filePath": file }));
        let (rows, _) = entries(conversation(
            json!([
                { "type": "user", "id": "u1", "time": { "created": 1 }, "text": "Go" },
                step("a", 2.0, json!([{ "type": "text", "text": "Looking." }, read("c1", "/p/a.ts")])),
                step("b", 3.0, json!([{ "type": "reasoning", "text": "hm", "time": { "completed": 4 } }, read("c2", "/p/b.ts"), tool_part("c3", "shell", "completed", json!({ "command": "ls" }))])),
                step("c", 5.0, json!([{ "type": "text", "text": "Done." }, read("c4", "/p/c.ts")])),
                { "type": "assistant", "id": "d", "time": { "created": 6 }, "content": [read("c5", "/p/d.ts")], "error": { "message": "Overloaded" } },
                step("e", 7.0, json!([read("c6", "/p/e.ts")])),
            ]),
            false,
            None,
        ));
        assert_eq!(keys(&rows), ["u1", "a", "a#1", "c", "c#1", "d#1", "e"]);
        let Entry::Steps { items, .. } = &rows[2] else { panic!("a group") };
        let held: Vec<(&str, usize)> = items.iter().map(|(step, n)| (step.id.as_str(), *n)).collect();
        assert_eq!(held, [("a", 1), ("b", 0), ("b", 1), ("b", 2)], "across steps, thinking too");
        assert!(matches!(&rows[5], Entry::Words { error: true, parts, .. } if parts.is_empty()), "an error shows on its own");
    }

    /// As a run goes on, its group stays the same row (so the list keeps its place) and is drawn
    /// again as each step comes.
    #[test]
    fn a_group_stays_one_row_as_its_steps_come_and_changes_with_each() {
        let first = json!([
            { "type": "user", "id": "u1", "time": { "created": 1 }, "text": "Go" },
            step("a", 2.0, json!([tool_part("c1", "read", "completed", json!({ "filePath": "/p/a.ts" }))])),
        ]);
        let mut more = first.clone();
        more.as_array_mut().unwrap().push(step("b", 3.0, json!([tool_part("c2", "shell", "running", json!({ "command": "npm test" }))])));
        let mut conversation = conversation(first, true, None);
        let (rows, revisions) = entries(conversation.clone());
        let mut newest_first: Vec<Message> = serde_json::from_value(more).unwrap();
        newest_first.reverse();
        conversation.catch_up(newest_first, None, &[], Some(true));
        let (later, later_revisions) = entries(conversation);
        assert_eq!(keys(&rows), ["u1", "a", "working"]);
        assert_eq!(keys(&later), ["u1", "a", "working"]);
        assert!(later_revisions[1] > revisions[1], "drawn again");
        assert!(matches!(&later[1], Entry::Steps { items, .. } if items.len() == 2));
    }

    fn calls(calls: &[(&str, Option<&str>, Value)]) -> String {
        let calls: Vec<Call> = calls.iter().map(|(name, title, state)| Call { name, title: *title, doing: None, state }).collect();
        summary(&calls)
    }

    #[test]
    fn a_group_sums_up_what_it_did_counting_files_once() {
        let read = |file: &str| ("read", None, json!({ "status": "completed", "input": { "filePath": file } }));
        let ran = |command: &str| ("shell", None, json!({ "status": "completed", "input": { "command": command } }));
        let grep = ("grep", None, json!({ "status": "completed", "input": { "pattern": "x" } }));
        assert_eq!(calls(&[read("/p/a.ts"), read("/p/b.ts"), read("/p/a.ts"), ran("ls"), ran("pwd"), grep.clone()]), "Read 2 files, ran 2 commands, searched once");
        assert_eq!(calls(&[grep.clone(), grep.clone(), read("/p/a.ts")]), "Searched twice, read 1 file");
        // Kuvo Code's own and an MCP server's, by their box's titles: never by their ids.
        let titled = |title: &'static str| ("kuvo-browser_schedule_wake_up", Some(title), json!({ "status": "completed" }));
        assert_eq!(calls(&[read("/p/a.ts"), titled("Scheduled a wake-up"), titled("Scheduled a wake-up")]), "Read 1 file, scheduled a wake-up ×2");
        assert_eq!(calls(&[("linear_list_issues", Some("Linear · List issues"), json!({})), read("/p/a.ts")]), "Linear · List issues, read 1 file");
        assert_eq!(calls(&[("kuvo-browser_plan_limits", None, json!({})), ("other_thing", None, json!({}))]), "Used 2 tools");
        // Past four kinds, how many more.
        let web = ("webfetch", None, json!({ "status": "completed", "input": { "url": "https://a" } }));
        let task = ("task", None, json!({ "status": "completed", "input": {} }));
        let plan = ("todowrite", None, json!({ "status": "completed", "input": {} }));
        assert_eq!(calls(&[read("/p/a.ts"), ran("ls"), grep, web, task.clone(), task, plan]), "Read 1 file, ran 1 command, searched once, read 1 web page, and 3 more steps");
    }

    #[test]
    fn a_tool_that_isnt_the_engines_own_shows_its_boxs_title_and_never_its_id() {
        let state = json!({ "status": "running", "input": { "session": "ses_1" } });
        let (_, label, target) = call_label("kuvo-browser_start_session", Some("Started a session"), Some("Starting a session"), &state, true);
        assert_eq!((label.as_ref(), target), ("Starting a session", None));
        let (_, label, _) = call_label("kuvo-browser_start_session", Some("Started a session"), Some("Starting a session"), &state, false);
        assert_eq!(label.as_ref(), "Started a session");
        // From a box before titles: no id either.
        let (_, label, target) = call_label("kuvo-browser_start_session", None, None, &state, false);
        assert_eq!((label.as_ref(), target), ("Used a tool", None));
        // The engine's own keep their verbs.
        let (_, label, _) = call_label("read", None, None, &json!({ "input": { "filePath": "/a/b.ts" } }), false);
        assert_eq!(label.as_ref(), "Read");
        let (_, label, _) = call_label("subagent", None, None, &json!({ "input": { "description": "Check the forms" } }), false);
        assert_eq!(label.as_ref(), "Asked a subagent");
    }

    /// Folded, a group still shows a step that failed, and the one under way while it waits on you.
    #[test]
    fn what_needs_you_shows_outside_a_folded_group() {
        let (rows, _) = entries(conversation(
            json!([step("a", 1.0, json!([
                tool_part("c1", "read", "completed", json!({ "filePath": "/p/a.ts" })),
                { "type": "tool", "id": "c2", "name": "shell", "state": { "status": "failed", "input": { "command": "tsc" }, "error": "1 error" } },
                tool_part("c3", "shell", "running", json!({ "command": "rm -rf build" })),
            ]))]),
            true,
            None,
        ));
        let Entry::Steps { key, items } = &rows[0] else { panic!("a group") };
        let open = HashSet::new();
        let shown = |waiting: bool| (0..items.len()).filter(|at| shown_in_group(key, items, *at, &open, waiting)).collect::<Vec<_>>();
        assert_eq!(shown(false), [1], "the failed one");
        assert_eq!(shown(true), [1, 2], "and the one waiting on you");
        let opened = HashSet::from([Fold::Group(key.clone())]);
        assert_eq!((0..items.len()).filter(|at| shown_in_group(key, items, *at, &opened, false)).count(), 3);
    }

    /// Another agent's message, and a lead's sessions' news as its box reads it, show as messages
    /// from them; the box's other notes don't.
    #[test]
    fn messages_from_other_agents_and_their_news_are_letters_from_them() {
        let (rows, _) = entries(conversation(
            json!([
                { "type": "user", "id": "m1", "time": { "created": 1 }, "text": "[From &Mira] Done", "metadata": { "kuvo": { "parts": { "words": "Done.\n\nAll green.", "mail": { "name": "Mira", "session": "ses_m" } } } } },
                { "type": "user", "id": "m2", "time": { "created": 2 }, "text": "[Session asks] …", "metadata": { "kuvo": { "note": { "from": "Its sessions", "line": "x" }, "news": [{ "session": "ses_a", "title": "Sign-in", "status": "asks", "said": "May I run npm i?" }] } } },
                { "type": "user", "id": "m3", "time": { "created": 3 }, "text": "[Goal started]", "metadata": { "kuvo": { "note": { "from": "Goal", "line": "Goal started" } } } },
                // Only one still working: nothing to show.
                { "type": "user", "id": "m4", "time": { "created": 4 }, "text": "[Session still working] …", "metadata": { "kuvo": { "note": { "from": "Its sessions", "line": "x" }, "news": [] } } },
                // Worded in a way its box couldn't read: whole, rather than lost.
                { "type": "user", "id": "m5", "time": { "created": 5 }, "text": "[Session dreamed] \"Sign-in\" saw sheep.", "metadata": { "kuvo": { "note": { "from": "Its sessions", "line": "x" } } } },
            ]),
            false,
            None,
        ));
        assert_eq!(keys(&rows), ["m1", "m2", "m5"]);
        let Entry::Message(message) = &rows[2] else { panic!() };
        let Message::User(user) = &**message else { panic!() };
        assert_eq!(letters(user)[0].said, "[Session dreamed] \"Sign-in\" saw sheep.");
        let Entry::Message(message) = &rows[0] else { panic!() };
        let Message::User(user) = &**message else { panic!() };
        let mail = letters(user);
        assert_eq!(mail, [Letter { kind: Kind::Mail, from: "Mira".into(), status: None, said: "Done.\n\nAll green.".into(), preview: None, session: Some("ses_m".into()) }]);
        assert!(mail[0].folds(), "more than its first line");
        let Entry::Message(message) = &rows[1] else { panic!() };
        let Message::User(user) = &**message else { panic!() };
        assert_eq!(letters(user), [Letter { kind: Kind::News, from: "Sign-in".into(), status: Some("asks".into()), said: "May I run npm i?".into(), preview: None, session: Some("ses_a".into()) }]);
        // Its words are selectable once it's shown whole.
        assert!(units(&rows[0], &HashSet::new()).is_empty());
        assert!(!units(&rows[0], &HashSet::from([Fold::Card("m1".into(), 0)])).is_empty());
    }

    /// A wake-up its box marked shows as a card of its own (#768): its summary folded, its note
    /// whole once opened, never as your words. One an older box didn't mark is drawn as it was.
    #[test]
    fn a_wake_up_is_a_card_with_its_summary_opening_to_its_note() {
        let note = "Pacing check: read plan_limits.\n\nReply only with the readings.";
        let (rows, _) = entries(conversation(
            json!([
                { "type": "user", "id": "m1", "time": { "created": 1 }, "text": format!("[Wake-up] {note}"), "metadata": { "kuvo": { "wake": { "summary": "Pacing check", "note": note } } } },
                { "type": "user", "id": "m2", "time": { "created": 2 }, "text": "[Wake-up] Unmarked." },
            ]),
            false,
            None,
        ));
        assert_eq!(keys(&rows), ["m1", "m2"]);
        let Entry::Message(message) = &rows[0] else { panic!() };
        let Message::User(user) = &**message else { panic!() };
        assert!(user.is_letter());
        let wake = letters(user);
        assert_eq!(wake, [Letter { kind: Kind::Wake, from: "Woke up".into(), status: None, said: note.into(), preview: Some("Pacing check".into()), session: None }]);
        assert_eq!(wake[0].folded_line(), "Pacing check");
        assert!(wake[0].folds(), "it opens to the whole note");
        assert!(units(&rows[0], &HashSet::new()).is_empty(), "folded, only its summary shows");
        assert!(!units(&rows[0], &HashSet::from([Fold::Card("m1".into(), 0)])).is_empty());
        let Entry::Message(message) = &rows[1] else { panic!() };
        let Message::User(user) = &**message else { panic!() };
        assert!(!user.is_letter(), "nothing marks it: the app reads no text to tell");
    }

    #[test]
    fn a_step_opens_to_its_changes_its_output_or_its_results_and_a_read_to_nothing() {
        let patch = "--- a\n+++ b\n@@ -1,2 +1,2 @@\n-old\n+new\n same\n";
        let edit = json!({ "status": "completed", "metadata": { "files": [{ "file": "/p/a.ts", "patch": patch, "additions": 1, "deletions": 1, "status": "modified" }] } });
        let shown = details("edit", &edit).expect("its changes");
        assert_eq!(shown.counts(), Some((1, 1)));
        let shell = json!({ "status": "completed", "input": { "command": "npm test" }, "content": [{ "type": "text", "text": "ok" }, { "type": "text", "text": "done" }] });
        assert_eq!(details("shell", &shell), Some(Details::Output { command: Some("npm test".into()), text: "ok\ndone".into() }));
        // A command that printed nothing still says so; a search that found nothing doesn't open.
        assert!(details("shell", &json!({ "status": "completed", "input": { "command": "true" } })).is_some());
        assert!(details("grep", &json!({ "status": "completed", "content": [] })).is_none());
        assert!(details("read", &json!({ "status": "completed", "content": [{ "type": "text", "text": "file" }] })).is_none());
        assert!(details("shell", &json!({ "status": "running", "input": { "command": "npm test" } })).is_none());
    }
}
