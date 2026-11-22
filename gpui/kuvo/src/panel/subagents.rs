//! The Subagents tab: the conversation's subagents, those at work and those done, as the box lists
//! them (`kuvo/work/subagents/<session>`, followed); one opens in the tab to its own conversation,
//! read as it goes (with a way back to the list).

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::follow::{Follow, Followed, Timing};
use kuvo_box::subagents::{Ended, Subagents};
use kuvo_box::{BoxClient, ConversationUpdate};
use kuvo_ui::selection::SelectionState;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize, small::status_dot, theme::{radius, space}};

use crate::entries::{self, Around, Entry, Fold};

use super::{Load, glass, run};

/// What the Subagents tab asks of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubagentsEvent {
    /// Open a subagent's conversation in the window, to talk to it.
    OpenConversation(String),
}

pub struct SubagentsView {
    client: BoxClient,
    live: bool,
    session: String,
    list: Load<Subagents>,
    /// The subagent open in the tab, and its conversation.
    opened: Option<(String, Entity<Transcript>)>,
    on_screen: bool,
    now: Option<f64>,
    focus: FocusHandle,
    _follow: Option<Follow<Followed<Subagents>>>,
    _task: Option<Task<()>>,
}

impl EventEmitter<SubagentsEvent> for SubagentsView {}

impl SubagentsView {
    pub fn new(client: BoxClient, live: bool, session: String, cx: &mut Context<Self>) -> SubagentsView {
        SubagentsView { client, live, session, list: Load::Loading, opened: None, on_screen: false, now: None, focus: cx.focus_handle(), _follow: None, _task: None }
    }

    pub fn list(&self) -> &Load<Subagents> {
        &self.list
    }

    /// The subagent open, by its session.
    pub fn opened(&self) -> Option<&str> {
        self.opened.as_ref().map(|(session, _)| session.as_str())
    }

    /// The clock its times are said against (tests and shots); the app's own otherwise.
    pub fn set_now(&mut self, now: f64) {
        self.now = Some(now);
    }

