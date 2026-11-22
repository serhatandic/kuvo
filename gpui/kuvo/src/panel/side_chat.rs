//! A side chat tab: one side thread of questions asked about the conversation without interrupting
//! it (`/btw`), answered from what it knows and kept out of it, as the box keeps and groups them
//! (`kuvo/agents/api/kuvo/btw/<session>/threads`, followed). A new side chat starts its thread
//! with its first question; each one after follows up in it.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::follow::{Follow, Followed, Timing};
use kuvo_box::side_chats::{Answering, SideThread, SideThreads};
use kuvo_box::BoxClient;
use kuvo_ui::markdown::Markdown;
use kuvo_ui::text_field::TextFieldEvent;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, IconName, StyledType, TextField, TypeSize, theme::{radius, space}};

use super::{Load, glass, run};

/// What a side chat tells the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SideChatEvent {
    /// Its first question started a side thread: the tab is that thread's from now on.
    Started(String),
}

pub struct SideChatView {
    client: BoxClient,
    live: bool,
    session: String,
    /// Its side thread, once it has one.
    thread: Option<String>,
    threads: Load<SideThreads>,
    field: Entity<TextField>,
    asking: bool,
    failed: Option<String>,
    on_screen: bool,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _field: Subscription,
    _follow: Option<Follow<Followed<SideThreads>>>,
    _task: Option<Task<()>>,
}

impl EventEmitter<SideChatEvent> for SideChatView {}

