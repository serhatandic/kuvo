//! A side chat tab: one side thread of questions asked about the conversation without interrupting
//! it (`/btw`), answered from what it knows and kept out of it, as the box keeps and groups them
//! (`kuvo/agents/api/kuvo/btw/<session>/threads`, followed). A new side chat starts its thread
//! with its first question; each one after follows up in it. Its composer is the conversation's,
//! as a side question can use it: files to attach, and a model with its effort (the conversation's
//! own until another's picked), which go to the box with the question. No mode: a side question
//! runs nothing.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::follow::{Follow, Followed};
use kuvo_box::models::{ModelState, Offered, OfferedModel};
use kuvo_box::typed::ModelRef;
use kuvo_ui::menu::PopoverMenuHandle;
use kuvo_ui::small::Chip;

use crate::model_picker::{Catalog, ModelPicker, RefreshModels};
use kuvo_box::side_chats::{Answering, SideThread, SideThreads};
use kuvo_box::BoxClient;
use kuvo_ui::markdown::Markdown;
use kuvo_ui::text_field::TextFieldEvent;
use kuvo_ui::{ActiveTheme, Button, IconName, StyledType, TextField, TypeSize, theme::{radius, space}};

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
    /// Files to go with the next question.
    attached: Vec<PathBuf>,
    /// What the box offers, and what it says of the conversation's model: the one a question's
    /// asked on until another's picked (`picked`).
    offered: Option<Offered>,
    own: Option<ModelState>,
    picked: Option<ModelRef>,
    model_menu: PopoverMenuHandle,
    _picking: Vec<Task<()>>,
    on_screen: bool,
    scroll: ScrollHandle,
    /// The height of the composer floating over the transcript's bottom, as last laid out.
    inset: std::rc::Rc<std::cell::Cell<gpui::Pixels>>,
    _field: Subscription,
    _follow: Option<Follow<Followed<SideThreads>>>,
    _task: Option<Task<()>>,
}

impl EventEmitter<SideChatEvent> for SideChatView {}