    /// Whether it's the tab on screen: followed while it is (the app), read as it shows (tests).
    pub fn set_shown(&mut self, on_screen: bool, cx: &mut Context<Self>) {
        if self.on_screen == on_screen {
            return;
        }
        self.on_screen = on_screen;
        self._follow = None;
        self._task = None;
        if let Some((_, transcript)) = &self.opened {
            transcript.update(cx, |transcript, cx| transcript.set_shown(on_screen, cx));
        }
        if !on_screen {
            return;
        }
        if !self.live {
            let (client, session) = (self.client.clone(), self.session.clone());
            run(self, false, cx, move || client.subagents(&session), |this, read, cx| this.take(read.map_err(|error| error.to_string()), cx));
            return;
        }
        let follow = self.client.follow_subagents(&self.session);
        let updates = follow.updates().clone();
        self._follow = Some(follow);
        self._task = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let taken = this.update(cx, |this, cx| match update {
                    Followed::Now(list) => this.take(Ok(list), cx),
                    Followed::Failed(error) => this.take(Err(error.to_string()), cx),
                });
                if taken.is_err() {
                    break;
                }
            }
        }));
    }

    fn take(&mut self, read: Result<Subagents, String>, cx: &mut Context<Self>) {
        match read {
            Ok(list) => self.list = Load::Ready(list),
            Err(why) if self.list.ready().is_none() => self.list = Load::Failed(why),
            Err(_) => {}
        }
        cx.notify();
    }

    /// Opens a subagent's conversation in the tab.
    pub fn open(&mut self, session: &str, cx: &mut Context<Self>) {
        let (client, live, on_screen) = (self.client.clone(), self.live, self.on_screen);
        let transcript = cx.new(|cx| Transcript::new(client, live, session.to_owned(), cx));
        transcript.update(cx, |transcript, cx| transcript.set_shown(on_screen, cx));
        self.opened = Some((session.to_owned(), transcript));
        cx.notify();
    }

    /// Back to the list.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        self.opened = None;
        cx.notify();
    }

    fn ago(&self, at: f64) -> String {
        crate::time::ago(at, self.now.unwrap_or_else(crate::time::now_ms))
    }

    fn row(&self, id: String, title: &str, trailing: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let glass = glass(cx);
        let opened = id.clone();
        div()
            .id(SharedString::from(format!("subagent-{id}")))
            .debug_selector(move || format!("subagent-{id}"))
            .flex()
            .items_center()
            .gap(space::S2)
            .h(px(40.))
            .px(space::S2)
            .rounded(radius::NAV)
            .cursor_pointer()
            .hover(move |s| s.bg(c.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.open(&opened, cx)))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(radius::SMALL)
                    .bg(glass.tile_hover)
                    .child(Icon::new(IconName::Bot).size(IconSize::XSmall).color(c.icon)),
            )
            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).child(if title.trim().is_empty() { "Subagent".to_owned() } else { title.to_owned() }))
            .child(trailing)
            .into_any_element()
    }

    fn heading(title: &str, count: usize, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        div().px(space::S2).pt(space::S3).pb(space::S1).text_ui(TypeSize::Small).medium().text_color(c.text).child(format!("{title} · {count}")).into_any_element()
    }

    fn listed(&self, list: &Subagents, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let active: Vec<AnyElement> = list
            .active
            .iter()
            .map(|active| {
                let trailing = div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S1_5)
                    .text_ui(TypeSize::Caption)
                    .text_color(if active.asking { c.attention } else { c.text_faint })
                    .child(status_dot(if active.asking { c.attention } else { c.success }))
                    .child(if active.asking { "Waiting for you" } else { "Working" })
                    .into_any_element();
                self.row(active.session.clone(), &active.title, trailing, cx)
            })
            .collect();
        let done: Vec<AnyElement> = list
            .done
            .iter()
            .map(|done| {
                let (word, color) = match done.state {
                    Ended::Failed => (Some("Failed"), c.danger),
                    Ended::Stopped => (Some("Stopped"), c.text_faint),
                    _ => (None, c.text_faint),
                };
                let trailing = div()
                    .flex()
                    .flex_none()
                    .gap(space::S1_5)
                    .text_ui(TypeSize::Caption)
                    .children(word.map(|word| div().text_color(color).child(word)))
                    .child(div().text_color(c.text_faint).child(self.ago(done.ended)))
                    .into_any_element();
                self.row(done.session.clone(), &done.title, trailing, cx)
            })
            .collect();
        let (active_count, done_count) = (active.len(), done.len());
        div()
            .id("subagents-list")
            .debug_selector(|| "subagents-list".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(space::S3)
            .pb(space::S3)
            .child(Self::heading("Active", active_count, cx))
            .when(active.is_empty(), |d| d.child(div().px(space::S2).py(space::S1).text_ui(TypeSize::Small).text_color(c.text_faint).child("No subagents at work now.")))
            .children(active)
            .when(done_count > 0, |d| d.child(div().mt(space::S3).child(Self::heading("Done", done_count, cx))).children(done))
            .into_any_element()
    }

    fn detail(&self, session: &str, transcript: &Entity<Transcript>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let (title, model, effort) = self.list.ready().and_then(|list| list.find(session)).map(|(title, model, effort)| (title.to_owned(), model.map(str::to_owned), effort.map(str::to_owned))).unwrap_or_default();
        let named = match (model, effort) {
            (Some(model), Some(effort)) => Some(format!("{model} · {effort}")),
            (Some(model), None) => Some(model),
            _ => None,
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S1_5)
                    .h(px(38.))
                    .pl(space::S2)
                    .pr(space::S3)
                    .border_b_1()
                    .border_color(c.border)
                    .child(Button::icon_only("subagent-back", IconName::ArrowLeft, "Back to Subagents").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.back(cx))))
                    .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).medium().child(if title.is_empty() { "Subagent".to_owned() } else { title }))
                    .children(named.map(|named| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(named)))
                    .child({
                        let session = session.to_owned();
                        Button::new("subagent-open", "Open")
                            .ghost()
                            .size(ButtonSize::Small)
                            .icon_after(IconName::External)
                            .tooltip("Open its conversation, to talk to it")
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(SubagentsEvent::OpenConversation(session.clone()))))
                    }),
            )
            .child(div().flex().flex_col().flex_1().min_h_0().child(transcript.clone()))
            .into_any_element()
    }
}

impl Focusable for SubagentsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SubagentsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match (&self.opened, &self.list) {
            (Some((session, transcript)), _) => self.detail(&session.clone(), &transcript.clone(), cx),
            (None, Load::Loading) => kuvo_ui::small::LoadingLabel::new("subagents-loading", "Reading…").fill().into_any_element(),
            (None, Load::Failed(why)) => super::changes::empty("Couldn’t Read the Subagents", why.clone(), None, cx),
            (None, Load::Ready(list)) => {
                let list = list.clone();
                self.listed(&list, cx)
            }
        };
        div().track_focus(&self.focus).size_full().flex().flex_col().child(body)
    }
}

