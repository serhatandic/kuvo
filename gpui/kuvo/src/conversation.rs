//! A conversation: its messages in the chat list, newest at the bottom, following a run live and
//! reading earlier messages as you scroll back to them, with the composer under it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::asks::Asks;
use kuvo_box::queue::{Item, Queue};
use kuvo_box::follow::{ConversationUpdate, Follow, Followed, Timing};
use kuvo_box::BoxClient;
use kuvo_box::error::Offered;
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    anchor::{Placement, Popover},
    chat_list::{ChatListState, chat_list},
    menu::{ContextMenuArea, Menu, MenuEntry, PopoverMenu},
    selection::{self, SelectionArea, SelectionState},
    small::spinner,
    theme::{radius, space},
};

use crate::ask_card::AskCard;
use crate::composer::{Composer, ComposerEvent};
use crate::pictures::{self, Looks, Read};
use kuvo_box::messages::{Content, Message};
use kuvo_box::pictures::Picture;
use crate::entries::{self, COLUMN, Entry, Fold};
use crate::work_bars::WorkBars;

const QUEUE_ROW_HEIGHT: f32 = 32.;

#[derive(Clone)]
struct QueueDrag {
    owner: gpui::EntityId,
    item: String,
    words: String,
    action: &'static str,
}

struct QueueDragPreview {
    drag: QueueDrag,
    row: gpui::Bounds<gpui::Pixels>,
    pointer_offset_x: gpui::Pixels,
}

impl Render for QueueDragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let c = &theme.colors;
        // GPUI places the preview at the pointer minus its initial grab offset.
        let left = self.row.left() - (window.mouse_position().x - self.pointer_offset_x);
        div().debug_selector(|| "queue-drag-preview".into())
            .relative().left(left).top(px(-3.)).w(self.row.size.width).h(px(QUEUE_ROW_HEIGHT))
            .flex().items_center().gap(space::S2).px(space::S2)
            .rounded(radius::NAV).bg(c.elevated).shadow(theme.card_shadow())
            .child(Icon::new(IconName::QueuedMessage).size(IconSize::Nav).color(c.text_faint))
            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(self.drag.words.clone()))
            .child(Button::new("drag-queue-action", self.drag.action).ghost().quiet().size(ButtonSize::Small)
                .when(self.drag.action == "Steer", |button| button.icon(IconName::Steer).icon_size(IconSize::Nav)))
            .child(Button::icon_only("drag-queue-remove", IconName::Trash, "Remove").ghost().quiet()
                .size(ButtonSize::Small).icon_size(IconSize::Nav))
            .child(Button::icon_only("drag-queue-more", IconName::More, "More actions").ghost().quiet()
                .size(ButtonSize::Small).icon_size(IconSize::Nav))
    }
}

#[derive(Clone, PartialEq)]
struct QueueMove {
    item: String,
    anchor: String,
    before: bool,
}

/// What's wrong, said over the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Reading it failed; it's tried again. Its words, and what the box offers to do about it
    /// (Try Again, for its coding agents down: PROTOCOL.md, "The coding agents' state").
    Failed(String, Option<Offered>),
    /// It isn't on the box (any more).
    Gone(String),
}

pub struct ConversationView {
    session: String,
    /// Whether its box has answered yet.
    read: bool,
    /// The way back to messages before those shown, while there may be some.
    earlier: Option<String>,
    /// How often its box's models were heard to change, as last seen: the composer reads the
    /// conversation's model again as it goes up.
    models_changed: Option<u64>,
    entries: Vec<Entry>,
    chat_references: crate::queue::chat_references::ChatReferences,
    /// Each row's revision: a row is drawn again only once it's changed.
    revisions: Vec<u64>,
    list: ChatListState,
    problem: Option<Problem>,
    /// The box's Try Again under way (its coding agents started again).
    taking_action: bool,
    earlier_failed: Option<String>,
    /// The way back already asked for, so it's asked once.
    asked_earlier: Rc<RefCell<Option<String>>>,
    /// Thinking blocks and tool steps opened, and outputs shown whole.
    open: HashSet<Fold>,
    /// Its composer, which the workspace keeps (with its draft) while you're elsewhere.
    composer: Entity<Composer>,
    /// What it waits on you for, in the composer's place while anything does.
    asks: Entity<AskCard>,
    /// The activity control in the header.
    bars: Entity<WorkBars>,
    follow: Option<Rc<Follow<ConversationUpdate>>>,
    _follow_task: Option<Task<()>>,
    _asks_follow: Option<Follow<Followed<Asks>>>,
    _asks_task: Option<Task<()>>,
    /// What waits its turn, as the box orders it, over the composer.
    queue: Queue,
    queue_failed: Option<String>,
    queue_scroll: gpui::ScrollHandle,
    queue_drag: Option<QueueMove>,
    queue_move_pending: Option<QueueMove>,
    queue_dragging: Option<String>,
    queue_drag_motion: u64,
    /// Why what it asks couldn't be read (what was shown stays meanwhile).
    asks_failed: Option<String>,
    /// Whether the card had the composer's place (and the keyboard with it) last it was drawn.
    card_took_keys: bool,
    /// The whole conversation's, so the card can tell the keyboard is somewhere in it.
    focus: FocusHandle,
    /// What's selected in its messages, and the keyboard for it (⌘C, ⌘A) once you select there.
    selection: SelectionState,
    transcript: FocusHandle,
    /// The row whose Copy was just used (it shows a check for a moment), and how many times Copy
    /// has been used, so only the last one's timer takes the check away.
    copied: Option<(String, u64)>,
    client: BoxClient,
    live: bool,
    _queue_follow: Option<Follow<Followed<Queue>>>,
    tasks: Vec<Task<()>>,
    /// Yours on its way, drawn again as the composer says it changed.
    _outgoing: Subscription,
    /// Whether a run is under way, as its box last said: one of yours its box took waits for the
    /// agent's next step meanwhile.
    running: bool,
    /// Its pictures as read, by their ids (each read once).
    pictures: HashMap<String, Read>,
    /// Where each picture was read from: one whose place changes (yours on its way, read from this
    /// Mac, then its box's copy) is read again from there.
    read_from: HashMap<String, kuvo_box::pictures::Source>,
    /// Each row's pictures, found as it changed (its revision, and which of its steps were open),
    /// not at each drawing: finding them copies what a message carries.
    row_pictures: HashMap<String, (u64, Vec<(String, usize)>, Vec<Picture>)>,
    /// The pictures of yours sent from here whose copy came without them, by message: drawn from
    /// here until it has them.
    sent_pictures: HashMap<String, Vec<Picture>>,
    /// The picture opened over it, by its id, and the keyboard for it (← →, Escape).
    viewing: Option<String>,
    viewer_focus: FocusHandle,
    /// The conversation's computer key, attached to `ShownFile`s before remote open.
    computer_key: Option<String>,
    /// Previews a shown text file in the workspace side panel.
    preview_file: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
    /// Each row's extracted `kuvo-browser_show_file` calls, cached by row revision.
    row_shown_files: HashMap<String, (u64, Vec<kuvo_box::shown_files::ShownFile>)>,
    /// Metadata fetched via `BoxClient::file_info` for non-picture shown files, by `ShownFile::id`.
    shown_info: HashMap<String, crate::shown_files::InfoRead>,
    /// Live state of opening a shown file via `open_shown::open_shown_file`, by `ShownFile::id`.
    shown_open: HashMap<String, crate::shown_files::OpenState>,
}

impl Focusable for ConversationView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        let asks = self.asks.read(cx);
        if asks.is_asking() && !asks.leaves_composer() { asks.focus_handle(cx) } else { self.composer.focus_handle(cx) }
    }
}

