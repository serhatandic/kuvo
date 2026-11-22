//! A conversation's rows: which of the box's messages show, and how each is drawn. Your messages
//! by their parts (what you wrote, the keyword it went with, who it's from, files it names); one
//! another agent wrote, and its sessions' news, as a card from that agent; the agent's words
//! (Markdown); what it did between its words (tool steps and thinking, across its steps) as one
//! group, folded to a line that sums it up; a turn that didn't finish, said at its end. What's only
//! for the agent (the box's other notes, a switch of mode) shows nothing.

use kuvo_ui::motion::{MotionExt as _};
use std::borrow::Cow;
use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, App, InteractiveElement, Window, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div,
    prelude::FluentBuilder as _, px, relative,
};
use kuvo_box::messages::{Content, Message, News, Step};
use kuvo_box::{Conversation, Parts, pictures};
use kuvo_box::pictures::Picture;

use crate::pictures::Looks;
use crate::queue::chat_references::Items;
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, Notice, StyledType, TextRow, Tone, TypeSize,
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
    /// A message this agent sent to another agent (`send_message`), standing on its own in the
    /// conversation in order with the words and steps around it.
    Sent { key: String, step: Rc<Step>, part: usize },
    /// How a turn ended, at its end, when it didn't finish (stopped, or failed). A finished turn
    /// shows nothing there (the user, 11-17: no "Worked for" line).
    TurnEnd { id: String, outcome: String },
    /// A run under way, after what it has said so far.
    Working { title: String },
    /// Yours, sent from here and on its way (`Composer::outgoing`): drawn at once as your message,
    /// until the box's copy (by the same id) takes its place. `failed`: why it couldn't go; `waits`:
    /// its box took it while the agent works, and it's read at the agent's next step.
    /// `files`: the names of those that aren't pictures; `pictures`: those that are, read from here.
    Outgoing { id: String, words: String, files: Vec<String>, pictures: Vec<Picture>, failed: Option<String>, interruption: Option<String>, waits: bool, held: bool },
}