impl SideChatView {
    pub fn new(client: BoxClient, live: bool, session: String, thread: Option<String>, cx: &mut Context<Self>) -> SideChatView {
        let field = cx.new(|cx| TextField::new(cx).multi_line(1, 10).bare().submits_on_enter(true).placeholder("Ask a side question…"));
        let watch = cx.subscribe(&field, |this: &mut SideChatView, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submit(_) => this.ask(cx),
            TextFieldEvent::Changed => {
                this.failed = None;
                cx.notify();
            }
            _ => {}
        });
        SideChatView {
            client, live, session, thread, threads: Load::Loading, field, asking: false, failed: None,
            attached: Vec::new(), offered: None, own: None, picked: None, model_menu: PopoverMenuHandle::default(), _picking: Vec::new(),
            on_screen: false, scroll: ScrollHandle::new(), inset: Default::default(), _field: watch, _follow: None, _task: None,
        }
    }

    /// The model the next question's asked on: the one picked, else the conversation's own.
    pub fn model(&self) -> Option<ModelRef> {
        self.picked.clone().or_else(|| self.own.as_ref().and_then(|own| own.model.clone()))
    }

    pub fn attached(&self) -> &[PathBuf] {
        &self.attached
    }

    /// Reads what the box offers and the conversation's model, for the chips.
    fn read_models(&mut self, cx: &mut Context<Self>) {
        let (client, session, before) = (self.client.clone(), self.session.clone(), self.offered.clone());
        run(self, self.live, cx, move || {
            let offered = client.offered_models_for_session(None, None, Some(&session)).ok().map(|offered| offered.keeping(before.as_ref()));
            let own = client.session_models(&[session.as_str()]).ok().and_then(|mut all| all.remove(&session));
            (offered, own)
        }, |this, (offered, own), cx| {
            if offered.is_some() {
                this.offered = offered;
            }
            if own.is_some() {
                this.own = own;
            }
            cx.notify();
        });
    }

    /// The models it can be asked on: those of the conversation's own side (its engine, or its
    /// coding agents), as the box answers a side question on them.
    fn offerable(&self, model: &OfferedModel) -> bool {
        let own = self.own.as_ref().and_then(|own| own.model.as_ref());
        let models = self.offered.as_ref().map(|offered| offered.models.as_slice()).unwrap_or_default();
        let side = own.and_then(|own| models.iter().find(|each| each.is(own))).map(|each| each.side);
        self.offered.as_ref().is_some_and(|offered| offered.scoped) || side.is_none_or(|side| model.side == side)
    }

    /// What the model chip says: the model's name, as the box lists it.
    pub fn chip_label(&self) -> Option<String> {
        let model = self.model()?;
        let listed = self.offered.as_ref().and_then(|offered| offered.models.iter().find(|each| each.is(&model))).map(|each| each.name.clone());
        let own = self.picked.is_none().then(|| self.own.as_ref().and_then(|own| own.name.clone())).flatten();
        Some(own.or(listed).unwrap_or(model.id))
    }

    fn efforts(&self) -> Vec<String> {
        let Some(model) = self.model() else { return Vec::new() };
        self.offered.as_ref().and_then(|offered| offered.models.iter().find(|each| each.is(&model))).map(|each| each.efforts.clone()).unwrap_or_default()
    }

    /// Picks the model (and effort) side questions here are asked on; the conversation's own model
    /// stays as it is.
    pub fn pick(&mut self, model: ModelRef, cx: &mut Context<Self>) {
        let own = self.own.as_ref().and_then(|own| own.model.as_ref());
        self.picked = (own != Some(&model)).then_some(model);
        cx.notify();
    }

    fn model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ModelPicker> {
        let catalog = Arc::new(self.offered.as_ref().map(|offered| Catalog::new(offered, |model| self.offerable(model))).unwrap_or_default());
        let starred = self.offered.as_ref().and_then(|offered| offered.starred.clone());
        let this = cx.entity().downgrade();
        let (current, client, before, session) = (self.model(), self.client.clone(), self.offered.clone(), self.session.clone());
        let picking = cx.entity().downgrade();
        let on_effort: crate::model_picker::OnEffort = Rc::new(move |effort, _, cx| {
            picking
                .update(cx, |this, cx| {
                    if let Some(model) = this.model() {
                        this.pick(ModelRef { variant: effort, ..model }, cx);
                    }
                })
                .ok();
        });
        cx.new(|cx| {
            ModelPicker::new(
                catalog,
                starred,
                current,
                Some(client),
                Rc::new(move |picked, _, cx| {
                    if let Some(picked) = picked {
                        this.update(cx, |this, cx| this.pick(picked, cx)).ok();
                    }
                }),
                window,
                cx,
            )
            .refresh_when_opened(before, RefreshModels::Offered { folder: None, session: Some(session), side: None }, window, cx)
            .reasoning(Some(on_effort), false)
            .with_speed()
            .note(Some("A side question is answered on the model picked here. The conversation stays on its own.".into()))
        })
    }

    /// Files and pictures to go with the next question.
    pub fn attach(&mut self, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !path.is_dir() && !self.attached.contains(&path) {
                self.attached.push(path);
            }
        }
        self.failed = None;
        cx.notify();
    }

    fn detach(&mut self, at: usize, cx: &mut Context<Self>) {
        if at < self.attached.len() {
            self.attached.remove(at);
            cx.notify();
        }
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach".into()) });
        self._picking.retain(|task| !task.is_ready());
        self._picking.push(cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update(cx, |this, cx| this.attach(paths, cx)).ok();
            }
        }));
    }

    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// Where its transcript is scrolled.
    pub fn scroll(&self) -> &ScrollHandle {
        &self.scroll
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
        if self.offered.is_none() || self.own.is_none() {
            self.read_models(cx);
        }
        if !self.live {
            self.read_again(cx);
            return;
        }
        let follow = self.client.follow_side_threads(&self.session);
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
        let (client, session, thread, model, paths) = (self.client.clone(), self.session.clone(), self.thread.clone(), self.picked.clone(), self.attached.clone());
        let (here, pasted) = (client.here(), crate::pasted::folder(cx));
        run(self, self.live, cx, move || {
            // (Its files read first, as a message's are: one that can't go asks nothing.)
            let files = kuvo_box::parts::carried(&paths, here, &|path| path.starts_with(&pasted))?;
            client.ask_aside(&session, &question, thread.as_deref(), model.as_ref(), &files).map_err(|error| error.to_string())
        }, |this, asked, cx| {
            this.asking = false;
            match asked {
                Ok(question) => {
                    this.field.update(cx, |field, cx| field.set_text("", cx));
                    this.attached.clear();
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
                Err(error) => this.failed = Some(error),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn transcript(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let glass = glass(cx);
        let inset = self.inset.get();
        let Some(thread) = self.thread() else {
            let (title, detail) = match (&self.threads, &self.thread) {
                (Load::Failed(why), _) => ("Couldn’t Read This Side Chat".to_owned(), why.clone()),
                (Load::Loading, Some(_)) => return div().flex_1().min_h_0().flex().flex_col().pb(inset).child(kuvo_ui::small::LoadingLabel::new("side-chat-loading", "Reading…").fill()).into_any_element(),
                _ => ("Ask on the Side".to_owned(), "Ask about this conversation without interrupting it. The answer stays here, out of the conversation.".to_owned()),
            };
            return div().flex_1().min_h_0().flex().flex_col().pb(inset).child(super::changes::empty(title, detail, None, cx)).into_any_element();
        };
        // At its end as last laid out: kept there as what's below grows (an answer coming in, the
        // composer's height as it's measured), as the conversation's list is. Read up: left there.
        if self.scroll.offset().y <= -self.scroll.max_offset().y + px(1.) {
            self.scroll.scroll_to_bottom();
        }
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
                    (Answering::Failed, _, why) => kuvo_ui::Notice::said(format!("side-failed-{id}"), kuvo_ui::Tone::Error, why.clone().unwrap_or_else(|| "It couldn’t be answered.".into())).into_any_element(),
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
                    // What went with it, by name.
                    .when(!question.files.is_empty(), |row| {
                        let qid = question.id.clone();
                        row.child(div().flex().flex_wrap().justify_end().gap(space::S1_5).children(question.files.iter().enumerate().map(|(at, name)| {
                            Chip::new(SharedString::from(format!("side-file-{qid}-{at}")), name.clone()).icon(IconName::File).into_any_element()
                        })))
                    })
                    .child(answer)
            }))
            // (Room for the composer floating over its end.)
            .child(div().flex_none().h((inset - space::S4).max(px(0.))).debug_selector(|| "side-chat-end".into()))
            .into_any_element()
    }

    /// The conversation's own composer, as it draws it (its card, attachments, field, paperclip,
    /// model chip and Send), with what a side question takes; no permissions, since it runs
    /// nothing. Its bottom lines up with the conversation's.
    fn composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let written = !self.field.read(cx).text().trim().is_empty();
        let strip = crate::composer::attachment_strip(&self.attached, &self.client, self.field.focus_handle(cx), |this: &mut SideChatView, at, cx| this.detach(at, cx), cx);
        let held = self.client.gate().is_held();
        let model = self.chip_label().map(|name| {
            let efforts = self.efforts();
            let model = self.model();
            let words = crate::model_picker::ChipWords { provider: model.as_ref().map(|model| model.provider_id.clone()), effort: model.and_then(|model| model.variant).filter(|_| !efforts.is_empty()).map(|effort| kuvo_box::models::effort_title(&effort)), name, holding: None };
            div().debug_selector(|| "side-chat-model".into()).flex().min_w_0().child(crate::model_picker::model_chip(words, held, &self.model_menu, |this: &mut SideChatView, window, cx| this.model_picker(window, cx), cx))
        });
        div()
            .flex_none()
            .px(space::S4)
            .pb(space::S5)
            .child(
                crate::composer::card(cx)
                    .debug_selector(|| "side-chat-composer".into())
                    .children(strip)
                    .child(crate::composer::field_room(self.field.clone()))
                    .children(self.failed.clone().map(|why| kuvo_ui::Notice::inline("side-chat-failed", kuvo_ui::Tone::Error, "It couldn’t be asked").detail(why)))
                    .child(
                        crate::composer::controls()
                            .child(div().debug_selector(|| "side-chat-attach".into()).child(crate::composer::attach_button().on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)))))
                            .children(model)
                            .child(div().flex_1())
                            .child(
                            Button::icon_only("side-chat-send", IconName::ArrowUp, if written { "Ask" } else { "Type to ask" })
                                .accent()
                                .pill()
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
        // The transcript fills the height and scrolls under the composer, which floats over its
        // bottom; the transcript's end is padded by the composer's height, as it's measured.
        div()
            .id("side-chat")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.attach(paths.paths().to_vec(), cx)))
            .child(self.transcript(cx))
            .child(div().absolute().left_0().right_0().bottom_0().child(kuvo_ui::chat_list::measured_height(self.inset.clone())).child(self.composer(cx)))
    }
}