impl ConversationView {
    /// A conversation of `client`'s box. `live`: follow it there (the app); else it shows what
    /// it's handed ([`ConversationView::take`]), for tests and shots.
    pub fn new(client: BoxClient, session: String, composer: Entity<Composer>, live: bool, cx: &mut Context<Self>) -> ConversationView {
        let outgoing = cx.subscribe(&composer, |this: &mut ConversationView, _, event: &ComposerEvent, cx| {
            if matches!(event, ComposerEvent::Outgoing) {
                this.show_outgoing(cx);
            }
        });
        let mut view = ConversationView {
            session: session.clone(),
            read: false,
            earlier: None,
            // (From the box's first word: a change heard before the first update counts too.)
            models_changed: Some(0),
            entries: Vec::new(),
            chat_references: Default::default(),
            revisions: Vec::new(),
            list: ChatListState::new(0, cx),
            problem: None,
            taking_action: false,
            earlier_failed: None,
            asked_earlier: Rc::default(),
            open: HashSet::new(),
            asks: cx.new(|cx| AskCard::new(client.clone(), session.clone(), cx)),
            bars: cx.new(|cx| WorkBars::new(client.clone(), session.clone(), live, cx)),
            composer,
            follow: None,
            _follow_task: None,
            _asks_follow: None,
            _asks_task: None,
            queue: Queue::default(),
            queue_failed: None,
            queue_scroll: gpui::ScrollHandle::new(),
            queue_drag: None,
            queue_move_pending: None,
            queue_dragging: None,
            queue_drag_motion: 0,
            asks_failed: None,
            card_took_keys: false,
            focus: cx.focus_handle(),
            selection: SelectionState::new(),
            transcript: cx.focus_handle(),
            copied: None,
            client: client.clone(),
            live,
            _queue_follow: None,
            tasks: Vec::new(),
            _outgoing: outgoing,
            running: false,
            pictures: HashMap::new(),
            read_from: HashMap::new(),
            row_pictures: HashMap::new(),
            sent_pictures: HashMap::new(),
            viewing: None,
            viewer_focus: cx.focus_handle(),
            computer_key: None,
            preview_file: None,
            row_shown_files: HashMap::new(),
            shown_info: HashMap::new(),
            shown_open: HashMap::new(),
        };
        if live {
            let follow = Rc::new(client.follow_conversation(&session, Timing::default()));
            let updates = follow.updates().clone();
            view.follow = Some(follow);
            view._follow_task = Some(cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    if this.update(cx, |this, cx| this.take(update, cx)).is_err() {
                        break;
                    }
                }
            }));
            // What it asks, as the box words it, now and as it changes.
            let asks = client.follow_asks(&session, Timing::default());
            let updates = asks.updates().clone();
            view._asks_follow = Some(asks);
            // What waits its turn, now and as it changes.
            let queue = client.follow_queue(&session, Timing::default());
            let queued = queue.updates().clone();
            view._queue_follow = Some(queue);
            view.tasks.push(cx.spawn(async move |this, cx| {
                while let Ok(update) = queued.recv().await {
                    let taken = match update {
                        Followed::Now(queue) => this.update(cx, |this, cx| this.take_queue(queue, cx)),
                        // What it said last stays, saying it couldn't be read now.
                        // (Not for the coding agents' state: the line over the composer says it.)
                        Followed::Failed(error) if about_agents(&error) => Ok(()),
                        Followed::Failed(error) => this.update(cx, |this, cx| {
                            this.queue_failed = Some(format!("The queue couldn’t be read: {error}"));
                            cx.notify();
                        }),
                    };
                    if taken.is_err() {
                        break;
                    }
                }
            }));
            view._asks_task = Some(cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    let taken = match update {
                        Followed::Now(asks) => this.update(cx, |this, cx| this.take_asks(asks, cx)),
                        Followed::Failed(error) if about_agents(&error) => Ok(()),
                        Followed::Failed(error) => this.update(cx, |this, cx| {
                            this.asks_failed = Some(format!("What it asks you couldn’t be read: {error}"));
                            cx.notify();
                        }),
                    };
                    if taken.is_err() {
                        break;
                    }
                }
            }));
        }
        view
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn set_computer_key(&mut self, key: Option<String>) {
        self.computer_key = key;
    }

    pub fn set_preview_file(&mut self, preview: Rc<dyn Fn(&str, &mut Window, &mut App)>) {
        self.preview_file = Some(preview);
    }

    pub(crate) fn set_wakes(&mut self, wakes: Option<crate::wakes::Status>, computer: String, now: Option<f64>, cx: &mut Context<Self>) {
        self.bars.update(cx, |bars, cx| bars.set_wakes(wakes, computer, now, cx));
    }

    pub(crate) fn tick_wakes(&self, cx: &mut Context<Self>) {
        self.bars.update(cx, |bars, cx| bars.tick_wakes(cx));
    }

    #[cfg(feature = "stand-in")]
    pub fn open_wake_schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bars.update(cx, |bars, cx| bars.open_activity_with_wakes(true, window, cx));
    }

    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    /// The activity control for this conversation.
    pub fn bars(&self) -> &Entity<WorkBars> {
        &self.bars
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn list(&self) -> &ChatListState {
        &self.list
    }

    pub fn selection(&self) -> &SelectionState {
        &self.selection
    }

    /// What's selected, as it's copied (empty: nothing is).
    pub fn selected_text(&self) -> String {
        let Some((start, end)) = self.selection.range() else { return String::new() };
        let order = self.selection.order();
        let rows = self.entries.iter().filter(|entry| {
            let at = selection::Spot::row_start(entry.key().to_owned());
            order.cmp(&at, &selection::Spot::row_start(start.unit.row.clone())) != Some(std::cmp::Ordering::Less)
                && order.cmp(&at, &selection::Spot::row_start(end.unit.row.clone())) != Some(std::cmp::Ordering::Greater)
        });
        selection::copy_text(&order, rows.map(|entry| (SharedString::from(entry.key().to_owned()), entries::units(entry, &self.open))), &start, &end)
    }

    /// The rows' text as copying sees it (for shots and tests to select in).
    pub fn units(&self) -> Vec<(SharedString, Vec<selection::Unit>)> {
        self.entries.iter().map(|entry| (SharedString::from(entry.key().to_owned()), entries::units(entry, &self.open))).collect()
    }

    /// Selects from the first `from` to the end of the first `to` after it (for shots and tests).
    pub fn select_text(&mut self, from: &str, to: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let found = self.selection.select_text(&self.units(), from, to);
        window.focus(&self.transcript, cx);
        cx.notify();
        found
    }

    /// The row whose Copy was just used (for tests).
    pub fn copied_row(&self) -> Option<String> {
        self.copied.as_ref().map(|(key, _)| key.clone())
    }

    /// Copies message `key`'s text (as Markdown) from the row's Copy: a check shows for two seconds.
    fn copy_message(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((text, _)) = self.entries.iter().find(|entry| entry.key() == key).and_then(entries::copyable) else { return };
        selection::copy_to_clipboard(text, cx);
        let clicks = self.copied.as_ref().map_or(0, |(_, clicks)| *clicks) + 1;
        self.copied = Some((key.to_owned(), clicks));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                if this.copied.as_ref().is_some_and(|(_, latest)| *latest == clicks) {
                    this.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Replies to message `key` whole, from the row's Quote: the same quote as the menu's Reply.
    fn quote_message(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.entries.iter().position(|entry| entry.key() == key) else { return };
        let text = self.message_text(ix);
        self.quote(&text, window, cx);
    }

    /// Adds what's selected to the composer as a quote (the menu's Add to Chat), and lets go of the
    /// selection.
    fn add_selection_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.selected_text();
        if text.is_empty() {
            return;
        }
        self.quote(&text, window, cx);
        self.selection.clear();
        cx.notify();
    }

    /// The "Add to Chat" card over the end of a selection just finished (the button came up on it):
    /// above it, flipping at the window's edges by the app's one rule. It takes no keyboard, so
    /// ⌘C still copies; it goes with the selection, a press elsewhere, Escape, or its use.
    fn selection_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let area = self.selection.card_anchor()?;
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let card = div()
            .id("add-to-chat-card")
            .debug_selector(|| "add-to-chat-card".into())
            .occlude()
            .p(space::S1)
            .rounded(radius::NAV)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            // (A press here is the card's: it neither starts a selection nor takes the keyboard.)
            .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(Button::new("add-to-chat", "Add to Chat").icon(IconName::Quote).ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, window, cx| this.add_selection_to_chat(window, cx))));
        Some(Popover::from_area("add-to-chat-motion", area, Placement::above(), card).into_any_element())
    }

    fn copy(&mut self, _: &selection::Copy, _: &mut Window, cx: &mut Context<Self>) {
        selection::copy_to_clipboard(self.selected_text(), cx);
    }

    fn select_all(&mut self, _: &selection::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        // From the first row with text to the last (not "Working…", which goes when the turn ends).
        let rows: Vec<&str> = self.entries.iter().filter(|entry| !entries::units(entry, &self.open).is_empty()).map(Entry::key).collect();
        if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
            self.selection.select_all(first, last);
            cx.notify();
        }
    }

    /// What a right-click in the messages offers: the link's address (on a link), Copy, Select All.
    fn transcript_menu(view: &Entity<ConversationView>, window: &mut Window, cx: &mut App) -> Entity<Menu> {
        let selection = view.read(cx).selection.clone();
        let link = selection.link_at(window.mouse_position());
        let has_selection = !selection.is_empty();
        // (Nothing selected: the message under the pointer is what's quoted.)
        let under = if has_selection { None } else { view.read(cx).message_at(window.mouse_position()) };
        let quotes = has_selection || under.is_some();
        let (copy_view, all_view, quote_view) = (view.clone(), view.clone(), view.clone());
        cx.new(|cx| {
            let mut menu = Menu::new(cx);
            if let Some(link) = link {
                menu = menu.entry(MenuEntry::new("Copy Link").on_select(move |_, cx| selection::copy_to_clipboard(link.clone(), cx))).separator();
            }
            menu.entry(MenuEntry::new("Copy").shortcut(selection::COPY_KEYS).disabled(!has_selection).on_select(move |_, cx| {
                let text = copy_view.read(cx).selected_text();
                selection::copy_to_clipboard(text, cx);
            }))
            .entry(MenuEntry::new(if has_selection { "Add to Chat" } else { "Reply" }).disabled(!quotes).on_select(move |window, cx| {
                let text = match under {
                    Some(ix) => quote_view.read(cx).message_text(ix),
                    None => quote_view.read(cx).selected_text(),
                };
                quote_view.update(cx, |this, cx| this.quote(&text, window, cx));
            }))
            .entry(MenuEntry::new("Select All").shortcut(selection::SELECT_ALL_KEYS).on_select(move |window, cx| {
                all_view.update(cx, |this, cx| this.select_all(&selection::SelectAll, window, cx));
            }))
        })
    }

    /// Replies to `text`: it goes into the composer as a quote, the keyboard there to answer it.
    pub fn quote(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| composer.quote(text, cx));
        // (Into its answer, under it, as 0.7's.)
        let composer = self.composer.read(cx);
        let focus = composer.answer_focus(cx).unwrap_or_else(|| composer.focus_handle(cx));
        window.focus(&focus, cx);
    }

    /// The message row at `point` in the window (not the way back, nor the run under way).
    pub fn message_at(&self, point: gpui::Point<gpui::Pixels>) -> Option<usize> {
        // (By the text drawn there, as a selection finds it.)
        let row = self.selection.spot_at(point)?.unit.row;
        self.entries.iter().position(|entry| entry.key() == row.as_ref() && matches!(entry, Entry::Message(_) | Entry::Outgoing { .. }) && !entries::units(entry, &self.open).is_empty())
    }

    /// Row `ix`'s words as a quote takes them: its text as copying sees it, the parts apart.
    pub fn message_text(&self, ix: usize) -> String {
        self.entries.get(ix).map(|entry| entries::units(entry, &self.open).into_iter().map(|unit| unit.text.to_string()).collect::<Vec<_>>().join("\n\n")).unwrap_or_default()
    }

    /// Why the page before couldn't be read, while that's said.
    pub fn earlier_failed(&self) -> Option<&str> {
        self.earlier_failed.as_deref()
    }

    pub fn problem(&self) -> Option<&Problem> {
        self.problem.as_ref()
    }

    /// Does what the box offered beside why it couldn't be read (Try Again: its coding agents
    /// started again). Once they run, the line goes and it's read again on its own; still not,
    /// the box's words as it says them now.
    pub fn take_action(&mut self, action: Offered, cx: &mut Context<Self>) {
        if self.taking_action {
            return;
        }
        self.taking_action = true;
        cx.notify();
        let client = self.client.clone();
        let done = move |this: &mut ConversationView, done: kuvo_box::Result<()>, cx: &mut Context<ConversationView>| {
            this.taking_action = false;
            match done {
                Ok(()) => {
                    if matches!(this.problem, Some(Problem::Failed(..))) {
                        this.problem = None;
                    }
                }
                Err(error) => this.take(ConversationUpdate::Failed(error), cx),
            }
            cx.notify();
        };
        if !self.live {
            let answer = client.take_action(&action);
            done(self, answer, cx);
            return;
        }
        self.tasks.push(cx.spawn(async move |this, cx| {
            let answer = cx.background_executor().spawn(async move { client.take_action(&action) }).await;
            this.update(cx, |this, cx| done(this, answer, cx)).ok();
        }));
    }

    /// Its line's button, where the box offers one: busy while it goes.
    fn action_button(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let Some(Problem::Failed(_, Some(action))) = &self.problem else { return None };
        let (action, label) = (action.clone(), action.label.clone());
        let view = cx.entity().downgrade();
        Some(
            div()
                .debug_selector(|| "conversation-problem-action".into())
                .flex_none()
                .child(Button::new("conversation-problem-action", label).size(ButtonSize::Small).disabled(self.taking_action).on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.take_action(action.clone(), cx)).ok();
                }))
                .into_any_element(),
        )
    }

    /// What waits its turn, as the box says it. One of yours on its way that it holds for after the
    /// run (Stop held it, or it was queued) shows there from now on, not in the conversation.
    pub fn take_queue(&mut self, queue: Queue, cx: &mut Context<Self>) {
        let held: HashSet<String> = queue.items.iter().flatten().filter(|item| item.held || item.delivery != "steer").map(|item| item.id.clone()).collect();
        self.queue = queue;
        self.queue_failed = None;
        let has_queue = !self.queued().is_empty();
        self.composer.update(cx, |composer, cx| composer.set_has_queue(has_queue, cx));
        if !held.is_empty() {
            self.composer.update(cx, |composer, cx| composer.arrived(|id| held.contains(id).then_some(true), cx));
        }
        cx.notify();
    }

    /// What waits its turn as it's drawn over the composer: not yours on its way to the agent's
    /// next step, which the conversation shows.
    fn queued_shown(&self, cx: &App) -> Vec<Item> {
        let outgoing = self.composer.read(cx).outgoing();
        self.queued().iter().filter(|item| item.held || item.delivery != "steer" || !outgoing.iter().any(|sent| sent.id == item.id)).cloned().collect()
    }

    /// The rows of the box's conversation (`rows`), with yours on their way after them (before the
    /// run under way, which stays last).
    fn with_outgoing(&self, mut rows: Vec<Entry>, mut revisions: Vec<u64>, cx: &App) -> (Vec<Entry>, Vec<u64>) {
        let at = if matches!(rows.last(), Some(Entry::Working { .. })) { rows.len() - 1 } else { rows.len() };
        let running = self.running;
        // A send for after the run belongs to the queue unless the transcript confirms it went
        // straight through. Keep failed sends here so Retry and Edit remain available.
        let (outgoing, outgoing_revisions): (Vec<Entry>, Vec<u64>) = self.composer.read(cx).outgoing().iter()
            .filter(|outgoing| !outgoing.arrived && (!outgoing.is_queued() || outgoing.failed.is_some()))
            .map(|outgoing| Entry::outgoing(outgoing, running)).unzip();
        rows.splice(at..at, outgoing);
        revisions.splice(at..at, outgoing_revisions);
        (rows, revisions)
    }

    /// Yours on their way drawn again, as the composer says they changed.
    fn show_outgoing(&mut self, cx: &mut Context<Self>) {
        if !self.read {
            return;
        }
        let (rows, revisions): (Vec<Entry>, Vec<u64>) = self.entries.iter().zip(&self.revisions).filter(|(entry, _)| !matches!(entry, Entry::Outgoing { .. })).map(|(entry, revision)| (entry.clone(), *revision)).unzip();
        let (rows, revisions) = self.with_outgoing(rows, revisions, cx);
        self.show(rows, revisions);
        // (Yours on their way show the pictures they carry.)
        self.note_sent(cx);
        self.find_pictures();
        self.read_pictures(cx);
        cx.notify();
    }

    /// The pictures of yours sent from here whose copy came without them.
    fn note_sent(&mut self, cx: &App) {
        self.sent_pictures = self.composer.read(cx).outgoing().iter().filter(|outgoing| outgoing.arrived).map(|outgoing| (outgoing.id.clone(), kuvo_box::pictures::of_sending(&outgoing.id, &outgoing.files))).filter(|(_, pictures)| !pictures.is_empty()).collect();
    }

    pub fn queued(&self) -> &[Item] {
        self.queue.items.as_deref().unwrap_or_default()
    }

    /// Takes `item` out of the queue; `edit`: its words and files back in the composer, to change
    /// and send again.
    pub fn unqueue(&mut self, item: String, edit: bool, cx: &mut Context<Self>) {
        let (client, session) = (self.client.clone(), self.session.clone());
        let task = cx.background_executor().spawn(async move { client.remove_queued(&session, &item).map(|removed| (item, removed)) });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((item, removed)) => {
                        this.queue_failed = None;
                        if let Some(items) = this.queue.items.as_mut() {
                            items.retain(|each| each.id != item);
                        }
                        let has_queue = !this.queued().is_empty();
                        this.composer.update(cx, |composer, cx| composer.set_has_queue(has_queue, cx));
                        // (One of yours waiting in the conversation goes from it too.)
                        this.composer.update(cx, |composer, cx| composer.let_go(&item, cx));
                        if let (true, Some(removed)) = (edit, removed) {
                            this.composer.update(cx, |composer, cx| composer.restore(&removed.draft, &removed.files, cx));
                        }
                    }
                    Err(error) => this.queue_failed = Some(format!("It couldn’t be taken out of the queue: {error}")),
                }
                // (Read again at once where nothing follows it: tests and shots.)
                if !this.live
                    && let Ok(queue) = this.client.queue(&this.session)
                {
                    this.queue = queue;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Steer: a queued message to the agent at its next step, its run going on.
    pub fn steer_queued(&mut self, item: String, cx: &mut Context<Self>) {
        let (client, session) = (self.client.clone(), self.session.clone());
        self.queue_change(move || client.steer_queued(&session, &item), "It couldn’t be steered in", cx);
    }

    /// Send Now: a waiting message to the agent right away, the step under way stopped for it.
    pub fn send_queued_now(&mut self, item: String, cx: &mut Context<Self>) {
        let (client, session) = (self.client.clone(), self.session.clone());
        self.queue_change(move || client.send_queued_now(&session, &item), "It couldn’t be sent now", cx);
    }

    /// Resume: the queue held by a Stop goes on, its next message now.
    pub fn resume_queue(&mut self, cx: &mut Context<Self>) {
        let (client, session) = (self.client.clone(), self.session.clone());
        self.queue_change(move || client.resume_queue(&session), "The queue couldn’t be resumed", cx);
    }

    #[cfg(feature = "stand-in")]
    pub fn queue_drag_points(&self) -> (gpui::Point<gpui::Pixels>, gpui::Point<gpui::Pixels>) {
        let origin = self.queue_scroll.bounds().origin;
        (origin + gpui::point(px(18.), px(16.)), origin + gpui::point(px(18.), px(48.)))
    }

    fn commit_queue_move(&mut self, change: QueueMove, cx: &mut Context<Self>) {
        if self.queue_move_pending.is_some() { return; }
        self.queue_move_pending = Some(change.clone());
        let (client, session) = (self.client.clone(), self.session.clone());
        let task = cx.background_executor().spawn(async move {
            client.reorder_queue(&session, &change.item, &change.anchor, change.before)
        });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.queue_move_pending = None;
                match result {
                    Ok(queue) => this.take_queue(queue, cx),
                    Err(error) => this.queue_failed = Some(format!("The message couldn’t be moved: {}", kuvo_box::accounts::said(&error))),
                }
                cx.notify();
            }).ok();
        }));
        cx.notify();
    }

    fn cancel_queue_drag(&mut self, cx: &mut Context<Self>) {
        self.queue_dragging = None;
        self.queue_drag = None;
        cx.notify();
    }

    fn queue_move_at(&self, drag: &QueueDrag, position: gpui::Point<gpui::Pixels>, cx: &App) -> Option<QueueMove> {
        let bounds = self.queue_scroll.bounds();
        if !bounds.contains(&position) { return None; }
        let y = position.y - bounds.top() - self.queue_scroll.offset().y;
        let index = (f32::from(y) / QUEUE_ROW_HEIGHT).floor() as usize;
        let items = self.queued_shown(cx);
        let from = items.iter().position(|item| item.id == drag.item && item.can_reorder())?;
        let anchor = items.get(index)?;
        (from != index && anchor.can_reorder()).then(|| QueueMove {
            item: drag.item.clone(), anchor: anchor.id.clone(), before: index < from,
        })
    }

    fn drag_over_queue(&mut self, event: &gpui::DragMoveEvent<QueueDrag>, _: &mut Window, cx: &mut Context<Self>) {
        let drag = event.drag(cx);
        if drag.owner != cx.entity().entity_id() { return; }
        let change = self.queue_move_at(drag, event.event.position, cx);
        if self.queue_drag != change {
            self.queue_drag = change;
            self.queue_drag_motion += 1;
            cx.notify();
        }
    }

    fn keyboard_queue_move(&mut self, id: &str, up: bool, cx: &mut Context<Self>) {
        let items: Vec<_> = self.queued_shown(cx).into_iter().filter(Item::can_reorder).collect();
        let Some(at) = items.iter().position(|item| item.id == id) else { return };
        let neighbor = if up { at.checked_sub(1) } else { Some(at + 1) };
        if let Some(anchor) = neighbor.and_then(|index| items.get(index)) {
            self.commit_queue_move(QueueMove { item: id.into(), anchor: anchor.id.clone(), before: up }, cx);
        }
    }

    /// Asks the box to change the queue (`work`), off the UI thread; what the box makes of it comes
    /// with the queue's follow, and a refusal is said over the queue (`failed`, and the box's why).
    fn queue_change(&mut self, work: impl FnOnce() -> kuvo_box::Result<()> + Send + 'static, failed: &'static str, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move { work() });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                // (Read again at once where nothing follows it: tests and shots.)
                if !this.live
                    && let Ok(queue) = this.client.queue(&this.session)
                {
                    this.take_queue(queue, cx);
                }
                this.queue_failed = result.err().map(|error| format!("{failed}: {}", kuvo_box::accounts::said(&error)));
                cx.notify();
            })
            .ok();
        }));
    }

    /// The keyboard follows the card: an ask that takes the composer's place takes the keyboard from
    /// it as it comes (from the composer, or anywhere in the conversation), and gives it back to the
    /// composer as it goes; the card's keys still wait a moment before they answer.
    fn keep_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let card = self.asks.read(cx);
        let taking = card.is_asking() && !card.leaves_composer();
        if taking == self.card_took_keys {
            return;
        }
        self.card_took_keys = taking;
        // Coming, it takes the keyboard from anywhere in the conversation (or from nowhere); going,
        // it gives it back to the composer if it had it.
        let (has, to) = if taking {
            // (The composer asked for by name too: before the conversation's first drawing, its focus
            // doesn't know what's inside it yet.)
            let composer = self.composer.focus_handle(cx).is_focused(window);
            (composer || self.focus.contains_focused(window, cx) || window.focused(cx).is_none(), self.asks.focus_handle(cx))
        } else {
            (self.asks.read(cx).focus_handle_any(cx).contains_focused(window, cx), self.composer.focus_handle(cx))
        };
        if has {
            window.focus(&to, cx);
        }
    }

    pub fn asks(&self) -> &Entity<AskCard> {
        &self.asks
    }

    /// What the box says it waits on you for: a card in the composer's place while anything does,
    /// with the keyboard on it as it comes.
    pub fn take_asks(&mut self, asks: Asks, cx: &mut Context<Self>) {
        self.asks_failed = None;
        self.asks.update(cx, |card, cx| card.take(asks, cx));
        self.bars.update(cx, |bars, cx| bars.asks_changed(cx));
        // The step it waits on shows outside its group while it does: the newest group drawn again.
        if let Some(ix) = self.entries.iter().rposition(|entry| matches!(entry, Entry::Steps { .. })) {
            self.list.changed(ix);
        }
        cx.notify();
    }

    /// What the box's follow says, shown.
    pub fn take(&mut self, update: ConversationUpdate, cx: &mut Context<Self>) {
        match update {
            ConversationUpdate::Conversation(conversation) => {
                self.problem = None;
                // A page that couldn't be read is said until Try Again, or until there's another
                // way back (another page came), so a run streaming meanwhile doesn't ask again.
                if self.earlier != conversation.earlier {
                    self.earlier_failed = None;
                }
                let (running, place) = (conversation.running, conversation.place);
                // The box's models (or this conversation's model) changed since: read again.
                let models_changed = conversation.models_changed;
                let read_models = self.models_changed.is_some_and(|seen| seen != models_changed);
                self.models_changed = Some(models_changed);
                self.earlier = conversation.earlier.clone();
                self.read = true;
                // Yours on their way that it has now: its copy takes their place (keeping the files
                // sent with it shown, where its copy came without them).
                let has: HashMap<&str, bool> = conversation
                    .messages
                    .iter()
                    .map(|message| {
                        let carries = match message {
                            Message::User(user) => !user.files.is_empty() || !user.parts().paths.is_empty(),
                            _ => true,
                        };
                        (message.id(), carries)
                    })
                    .collect();
                self.composer.update(cx, |composer, cx| composer.arrived(|id| has.get(id).copied(), cx));
                self.running = running;
                let (rows, revisions) = entries::entries(conversation);
                let (rows, revisions) = self.with_outgoing(rows, revisions, cx);
                self.show(rows, revisions);
                self.note_sent(cx);
                self.find_pictures();
                self.read_pictures(cx);
                self.find_shown_files();
                self.read_shown_files(cx);
                self.composer.update(cx, |composer, cx| {
                    composer.set_running(running, cx);
                    composer.set_place(place, cx);
                    if read_models {
                        composer.read_models(cx);
                    }
                });
            }
            ConversationUpdate::Failed(error) => match error.refusal() {
                // Its coding agents restarting: not an error. What's shown stays (and a line about
                // them goes), as it's read again.
                Some(refusal) if refusal.agents_restarting() => {
                    if matches!(self.problem, Some(Problem::Failed(..))) {
                        self.problem = None;
                    }
                }
                // Else the box's words as they are, with what it offers to do about it.
                Some(refusal) => self.problem = Some(Problem::Failed(refusal.message.clone(), refusal.action.clone())),
                None => self.problem = Some(Problem::Failed(error.to_string(), None)),
            },
            ConversationUpdate::Gone(refusal) => {
                self.problem = Some(Problem::Gone(refusal.message));
                self.composer.update(cx, |composer, cx| composer.set_running(false, cx));
            }
            ConversationUpdate::EarlierFailed(error) => {
                self.earlier_failed = Some(error.to_string());
                if let Some(ix) = self.entries.iter().position(|entry| matches!(entry, Entry::Earlier)) {
                    self.list.changed(ix);
                }
            }
        }
        cx.notify();
    }

    /// The list made to show `new`, keeping the reader's place: rows put ahead (an earlier page)
    /// go above where they are, rows that changed are measured again, and new ones added at the
    /// end, which the list follows while the reader's at the bottom.
    fn show(&mut self, new: Vec<Entry>, revisions: Vec<u64>) {
        let old = std::mem::replace(&mut self.entries, new);
        let old_revisions = std::mem::replace(&mut self.revisions, revisions);
        self.selection.set_rows(self.entries.iter().map(Entry::key));
        let new = &self.entries;
        if old.is_empty() {
            self.list.reset(new.len());
            return;
        }
        let state = self.list.list_state().clone();
        let is_earlier = |entries: &[Entry]| matches!(entries.first(), Some(Entry::Earlier));
        // The rows at the end that aren't the box's messages: yours on their way, then the run
        // under way. They go as one and come again after what's added.
        let tail = |entries: &[Entry]| entries.iter().rev().take_while(|entry| matches!(entry, Entry::Working { .. } | Entry::Outgoing { .. })).count();
        let (old_tail, new_tail) = (tail(&old), tail(new));
        // Each message's row by its key and revision: nothing is compared deeper.
        fn messages<'a>(entries: &'a [Entry], revisions: &[u64]) -> Vec<(&'a str, u64)> {
            entries.iter().zip(revisions).filter(|(entry, _)| !matches!(entry, Entry::Earlier)).map(|(entry, revision)| (entry.key(), *revision)).collect()
        }
        let (old_messages, new_messages) = (messages(&old[..old.len() - old_tail], &old_revisions), messages(&new[..new.len() - new_tail], &self.revisions));
        // The first rows shown may have gone into one an earlier page put ahead of them: what the
        // agent did at the top of what was shown goes on a group the page started. They go, and
        // that group comes with the page.
        let gone = old_messages.iter().take_while(|(key, _)| !new_messages.iter().any(|(now, _)| now == key)).count();
        if gone > 0 && gone == old_messages.len() {
            self.list.reset(new.len());
            return;
        }
        let old_messages = &old_messages[gone..];
        // Where what was shown sits among what's shown now.
        let ahead = match old_messages.first() {
            None => 0,
            Some(first) => match new_messages.iter().position(|(key, _)| *key == first.0) {
                Some(ahead) => ahead,
                None => {
                    self.list.reset(new.len());
                    return;
                }
            },
        };
        let kept_in_order = new_messages.len() >= ahead + old_messages.len()
            && old_messages.iter().zip(&new_messages[ahead..]).all(|(was, now)| was.0 == now.0);
        if !kept_in_order {
            self.list.reset(new.len());
            return;
        }
        let added = new_messages.len() - ahead - old_messages.len();
        let mut had_earlier = is_earlier(&old);
        let same_tail = old_tail == new_tail && old[old.len() - old_tail..].iter().zip(&new[new.len() - new_tail..]).all(|(was, now)| was.key() == now.key());
        let mut had_tail = old_tail > 0;
        let mut count = old.len();
        // An earlier page: the way back goes from where it was and comes again above the page, so
        // what the reader had under it stays where it was (the anchor moves with it).
        if had_earlier && (!is_earlier(new) || ahead > 0) {
            state.splice(0..1, 0);
            had_earlier = false;
            count -= 1;
        }
        if gone > 0 {
            let offset = usize::from(had_earlier);
            state.splice(offset..offset + gone, 0);
            count -= gone;
        }
        if had_tail && (!same_tail || added > 0) {
            state.splice(count - old_tail..count, 0);
            had_tail = false;
        }
        let offset = usize::from(had_earlier);
        if ahead > 0 {
            state.splice(offset..offset, ahead);
        }
        for (n, (was, now)) in old_messages.iter().zip(&new_messages[ahead..]).enumerate() {
            if was.1 != now.1 {
                self.list.changed(offset + ahead + n);
            }
        }
        if added > 0 {
            self.list.push(added);
        }
        if !had_earlier && is_earlier(new) {
            state.splice(0..0, 1);
        }
        if !had_tail && new_tail > 0 {
            self.list.push(new_tail);
        } else if had_tail {
            let (from_old, from_new) = (old.len() - old_tail, new.len() - new_tail);
            for n in 0..new_tail {
                if old[from_old + n] != new[from_new + n] || old_revisions[from_old + n] != self.revisions[from_new + n] {
                    self.list.changed(from_new + n);
                }
            }
        }
        debug_assert_eq!(state.item_count(), self.entries.len());
    }

    /// The pictures a row shows: a message of yours carried, a step brought to show you or wrote
    /// in its words, and those a step opened (`open`) brought back for itself.
    fn pictures_of(entry: &Entry, open: &HashSet<Fold>, kept: &HashMap<String, Vec<Picture>>) -> Vec<Picture> {
        match entry {
            // Yours on its way: what you sent, from here.
            Entry::Outgoing { pictures, .. } => pictures.clone(),
            Entry::Message(message) => match &**message {
                // (Its copy came without its files: what you sent from here, meanwhile.)
                Message::User(user) => {
                    let carried = kuvo_box::pictures::of_user(user);
                    if carried.is_empty() { kept.get(&user.id).cloned().unwrap_or_default() } else { carried }
                }
                Message::Assistant(step) => step
                    .content
                    .iter()
                    .enumerate()
                    .flat_map(|(n, content)| match content {
                        Content::Tool { name, state, .. } => {
                            let at = format!("{}:{n}", step.id);
                            let mut all = kuvo_box::pictures::of_step(&at, name, state);
                            if open.contains(&Fold::Step(step.id.clone(), n)) {
                                all.extend(kuvo_box::pictures::of_step_own(&at, name, state));
                            }
                            all
                        }
                        Content::Text(text) => pictures::in_words(&step.id, n, text),
                        _ => Vec::new(),
                    })
                    .collect(),
                _ => Vec::new(),
            },
            Entry::Words { step, parts, .. } => parts
                .iter()
                .flat_map(|n| match &step.content[*n] {
                    Content::Text(text) => pictures::in_words(&step.id, *n, text),
                    _ => Vec::new(),
                })
                .collect(),
            Entry::Steps { items, .. } => items
                .iter()
                .flat_map(|(step, n)| match &step.content[*n] {
                    Content::Tool { name, state, .. } => {
                        let at = format!("{}:{n}", step.id);
                        let mut all = kuvo_box::pictures::of_step(&at, name, state);
                        if open.contains(&Fold::Step(step.id.clone(), *n)) {
                            all.extend(kuvo_box::pictures::of_step_own(&at, name, state));
                        }
                        all
                    }
                    _ => Vec::new(),
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Every picture it shows, in order: what one opened goes among.
    pub fn all_pictures(&self) -> Vec<Picture> {
        self.entries.iter().filter_map(|entry| self.row_pictures.get(entry.key())).flat_map(|(_, _, pictures)| pictures.iter().cloned()).collect()
    }

    /// Each row's pictures found again where it changed (or a step of it was opened or closed).
    fn find_pictures(&mut self) {
        let mut found = HashMap::new();
        for (entry, revision) in self.entries.iter().zip(&self.revisions) {
            if !matches!(entry, Entry::Message(_) | Entry::Words { .. } | Entry::Steps { .. } | Entry::Outgoing { .. }) {
                continue;
            }
            let key = entry.key().to_owned();
            // (Which of its steps are opened, as a step opened shows what it brought back.)
            let opened: Vec<(String, usize)> = self.open.iter().filter_map(|fold| match fold {
                Fold::Step(step, n) if entry.holds(fold) => Some((step.clone(), *n)),
                _ => None,
            }).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            let kept = self.row_pictures.remove(&key).filter(|(was, folds, _)| was == revision && *folds == opened);
            let row = kept.unwrap_or_else(|| (*revision, opened, Self::pictures_of(entry, &self.open, &self.sent_pictures)));
            found.insert(key, row);
        }
        self.row_pictures = found;
    }

    /// The rows showing picture `id`, drawn again (it was read, or is read again).
    fn picture_changed(&self, id: &str) {
        for (ix, entry) in self.entries.iter().enumerate() {
            if self.row_pictures.get(entry.key()).is_some_and(|(_, _, pictures)| pictures.iter().any(|picture| picture.id == id)) {
                self.list.changed(ix);
            }
        }
    }

    /// Reads the pictures shown that aren't read yet, each once, off the UI thread: from the
    /// message, or from the computer the conversation runs on. Their rows are drawn again as they come.
    fn read_pictures(&mut self, cx: &mut Context<Self>) {
        for picture in self.all_pictures() {
            let key = picture.id.clone();
            let moved = self.read_from.get(&key).is_some_and(|was| !pictures::same_place(was, &picture.source));
            if self.pictures.contains_key(&key) && !moved {
                continue;
            }
            // (One read from elsewhere before stays shown until it's read from where it is now.)
            if !moved {
                self.pictures.insert(key.clone(), Read::Reading);
            }
            self.read_from.insert(key.clone(), picture.source.clone());
            let (client, source) = (self.client.clone(), picture.source.clone());
            let read_source = source.clone();
            let task = cx.background_executor().spawn(async move { pictures::read(&client, &read_source) });
            self.tasks.retain(|task| !task.is_ready());
            self.tasks.push(cx.spawn(async move |this, cx| {
                let read = task.await;
                this.update(cx, |this, cx| {
                    // (Unless it has moved again since; and one shown stays, if where it is now can't give it.)
                    let keeps = matches!(read, Read::Failed { .. } | Read::Missing) && matches!(this.pictures.get(&key), Some(Read::Shown { .. }));
                    if !keeps && this.read_from.get(&key).is_some_and(|now| pictures::same_place(now, &source)) {
                        this.pictures.insert(key.clone(), read);
                        this.picture_changed(&key);
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
    }

    /// Reads a picture that couldn't be read again (Try Again), by its source's key.
    pub fn read_picture_again(&mut self, key: &str, cx: &mut Context<Self>) {
        if matches!(self.pictures.get(key), Some(Read::Failed { .. } | Read::Missing)) {
            self.pictures.remove(key);
            self.read_pictures(cx);
            self.picture_changed(key);
            cx.notify();
        }
    }

    /// Completed `kuvo-browser_show_file` calls in `entry`, extracted via `kuvo_box::shown_files::of_step`.
    fn shown_files_of(&self, entry: &Entry) -> Vec<kuvo_box::shown_files::ShownFile> {
        let computer = self.computer_key.as_deref();
        match entry {
            Entry::Message(message) => match &**message {
                Message::Assistant(step) => kuvo_box::shown_files::of_step(step)
                    .into_iter()
                    .map(|file| file.in_conversation(&self.session, computer))
                    .collect(),
                _ => Vec::new(),
            },
            Entry::Steps { items, .. } => {
                // One Step can be split across multiple Entry::Steps rows; extract once per step
                // and keep only the items that belong to this row, in row order.
                let mut by_step: HashMap<&str, HashMap<String, kuvo_box::shown_files::ShownFile>> = HashMap::new();
                for (step, _) in items {
                    by_step.entry(step.id.as_str()).or_insert_with(|| {
                        kuvo_box::shown_files::of_step(step)
                            .into_iter()
                            .map(|file| (file.id.clone(), file.in_conversation(&self.session, computer)))
                            .collect()
                    });
                }
                items
                    .iter()
                    .filter_map(|(step, n)| {
                        let id = format!("{}:{n}-shown", step.id);
                        by_step.get(step.id.as_str())?.get(&id).cloned()
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Extracts each row's shown files whenever its revision changes.
    fn find_shown_files(&mut self) {
        let mut found = HashMap::new();
        for (entry, revision) in self.entries.iter().zip(&self.revisions) {
            if !matches!(entry, Entry::Message(_) | Entry::Steps { .. }) {
                continue;
            }
            let key = entry.key().to_owned();
            let kept = self.row_shown_files.remove(&key).filter(|(was, _)| was == revision);
            let row = kept.unwrap_or_else(|| (*revision, self.shown_files_of(entry)));
            found.insert(key, row);
        }
        self.row_shown_files = found;
    }

    /// Marks rows containing shown file `id` dirty so the chat list re-renders them.
    fn shown_file_changed(&self, id: &str) {
        for (ix, entry) in self.entries.iter().enumerate() {
            if self
                .row_shown_files
                .get(entry.key())
                .is_some_and(|(_, files)| files.iter().any(|file| file.id == id))
            {
                self.list.changed(ix);
            }
        }
    }

    /// Fetches `file_info` metadata off the UI thread for non-picture shown files not yet fetched.
    /// (Picture shown files are fetched by `read_pictures` via `file_bytes`.)
    fn read_shown_files(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<(String, String)> = self
            .entries
            .iter()
            .filter_map(|entry| self.row_shown_files.get(entry.key()))
            .flat_map(|(_, files)| files.iter())
            .filter(|file| file.kind != kuvo_box::shown_files::FileKind::Picture)
            .filter(|file| !self.shown_info.contains_key(&file.id))
            .map(|file| (file.id.clone(), file.uri.clone()))
            .collect();
        for (id, uri) in pending {
            self.shown_info.insert(id.clone(), crate::shown_files::InfoRead::Loading);
            let client = self.client.clone();
            let task = cx.background_executor().spawn(async move { crate::shown_files::read_info(&client, &uri) });
            self.tasks.retain(|task| !task.is_ready());
            self.tasks.push(cx.spawn(async move |this, cx| {
                let info = task.await;
                this.update(cx, |this, cx| {
                    this.shown_info.insert(id.clone(), info);
                    this.shown_file_changed(&id);
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    /// Opens shown file `id` via `open_shown::open_shown_file` off the UI thread.
    pub fn open_shown_file(&mut self, id: &str, cx: &mut Context<Self>) {
        if matches!(self.shown_open.get(id), Some(crate::shown_files::OpenState::Opening)) {
            return;
        }
        let Some(shown) = self
            .row_shown_files
            .values()
            .flat_map(|(_, files)| files.iter())
            .find(|file| file.id == id)
            .cloned()
        else {
            return;
        };
        let shown = shown.in_conversation(&self.session, self.computer_key.as_deref());
        let id = id.to_owned();
        self.shown_open.insert(id.clone(), crate::shown_files::OpenState::Opening);
        self.shown_file_changed(&id);
        cx.notify();
        let client = self.client.clone();
        let task = cx.background_executor().spawn(async move { crate::open_shown::open_shown_file(&client, &shown) });
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.shown_open.remove(&id);
                    }
                    Err(crate::open_shown::OpenShownError::Box(kuvo_box::Error::NotFound)) => {
                        this.shown_info.insert(id.clone(), crate::shown_files::InfoRead::Missing);
                        this.shown_open.insert(id.clone(), crate::shown_files::OpenState::Missing);
                    }
                    Err(error) => {
                        this.shown_open.insert(id.clone(), crate::shown_files::OpenState::Failed(error.to_string()));
                    }
                }
                this.shown_file_changed(&id);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Retries a failed shown-file fetch (either picture bytes or `file_info`).
    pub fn retry_shown_file(&mut self, id: &str, cx: &mut Context<Self>) {
        if matches!(self.pictures.get(id), Some(Read::Failed { .. })) {
            self.read_picture_again(id, cx);
        }
        if matches!(self.shown_info.get(id), Some(crate::shown_files::InfoRead::Failed(_))) {
            self.shown_info.remove(id);
            self.shown_open.remove(id);
            self.read_shown_files(cx);
            self.shown_file_changed(id);
            cx.notify();
        }
    }

    /// Opens picture `id` over the conversation.
    pub fn open_picture(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.viewing = Some(id.to_owned());
        window.focus(&self.viewer_focus, cx);
        cx.notify();
    }

    /// The picture opened, if any.
    pub fn viewing(&self) -> Option<&str> {
        self.viewing.as_deref()
    }

    fn close_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.viewing = None;
        let focus = self.composer.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// The one before (`by` -1) or after (1) the picture opened, among the conversation's.
    fn step_picture(&mut self, by: isize, cx: &mut Context<Self>) {
        let all = self.all_pictures();
        let Some(at) = self.viewing.as_ref().and_then(|id| all.iter().position(|picture| &picture.id == id)) else { return };
        let to = at as isize + by;
        if let Some(next) = usize::try_from(to).ok().and_then(|to| all.get(to)) {
            self.viewing = Some(next.id.clone());
            cx.notify();
        }
    }

    /// The picture opened, over the whole conversation.
    fn picture_viewer(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.viewing.as_ref()?;
        let all = self.all_pictures();
        let at = all.iter().position(|picture| &picture.id == id)?;
        let picture = &all[at];
        Some(
            div()
                .id("picture-viewer-layer")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .track_focus(&self.viewer_focus)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| match event.keystroke.key.as_str() {
                    "escape" => this.close_picture(window, cx),
                    "left" => this.step_picture(-1, cx),
                    "right" => this.step_picture(1, cx),
                    _ => {}
                }))
                .on_click(cx.listener(|this, _, window, cx| this.close_picture(window, cx)))
                .child(pictures::viewer(picture, at, all.len(), self.pictures.get(picture.id.as_str()), window))
                .child(
                    div()
                        .absolute()
                        .top(space::S4)
                        .right(space::S4)
                        .debug_selector(|| "picture-close".into())
                        .child(Button::new("picture-close", "Close").size(ButtonSize::Small).on_click(cx.listener(|this, _, window, cx| this.close_picture(window, cx)))),
                )
                .into_any_element(),
        )
    }

    /// Opens or closes a thinking block or a tool step, or folds an output again.
    pub fn toggle(&mut self, fold: Fold, cx: &mut Context<Self>) {
        if !self.open.remove(&fold) {
            self.open.insert(fold.clone());
        }
        if let Some(ix) = self.entries.iter().position(|entry| entry.holds(&fold)) {
            self.list.changed(ix);
        }
        // (A step opened shows what it brought back for itself.)
        self.find_pictures();
        self.read_pictures(cx);
        cx.notify();
    }

    /// Opens step `n` of `step` (a tool step, to what it printed or changed), and the group it's in.
    pub fn open_step(&mut self, step: &str, n: usize, cx: &mut Context<Self>) {
        let fold = Fold::Step(step.to_owned(), n);
        if let Some(ix) = self.entries.iter().position(|entry| entry.holds(&fold)) {
            if let Entry::Steps { key, .. } = &self.entries[ix] {
                self.open.insert(Fold::Group(key.clone()));
            }
            self.list.changed(ix);
        }
        self.open.insert(fold);
        self.find_pictures();
        self.read_pictures(cx);
        cx.notify();
    }

    /// Asks for the page that couldn't be read, again.
    fn try_earlier_again(&mut self, cx: &mut Context<Self>) {
        self.earlier_failed = None;
        if let Some(follow) = &self.follow {
            follow.load_earlier();
        }
        if let Some(ix) = self.entries.iter().position(|entry| matches!(entry, Entry::Earlier)) {
            self.list.changed(ix);
        }
        cx.notify();
    }

    /// Under your message whose box's copy came without the files you sent with it: their names,
    /// until a copy with its own comes (never dropped from sight in between).
    fn files_kept(&self, id: &str, cx: &App) -> AnyElement {
        let composer = self.composer.read(cx);
        let Some(kept) = composer.outgoing().iter().find(|outgoing| outgoing.arrived && outgoing.id == id) else { return div().into_any_element() };
        let c = &cx.theme().colors;
        let selector = format!("kept-files-{id}");
        div()
            .debug_selector(move || selector)
            .pt(space::S1_5)
            .flex()
            .flex_wrap()
            .justify_end()
            .gap(space::S1)
            // (Its pictures show as pictures, from here.)
            .children(kept.files.iter().filter(|path| !kuvo_box::pictures::is_picture_path(&path.to_string_lossy())).map(|path| {
                let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                div().flex().items_center().gap(space::S1).px(space::S2).py(space::S0_5).rounded(radius::NAV).bg(c.sunken).text_ui(TypeSize::Caption).text_color(c.text_muted).child(Icon::new(IconName::File).size(IconSize::Nav).color(c.text_faint)).child(name)
            }))
            .into_any_element()
    }

    /// Under one of yours waiting for the agent's next step (its box holds it): Edit takes it back to
    /// the composer, Remove takes it back for good, as the queue offers them.
    fn waiting_actions(&self, id: &str, view: &Entity<ConversationView>) -> AnyElement {
        let (edit, remove) = (view.clone(), view.clone());
        let (edit_id, remove_id) = (id.to_owned(), id.to_owned());
        let (edit_selector, remove_selector) = (format!("edit-waiting-{id}"), format!("remove-waiting-{id}"));
        div()
            .flex()
            .gap(space::S1)
            .child(div().debug_selector(move || edit_selector).child(Button::new(SharedString::from(format!("edit-waiting-{id}")), "Edit").ghost().size(ButtonSize::Small).on_click(move |_, _, cx| {
                edit.update(cx, |this, cx| this.unqueue(edit_id.clone(), true, cx));
            })))
            .child(div().debug_selector(move || remove_selector).child(Button::new(SharedString::from(format!("remove-waiting-{id}")), "Remove").ghost().size(ButtonSize::Small).on_click(move |_, _, cx| {
                remove.update(cx, |this, cx| this.unqueue(remove_id.clone(), false, cx));
            })))
            .into_any_element()
    }

    /// Under one of yours that couldn't go: Retry sends it again, Edit puts it back in the composer.
    fn unsent_actions(&self, id: &str, cx: &App) -> AnyElement {
        let sending = self.composer.read(cx).is_sending();
        let (retry, edit) = (self.composer.clone(), self.composer.clone());
        let (retry_id, edit_id) = (id.to_owned(), id.to_owned());
        let (retry_selector, edit_selector) = (format!("retry-{id}"), format!("edit-unsent-{id}"));
        div()
            .flex()
            .gap(space::S1)
            .child(div().debug_selector(move || retry_selector).child(Button::new(SharedString::from(format!("retry-{id}")), "Retry").size(ButtonSize::Small).disabled(sending).on_click(move |_, _, cx| {
                retry.update(cx, |composer, cx| composer.retry(&retry_id, cx));
            })))
            .child(div().debug_selector(move || edit_selector).child(Button::new(SharedString::from(format!("edit-unsent-{id}")), "Edit").ghost().size(ButtonSize::Small).on_click(move |_, window, cx| {
                edit.update(cx, |composer, cx| composer.edit_unsent(&edit_id, cx));
                let focus = edit.focus_handle(cx);
                window.focus(&focus, cx);
            })))
            .into_any_element()
    }

    /// The top row, while there are earlier messages: they're read as it comes into view.
    fn earlier_row(&self, cx_view: &Entity<ConversationView>, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        if let Some(error) = &self.earlier_failed {
            let view = cx_view.clone();
            return div()
                .flex()
                .flex_col()
                .items_center()
                .gap(space::S2)
                .text_ui(TypeSize::Caption)
                .text_color(c.text_muted)
                .child(SharedString::from(format!("Earlier messages couldn’t be read. {error}")))
                .child(Button::new("earlier-again", "Try Again").size(ButtonSize::Small).on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.try_earlier_again(cx));
                }))
                .into_any_element();
        }
        // Asked once for each way back, as the row first shows.
        let cursor = self.earlier.clone();
        if let (Some(follow), Some(cursor)) = (&self.follow, cursor) {
            let mut asked = self.asked_earlier.borrow_mut();
            if asked.as_deref() != Some(cursor.as_str()) {
                *asked = Some(cursor);
                follow.load_earlier();
            }
        }
        div()
            .flex()
            .justify_center()
            .items_center()
            .gap(space::S1_5)
            .text_ui(TypeSize::Caption)
            .text_color(c.text_faint)
            .child(spinner("earlier", IconSize::Nav, c.text_faint))
            .child("Loading earlier messages…")
            .into_any_element()
    }
}

impl ConversationView {
    /// Why what waits waits, as its box says it (its engine being updated), for tests.
    pub fn queue_line(&self) -> Option<String> {
        self.queue.line.clone()
    }

    /// What waits its turn, over the composer, in the order it goes: each message as you wrote it
    /// (who it's from, when it isn't yours), with what can be done with it as 0.7 offered it: Steer
    /// (into the run under way) or Send, Send Now for one already for the agent's next step, and
    /// Edit (yours) and Remove under the pointer; Resume while a Stop holds it.
    fn queue_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mut items = self.queued_shown(cx);
        if items.is_empty() {
            return None;
        }
        let c = cx.theme().colors.clone();
        let running = self.running;
        let paused = self.queue.paused && !running;
        let can_drag = self.queue_move_pending.is_none() && items.iter().filter(|item| item.can_reorder()).take(2).count() == 2;
        let mut previous_slots = HashMap::new();
        let change = self.queue_drag.as_ref().or(self.queue_move_pending.as_ref());
        if let Some(change) = change {
            let mut movable: Vec<_> = items.iter().filter(|item| item.can_reorder()).cloned().collect();
            if let (Some(from), Some(to)) = (
                movable.iter().position(|item| item.id == change.item),
                movable.iter().position(|item| item.id == change.anchor),
            ) {
                if self.queue_drag.is_some() {
                    previous_slots.extend(items.iter().enumerate().map(|(index, item)| (item.id.clone(), index)));
                }
                let item = movable.remove(from);
                let to = to - usize::from(from < to) + usize::from(!change.before);
                movable.insert(to, item);
                for (row, moved) in items.iter_mut().filter(|item| item.can_reorder()).zip(movable) {
                    *row = moved;
                }
            }
        }
        let rows = items.iter().enumerate().map(|(index, item)| {
            let (remove_id, go_id) = (item.id.clone(), item.id.clone());
            let selector = format!("queued-{}", item.id);
            // Where it stands: for after the run, or for the agent's next step.
            let steered = !item.is_queued() && !item.is_on_box();
            let mark = Icon::new(if steered { IconName::ArrowRight } else { IconName::QueuedMessage }).size(IconSize::Nav).color(c.text_faint);
            let lead: AnyElement = match &item.from {
                Some(from) => div().flex().flex_none().items_center().gap(space::S1_5).child(mark).child(div().max_w(px(100.)).truncate().text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child(from.clone())).into_any_element(),
                None => mark.into_any_element(),
            };
            // Its way on: Steer while the run goes on (Send with none); one already for the agent's
            // next step says so, with Send Now; one held on the box goes as Send Now sends it.
            let (go_label, go_steers) = match (running, item.is_on_box(), steered) {
                (true, false, false) => ("Steer", true),
                (true, _, _) => ("Send Now", false),
                (false, _, _) => ("Send", false),
            };
            let go_selector = format!("go-{}", item.id);
            let go = div().debug_selector(move || go_selector).child(
                Button::new(SharedString::from(format!("go-{}", item.id)), go_label)
                    .ghost()
                    .quiet()
                    .when(go_steers, |button| button.icon(IconName::Steer).icon_size(IconSize::Nav))
                    .size(ButtonSize::Small)
                    .tooltip(match go_label {
                        "Steer" => "Steer: the agent takes this at its next step, without waiting for the run to end",
                        "Send Now" => "Send Now: stops the agent's current step and gives it this",
                        _ => "Send it now",
                    })
                    .on_click(cx.listener(move |this, _, _, cx| if go_steers { this.steer_queued(go_id.clone(), cx) } else { this.send_queued_now(go_id.clone(), cx) })),
            );
            let next_step = (running && steered).then(|| {
                let selector = format!("next-step-{}", item.id);
                div().debug_selector(move || selector).flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child("At its next step")
            });
            // A right-click offers what 0.7's did: Steer (while it runs), Send Now, Edit (yours), Remove.
            let view = cx.entity();
            let (menu_item, yours, steers) = (item.id.clone(), item.is_yours(), running && item.is_queued() && !item.is_on_box());
            let menu = move |_: &mut Window, cx: &mut App| {
                let (steer, now, edit, remove) = (view.clone(), view.clone(), view.clone(), view.clone());
                let (a, b, e, r) = (menu_item.clone(), menu_item.clone(), menu_item.clone(), menu_item.clone());
                cx.new(|cx| {
                    let mut menu = Menu::new(cx);
                    if steers {
                        menu = menu.entry(MenuEntry::new("Steer").on_select(move |_, cx| steer.update(cx, |this, cx| this.steer_queued(a.clone(), cx))));
                    }
                    menu = menu.entry(MenuEntry::new("Send Now").on_select(move |_, cx| now.update(cx, |this, cx| this.send_queued_now(b.clone(), cx))));
                    if yours {
                        menu = menu.entry(MenuEntry::new("Edit").on_select(move |_, cx| edit.update(cx, |this, cx| this.unqueue(e.clone(), true, cx))));
                    }
                    menu.separator().entry(MenuEntry::new("Remove").destructive().on_select(move |_, cx| remove.update(cx, |this, cx| this.unqueue(r.clone(), false, cx))))
                })
            };
            let more = format!("queue-more-{}", item.id);
            let draggable = can_drag && item.can_reorder();
            let dragging_here = self.queue_dragging.as_deref() == Some(item.id.as_str());
            let row_id: SharedString = format!("queued-{}", item.id).into();
            let keyboard = item.id.clone();
            let drag = QueueDrag {
                owner: cx.entity().entity_id(), item: item.id.clone(),
                words: item.words.lines().next().unwrap_or("").to_owned(), action: go_label,
            };
            let owner = cx.entity().downgrade();
            let row = div()
                .id(row_id.clone())
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(space::S2)
                .h(px(QUEUE_ROW_HEIGHT))
                .px(space::S2)
                .rounded(radius::NAV)
                .hover(|s| s.bg(c.hover))
                .when(item.can_reorder(), |d| d.tab_index(0).focus_visible(|d| d.bg(c.hover)))
                .when(draggable, |d| d
                    .cursor_grab()
                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                        let key = event.keystroke.key.as_str();
                        if event.keystroke.modifiers.alt && matches!(key, "up" | "down") {
                            this.keyboard_queue_move(&keyboard, key == "up", cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_drag(drag, move |drag, offset, _, cx| {
                        let row = owner.update(cx, |this, cx| {
                            this.queue_dragging = Some(drag.item.clone());
                            this.queue_drag = None;
                            cx.notify();
                            this.queue_scroll.bounds()
                        }).unwrap_or_default();
                        cx.new(|_| QueueDragPreview { drag: drag.clone(), row, pointer_offset_x: offset.x })
                    }))
                .when(dragging_here, |d| d.opacity(0.))
                .child(lead)
                .children(item.keyword.clone().map(|keyword| div().flex_none().text_ui(TypeSize::Caption).text_color(c.accent).child(keyword)))
                .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(item.words.lines().next().unwrap_or("").to_owned()))
                .children((!item.files.is_empty()).then(|| {
                    let files = if item.files.len() == 1 { item.files[0].clone() } else { format!("{} files", item.files.len()) };
                    div().flex().flex_none().items_center().gap(space::S1).text_ui(TypeSize::Caption).text_color(c.text_muted).child(Icon::new(IconName::Paperclip).size(IconSize::Nav).color(c.text_faint)).child(div().max_w(px(90.)).truncate().child(files))
                }))
                .children(next_step)
                .child(go)
                .child({
                    let selector = format!("remove-{}", item.id);
                    div().flex_none().debug_selector(move || selector).child(
                        Button::icon_only(SharedString::from(format!("remove-{}", item.id)), IconName::Trash, "Remove")
                            .ghost().quiet().size(ButtonSize::Small).icon_size(IconSize::Nav)
                            .on_click(cx.listener(move |this, _, _, cx| this.unqueue(remove_id.clone(), false, cx))),
                    )
                })
                .child(div().flex_none().debug_selector({
                    let selector = more.clone();
                    move || selector
                }).child(
                    PopoverMenu::button(SharedString::from(more.clone()), Button::icon_only(SharedString::from(format!("open-{more}")), IconName::More, "More actions")
                            .ghost().quiet().size(ButtonSize::Small).icon_size(IconSize::Nav), menu.clone())
                        .above().align_right(),
                ));
            let row = match previous_slots.get(&item.id) {
                Some(previous) if !dragging_here && *previous != index => {
                    let distance = (*previous as f32 - index as f32) * QUEUE_ROW_HEIGHT;
                    row.with_animation((row_id, self.queue_drag_motion as usize),
                        Animation::new(std::time::Duration::from_millis(130)).with_easing(gpui::ease_out_quint()),
                        move |row, progress| row.relative().top(px(distance * (1. - progress))))
                        .into_any_element()
                }
                _ => row.into_any_element(),
            };
            ContextMenuArea::new(SharedString::from(format!("queued-menu-{}", item.id)), row, menu)
        });
        Some(
            div()
                .debug_selector(|| "queue".into())
                .w_full()
                .max_w(px(COLUMN))
                .flex()
                .flex_col()
                .px(space::S1)
                .pt(space::S1)
                .pb(space::S4)
                .mb(-space::S3)
                .rounded_t(radius::CARD)
                .bg(c.surface.blend(c.sunken.opacity(0.45)))
                .border_1()
                .border_color(c.border)
                .children(paused.then(|| {
                    div().flex().items_center().justify_between().px(space::S2)
                        .text_ui(TypeSize::Caption).text_color(c.text_muted)
                        .child("Queue paused")
                        .child(div().debug_selector(|| "queue-resume".into()).child(
                            Button::new("queue-resume", "Resume").ghost().size(ButtonSize::Small).tooltip("Send the waiting messages, one after another").on_click(cx.listener(|this, _, _, cx| this.resume_queue(cx))),
                        ))
                }))
                .children(self.queue.line.clone().map(|line| div().debug_selector(|| "queue-line".into()).px(space::S2).pb(space::S1).text_ui(TypeSize::Caption).text_color(c.text_muted).child(line)))
                .child(div().id("queue-rows").max_h(px(QUEUE_ROW_HEIGHT * 4.)).overflow_y_scroll().track_scroll(&self.queue_scroll)
                    .on_drag_move(cx.listener(Self::drag_over_queue))
                    .on_drop(cx.listener(|this, drag: &QueueDrag, window, cx| {
                        if drag.owner != cx.entity().entity_id() { return; }
                        if let Some(change) = this.queue_move_at(drag, window.mouse_position(), cx) {
                            this.commit_queue_move(change, cx);
                        }
                        this.cancel_queue_drag(cx);
                    }))
                    .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                        if this.queue_dragging.is_some() { this.cancel_queue_drag(cx); }
                    }))
                    .children(rows))
                .children(self.queue_failed.clone().map(|why| div().px(space::S2).pb(space::S1).text_ui(TypeSize::Caption).text_color(c.warning).child(why)))
                .into_any_element(),
        )
    }
}

/// `session`'s conversation as its box has it now, read at once (no follow): for shots and tests,
/// handed to [`ConversationView::take`] as a follow would hand it.
#[cfg(feature = "stand-in")]
pub fn read_now(client: &BoxClient, session: &str) -> kuvo_box::Result<kuvo_box::Conversation> {
    use kuvo_box::Conversation;
    use kuvo_box::messages::{self, Page};
    let place = client.where_is(session)?;
    let page: Page = client.get(&messages::newest_route(place, session, 100))?;
    let running: Option<bool> = client.get::<serde_json::Value>(&format!("{}api/session/active", place.prefix())).ok().map(|active| active["data"].get(session).is_some());
    let earlier = page.earlier(100);
    let mut conversation = Conversation::new(session, place);
    conversation.catch_up(page.data, earlier, &[], running);
    Ok(conversation)
}

impl Render for ConversationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.chat_references.refresh(self.entries.iter().rev().flat_map(Entry::reference_text), &self.session, &self.client, self.live, self.list.clone(), cx);
        self.keep_focus(window, cx);
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let view = cx.entity();
        let body: AnyElement = if !self.read {
            match &self.problem {
                Some(Problem::Gone(why)) => centred(SharedString::from(why.clone()), false, cx),
                // (With what the box offers to do about it, under its words.)
                Some(Problem::Failed(why, _)) => {
                    let button = self.action_button(cx);
                    div().size_full().flex().flex_col().items_center().justify_center().gap(space::S3).px(space::S6).text_ui(TypeSize::Small).text_color(c.text_muted).child(div().max_w(px(COLUMN)).text_center().child(why.clone())).children(button).into_any_element()
                }
                None => centred("Loading the conversation…".into(), true, cx),
            }
        } else if self.entries.is_empty() {
            let asking = self.asks.read(cx).is_asking() && !self.asks.read(cx).leaves_composer();
            centred(if asking { "Nothing here yet.".into() } else { "Nothing here yet. Write below to start.".into() }, false, cx)
        } else {
            let list_view = view.clone();
            let opener = view.clone();
            let open: std::rc::Rc<dyn Fn(&str, &mut Window, &mut App)> = std::rc::Rc::new(move |id: &str, window: &mut Window, cx: &mut App| {
                opener.update(cx, |this, cx| this.open_picture(id, window, cx));
            });
            let reader = view.clone();
            let again: std::rc::Rc<dyn Fn(&str, &mut Window, &mut App)> = std::rc::Rc::new(move |key: &str, _: &mut Window, cx: &mut App| {
                reader.update(cx, |this, cx| this.read_picture_again(key, cx));
            });
            let file_opener = view.clone();
            let open_file: std::rc::Rc<dyn Fn(&str, &mut Window, &mut App)> = std::rc::Rc::new(move |id: &str, _: &mut Window, cx: &mut App| {
                file_opener.update(cx, |this, cx| this.open_shown_file(id, cx));
            });
            let file_retrier = view.clone();
            let retry_file: std::rc::Rc<dyn Fn(&str, &mut Window, &mut App)> = std::rc::Rc::new(move |id: &str, _: &mut Window, cx: &mut App| {
                file_retrier.update(cx, |this, cx| this.retry_shown_file(id, cx));
            });
            let list = chat_list(&self.list, move |ix, _window, cx| {
                let cx: &App = cx;
                let this = list_view.read(cx);
                let Some(entry) = this.entries.get(ix) else { return div().into_any_element() };
                let own = match entry {
                    Entry::Earlier => this.earlier_row(&list_view, cx),
                    Entry::Outgoing { id, failed: Some(_), .. } => this.unsent_actions(id, cx),
                    Entry::Outgoing { id, waits: true, .. } => this.waiting_actions(id, &list_view),
                    Entry::Message(message) => this.files_kept(message.id(), cx),
                    _ => div().into_any_element(),
                };
                let toggle_view = list_view.clone();
                let toggle = move |fold: Fold, cx: &mut App| toggle_view.update(cx, |this, cx| this.toggle(fold, cx));
                let bars = this.bars.clone();
                let around = entries::Around { open_session: Rc::new(move |session: &str, window: &mut Window, cx: &mut App| bars.update(cx, |bars, cx| bars.open(session, window, cx))), waiting: this.asks.read(cx).is_asking() && this.entries.iter().rposition(|entry| matches!(entry, Entry::Steps { .. })) == Some(ix) };
                let shown = this.row_pictures.get(entry.key()).map(|(_, _, pictures)| pictures.as_slice()).unwrap_or(&[]);
                let files: Vec<crate::shown_files::ViewModel> = this
                    .row_shown_files
                    .get(entry.key())
                    .map(|(_, row_files)| {
                        row_files
                            .iter()
                            .map(|file| {
                                crate::shown_files::adapt(
                                    file,
                                    this.shown_info.get(&file.id),
                                    this.pictures.get(&file.id),
                                    this.shown_open.get(&file.id),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let file_actions = crate::shown_files::Actions {
                    open_picture: open.clone(),
                    open_file: open_file.clone(),
                    preview_file: this.preview_file.clone(),
                    retry: retry_file.clone(),
                };
                let looks = Looks { read: &this.pictures, shown, open: open.clone(), again: again.clone(), files: &files, file_actions };
                let actions = entries::copyable(entry).map(|(_, time)| {
                    let key = entry.key().to_owned();
                    let (copier, quoter) = (list_view.clone(), list_view.clone());
                    let (copy_key, quote_key) = (key.clone(), key.clone());
                    entries::Actions {
                        time,
                        now: crate::time::now_ms(),
                        copied: this.copied.as_ref().is_some_and(|(copied, _)| *copied == key),
                        copy: Rc::new(move |_, cx| copier.update(cx, |this, cx| this.copy_message(&copy_key, cx))),
                        quote: Rc::new(move |window, cx| quoter.update(cx, |this, cx| this.quote_message(&quote_key, window, cx))),
                    }
                });
                let references = this.chat_references.chips(entry.reference_text(), &this.client, this.live, cx);
                entries::render(ix, entry, &this.open, toggle, own, &around, &this.selection, &looks, actions, references, cx)
            });
            // One selection over all its messages: drag, double and triple click, shift-click, ⌘A
            // and ⌘C, and a right-click's Copy.
            let scroller = self.list.clone();
            let area = SelectionArea::new(self.selection.clone(), self.transcript.clone(), list).on_scroll(move |by, _, _| scroller.scroll_by(by));
            let menu_view = view.clone();
            div()
                .key_context(selection::CONTEXT)
                .track_focus(&self.transcript)
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::select_all))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" && this.selection.is_finished() {
                        this.selection.clear();
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .size_full()
                .child(ContextMenuArea::new("transcript-menu", area, move |window, cx| Self::transcript_menu(&menu_view, window, cx)).fill())
                .into_any_element()
        };
        // A problem while it's shown: said above the composer; it's tried again meanwhile.
        let banner = match (&self.problem, self.read) {
            (Some(Problem::Failed(why, _)), true) => Some((why.clone(), c.warning)),
            (Some(Problem::Gone(why)), true) => Some((why.clone(), c.danger)),
            _ => None,
        };
        let gone = matches!(self.problem, Some(Problem::Gone(_)));
        let viewer = self.picture_viewer(window, cx);
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.queue_dragging.is_some() && cx.has_active_drag() {
                    cx.stop_active_drag(window);
                    this.cancel_queue_drag(cx);
                    cx.stop_propagation();
                }
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(div().relative().flex_1().min_h_0().child(body))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(space::S6)
                    .pb(space::S5)
                    .children(banner.map(|(why, color)| {
                        let button = self.action_button(cx);
                        div()
                            .debug_selector(|| "conversation-problem".into())
                            .w_full()
                            .max_w(px(COLUMN))
                            .pb(space::S2)
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .text_ui(TypeSize::Caption)
                            .text_color(color)
                            .child(div().flex_1().min_w_0().child(why))
                            .children(button)
                    }))
                    .children(self.asks_failed.clone().filter(|_| !gone).map(|why| {
                        div().w_full().max_w(px(COLUMN)).pb(space::S2).text_ui(TypeSize::Caption).text_color(c.warning).child(why)
                    }))
                    // What it waits on you for takes the composer's place, as on the Mac and the phone.
                    // One this app can't answer sits over the composer, which stays usable.
                    .when(!gone, |d| {
                        let card = self.asks.read(cx);
                        let (asking, beside) = (card.is_asking(), card.leaves_composer());
                        d.when(asking && beside, |d| d.child(div().w_full().max_w(px(COLUMN)).pb(space::S2).child(self.asks.clone())))
                            .children(self.queue_view(cx))
                            .child(div().w_full().max_w(px(COLUMN)).child(if asking && !beside { self.asks.clone().into_any_element() } else { self.composer.clone().into_any_element() }))
                    }),
            )
            .children(self.selection_card(cx))
            .children(viewer)
    }
}

/// Whether a read was refused for the box's coding agents' state (said once, over the composer).
fn about_agents(error: &kuvo_box::Error) -> bool {
    error.refusal().is_some_and(kuvo_box::Refusal::about_agents)
}

/// Centred words in the conversation's place (loading, nothing yet, gone).
fn centred(words: SharedString, busy: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .gap(space::S2)
        .text_ui(TypeSize::Small)
        .text_color(c.text_muted)
        .children(busy.then(|| spinner("loading", IconSize::Nav, c.text_muted)))
        .child(words)
        .into_any_element()
}