impl SideChatView {
    pub fn new(client: BoxClient, live: bool, session: String, thread: Option<String>, cx: &mut Context<Self>) -> SideChatView {
        let field = cx.new(|cx| TextField::new(cx).multi_line(1, 6).bare().submits_on_enter(true).placeholder("Ask a side question…"));
        let watch = cx.subscribe(&field, |this: &mut SideChatView, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submit(_) => this.ask(cx),
            TextFieldEvent::Changed => {
                this.failed = None;
                cx.notify();
            }
            _ => {}
        });
        SideChatView { client, live, session, thread, threads: Load::Loading, field, asking: false, failed: None, on_screen: false, scroll: ScrollHandle::new(), focus: cx.focus_handle(), _field: watch, _follow: None, _task: None }
    }

    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// Its side thread, as the box last said it.
    pub fn thread(&self) -> Option<&SideThread> {
        let id = self.thread.as_deref()?;
        self.threads.ready()?.thread(id)
    }

    /// Whether it's the tab on screen: followed while it is (the app), read as it shows (tests and
    /// shots).
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
            self.read_again(cx);
            return;
        }
        let follow = self.client.follow_side_threads(&self.session, Timing::default());
        let updates = follow.updates().clone();
        self._follow = Some(follow);
        self._task = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let taken = this.update(cx, |this, cx| match update {
                    Followed::Now(threads) => this.take(Ok(threads), cx),
                    Followed::Failed(error) => this.take(Err(error.to_string()), cx),
                });
                if taken.is_err() {
                    break;
                }
            }
        }));
    }

    fn read_again(&mut self, cx: &mut Context<Self>) {
        let (client, session) = (self.client.clone(), self.session.clone());
        run(self, self.live, cx, move || client.side_threads(&session), |this, read, cx| this.take(read.map_err(|error| error.to_string()), cx));
    }

    fn take(&mut self, read: Result<SideThreads, String>, cx: &mut Context<Self>) {
        match read {
            Ok(threads) => self.threads = Load::Ready(threads),
            // What showed stays through a read that failed.
            Err(why) if self.threads.ready().is_none() => self.threads = Load::Failed(why),
            Err(_) => {}
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Asks what's written: the side thread's first question, or a follow-up in it.
    pub fn ask(&mut self, cx: &mut Context<Self>) {
        let question = self.field.read(cx).text().trim().to_owned();
        if question.is_empty() || self.asking {
            return;
        }
        self.asking = true;
        self.failed = None;
        let (client, session, thread) = (self.client.clone(), self.session.clone(), self.thread.clone());
        run(self, self.live, cx, move || client.ask_aside(&session, &question, thread.as_deref()), |this, asked, cx| {
            this.asking = false;
            match asked {
                Ok(question) => {
                    this.field.update(cx, |field, cx| field.set_text("", cx));
                    if this.thread.is_none() {
                        let thread = question.thread.clone().unwrap_or_else(|| question.id.clone());
                        this.thread = Some(thread.clone());
                        cx.emit(SideChatEvent::Started(thread));
                    }
                    // (The follow says it's being answered; a read here says so at once where
                    // nothing follows.)
                    if !this.live {
                        this.read_again(cx);
                    }
                }
                Err(error) => this.failed = Some(error.to_string()),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn transcript(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let glass = glass(cx);
        let Some(thread) = self.thread() else {
            let (title, detail) = match (&self.threads, &self.thread) {
                (Load::Failed(why), _) => ("Couldn’t Read This Side Chat".to_owned(), why.clone()),
                (Load::Loading, Some(_)) => return kuvo_ui::small::LoadingLabel::new("side-chat-loading", "Reading…").fill().into_any_element(),
                _ => ("Ask on the Side".to_owned(), "Ask about this conversation without interrupting it. The answer stays here, out of the conversation.".to_owned()),
            };
            return super::changes::empty(title, detail, None, cx);
        };
        div()
            .id("side-chat-transcript")
            .debug_selector(|| "side-chat-transcript".into())
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(space::S4)
            .py(space::S4)
            .flex()
            .flex_col()
            .gap(space::S4)
            .children(thread.questions.iter().map(|question| {
                let id = question.id.clone();
                let answer: AnyElement = match (question.state, &question.answer, &question.error) {
                    (Answering::Answered, Some(answer), _) => div().text_ui(TypeSize::Body).child(Markdown::new(SharedString::from(format!("side-answer-{id}")), answer)).into_any_element(),
                    (Answering::Failed, _, why) => div().text_ui(TypeSize::Small).text_color(c.danger).child(why.clone().unwrap_or_else(|| "It couldn’t be answered.".into())).into_any_element(),
                    _ => kuvo_ui::small::LoadingLabel::new(SharedString::from(format!("side-answering-{id}")), "Answering…").compact().into_any_element(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S3)
                    .child(
                        div().flex().justify_end().child(
                            div()
                                .debug_selector(move || format!("side-question-{id}"))
                                .max_w(px(420.))
                                .px(space::S3)
                                .py(space::S2)
                                .rounded(radius::POPOVER)
                                .bg(glass.tile_hover)
                                .border_1()
                                .border_color(glass.tile_edge)
                                .text_ui(TypeSize::Body)
                                .child(question.question.clone()),
                        ),
                    )
                    .child(answer)
            }))
            .into_any_element()
    }

    fn composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let written = !self.field.read(cx).text().trim().is_empty();
        div()
            .flex_none()
            .p(space::S3)
            .child(
                div()
                    .debug_selector(|| "side-chat-composer".into())
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .px(space::S3)
                    .pt(space::S2)
                    .pb(space::S2)
                    .rounded(radius::CARD)
                    .bg(c.card)
                    .border_1()
                    .border_color(c.border)
                    .shadow(theme.card_shadow())
                    .child(div().text_ui(TypeSize::Body).child(self.field.clone()))
                    .children(self.failed.clone().map(|why| div().text_ui(TypeSize::Caption).text_color(c.danger).child(format!("It couldn’t be asked: {why}"))))
                    .child(
                        div().flex().justify_end().child(
                            Button::icon_only("side-chat-send", IconName::ArrowUp, "Ask")
                                .primary()
                                .pill()
                                .size(ButtonSize::Small)
                                .disabled(!written || self.asking)
                                .loading(self.asking)
                                .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                        ),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for SideChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.focus_handle(cx)
    }
}

impl Render for SideChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_col().child(self.transcript(cx)).child(self.composer(cx))
    }
}