/// The most of a subagent's rows its tab draws: its latest.
const ROWS_MOST: usize = 200;

/// A subagent's conversation, read-only, drawn as a conversation draws its rows.
pub struct Transcript {
    client: BoxClient,
    live: bool,
    session: String,
    entries: Rc<Vec<Entry>>,
    read: Load<()>,
    open: HashSet<Fold>,
    scroll: gpui::ScrollHandle,
    selection: SelectionState,
    on_screen: bool,
    _follow: Option<Follow<ConversationUpdate>>,
    _task: Option<Task<()>>,
}

impl Transcript {
    pub fn new(client: BoxClient, live: bool, session: String, _cx: &mut Context<Self>) -> Transcript {
        Transcript { client, live, session, entries: Rc::default(), read: Load::Loading, open: HashSet::new(), scroll: gpui::ScrollHandle::new(), selection: SelectionState::new(), on_screen: false, _follow: None, _task: None }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn set_shown(&mut self, on_screen: bool, cx: &mut Context<Self>) {
        if self.on_screen == on_screen {
            return;
        }
        self.on_screen = on_screen;
        self._follow = None;
        self._task = None;
        if !on_screen {
            return;
        }
        if !self.live {
            // (Read at once, as the window reads a conversation in tests and shots.)
            #[cfg(feature = "stand-in")]
            {
                let read = match crate::conversation::read_now(&self.client, &self.session) {
                    Ok(conversation) => ConversationUpdate::Conversation(conversation),
                    Err(error) => ConversationUpdate::Failed(error),
                };
                self.take(read, cx);
            }
            return;
        }
        let follow = self.client.follow_conversation(&self.session, Timing::default());
        let updates = follow.updates().clone();
        self._follow = Some(follow);
        self._task = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this.update(cx, |this, cx| this.take(update, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn take(&mut self, update: ConversationUpdate, cx: &mut Context<Self>) {
        match update {
            ConversationUpdate::Conversation(conversation) => {
                let (entries, _) = entries::entries(conversation);
                self.entries = Rc::new(entries);
                self.read = Load::Ready(());
                self.scroll.scroll_to_bottom();
            }
            ConversationUpdate::Failed(error) if self.read.ready().is_none() => self.read = Load::Failed(error.to_string()),
            ConversationUpdate::Gone(refusal) => self.read = Load::Failed(refusal.message),
            _ => {}
        }
        cx.notify();
    }

    fn row(&self, ix: usize, view: &Entity<Transcript>, cx: &App) -> AnyElement {
        let Some(entry) = self.entries.get(ix) else { return div().into_any_element() };
        let weak = view.downgrade();
        let toggle = move |fold: Fold, cx: &mut App| {
            weak.update(cx, |this, cx| {
                if !this.open.remove(&fold) {
                    this.open.insert(fold);
                }
                cx.notify();
            })
            .ok();
        };
        let around = Around { open_session: Rc::new(|_, _, _| {}), waiting: false, now: crate::time::now_ms() };
        entries::render(ix, entry, &self.open, toggle, div().into_any_element(), &around, &self.selection, &crate::pictures::Looks::none(), None, cx)
    }
}

impl Render for Transcript {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let body = match &self.read {
            Load::Loading => kuvo_ui::small::LoadingLabel::new("transcript-loading", "Reading…").fill().into_any_element(),
            Load::Failed(why) => super::changes::empty("Couldn’t Read This Subagent", why.clone(), None, cx),
            Load::Ready(()) if self.entries.is_empty() => super::changes::empty("Nothing Here Yet", "It hasn’t said anything yet.", None, cx),
            // (The latest rows; what came before is in its conversation, a click on Open away.)
            Load::Ready(()) => {
                let start = self.entries.len().saturating_sub(ROWS_MOST);
                let rows: Vec<AnyElement> = (start..self.entries.len()).map(|ix| self.row(ix, &view, cx)).collect();
                div().id("transcript-rows").track_scroll(&self.scroll).size_full().overflow_y_scroll().pt(space::S3).children(rows).into_any_element()
            }
        };
        div().debug_selector(|| "subagent-transcript".into()).size_full().flex().flex_col().child(body)
    }
}