impl Entry {
    /// The text it names Work Queue items in, as the box is asked what it names: its words, and
    /// the item a Work Queue step's row is about ("#954").
    pub(crate) fn reference_text(&self) -> Vec<Cow<'_, str>> {
        match self {
            Entry::Message(message) => match &**message {
                Message::User(user) if user.is_letter() => letters(user).into_iter().map(|letter| Cow::Owned(letter.said)).collect(),
                message => crate::queue::chat_references::message_text(message).into_iter().map(Cow::Borrowed).collect(),
            },
            Entry::Words { step, parts, .. } => parts.iter().filter_map(|&at| match step.content.get(at) {
                Some(Content::Text(text)) => Some(Cow::Borrowed(text.as_str())),
                _ => None,
            }).collect(),
            Entry::Steps { items, .. } => items.iter().filter_map(|(step, n)| match step.content.get(*n) {
                Some(Content::Tool { name, state, .. }) => crate::tool_step::queue_item(name, state).map(|number| Cow::Owned(format!("#{number}"))),
                _ => None,
            }).collect(),
            Entry::Sent { step, part, .. } => sent_letters(step, *part).into_iter().map(|letter| Cow::Owned(letter.said)).collect(),
            Entry::Outgoing { words, .. } => vec![Cow::Borrowed(words.as_str())],
            _ => Vec::new(),
        }
    }

    /// What it's known by from one read to the next.
    pub fn key(&self) -> &str {
        match self {
            Entry::Earlier => "earlier",
            Entry::Working { .. } => "working",
            Entry::Message(message) => message.id(),
            Entry::TurnEnd { id, .. } | Entry::Outgoing { id, .. } => id,
            Entry::Words { key, .. } | Entry::Steps { key, .. } | Entry::Sent { key, .. } => key,
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
    /// `waits`: its box holds it for the agent's next step (its queue lists it so), so it can still
    /// be taken back.
    pub fn outgoing(outgoing: &crate::composer::Outgoing, waits: bool) -> (Entry, u64) {
        let files = outgoing.files.iter().filter(|path| !pictures::is_picture_path(&path.to_string_lossy())).map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()).collect();
        let shown = pictures::of_sending(&outgoing.id, &outgoing.files);
        let waits = waits && outgoing.failed.is_none();
        let interruption = outgoing.interruption().map(str::to_owned);
        let revision = if outgoing.failed.is_some() { u64::MAX - 1 } else if waits { u64::MAX - 2 } else if interruption.is_some() { u64::MAX - 3 } else { u64::MAX };
        (Entry::Outgoing { id: outgoing.id.clone(), words: outgoing.words.clone(), files, pictures: shown, failed: outgoing.failed.clone(), interruption, waits, held: outgoing.held && outgoing.failed.is_none() }, revision)
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
            Message::User(_) => Some(Entry::Message(Box::new(message))),
            Message::System(note) if !note.text.trim().is_empty() => Some(Entry::Message(Box::new(message))),
            Message::Shell(_) => Some(Entry::Message(Box::new(message))),
            Message::Other { kind, raw, .. } if kind == "compaction" => {
                summarizing = (raw["status"].as_str() == Some("running")).then(|| raw["workingTitle"].as_str().unwrap_or_default().to_owned());
                Some(Entry::Message(Box::new(message)))
            }
            Message::Idle { id, outcome, .. } => {
                summarizing = None;
                ends_unfinished(outcome).then(|| Entry::TurnEnd { id: id.clone(), outcome: outcome.clone() })
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
/// one. Messages sent to other agents (`send_message`) leave the step group and stand on their own
/// in order with the text and steps around them.
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
            Content::Tool { name, .. } if crate::tool_step::is_send_message(name) => {
                flush(&mut words, false, n, rows, revisions);
                *group = None;
                rows.push(Entry::Sent { key: key(n), step: step.clone(), part: n });
                revisions.push(revision);
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
/// conversations (one an agent's message is from), whether it waits on you now (a step under way
/// then is what it waits on, and shows outside its group), and the clock for formatting times.
#[derive(Clone)]
pub struct Around {
    pub open_session: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    pub waiting: bool,
    pub now: f64,
}

impl Around {
    /// Rows drawn on their own (a group chat's): nothing to open, and nothing waits.
    pub fn none() -> Around {
        Around { open_session: Rc::new(|_, _, _| {}), waiting: false, now: time::now_ms() }
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
    actions: Option<Actions>,
    cx: &App,
) -> AnyElement {
    let actions = actions.map(|actions| (actions_row(entry.key(), matches!(entry, Entry::Message(_)), &actions, cx), actions.shown));
    let body: AnyElement = match entry {
        Entry::Earlier => own,
        Entry::Outgoing { words, files, pictures, failed, interruption, waits, held, .. } => {
            let sel = Selectable::new(selection.clone(), SharedString::from(entry.key().to_owned()), &[]);
            let shown = (!pictures.is_empty()).then(|| yours_pictures(pictures, looks, cx));
            outgoing(ix, words, files, shown, failed.as_deref(), interruption.as_deref(), *waits, *held, sel, own, cx)
        }
        Entry::Message(message) => match &**message {
            Message::User(user) if user.is_letter() => {
                let sel = |base: &[u32]| Selectable::new(selection.clone(), SharedString::from(entry.key().to_owned()), base);
                from_agents(&user.id, &letters(user), user.time.created, open, toggle, around, &sel, looks, cx)
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
            steps(key, items, open, toggle, around.waiting, around.now, &sel, looks, cx)
        }
        Entry::Sent { key, step, part } => {
            let sel = |base: &[u32]| Selectable::new(selection.clone(), SharedString::from(key.clone()), base);
            from_agents(key, &sent_letters(step, *part), step.time.created, open, toggle, around, &sel, looks, cx)
        }
        Entry::TurnEnd { outcome, .. } => turn_end(outcome, cx),
        Entry::Working { title } => working(title, cx),
    };
    frame_with(ix, entry.key(), body, matches!(entry, Entry::Working { .. }), actions)
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
            let words = user_message(ix, &user.parts(), sel(&[]), looks, cx);
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
    frame_with(ix, key, body, last, None)
}

/// [`frame`], with `actions` (a message's time, Copy and Quote) under it, in the gap to the next
/// row: shown while the pointer is over the row (or, `true`, over another row of its reply), and
/// taking no room, so nothing moves.
pub fn frame_with(ix: usize, key: &str, body: AnyElement, last: bool, actions: Option<(AnyElement, bool)>) -> AnyElement {
    let last_gap = if last { space::S8 } else { space::S5 };
    let selector = format!("entry-{key}");
    let group = SharedString::from(format!("row-{key}"));
    div()
        .debug_selector(move || selector)
        .group(group.clone())
        .w_full()
        .flex()
        .justify_center()
        .px(space::S6)
        .pb(last_gap)
        .when(ix == 0, |d| d.pt(space::S6))
        .child(
            div()
                .relative()
                .w_full()
                .max_w(px(COLUMN))
                .min_w_0()
                .child(body)
                .children(actions.map(|(actions, shown)| {
                    let row = div().absolute().left_0().right_0().bottom(-ACTIONS).h(ACTIONS);
                    if shown { row.child(actions) } else { row.invisible().group_hover(group, |s| s.visible()).child(actions) }
                })),
        )
        .into_any_element()
}

/// The height of a message's actions row: the gap under a row, which it fills without growing.
const ACTIONS: gpui::Pixels = space::S5;

/// What a message's row can do under the pointer: its time, Copy and Quote.
#[derive(Clone)]
pub struct Actions {
    /// When the message was written (ms since 1970).
    pub time: f64,
    /// Shown now, the pointer being on another row of its reply.
    pub shown: bool,
    /// Now, for saying its day.
    pub now: f64,
    /// Copy was just used on it.
    pub copied: bool,
    /// Copies its text, as Markdown.
    pub copy: Rc<dyn Fn(&mut Window, &mut App)>,
    /// Quotes it into the composer, as Add to Chat does.
    pub quote: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// The replies among `rows`: for each of the agent's rows (its words, what it did, its messages to
/// other agents), the row its reply ends with (the last of its words before anything else shows: you
/// writing, a note, how the turn ended), which carries the reply's one time, Copy and Quote. None
/// for other rows, and for a reply still under way.
pub fn reply_ends(rows: &[Entry]) -> Vec<Option<usize>> {
    let mut ends = vec![None; rows.len()];
    let mut start: Option<usize> = None;
    let mut last_words: Option<usize> = None;
    let close = |ends: &mut Vec<Option<usize>>, start: Option<usize>, last_words: Option<usize>, upto: usize| {
        if let (Some(start), Some(end)) = (start, last_words) {
            ends[start..upto].iter_mut().for_each(|at| *at = Some(end));
        }
    };
    for (ix, row) in rows.iter().enumerate() {
        match row {
            Entry::Words { .. } | Entry::Steps { .. } | Entry::Sent { .. } => {
                start.get_or_insert(ix);
                if matches!(row, Entry::Words { .. }) {
                    last_words = Some(ix);
                }
            }
            // (Under way: it has no end yet.)
            Entry::Working { .. } => (start, last_words) = (None, None),
            _ => {
                close(&mut ends, start, last_words, ix);
                (start, last_words) = (None, None);
            }
        }
    }
    close(&mut ends, start, last_words, rows.len());
    ends
}

/// What Copy takes of `entry`, as Markdown, and when it was written: yours, and the agent's words.
pub fn copyable(entry: &Entry) -> Option<(String, f64)> {
    match entry {
        Entry::Message(message) => match &**message {
            Message::User(user) if !user.is_letter() => {
                let words = user.parts().words.trim().to_owned();
                (!words.is_empty()).then_some((words, user.time.created))
            }
            _ => None,
        },
        Entry::Words { step, parts, .. } => {
            let text = parts.iter().filter_map(|n| match &step.content[*n] {
                Content::Text(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
                _ => None,
            });
            let text = text.collect::<Vec<_>>().join("\n\n");
            (!text.is_empty()).then_some((text, step.time.created))
        }
        _ => None,
    }
}

/// A message's actions: its time (the day too, in its tooltip, when it isn't today), then Copy and
/// Quote, at the row's end under yours and at its start under the agent's.
fn actions_row(key: &str, yours: bool, actions: &Actions, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let clock = crate::time::clock(actions.time);
    let when = crate::time::when(actions.time, actions.now);
    let id = |what: &str| SharedString::from(format!("{what}-{key}"));
    let (copy, quote) = (actions.copy.clone(), actions.quote.clone());
    let time = div().id(id("message-time")).debug_selector({ let selector = format!("message-time-{key}"); move || selector }).px(space::S1).text_ui(TypeSize::Caption).text_color(c.text_faint).child(clock.clone()).when(when != clock, |d| {
        let when = SharedString::from(when.clone());
        d.tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(when.clone()).build(cx))
    });
    div()
        .debug_selector({ let selector = format!("message-actions-{key}"); move || selector })
        .size_full()
        .flex()
        .items_center()
        .gap(space::S0_5)
        .when(yours, |d| d.justify_end())
        .child(time)
        .child(
            Button::icon_only(id("message-copy"), if actions.copied { IconName::Check } else { IconName::Copy }, if actions.copied { "Copied" } else { "Copy" })
                .ghost()
                .size(ButtonSize::Small)
                .icon_size(IconSize::Nav)
                .on_click(move |_, window, cx| copy(window, cx)),
        )
        .child(Button::icon_only(id("message-quote"), IconName::Quote, "Add to Chat").ghost().size(ButtonSize::Small).icon_size(IconSize::Nav).on_click(move |_, window, cx| quote(window, cx)))
        .into_any_element()
}

/// The selectable text of a row, as [`render`] draws it, in reading order: what you wrote; the
/// agent's words (`[part, block…]`), its thinking (`[part, 0]`) and what an opened step printed
/// or changed (`[part, 0 and 1]`, `[part, file, line]`). For copying, without laying anything out.
///
/// `items`: the Work Queue items the conversation names, drawn (and so selected) as their chips'
/// labels.
pub fn units(entry: &Entry, open: &HashSet<Fold>, items: Option<&Items>) -> Vec<Unit> {
    match entry {
        Entry::Message(message) => match &**message {
            Message::User(user) if user.is_letter() => letters(user)
                .iter()
                .enumerate()
                .filter(|(n, letter)| letter.opens(open, &user.id, *n))
                .flat_map(|(n, letter)| markdown::units(&labeled(items, &letter.said, true), &[n as u32]))
                .collect(),
            message => message_units(message, open, items),
        },
        Entry::Words { step, parts, .. } => parts
            .iter()
            .filter_map(|n| match &step.content[*n] {
                Content::Text(text) => Some(markdown::units(&labeled(items, text, true), &[*n as u32])),
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
        Entry::Sent { key, step, part } => sent_letters(step, *part)
            .iter()
            .enumerate()
            .filter(|(n, letter)| letter.opens(open, key, *n))
            .flat_map(|(n, letter)| markdown::units(&labeled(items, &letter.said, true), &[n as u32]))
            .collect(),
        // (As the box's copy will be: what you wrote.)
        Entry::Outgoing { words, .. } => words_units(words),
        _ => Vec::new(),
    }
}

/// The selectable text of a message, as [`message_body`] draws it.
pub fn message_units(message: &Message, open: &HashSet<Fold>, items: Option<&Items>) -> Vec<Unit> {
    match message {
        Message::User(user) => {
            words_units(&labeled(items, &user.parts().words, false))
        }
        Message::Shell(run) => run.output.as_ref().map(|output| well_units(&output.output, Some(&run.command), open.contains(&Fold::Whole(run.id.clone(), 0, 0)), &[0])).unwrap_or_default(),
        Message::Assistant(step) => {
            let mut units = Vec::new();
            for (n, content) in step.content.iter().enumerate() {
                match content {
                    Content::Text(text) if !text.trim().is_empty() => units.extend(markdown::units(&labeled(items, text, true), &[n as u32])),
                    Content::Tool { name, .. } if crate::tool_step::is_send_message(name) => {
                        let card_key = format!("{}:{n}", step.id);
                        for (c_ix, letter) in sent_letters(step, n).iter().enumerate() {
                            if letter.opens(open, &card_key, c_ix) {
                                units.extend(markdown::units(&labeled(items, &letter.said, true), &[n as u32, c_ix as u32]));
                            }
                        }
                    }
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
            Some(Details::Note { primary, secondary }) => {
                units.push(Unit::new(&[at, 0], primary, Lead::Paragraph));
                if let Some(secondary) = secondary {
                    units.push(Unit::new(&[at, 1], secondary, Lead::Paragraph));
                }
            }
            None => {}
        },
        _ => {}
    }
    units
}

/// Yours on its way: your message as it will show, its files' names under it, and under them
/// nothing more while it's on its way, "At its next step", or why its box won't take it with `actions` (Edit, Remove) beside it.
#[allow(clippy::too_many_arguments)]
fn outgoing(ix: usize, words: &str, files: &[String], shown: Option<AnyElement>, failed: Option<&str>, interruption: Option<&str>, waits: bool, held: bool, sel: Selectable, actions: AnyElement, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let parts = Parts::words(words.to_owned());
    let names = files.iter().enumerate().map(|(n, name)| Chip::new(("sending-file", ix * 1000 + n), name.clone()).icon(IconName::File).into_any_element());
    let sending = failed.is_none() && interruption.is_none() && !waits;
    let status = match (failed, interruption) {
        (_, Some(why)) => Some(Notice::inline("interrupted", Tone::Warning, SharedString::from(why.to_owned())).into_any_element()),
        // Its box has it; the agent reads it at its next step (0.7's words), and it can be taken back.
        (None, None) if waits => Some(div()
            .debug_selector(|| "at-next-step".into())
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap(space::S2)
            .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("At its next step"))
            .child(actions)
            .into_any_element()),
        // Still here, its computer not answering: it goes by itself once it does (the composer's
        // one line says so); Edit and Remove meanwhile, nothing else.
        (None, None) if held => Some(div().debug_selector(|| "held-here".into()).flex().justify_end().child(actions).into_any_element()),
        // On its way: drawn as yours at once with nothing under it, as Claude Code and Codex draw
        // one (it's the box's to deliver; a failure says so here), so it doesn't move as its copy
        // takes its place.
        (None, None) => None,
        // Its box won't take it: the one notice under it, the box's why as its line, Edit at its end.
        (Some(why), None) => Some(Notice::inline("not-sent", Tone::Error, "Not sent").detail(SharedString::from(why.to_owned())).trailing(actions).into_any_element()),
    };
    div()
        .when(sending, |d| d.debug_selector(|| "sending".into()))
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(space::S1_5)
        .children(shown)
        // The bubble across the row's width, as a sent one's is: its width (and where its words
        // wrap) the same Sending as Sent, its state on its own line under it.
        .child(div().w_full().child(user_message(ix, &parts, sel, &Looks::none(), cx)))
        .when(!files.is_empty(), |d| d.child(div().flex().flex_wrap().justify_end().gap(space::S1).children(names)))
        .children(status)
        .into_any_element()
}

/// A part of a message's words as it shows them: what it quotes, if anything, and the words under
/// it with their unit's place among the message's selectable text (none when it has no words).
struct Piece {
    quote: Option<String>,
    text: String,
    unit: Option<u32>,
}

/// A message's words as it shows them, in the order written: each quote it replies to, with the
/// words that answer it under it (KuvoClient's `Quoting.segments`, as the phone draws them).
fn pieces(words: &str) -> Vec<Piece> {
    let mut units = 0u32;
    kuvo_box::quoting::segments(words)
        .into_iter()
        .filter(|(quote, text)| quote.is_some() || !text.is_empty())
        .map(|(quote, text)| {
            let unit = (!text.is_empty()).then(|| {
                units += 1;
                units - 1
            });
            Piece { quote, text, unit }
        })
        .collect()
}

/// The selectable text of a message's words, as [`user_message`] draws it: each part of its own
/// words, in order, a paragraph apart (what it quotes isn't selectable).
fn words_units(words: &str) -> Vec<Unit> {
    pieces(words.trim()).into_iter().filter_map(|piece| piece.unit.map(|at| Unit::new(&[at], piece.text, Lead::Paragraph))).collect()
}

/// `text` with the Work Queue items it names as their chips' labels, where they're known
/// ([`Items::labeled`]).
fn labeled<'a>(items: Option<&Items>, text: &'a str, markdown: bool) -> Cow<'a, str> {
    items.map_or(Cow::Borrowed(text), |items| items.labeled(text, markdown))
}

/// Markdown that draws the items it names as their chips.
fn with_items(words: Markdown, looks: &Looks) -> Markdown {
    match &looks.items {
        Some(items) => words.inline_items(items.inline()),
        None => words,
    }
}

/// Plain text `plain` (yours) drawn with the items it names as their chips, their numbers muted.
fn item_text(text: SelectableText, plain: &str, inline: Option<&markdown::InlineItems>, cx: &App) -> SelectableText {
    let c = &cx.theme().colors;
    let (Some(draw), Some(runs)) = (inline, markdown::item_runs(plain, c.text, c.text_muted)) else { return text };
    let items = markdown::item_labels(plain).into_iter().filter_map(|label| Some((label.range, draw(label.number, cx)?))).collect();
    text.runs(runs).items(items)
}

fn user_message(ix: usize, parts: &Parts, sel: Selectable, looks: &Looks, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    // What went with it, above it: the keyword it was sent with as typed, who wrote it.
    let mut above: Vec<AnyElement> = Vec::new();
    if let Some(mail) = &parts.mail {
        let name = crate::tool_step::clean_person(&mail.name).unwrap_or_else(|| "Another agent".to_owned());
        above.push(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(format!("From {name}")).into_any_element());
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
    // What it replies to, in its bubble, each quote above the words that answer it, in the order
    // they were written.
    let inline = looks.items.as_ref().map(|items| items.inline());
    let blocks: Vec<AnyElement> = pieces(labeled(looks.items.as_deref(), &parts.words, false).trim())
        .into_iter()
        .enumerate()
        .flat_map(|(n, Piece { quote, text, unit })| {
            let quote = quote.map(|quote| {
                div()
                    .debug_selector(move || format!("quoted-{ix}-{n}"))
                    // (A little apart from the answer before it, so each quote reads with its own, as the phone draws it.)
                    .when(n > 0, |d| d.mt(space::S1))
                    .pl(space::S2)
                    .border_l_2()
                    .border_color(c.border_strong)
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_muted)
                    .child(quote)
                    .into_any_element()
            });
            let words = unit.map(|at| {
                div()
                    .debug_selector(move || format!("said-{ix}-{n}"))
                    .child(item_text(SelectableText::new(("words", ix * 1000 + at as usize), text.clone()), &text, inline.as_ref(), cx).selectable(Some(sel.unit(&[at]))))
                    .into_any_element()
            });
            quote.into_iter().chain(words)
        })
        .collect();
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
                .when(!blocks.is_empty(), |d| {
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
                            .children(blocks),
                    )
                }),
        )
        .when(!below.is_empty(), |d| d.child(div().flex().flex_wrap().justify_end().gap(space::S1).children(below)))
        .into_any_element()
}

fn agent_step(step: &Step, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks, cx: &App) -> AnyElement {
    let mut blocks: Vec<AnyElement> = Vec::new();
    let mut pending_files: Vec<crate::shown_files::ViewModel> = Vec::new();
    let flush_files = |pending: &mut Vec<crate::shown_files::ViewModel>, blocks: &mut Vec<AnyElement>| {
        if !pending.is_empty() {
            let batch = std::mem::take(pending);
            blocks.push(crate::shown_files::render_group(&batch, &looks.file_actions, cx));
        }
    };
    for (n, content) in step.content.iter().enumerate() {
        if let Some(file) = rendered_shown_file(step, n, looks) {
            pending_files.push(file.clone());
            continue;
        }
        flush_files(&mut pending_files, &mut blocks);
        match content {
            Content::Text(text) if !text.trim().is_empty() => {
                blocks.push(words_block(step, n, text, sel, looks));
            }
            Content::Tool { name, .. } if crate::tool_step::is_send_message(name) => {
                let card_key = format!("{}:{n}", step.id);
                let nested_sel = |base: &[u32]| sel(&[n as u32, base.first().copied().unwrap_or(0)]);
                blocks.push(from_agents(&card_key, &sent_letters(step, n), step.time.created, open, toggle.clone(), &Around::none(), &nested_sel, looks, cx));
            }
            _ => blocks.extend(content_blocks(step, n, n as u32, open, toggle.clone(), false, time::now_ms(), sel, looks, cx)),
        }
    }
    flush_files(&mut pending_files, &mut blocks);
    if let Some(error) = &step.error {
        blocks.push(callout(&step.id, error_words(error), cx));
    }
    div().w_full().min_w_0().flex().flex_col().gap(GAP).children(blocks).into_any_element()
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
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(GAP)
        .children(blocks)
        .when_some(step.error.as_ref().filter(|_| error), |d, error| d.child(callout(&step.id, error_words(error), cx)))
        .into_any_element()
}

/// Block `n` of a step's words, as Markdown, with the pictures written in it.
fn words_block(step: &Step, n: usize, text: &str, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks) -> AnyElement {
    let mut words = with_items(Markdown::new(SharedString::from(format!("{}:{n}", step.id)), &labeled(looks.items.as_deref(), text, true)).selectable(sel(&[n as u32])), looks);
    // (Pictures written in its words, drawn as the conversation reads them.)
    if text.contains("![") {
        words = words.pictures(crate::pictures::words_pictures(&step.id, n, text, MADE.0, looks));
    }
    div().child(words).into_any_element()
}

/// The adapted shown file for tool item `(step, n)`, when it is a completed
/// `kuvo-browser_show_file` call that renders directly in the conversation flow.
fn rendered_shown_file<'a>(step: &Step, n: usize, looks: &'a Looks<'a>) -> Option<&'a crate::shown_files::ViewModel> {
    let Content::Tool { name, .. } = &step.content[n] else { return None };
    if name != "kuvo-browser_show_file" {
        return None;
    }
    let id = format!("{}:{n}-shown", step.id);
    looks.files.iter().find(|file| file.id == id)
}

/// Whether `content` is a completed `kuvo-browser_show_file` call (which renders in the flow
/// instead of as a step line, matching `kuvo_box::shown_files::of_step`).
fn is_completed_show_file(content: &Content) -> bool {
    let Content::Tool { name, state, .. } = content else { return false };
    name == "kuvo-browser_show_file"
        && state.get("status").and_then(Value::as_str) == Some("completed")
        && state
            .pointer("/input/path")
            .and_then(Value::as_str)
            .and_then(|path| path.rsplit('/').next())
            .is_some_and(|name| !name.is_empty())
}

/// What tool step `n` of a step made or took of a page to show you: large, in the flow,
/// shown whether or not its group is opened. (`kuvo-browser_show_file` renders via `shown_files`.)
fn made_pictures(step: &Step, n: usize, looks: &Looks, cx: &App) -> Option<AnyElement> {
    let Content::Tool { name, state, .. } = &step.content[n] else { return None };
    if name == "kuvo-browser_show_file" || !pictures::is_made(name, state) {
        return None;
    }
    let prefix = format!("{}:{n}-", step.id);
    let made: Vec<Picture> = looks.shown.iter().filter(|picture| picture.id.starts_with(&prefix)).cloned().collect();
    (!made.is_empty()).then(|| crate::pictures::pictures(&made, MADE.0, MADE.1, looks, cx))
}

/// A step's thinking or tool step `n`, drawn as item `at` of its row: its line, why it failed and
/// what it's opened to. `waits`: it's what the conversation waits on you for.
#[allow(clippy::too_many_arguments)]
fn content_blocks(step: &Step, n: usize, at: u32, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, waits: bool, now: f64, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks, cx: &App) -> Vec<AnyElement> {
    let c = &cx.theme().colors;
    let chevron = |opened: bool| Icon::new(if opened { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Nav).color(c.text_faint);
    let key = format!("{}:{n}", step.id);
    let mut blocks: Vec<AnyElement> = Vec::new();
    match &step.content[n] {
        Content::Reasoning { text, done } => {
            let fold = Fold::Thinking(step.id.clone(), n);
            let opened = open.contains(&fold);
            let label = if *done { "Thought" } else { "Thinking" };
            let row = step_row(SharedString::from(format!("think-{key}")), IconName::Sparkle, label.into(), None, false, !*done, false, cx)
                .trailing(chevron(opened))
                .into_div(cx)
                .cursor_pointer()
                .hover(|s| s.opacity(0.8))
                .on_click(move |_, _, cx| toggle(fold.clone(), cx));
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
        Content::Tool { name, state, title, doing, failure, .. } => {
            let (busy, failed) = call_status(state);
            let mut pres = call_presentation(name, title.as_deref(), doing.as_deref(), state, busy, step.time.created, now);
            let failure = failure.as_ref().filter(|_| failed);
            // A failed step's row is its box's one short line; all it said shows once it's opened
            // (a command's in its output, under the command).
            if let Some(failure) = failure {
                pres.label = failure.line.clone().into();
                pres.target = None;
            }
            let shown = match failure {
                Some(failure) if name == "shell" => Some(Details::Output {
                    command: state.pointer("/input/command").and_then(Value::as_str).map(|command| command.trim().to_owned()),
                    text: failure.error.clone(),
                }),
                _ => details(name, state),
            };
            let error = match failure {
                Some(_) if name == "shell" => None,
                Some(failure) => Some(failure.error.clone()).filter(|error| !error.trim().is_empty()),
                None => failed.then(|| tool_error(state)).flatten(),
            };
            let fold = Fold::Step(step.id.clone(), n);
            // (What it brought back for itself shows once it's opened, and so does why it failed.)
            let opens = shown.is_some() || error.is_some() || pictures::has_own(name, state);
            let opened = opens && open.contains(&fold);
            // A file it read or changed opens in its own tab in the side panel, by its name.
            let file = looks.file_actions.preview_file.clone().and_then(|preview| step_file(name, state).map(|path| (path, preview)));
            let target = if file.is_some() { None } else { pres.target.clone() };
            // A Work Queue step's item, as its chip in its row ("Updated [○ #954 Remove slash…]").
            let item = (failure.is_none()).then(|| crate::tool_step::queue_item(name, state)).flatten()
                .and_then(|number| looks.items.as_ref().and_then(|items| Some((items.row_chip(number, &key, cx)?, items.title(number).map(str::to_owned), number))));
            let mut row = match item {
                Some((chip, title, number)) => {
                    let named = format!("#{number}");
                    let (before, after) = pres.label.split_once(named.as_str()).map(|(before, after)| (before.trim().to_owned(), after.trim().to_owned())).unwrap_or_else(|| (pres.label.to_string(), String::new()));
                    // (Its title is the chip's: said again only when the step gave it another.)
                    let target = target.filter(|target| title.as_deref().is_none_or(|title| target.trim_start_matches("· ").trim() != title.trim()));
                    let body = div()
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .min_w_0()
                        .when(!before.is_empty(), |d| d.child(div().flex_none().child(before)))
                        .child(chip)
                        .when(!after.is_empty(), |d| d.child(div().flex_none().child(after)))
                        .children(target.map(|target| div().min_w_0().truncate().text_color(c.text_faint).child(target)));
                    step_row(SharedString::from(format!("tool-{key}")), pres.icon, pres.label.clone(), None, false, busy, failed, cx).body(body)
                }
                None => step_row(SharedString::from(format!("tool-{key}")), pres.icon, pres.label, target, pres.target_mono, busy, failed, cx),
            };
            if let (Some((path, preview)), Some(named)) = (file, pres.target.clone()) {
                row = row.trailing(file_link(SharedString::from(format!("tool-file-{key}")), named, path, preview, cx));
            }
            // One with something to show opens to it.
            if opens {
                if let Some((added, removed)) = shown.as_ref().and_then(Details::counts) {
                    // What it changed, opening the folder's changes in the side panel.
                    row = row.trailing(match looks.file_actions.show_changes.clone() {
                        Some(show) => {
                            let selector = format!("tool-changes-{key}");
                            let hover = c.hover;
                            div()
                                .id(SharedString::from(format!("tool-changes-{key}")))
                                .debug_selector(move || selector)
                                .px(space::S1)
                                .rounded(radius::SMALL)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    show(window, cx);
                                })
                                .child(counts(added, removed, cx))
                                .into_any_element()
                        }
                        None => counts(added, removed, cx),
                    });
                }
                row = row.trailing(chevron(opened));
            }
            let mut row = row.into_div(cx);
            if opens {
                let toggle = toggle.clone();
                let fold = fold.clone();
                row = row
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .on_click(move |_, _, cx| toggle(fold.clone(), cx));
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
            // Why it failed shows only once it's opened: closed, its row says it in a word.
            if opened && (shown.is_some() || error.is_some()) {
                let guide = div()
                    .ml(px(7.))
                    .pl(space::S3)
                    .border_l_1()
                    .border_color(c.border)
                    .flex()
                    .flex_col()
                    .gap(space::S1_5)
                    .children(shown.map(|shown| details_view(&key, step, n, at, shown, open, toggle, sel, cx)))
                    .children(error.map(|error| {
                        div()
                            .debug_selector(|| "step-error".into())
                            .text_ui(TypeSize::Small)
                            .text_color(c.text_muted)
                            .child(SelectableText::new(SharedString::from(format!("error-{key}")), error).selectable(Some(sel(&[at]).unit(&[1]))))
                            .into_any_element()
                    }));
                blocks.push(guide.into_any_element());
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

/// Whether a group folds to a line: it holds more than one tool step (excluding completed
/// `kuvo-browser_show_file` calls, which render directly in the flow without a step line).
fn folds(items: &[(Rc<Step>, usize)]) -> bool {
    items
        .iter()
        .filter(|(step, n)| matches!(step.content[*n], Content::Tool { .. }) && !is_completed_show_file(&step.content[*n]))
        .count()
        > 1
}

/// What the agent did between its words, as one group: a line summing it up ("Read 4 files, ran 2
/// commands") that opens to each step, with the step under way on it as it runs. A step that
/// failed, or that waits on you, shows under it even while it's folded. Completed
/// `kuvo-browser_show_file` calls render in the conversation flow without a redundant step line.
#[allow(clippy::too_many_arguments)]
fn steps(key: &str, items: &[(Rc<Step>, usize)], open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, waiting: bool, now: f64, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    // What it waits on you for: its last step under way.
    let waits_at = waiting.then(|| waiting_at(items)).flatten();
    let rendered_files: Vec<Option<&crate::shown_files::ViewModel>> = items.iter().map(|(step, n)| rendered_shown_file(step, *n, looks)).collect();
    let non_shown_tools = items
        .iter()
        .enumerate()
        .filter(|(at, (step, n))| rendered_files[*at].is_none() && matches!(step.content[*n], Content::Tool { .. }))
        .count();
    if non_shown_tools <= 1 {
        let mut blocks: Vec<AnyElement> = Vec::new();
        let mut pending_files: Vec<crate::shown_files::ViewModel> = Vec::new();
        let flush_files = |pending: &mut Vec<crate::shown_files::ViewModel>, blocks: &mut Vec<AnyElement>| {
            if !pending.is_empty() {
                let batch = std::mem::take(pending);
                blocks.push(crate::shown_files::render_group(&batch, &looks.file_actions, cx));
            }
        };
        for (at, (step, n)) in items.iter().enumerate() {
            if let Some(file) = rendered_files[at] {
                pending_files.push(file.clone());
                continue;
            }
            flush_files(&mut pending_files, &mut blocks);
            blocks.extend(content_blocks(step, *n, at as u32, open, toggle.clone(), waits_at == Some(at), now, sel, looks, cx));
        }
        flush_files(&mut pending_files, &mut blocks);
        return div().w_full().min_w_0().flex().flex_col().gap(GAP).children(blocks).into_any_element();
    }
    let fold = Fold::Group(key.to_owned());
    let opened = open.contains(&fold);
    let calls: Vec<Call> = items
        .iter()
        .enumerate()
        .filter(|(at, _)| rendered_files[*at].is_none())
        .filter_map(|(_, (step, n))| match &step.content[*n] {
            Content::Tool { name, state, title, doing, .. } => Some(Call { name, title: title.as_deref(), doing: doing.as_deref(), state, created: step.time.created }),
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
    let (label, target, target_mono, rest) = match under_way {
        Some(call) => {
            let pres = call_presentation(call.name, call.title, call.doing, call.state, true, call.created, now);
            let done: Vec<Call> = calls.iter().filter(|each| !call_status(each.state).0).cloned().collect();
            (pres.label, pres.target, pres.target_mono, (!done.is_empty()).then(|| summary(&done)))
        }
        None => (SharedString::from(summary(&calls)), None, false, None),
    };
    let selector = id.to_string();
    let toggle_group = toggle.clone();
    let mut row = TextRow::truncated(id, label)
        .hug()
        .debug_selector(selector)
        .gap(space::S1_5)
        .text_size(TypeSize::Small)
        .text_color(c.text_muted)
        .leading_slot(px(16.), leading);
    if let Some(target) = target {
        row = row.secondary(target, target_mono, Some(c.text_faint));
    }
    if let Some(rest) = rest {
        row = row.extra(format!("· {}", lowered(&rest)), Some(c.text_faint));
    }
    let line = row
        .trailing(Icon::new(if opened { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Nav).color(c.text_faint))
        .into_div(cx)
        .cursor_pointer()
        .hover(|s| s.opacity(0.8))
        .on_click(move |_, _, cx| toggle_group(fold.clone(), cx));
    let body = if opened {
        let expanded: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .filter(|(at, _)| rendered_files[*at].is_none())
            .flat_map(|(at, (step, n))| content_blocks(step, *n, at as u32, open, toggle.clone(), waits_at == Some(at), now, sel, looks, cx))
            .collect();
        // Each step, along a line from the group's.
        (!expanded.is_empty()).then(|| div().min_w_0().ml(px(7.)).pl(space::S3).border_l_1().border_color(c.border).flex().flex_col().gap(GAP).children(expanded).into_any_element())
    } else {
        // What needs you, and what it showed you, in its place.
        let mut needs_you = Vec::new();
        for (at, (step, n)) in items.iter().enumerate() {
            if rendered_files[at].is_some() {
                continue;
            }
            if shown_in_group(key, items, at, open, waiting) {
                needs_you.extend(content_blocks(step, *n, at as u32, open, toggle.clone(), waits_at == Some(at), now, sel, looks, cx));
            } else {
                needs_you.extend(made_pictures(step, *n, looks, cx));
            }
        }
        (!needs_you.is_empty()).then(|| div().w_full().min_w_0().flex().flex_col().gap(GAP).children(needs_you).into_any_element())
    };
    let shown_batch: Vec<crate::shown_files::ViewModel> = rendered_files.into_iter().flatten().cloned().collect();
    let flow = (!shown_batch.is_empty()).then(|| crate::shown_files::render_group(&shown_batch, &looks.file_actions, cx));
    div().w_full().min_w_0().flex().flex_col().gap(GAP).child(line).children(body).children(flow).into_any_element()
}

/// A tool step, as a group's line sums it up.
#[derive(Clone)]
struct Call<'a> {
    name: &'a str,
    title: Option<&'a str>,
    doing: Option<&'a str>,
    state: &'a Value,
    created: f64,
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

/// What a card in a conversation is: a message from or to another agent, one of its sessions'
/// news, or a wake-up (or watch) of its own. Each has its own mark and title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mail,
    Sent,
    News,
    Wake,
}

/// A message between agents (or a wake-up / session news card): what it is, who it's from or to
/// (a wake-up: "Woke up", as the phone says it), what happened ("finished", "asks"), what it said,
/// the line it shows folded where it isn't its first (a wake-up's summary), the conversation it
/// connects to (none where it can't be opened), whether a send is still running, and any failure
/// reason.
#[derive(Clone, Debug, PartialEq)]
pub struct Letter {
    pub kind: Kind,
    pub from: String,
    pub status: Option<String>,
    pub said: String,
    pub preview: Option<String>,
    pub session: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
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

    /// Its mark, and the colour it's drawn in: a quiet send mark for a message (both directions),
    /// an alarm clock for a wake-up, and for a session's news what happened (finished, asks, stopped).
    fn mark(&self, c: &kuvo_ui::theme::Colors) -> (IconName, gpui::Hsla) {
        match (self.kind, self.status.as_deref()) {
            (Kind::Mail | Kind::Sent, _) => (IconName::Send, c.text_muted),
            (Kind::Wake, _) => (IconName::Alarm, c.text_muted),
            (Kind::News, Some("finished")) => (IconName::Check, c.success),
            (Kind::News, Some("asks")) => (IconName::Question, c.accent),
            (Kind::News, Some("stopped" | "not summarized")) => (IconName::Warning, c.warning),
            (Kind::News, Some("stopped by you")) => (IconName::Close, c.text_muted),
            (Kind::News, _) => (IconName::Send, c.text_muted),
        }
    }
}

/// The letters a message of another agent's, or the box's note of its sessions' news, is: one for
/// each session it tells of, or the one from the agent that wrote it.
pub fn letters(user: &kuvo_box::messages::UserMessage) -> Vec<Letter> {
    // Its sessions' news its box couldn't read: whole, from them, rather than lost.
    if let Some(text) = user.unread_news() {
        return vec![Letter { kind: Kind::News, from: "Its sessions".into(), status: None, said: text, preview: None, session: None, busy: false, error: None }];
    }
    // A wake-up of its own: its summary, opening to its note.
    if let Some(wake) = user.wake() {
        return vec![Letter { kind: Kind::Wake, from: "Woke up".into(), status: None, said: wake.note, preview: Some(wake.summary), session: None, busy: false, error: None }];
    }
    let news: Vec<News> = user.news();
    if !news.is_empty() {
        return news
            .into_iter()
            .map(|news| Letter { kind: Kind::News, from: news.title, status: news.status, said: news.said, preview: None, session: Some(news.session).filter(|session| session.starts_with("ses")), busy: false, error: None })
            .collect();
    }
    let parts = user.parts();
    let Some(mail) = parts.mail else { return Vec::new() };
    let from = crate::tool_step::clean_person(&mail.name).unwrap_or_else(|| "Another agent".to_owned());
    vec![Letter { kind: Kind::Mail, from, status: None, said: parts.words.trim().to_owned(), preview: None, session: Some(mail.session).filter(|session| session != "unknown" && !session.is_empty()), busy: false, error: None }]
}

/// The card(s) for a `send_message` tool call: "To <name>" with the sent text (and any error if it
/// failed), followed immediately by "From <name>" if the call waited and got a reply back.
pub fn sent_letters(step: &Step, part: usize) -> Vec<Letter> {
    let Some(Content::Tool { state, title, .. }) = step.content.get(part) else {
        return Vec::new();
    };
    let sent = crate::tool_step::sent_message(title.as_deref(), state);
    let mut cards = vec![Letter {
        kind: Kind::Sent,
        from: sent.to,
        status: None,
        said: sent.text,
        preview: None,
        session: sent.session.clone(),
        busy: sent.busy,
        error: sent.error,
    }];
    if let Some((reply_from, reply_text)) = sent.reply {
        cards.push(Letter {
            kind: Kind::Mail,
            from: reply_from,
            status: None,
            said: reply_text,
            preview: None,
            session: sent.session,
            busy: false,
            error: None,
        });
    }
    cards
}

/// Messages between agents (both "From <name>" and "To <name>"), a lead's sessions' news, and
/// wake-ups, each as one calm bordered card: its mark, direction + name, time and Open (its
/// conversation), and its text (or first line, opening to all of it).
#[allow(clippy::too_many_arguments)]
fn from_agents(message: &str, letters: &[Letter], at: f64, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, around: &Around, sel: &dyn Fn(&[u32]) -> Selectable, looks: &Looks, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let cards = letters.iter().enumerate().map(|(n, letter)| {
        let whole = letter.opens(open, message, n);
        let fold = Fold::Card(message.to_owned(), n);
        let toggle = toggle.clone();
        let id = SharedString::from(format!("letter-{message}-{n}"));
        let selector = id.to_string();
        let preview = letter.folded_line();
        let failed = letter.error.is_some();
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
                .text_color(c.text_muted)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover).text_color(c.text))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    open_session(&session, window, cx)
                })
                .child("Open")
        });
        // One line: "From <name>" or "To <name>" for messages; a session's name and what happened
        // ("Sign-in finished"); "Woke up".
        let direction = match letter.kind {
            Kind::Mail => Some("From"),
            Kind::Sent => Some("To"),
            Kind::News | Kind::Wake => None,
        };
        let from = div()
            .flex()
            .items_center()
            .gap(space::S1)
            .min_w_0()
            .text_color(c.text_muted)
            .children(direction.map(|dir| div().flex_none().child(dir)))
            .child(div().min_w_0().truncate().text_color(c.text).font_weight(kuvo_ui::theme::MEDIUM).child(letter.from.clone()))
            .children(letter.status.clone().map(|status| div().flex_none().child(status)));
        let (mark, tint) = letter.mark(c);
        let leading: AnyElement = if letter.busy {
            spinner(SharedString::from(format!("{id}-spin")), IconSize::Nav, c.text_muted).into_any_element()
        } else if failed {
            Icon::new(IconName::Warning).size(IconSize::Nav).color(c.danger).into_any_element()
        } else {
            Icon::new(mark).size(IconSize::Nav).color(tint).into_any_element()
        };
        let head = div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S1_5)
            .min_w_0()
            .overflow_hidden()
            .text_ui(TypeSize::Small)
            .child(div().flex().flex_none().w(px(16.)).justify_center().child(leading))
            .child(div().flex().flex_1().min_w_0().child(from))
            .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(time::when(at, around.now)))
            .children(open_it)
            .when(letter.folds(), |d| d.child(div().flex().flex_none().items_center().child(Icon::new(if whole { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Nav).color(c.text_faint))));
        let said: AnyElement = if whole {
            div().min_w_0().ml(UNDER).text_ui(TypeSize::Small).child(with_items(Markdown::new(SharedString::from(format!("{id}-said")), &labeled(looks.items.as_deref(), &letter.said, true)).selectable(sel(&[n as u32])), looks)).into_any_element()
        } else {
            div().min_w_0().ml(UNDER).truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(preview).into_any_element()
        };
        let error = letter.error.clone().map(|error| {
            div().min_w_0().ml(UNDER).child(Notice::inline(SharedString::from(format!("{id}-error")), Tone::Error, "Not delivered").detail(error)).into_any_element()
        });
        div()
            .id(id)
            .debug_selector(move || selector)
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(space::S1)
            .px(space::S3)
            .py(space::S2)
            .rounded(radius::POPOVER)
            .border_1()
            .border_color(if failed { c.danger.opacity(0.35) } else { c.border })
            .bg(c.sunken.opacity(0.5))
            .when(letter.folds(), |d| d.cursor_pointer().on_click(move |_, _, cx| toggle(fold.clone(), cx)))
            .child(head)
            .when(!letter.said.trim().is_empty(), |d| d.child(said))
            .children(error)
    });
    div().w_full().min_w_0().flex().flex_col().gap(GAP).children(cards).into_any_element()
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
    /// Human-only content for a Kuvo or MCP tool call (message text, Work Queue note,
    /// wake-up note, or an unknown tool's main text argument and main text result).
    Note { primary: String, secondary: Option<String> },
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
            Details::Output { .. } | Details::Note { .. } => None,
        }
    }
}

/// What a step printed, as the engine keeps it: its text blocks (`state.content`), one after another.
fn printed(state: &Value) -> String {
    crate::tool_step::printed(state)
}

/// Why a step failed, in plain words without a repeated "Error:" prefix.
fn tool_error(state: &Value) -> Option<String> {
    crate::tool_step::tool_error(state)
}

/// What a tool step shows when it's opened (none: it has nothing to show, and doesn't open): the
/// files it changed, as diffs; a command's output; a search's results; or a Kuvo/MCP tool's human
/// note. A file read, or a step that's still running, shows nothing more.
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
        _ => crate::tool_step::note(name, state).map(|note| Details::Note {
            primary: note.primary,
            secondary: note.secondary,
        }),
    }
}

/// An opened step's output, changes, or human note.
#[allow(clippy::too_many_arguments)]
fn details_view(key: &str, step: &Step, n: usize, at: u32, shown: Details, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, sel: &dyn Fn(&[u32]) -> Selectable, cx: &App) -> AnyElement {
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
        Details::Note { primary, secondary } => {
            let c = &cx.theme().colors;
            div()
                .flex()
                .flex_col()
                .gap(space::S1_5)
                .child(
                    div()
                        .text_ui(TypeSize::Small)
                        .text_color(c.text)
                        .child(SelectableText::new(SharedString::from(format!("note-{key}-0")), primary).selectable(Some(sel(&[at]).unit(&[0])))),
                )
                .children(secondary.map(|secondary| {
                    div()
                        .text_ui(TypeSize::Small)
                        .text_color(c.text_muted)
                        .child(SelectableText::new(SharedString::from(format!("note-{key}-1")), secondary).selectable(Some(sel(&[at]).unit(&[1]))))
                }))
                .into_any_element()
        }
    }
}

/// The file a step read or changed, by its full path, as the engine's call names it.
fn step_file(name: &str, state: &Value) -> Option<String> {
    if !matches!(name, "read" | "edit" | "multiedit" | "patch" | "apply_patch" | "write") {
        return None;
    }
    let input = state.get("input")?;
    input.get("filePath").or_else(|| input.get("path")).and_then(Value::as_str).filter(|path| path.starts_with('/')).map(str::to_owned)
}

/// A file's name in a step, opening it in its own tab in the side panel.
fn file_link(id: SharedString, name: SharedString, path: String, open: std::rc::Rc<dyn Fn(&str, &mut Window, &mut App)>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let (faint, text) = (c.text_faint, c.text);
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector)
        .min_w_0()
        .truncate()
        .text_ui(TypeSize::Small)
        .text_color(faint)
        .cursor_pointer()
        .hover(move |s| s.text_color(text).underline())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            open(&path, window, cx);
        })
        .child(name)
        .into_any_element()
}

/// A tool step or thinking, as one quiet line: its icon (or a spinner while it runs), what it did,
/// and what it did it to.
#[allow(clippy::too_many_arguments)]
fn step_row(id: SharedString, icon: IconName, label: SharedString, target: Option<SharedString>, target_mono: bool, busy: bool, failed: bool, cx: &App) -> TextRow {
    let c = &cx.theme().colors;
    let leading: AnyElement = if busy {
        spinner(SharedString::from(format!("{id}-spin")), IconSize::Nav, c.text_muted).into_any_element()
    } else if failed {
        Icon::new(IconName::Warning).size(IconSize::XSmall).color(c.warning).into_any_element()
    } else {
        Icon::new(icon).size(IconSize::Nav).color(c.text_faint).into_any_element()
    };
    let selector = id.to_string();
    let mut row = TextRow::truncated(id, label)
        .hug()
        .debug_selector(selector)
        .gap(space::S1_5)
        .text_size(TypeSize::Small)
        .text_color(c.text_muted)
        .leading_slot(px(16.), leading);
    if let Some(target) = target {
        row = row.secondary(target, target_mono, Some(c.text_faint));
    }
    row
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

/// Full closed-row presentation for any tool step, formatting wake-up times against `created_ms`/`now_ms`.
fn call_presentation(name: &str, title: Option<&str>, doing: Option<&str>, state: &Value, busy: bool, created_ms: f64, now_ms: f64) -> crate::tool_step::Presentation {
    if name == "kuvo-browser_show_file" {
        let path = state.pointer("/input/path").and_then(Value::as_str);
        let icon = match path.map(kuvo_box::shown_files::guess_type) {
            Some((kuvo_box::shown_files::FileKind::Picture, _)) => IconName::Image,
            _ => IconName::File,
        };
        let target = path
            .and_then(|path| path.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .map(|name| SharedString::from(name.to_owned()));
        let (_, failed) = call_status(state);
        let label = if busy {
            doing.unwrap_or("Showing a file")
        } else if failed {
            "Couldn’t show file"
        } else {
            title.unwrap_or("Showed a file")
        };
        return crate::tool_step::Presentation {
            icon,
            label: SharedString::from(label.to_owned()),
            target,
            target_mono: true,
        };
    }
    if let Some(pres) = crate::tool_step::present(name, title, doing, state, busy, created_ms, now_ms) {
        return pres;
    }
    let (icon, label, target) = tool_label(name, state, busy);
    crate::tool_step::Presentation { icon, label, target, target_mono: true }
}

/// What a tool step did, in words, with what its box says of it: the engine's own tools by what
/// they did and to what ([`tool_label`]); Kuvo tools and MCP servers in plain human words
/// ([`crate::tool_step::present`]).
pub fn call_label(name: &str, title: Option<&str>, doing: Option<&str>, state: &Value, busy: bool) -> (IconName, SharedString, Option<SharedString>) {
    let pres = call_presentation(name, title, doing, state, busy, 0.0, time::now_ms());
    (pres.icon, pres.label, pres.target)
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

/// Why the agent's turn stopped, under it: the one notice, the box's words as its line.
// BOX-NEEDED: plain words for a turn stopped at a model's limit (which account, when it's back);
// the engine's raw error is all a conversation's step carries today.
fn callout(id: &str, words: String, _cx: &App) -> AnyElement {
    Notice::inline(SharedString::from(format!("stopped-{id}")), Tone::Error, "Stopped").detail(words).into_any_element()
}

fn shell_run(run: &kuvo_box::messages::ShellRun, open: &HashSet<Fold>, toggle: impl Fn(Fold, &mut App) + Clone + 'static, sel: &dyn Fn(&[u32]) -> Selectable, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let busy = run.status == "running";
    let failed = run.status == "failed" || run.exit.is_some_and(|exit| exit != 0);
    let exit = run.exit.filter(|exit| *exit != 0).map(|exit| format!("exit {exit}"));
    let fold = Fold::Whole(run.id.clone(), 0, 0);
    let output = run.output.as_ref().map(|output| OutputWell::new(SharedString::from(format!("shell-output-{}", run.id)), output.output.clone()).command(run.command.clone()).whole(open.contains(&fold)).on_toggle(move |_, cx| toggle(fold.clone(), cx)).selectable(sel(&[0])));
    let mut head = step_row(SharedString::from(format!("shell-{}", run.id)), IconName::Terminal, if busy { "Running".into() } else { "You ran".into() }, Some(first_line(&run.command).into()), true, busy, failed, cx);
    if let Some(exit) = exit {
        head = head.trailing(div().text_ui(TypeSize::Caption).text_color(c.danger).child(exit));
    }
    div().w_full().min_w_0().flex().flex_col().gap(space::S2).child(head.into_div(cx))
        .children(output)
        .when(run.output.as_ref().is_some_and(|output| output.truncated), |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Output cut short")))
        .into_any_element()
}

/// Whether a turn that ended so says so at its end: one stopped, or one that failed.
fn ends_unfinished(outcome: &str) -> bool {
    matches!(outcome, "failed" | "interrupted")
}

fn turn_end(outcome: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let (words, color) = match outcome {
        "failed" => ("The turn didn’t finish", c.danger),
        _ => ("Stopped", c.text_faint),
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
    TextRow::truncated("working-row", title.to_owned())
        .hug()
        .gap(space::S1_5)
        .text_size(TypeSize::Small)
        .text_color(c.text_faint)
        .leading_slot(px(16.), spinner("working", IconSize::Nav, c.text_muted))
        .body(div().id("working-label").min_w_0().truncate().child(title.to_owned()).pulsing())
        .into_div(cx)
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
    fn a_finished_turn_shows_your_message_and_the_agents_words_and_nothing_after_them() {
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
        // The box's note to the agent, an empty step, a switch of mode and the finished turn's end
        // show nothing (no "Worked for" line).
        assert_eq!(keys, ["m1", "m1b", "m3"]);
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
        let calls: Vec<Call> = calls.iter().map(|(name, title, state)| Call { name, title: *title, doing: None, state, created: 0.0 }).collect();
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
        assert_eq!(mail, [Letter { kind: Kind::Mail, from: "Mira".into(), status: None, said: "Done.\n\nAll green.".into(), preview: None, session: Some("ses_m".into()), busy: false, error: None }]);
        assert!(mail[0].folds(), "more than its first line");
        let Entry::Message(message) = &rows[1] else { panic!() };
        let Message::User(user) = &**message else { panic!() };
        assert_eq!(letters(user), [Letter { kind: Kind::News, from: "Sign-in".into(), status: Some("asks".into()), said: "May I run npm i?".into(), preview: None, session: Some("ses_a".into()), busy: false, error: None }]);
        // Its words are selectable once it's shown whole.
        assert!(units(&rows[0], &HashSet::new(), None).is_empty());
        assert!(!units(&rows[0], &HashSet::from([Fold::Card("m1".into(), 0)]), None).is_empty());
    }

    #[test]
    fn sent_messages_leave_step_groups_and_show_waited_replies_and_errors() {
        let (rows, _) = entries(conversation(
            json!([
                { "type": "user", "id": "u_in", "time": { "created": 1 }, "text": "[From &Check the other forms] Same bug here.", "metadata": { "kuvo": { "parts": { "words": "Same bug here.", "mail": { "name": "&Check the other forms", "session": "ses_sub" } } } } },
                step("a1", 2.0, json!([
                    tool_part("t0", "read", "completed", json!({ "filePath": "/p/a.ts" })),
                    { "type": "tool", "id": "t1", "name": "kuvo-browser_send_message", "title": "Sent a message", "state": {
                        "status": "completed",
                        "input": { "to": "ses_mira", "wait": false, "text": "Taking a look now." },
                        "content": [{ "type": "text", "text": "Sent to Mira. Its answer, if it writes one, comes to you as a message." }]
                    }},
                    tool_part("t2", "shell", "completed", json!({ "command": "cargo test" })),
                    { "type": "tool", "id": "t3", "name": "kuvo-browser_send_message", "title": "Sent a message", "state": {
                        "status": "completed",
                        "input": { "to": "&Release helper (ses_rel)", "wait": true, "text": "Is beta.35 green?" },
                        "content": [{ "type": "text", "text": "Release helper answered:\n\nGreen on macOS and Linux." }]
                    }},
                    { "type": "tool", "id": "t4", "name": "kuvo-browser_send_message", "title": "Sent a message", "state": {
                        "status": "failed",
                        "input": { "to": "ses_build", "wait": false, "text": "Run notarized build." },
                        "error": "Error: Build runner can't be reached now. Try again later."
                    }},
                    { "type": "text", "text": "Done checking." }
                ])),
            ]),
            false,
            None,
        ));
        // Received card strips leading &.
        let Entry::Message(msg) = &rows[0] else { panic!("received") };
        let Message::User(user) = &**msg else { panic!("user") };
        assert_eq!(letters(user)[0].from, "Check the other forms");

        // Order: received, steps[read], sent(Mira), steps[shell], sent(Release helper + reply), sent(Build runner failed), words.
        assert_eq!(rows.len(), 7);
        let Entry::Steps { items: before, .. } = &rows[1] else { panic!("steps before") };
        assert_eq!(before.len(), 1);
        let Entry::Sent { step, part, .. } = &rows[2] else { panic!("sent 1") };
        let cards1 = sent_letters(step, *part);
        assert_eq!(cards1.len(), 1);
        assert_eq!(cards1[0].kind, Kind::Sent);
        assert_eq!(cards1[0].from, "Mira");
        assert_eq!(cards1[0].said, "Taking a look now.");
        assert_eq!(cards1[0].error, None);
        // Short sent message is immediately selectable without folding.
        assert!(!units(&rows[2], &HashSet::new(), None).is_empty());

        let Entry::Steps { items: mid, .. } = &rows[3] else { panic!("steps mid") };
        assert_eq!(mid.len(), 1);

        let Entry::Sent { step, part, .. } = &rows[4] else { panic!("sent 2") };
        let cards2 = sent_letters(step, *part);
        assert_eq!(cards2.len(), 2);
        assert_eq!((cards2[0].kind, cards2[0].from.as_str(), cards2[0].said.as_str()), (Kind::Sent, "Release helper", "Is beta.35 green?"));
        assert_eq!((cards2[1].kind, cards2[1].from.as_str(), cards2[1].said.as_str()), (Kind::Mail, "Release helper", "Green on macOS and Linux."));

        let Entry::Sent { step, part, .. } = &rows[5] else { panic!("sent 3") };
        let cards3 = sent_letters(step, *part);
        assert_eq!(cards3.len(), 1);
        assert_eq!(cards3[0].from, "Build runner");
        assert_eq!(cards3[0].error.as_deref(), Some("Build runner can't be reached now. Try again later."));

        assert!(matches!(&rows[6], Entry::Words { .. }));
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
        assert_eq!(wake, [Letter { kind: Kind::Wake, from: "Woke up".into(), status: None, said: note.into(), preview: Some("Pacing check".into()), session: None, busy: false, error: None }]);
        assert_eq!(wake[0].folded_line(), "Pacing check");
        assert!(wake[0].folds(), "it opens to the whole note");
        assert!(units(&rows[0], &HashSet::new(), None).is_empty(), "folded, only its summary shows");
        assert!(!units(&rows[0], &HashSet::from([Fold::Card("m1".into(), 0)]), None).is_empty());
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

    #[test]
    fn completed_show_file_does_not_fold_a_group_while_running_and_failed_keep_their_line() {
        let (rows, _) = entries(conversation(
            json!([step("a", 1.0, json!([
                tool_part("s1", "kuvo-browser_show_file", "completed", json!({ "path": "/p/one.png" })),
                tool_part("s2", "kuvo-browser_show_file", "completed", json!({ "path": "/p/two.png" })),
                tool_part("s3", "kuvo-browser_show_file", "completed", json!({ "path": "/p/three.png" })),
            ]))]),
            false,
            None,
        ));
        let Entry::Steps { items, .. } = &rows[0] else { panic!("steps") };
        assert!(!folds(items), "three completed show_file calls render directly in the flow without a group header");

        let running = json!({ "status": "running", "input": { "path": "/docs/pricing-hero.png" } });
        let (icon, label, target) = call_label("kuvo-browser_show_file", Some("Showed a file"), Some("Showing a file"), &running, true);
        assert_eq!(icon, IconName::Image);
        assert_eq!((label.as_ref(), target.as_deref()), ("Showing a file", Some("pricing-hero.png")));

        let failed = json!({ "status": "failed", "input": { "path": "/docs/report.pdf" }, "error": "Permission denied" });
        let (icon, label, target) = call_label("kuvo-browser_show_file", Some("Showed a file"), Some("Showing a file"), &failed, false);
        assert_eq!(icon, IconName::File);
        assert_eq!((label.as_ref(), target.as_deref()), ("Couldn’t show file", Some("report.pdf")));
    }
}
