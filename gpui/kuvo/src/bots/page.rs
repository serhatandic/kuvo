//! Drawing the Bots area's main view: a bot's head (face, name, job, what it's doing, Chat), what
//! waits for another computer, its recent sessions, its settings as Settings draws rows (a name and
//! a grey line on the left, the control on the right), New Bot, and the sheets over them (a name,
//! a job, a look, Delete asked first).

use kuvo_ui::motion::{MotionExt as _, Direction};
use gpui::{
    AnyElement, App, AppContext as _, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, list,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::areas::{Bot, Look};
use kuvo_box::bots::{WorkSession, WorkState};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    button::{Button, ButtonSize},
    menu::{Menu, MenuEntry, PopoverMenu},
    small::{count_badge, spinner, status_dot},
    theme::{radius, space},
};

use std::{rc::Rc, sync::Arc};

use super::{Act, BotsView, Control, Item, Row, Sheet, Shown, Tone};
use crate::model_picker::{Catalog, ModelPicker};
use crate::areas::bot_face;

/// The widest the page's cards grow (Settings' width).
const PAGE_WIDTH: f32 = 720.;

impl BotsView {
    fn item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Some(item) = self.items.get(ix).cloned() else { return div().into_any_element() };
        let body: AnyElement = match item {
            Item::Title(title) => div().pt(space::S8).pb(space::S2).text_ui(TypeSize::Title).medium().text_color(c.text).child(title).into_any_element(),
            Item::Head { bot, doing } => self.head(&bot, doing, cx),
            Item::ElsewhereHead { key, name, look, line } => div()
                .debug_selector({
                    let key = key.clone();
                    move || format!("elsewhere-head-{key}")
                })
                .pt(space::S8)
                .pb(space::S2)
                .flex()
                .items_center()
                .gap(space::S4)
                .child(face_or_blank(&key, look.as_ref(), px(64.), cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::S1)
                        .child(div().truncate().text_ui(TypeSize::Title).medium().text_color(c.text).child(name))
                        .child(div().text_ui(TypeSize::Body).text_color(c.text_muted).child(line)),
                )
                .into_any_element(),
            Item::Notice { key, tone, title, words } => {
                let (icon, color) = match tone {
                    Tone::Waiting => (IconName::Clock, c.icon),
                    Tone::Refused => (IconName::Warning, c.warning),
                };
                div()
                    .pt(space::S4)
                    .child(
                        div()
                            .debug_selector(move || key)
                            .flex()
                            .items_start()
                            .gap(space::S3)
                            .px(space::S5)
                            .py(space::S4)
                            .rounded(radius::POPOVER)
                            .bg(c.card)
                            .shadow(cx.theme().card_shadow())
                            .child(div().pt(px(2.)).child(Icon::new(icon).color(color)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(space::S0_5)
                                    .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                                    .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words)),
                            ),
                    )
                    .into_any_element()
            }
            Item::Group(label) => div().pt(space::S6).pb(space::S2).text_ui(TypeSize::Body).text_color(c.text_muted).child(label).into_any_element(),
            Item::Row { row, first, last } => self.row(*row, first, last, cx),
            Item::Session { session, project, ago, first, last } => self.session(session, project, ago, first, last, cx),
            Item::More { count, last } => div()
                .id("work-more")
                .debug_selector(|| "work-more".into())
                .w_full()
                .bg(c.card)
                .when(last, |d| d.rounded_b(radius::POPOVER))
                .px(space::S5)
                .py(space::S3)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(format!("Show all {count}"))
                .on_click(cx.listener(|this, _, _, cx| this.act(Act::ShowAll, cx)))
                .into_any_element(),
            Item::Note { key, words, busy: true } => kuvo_ui::small::LoadingLabel::new(gpui::SharedString::from(key), words).into_any_element(),
            Item::Note { key, words, busy } => div()
                .debug_selector(move || key)
                .pt(space::S2)
                .flex()
                .items_start()
                .gap(space::S2)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .children(busy.then(|| div().pt(px(3.)).child(spinner("bots-reading", IconSize::Nav, c.text_muted))))
                .child(div().flex_1().min_w_0().child(words))
                .into_any_element(),
            Item::Templates { picked } => self.templates_grid(picked, cx),
            Item::Foot { error, creating } => div()
                .pt(space::S5)
                .flex()
                .items_center()
                .gap(space::S3)
                .child(div().flex_1().min_w_0().children(error.map(|error| div().debug_selector(|| "new-error".into()).text_ui(TypeSize::Small).text_color(c.danger).child(error))))
                .child(Button::new("new-cancel", "Cancel").disabled(creating).on_click(cx.listener(|this, _, _, cx| this.act(Act::CancelNew, cx))))
                .child(Button::new("new-create", "Create Bot").primary().loading(creating).on_click(cx.listener(|this, _, _, cx| this.act(Act::Create, cx))))
                .into_any_element(),
            Item::Empty { none } => self.empty(none, cx),
            Item::End => div().h(space::S10).into_any_element(),
        };
        // Each item in a column as wide as a page's cards, in the middle.
        div().w_full().flex().justify_center().px(space::S8).child(div().w_full().max_w(px(PAGE_WIDTH)).child(body)).into_any_element()
    }

    /// Who it is, what it's doing now, and the way into its conversation.
    fn head(&mut self, bot: &Bot, doing: Option<crate::areas::Doing>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let (working, asking) = (bot.now.working, bot.now.asking);
        // What it's doing is the box's (`now`): a count where a session waits on you, a spinner
        // where one's at work; with neither, when the newest finished, or nothing.
        let mark: Option<AnyElement> = if asking > 0 {
            Some(count_badge(asking, cx).into_any_element())
        } else if working > 0 {
            Some(spinner("bot-head-spin", IconSize::Nav, c.text_muted).into_any_element())
        } else {
            None
        };
        let now = doing.map(|doing| {
            div()
                .debug_selector(|| "bot-now".into())
                .flex()
                .items_center()
                .gap(space::S2)
                .pt(space::S1)
                .children(mark)
                .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(kuvo_ui::small::status_words(doing.words, doing.waiting, cx)))
        });
        div()
            .debug_selector(|| "bot-head".into())
            .pt(space::S8)
            .pb(space::S2)
            .flex()
            .items_center()
            .gap(space::S4)
            .child(bot_face(&bot.id, &bot.look, px(64.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().debug_selector(|| "bot-head-name".into()).truncate().text_ui(TypeSize::Title).medium().text_color(c.text).child(bot.name.clone()))
                    .when(!bot.job.is_empty(), |d| d.child(div().text_ui(TypeSize::Body).text_color(c.text_muted).child(bot.job.clone())))
                    .children(now),
            )
            .child(div().flex_none().child(Button::new("bot-chat", "Chat").primary().icon(IconName::Message).on_click(cx.listener(|this, _, _, cx| this.act(Act::Chat, cx)))))
            .into_any_element()
    }

    fn row(&mut self, row: Row, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Row { key, title, detail, control } = row;
        let control = self.control(&key, control, cx);
        let selector = key.clone();
        let detail_selector = format!("{key}-detail");
        div()
            .id(SharedString::from(key))
            .debug_selector(move || selector)
            .w_full()
            .bg(c.card)
            .when(first, |d| d.rounded_t(radius::POPOVER))
            .when(last, |d| d.rounded_b(radius::POPOVER))
            .shadow(if first && last { cx.theme().card_shadow() } else { Vec::new() })
            .px(space::S5)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .py(space::S4)
                    .when(!last, |d| d.border_b_1().border_color(c.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::S0_5)
                            .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                            .when(!detail.is_empty(), |d| d.child(div().debug_selector(move || detail_selector).text_ui(TypeSize::Small).text_color(c.text_muted).child(detail))),
                    )
                    .child(div().flex_none().max_w(px(320.)).child(control)),
            )
            .into_any_element()
    }

    fn control(&mut self, key: &str, control: Control, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let button = |key: &str, label: String, act: Act, danger: bool, cx: &mut Context<Self>| {
            let button = Button::new(SharedString::from(format!("{key}-button")), label).size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx)));
            if danger { button.destructive() } else { button }
        };
        match control {
            Control::Text(words) => {
                let selector = format!("{key}-value");
                div().debug_selector(move || selector).truncate().text_ui(TypeSize::Body).text_color(c.text_muted).child(words).into_any_element()
            }
            Control::Busy(words) => {
                // Dependencies can arrive separately. The sessions note owns progress while
                // present; otherwise only the first pending control says that we're reading.
                if words.starts_with("Loading") && self.loading_control.as_deref() != Some(key) {
                    div().h(px(24.)).into_any_element()
                } else {
                    kuvo_ui::small::LoadingLabel::new(SharedString::from(format!("{key}-busy")), words).compact().into_any_element()
                }
            },
            Control::Button(label, act) => button(key, label, act, false, cx).into_any_element(),
            Control::Danger(label, act) => button(key, label, act, true, cx).into_any_element(),
            Control::Face(look, label, act) => {
                let id = self.face_key();
                div().flex().items_center().gap(space::S3).children(look.map(|look| bot_face(&id, &look, px(28.)))).child(button(key, label, act, false, cx)).into_any_element()
            }
            Control::NameField => div().debug_selector(|| "new-name-field".into()).w(px(260.)).child(self.new_name.clone()).into_any_element(),
            Control::Model { shown, current, new } => {
                let (this, kept) = (cx.entity().downgrade(), cx.entity().downgrade());
                let client = self.client().clone();
                crate::model_picker::setting_button(
                    key,
                    shown,
                    move |window, cx| {
                        // (What the box last said, read as it opens rather than copied as it's drawn.)
                        let offered = this.upgrade().and_then(|view| view.read(cx).offered_models().cloned()).unwrap_or_default();
                        let mut catalog = Catalog::new(&offered, |_| true);
                        let is_new = new.is_some();
                        if let Some((title, line)) = new.clone() {
                            catalog = catalog.with_unset(title, line, current.is_none());
                        }
                        let (this, kept, client, current, starred) = (this.clone(), kept.clone(), client.clone(), current.clone(), offered.starred.clone());
                        cx.new(|cx| {
                            ModelPicker::new(
                                Arc::new(catalog),
                                starred,
                                current,
                                Some(client),
                                Rc::new(move |picked, _, cx| {
                                    let act = match (is_new, picked) {
                                        (true, picked) => Act::FormModel(picked),
                                        (false, Some(picked)) => Act::SetModel(picked),
                                        (false, None) => return,
                                    };
                                    this.update(cx, |this, cx| this.act(act, cx)).ok();
                                }),
                                window,
                                cx,
                            )
                            .refresh_when_opened(Some(offered), None, None, None, window, cx)
                            .on_starred(Rc::new(move |starred, cx| {
                                kept.update(cx, |this, _| this.keep_starred(starred)).ok();
                            }))
                        })
                    },
                    cx,
                )
            }
            Control::Menu { shown, choices } => {
                let this = cx.entity().downgrade();
                let id = SharedString::from(format!("{key}-menu"));
                let trigger = div()
                    .id(id.clone())
                    .debug_selector({
                        let id = id.to_string();
                        move || id
                    })
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .min_w(px(160.))
                    .max_w(px(280.))
                    .h(px(32.))
                    .px(space::S3)
                    .rounded(radius::NAV)
                    .border_1()
                    .border_color(c.border_strong)
                    .bg(c.card)
                    .cursor_pointer()
                    .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(shown))
                    .child(div().flex().flex_none().flex_col().child(Icon::new(IconName::ChevronUp).size(IconSize::Indicator).color(c.icon)).child(Icon::new(IconName::ChevronDown).size(IconSize::Indicator).color(c.icon)));
                // (The menu is made only as it opens.)
                PopoverMenu::new(SharedString::from(format!("{key}-popover")), trigger, move |_, cx| {
                    let (choices, this) = (choices.clone(), this.clone());
                    cx.new(|cx| {
                        let mut menu = Menu::new(cx).width(px(300.));
                        for (label, detail, act, checked) in choices {
                            let this = this.clone();
                            let mut entry = MenuEntry::new(label).checked(checked).on_select(move |_, cx| {
                                this.update(cx, |this, cx| this.act(act.clone(), cx)).ok();
                            });
                            if !detail.is_empty() {
                                entry = entry.detail(detail);
                            }
                            menu = menu.entry(entry);
                        }
                        menu
                    })
                })
                .align_right()
                .into_any_element()
            }
        }
    }

    /// One of its sessions: how it stands, its title and project, when it started; it opens there.
    fn session(&mut self, session: WorkSession, project: String, ago: String, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let id = session.session.clone();
        let mark: AnyElement = match session.state {
            WorkState::Working => {
                let selector = format!("work-working-{id}");
                div().debug_selector(move || selector).child(spinner(SharedString::from(format!("work-spin-{id}")), IconSize::Nav, c.text_muted)).into_any_element()
            }
            WorkState::Asking => {
                let selector = format!("work-asking-{id}");
                div().debug_selector(move || selector).flex().items_center().justify_center().size(px(16.)).child(status_dot(c.accent)).into_any_element()
            }
            WorkState::Failed => Icon::new(IconName::Warning).color(c.warning).into_any_element(),
            WorkState::Finished => Icon::new(IconName::Check).color(c.icon).into_any_element(),
            WorkState::Stopped | WorkState::Idle | WorkState::Other => Icon::new(IconName::Message).color(c.icon).into_any_element(),
        };
        let (line, waiting) = super::session_line(&project, session.state);
        let title = session.title.clone().filter(|title| !title.trim().is_empty()).unwrap_or_else(|| "New Session".into());
        let (selector, line_selector) = (format!("work-{id}"), format!("work-{id}-line"));
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .w_full()
            .bg(c.card)
            .when(first, |d| d.rounded_t(radius::POPOVER))
            .when(last, |d| d.rounded_b(radius::POPOVER))
            .px(space::S5)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.act(Act::OpenSession(id.clone()), cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .py(space::S3)
                    .when(!last, |d| d.border_b_1().border_color(c.border))
                    .child(div().flex().flex_none().items_center().justify_center().size(px(20.)).child(mark))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_ui(TypeSize::Body).text_color(c.text).child(title))
                            .when(!line.is_empty(), |d| d.child(div().debug_selector(move || line_selector).truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(kuvo_ui::small::status_words(line, waiting, cx)))),
                    )
                    .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(ago))
                    .child(Icon::new(IconName::ChevronRight).color(c.icon)),
            )
            .into_any_element()
    }

    /// New Bot's templates as the box offers them, with a blank one first.
    fn templates_grid(&mut self, picked: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let templates = self.templates().map(|templates| templates.templates.clone()).unwrap_or_default();
        let tile = |id: Option<String>, leading: AnyElement, name: String, line: String, on: bool, cx: &mut Context<Self>| {
            let selector = format!("template-{}", id.as_deref().unwrap_or("blank"));
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .w(px(228.))
                .h(px(104.))
                .flex()
                .flex_col()
                .gap(space::S2)
                .p(space::S3)
                .rounded(radius::POPOVER)
                .bg(c.card)
                .border_1()
                .border_color(if on { c.accent } else { c.border })
                .when(on, |d| d.shadow(cx.theme().selected_shadow()))
                .cursor_pointer()
                .hover(|s| s.border_color(if on { c.accent } else { c.border_strong }))
                .on_click(cx.listener(move |this, _, _, cx| this.act(Act::PickTemplate(id.clone()), cx)))
                .child(div().flex().items_center().gap(space::S2).child(leading).child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).medium().text_color(c.text).child(name)))
                .child(div().line_clamp(2).text_ellipsis().text_ui(TypeSize::Small).text_color(c.text_muted).child(line))
        };
        let blank = div().flex().flex_none().items_center().justify_center().size(px(28.)).rounded(radius::PILL).bg(c.sunken).child(Icon::new(IconName::Plus).color(c.icon)).into_any_element();
        let mut tiles = vec![tile(None, blank, "Blank".into(), "Tell it its job yourself, in its chat.".into(), picked.is_none(), cx)];
        for template in templates {
            let leading = face_or_blank(&format!("template:{}", template.id), template.look.as_ref(), px(28.), cx);
            tiles.push(tile(Some(template.id.clone()), leading, template.name, template.job, picked.as_ref() == Some(&template.id), cx));
        }
        div().debug_selector(|| "new-templates".into()).flex().flex_wrap().gap(space::S3).children(tiles).into_any_element()
    }

    /// Nothing picked: what the area is for, and the way to a new bot.
    fn empty(&mut self, none: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let words = if none { format!("There are no bots on {} yet. A bot keeps a job going by itself, and tells you when it needs you.", self.this_box) } else { "Pick a bot to see what it’s doing, or make a new one.".to_owned() };
        div()
            .debug_selector(|| "bots-empty".into())
            .h(px(480.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(space::S3)
            .child(div().flex().items_center().justify_center().size(px(48.)).rounded(radius::PILL).bg(c.sunken).child(Icon::new(IconName::Bot).size(IconSize::Card).color(c.icon)))
            .child(div().text_ui(TypeSize::Title).medium().text_color(c.text).child("Bots"))
            .child(div().max_w(px(420.)).text_center().text_ui(TypeSize::Body).text_color(c.text_muted).child(words))
            .child(div().pt(space::S2).child(Button::new("bots-empty-new", "New Bot").disabled(self.client.gate().is_held()).icon(IconName::Plus).on_click(cx.listener(|this, _, _, cx| this.act(Act::New, cx)))))
            .into_any_element()
    }

    // MARK: Sheets

    fn sheet_content(&mut self, sheet: Sheet, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let (subject_id, name) = self.subject().map(|(id, name, _)| (id, name)).unwrap_or_default();
        let label = |words: &'static str| div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words);
        let (title, width, body, save, danger): (String, f32, AnyElement, &'static str, bool) = match &sheet {
            Sheet::Name => (format!("Rename {name}"), 400., div().debug_selector(|| "sheet-name-field".into()).child(self.name.clone()).into_any_element(), "Save", false),
            Sheet::Job => (
                format!("{name}’s job"),
                520.,
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .child(label("In a few words"))
                    .child(div().debug_selector(|| "sheet-job-line".into()).child(self.line.clone()))
                    .child(div().pt(space::S2).child(label("All of it")))
                    .child(div().debug_selector(|| "sheet-job-details".into()).child(self.details.clone()))
                    .into_any_element(),
                "Save",
                false,
            ),
            Sheet::Look { look, new } => (if *new { "The new bot’s look".to_owned() } else { format!("{name}’s look") }, 440., self.look_picker(look, cx), if *new { "Use This Look" } else { "Save" }, false),
            Sheet::Delete => (
                format!("Delete {name}?"),
                440.,
                // What goes with it, and what stays, in the box's words, a line each.
                div()
                    .debug_selector(|| "sheet-deletes".into())
                    .flex()
                    .flex_col()
                    .gap(space::S1_5)
                    .children(self.deletes().into_iter().enumerate().map(|(ix, line)| {
                        let (icon, color) = if line.kept { (IconName::Check, c.text_muted) } else { (IconName::Trash, c.text) };
                        div()
                            .debug_selector(move || format!("sheet-delete-{ix}"))
                            .flex()
                            .items_start()
                            .gap(space::S2)
                            .child(div().flex_none().pt(px(1.)).child(Icon::new(icon).color(c.icon)))
                            .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).text_color(color).child(line.words))
                    }))
                    .into_any_element(),
                "Delete Bot",
                true,
            ),
        };
        let saving = self.saving;
        // (A delete the box said nothing of isn't offered: its confirm would show nothing.)
        let unsaid = matches!(sheet, Sheet::Delete) && self.deletes().is_empty();
        let go = Button::new("sheet-save", save).loading(saving).disabled(unsaid).on_click(cx.listener(move |this, _, _, cx| {
            if !unsaid {
                this.save_sheet(cx);
            }
        }));
        let enter_saves = matches!(sheet, Sheet::Look { .. });
        div()
            .id(SharedString::from(format!("bot-sheet-{:?}-{subject_id}", std::mem::discriminant(&sheet))))
            .debug_selector(|| "bot-sheet".into())
            .track_focus(&self.sheet_focus)
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| match event.keystroke.key.as_str() {
                "escape" => this.close_sheet(cx),
                "enter" if enter_saves => this.save_sheet(cx),
                _ => {}
            }))
            .w(px(width))
            .flex()
            .flex_col()
            .gap(space::S3)
            .p(space::S6)
            .rounded(radius::CARD)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(cx.theme().elevated_shadow())
            .child(div().debug_selector(|| "sheet-title".into()).text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
            .child(body)
            // Why the box turned it down, in its words.
            .children(self.sheet_error.clone().map(|error| div().debug_selector(|| "sheet-error".into()).text_ui(TypeSize::Small).text_color(c.danger).child(error)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(space::S2)
                    .pt(space::S2)
                    .child(Button::new("sheet-cancel", "Cancel").disabled(saving).on_click(cx.listener(|this, _, _, cx| this.close_sheet(cx))))
                    .child(if danger { go.destructive() } else { go.primary() }),
            )
            .animate_in(Direction::FromTop, true)
            .into_any_element()
    }

    /// The looks the box offers: its colours and shapes, with the face as it would be.
    fn look_picker(&mut self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Some(looks) = self.templates().map(|templates| templates.looks.clone()) else {
            let words = match &self.templates {
                crate::areas::Reading::Failed(why) => format!("The looks couldn’t be read: {why}"),
                _ => "Loading the looks…".to_owned(),
            };
            return div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words).into_any_element();
        };
        let label = |words: &'static str| div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words);
        let dark = cx.theme().appearance.is_dark();
        // The preview is the bot itself (the same creature as its other copies); New Bot's its own.
        let id = self.face_key();
        let colors = looks.colors.iter().map(|color| {
            let on = *color == look.color;
            let (selector, picked) = (format!("look-color-{color}"), color.clone());
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded(radius::PILL)
                .border_2()
                .border_color(if on { c.text } else { gpui::transparent_black() })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.pick_look(Some(&picked), None, cx)))
                .child(div().size(px(22.)).rounded(radius::PILL).bg(kuvo_ui::bot::swatch(color, dark)))
        });
        let colors: Vec<_> = colors.collect();
        let shapes: Vec<_> = looks
            .shapes
            .iter()
            .map(|shape| {
                let on = *shape == look.shape;
                let (selector, picked) = (format!("look-shape-{shape}"), shape.clone());
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(52.))
                    .rounded(radius::NAV)
                    .border_1()
                    .border_color(if on { c.accent } else { c.border })
                    .bg(c.card)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_look(None, Some(&picked), cx)))
                    .child(bot_face(&format!("look:{shape}"), &Look { color: look.color.clone(), shape: shape.clone(), face: look.face.clone() }, px(32.)))
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(space::S3)
            .child(div().debug_selector(|| "look-preview".into()).flex().justify_center().py(space::S2).child(bot_face(&id, look, px(72.))))
            .child(label("Colour"))
            .child(div().flex().flex_wrap().gap(space::S1).children(colors))
            .child(div().pt(space::S1).child(label("Shape")))
            .child(div().flex().flex_wrap().gap(space::S2).children(shapes))
            .into_any_element()
    }
}

/// A look's face, the creature `id`'s; with none (the box's to pick), a quiet blank of the same size.
fn face_or_blank(id: &str, look: Option<&Look>, size: gpui::Pixels, cx: &App) -> AnyElement {
    match look {
        Some(look) => bot_face(id, look, size),
        None => div().flex().flex_none().items_center().justify_center().size(size).rounded(radius::PILL).bg(cx.theme().colors.sunken).child(Icon::new(IconName::Bot).color(cx.theme().colors.icon)).into_any_element(),
    }
}

impl Render for BotsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        if self.client.gate().is_held() {
            return div().id("bots-page").debug_selector(|| "held-area".into()).size_full().bg(c.sidebar);
        }
        if let Some(focus) = self.wants_focus.take() {
            window.focus(&focus, cx);
        }
        let loading = self.items.iter().all(|item| matches!(item, Item::Title(_) | Item::End | Item::Note { busy: true, .. }))
            .then(|| self.items.iter().find_map(|item| match item { Item::Note { words, busy: true, .. } => Some(words.clone()), _ => None })).flatten();
        let page = match loading {
            Some(words) => div().size_full().flex().items_center().justify_center().child(kuvo_ui::small::LoadingLabel::new("page-loading", words)).into_any_element(),
            None => list(self.list.clone(), cx.processor(|this, ix, _, cx| this.item(ix, cx))).size_full().into_any_element(),
        };
        let problem = self.problem.clone().map(|problem| {
            div().absolute().top(space::S4).left_0().right_0().flex().justify_center().px(space::S8).child(
                div()
                    .debug_selector(|| "bots-problem".into())
                    .max_w(px(PAGE_WIDTH))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .px(space::S4)
                    .py(space::S3)
                    .rounded(radius::NAV)
                    .bg(c.card)
                    .border_1()
                    .border_color(c.danger)
                    .shadow(cx.theme().card_shadow())
                    .child(Icon::new(IconName::Warning).color(c.danger))
                    .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.text).child(problem))
                    .child(Button::icon_only("bots-problem-close", IconName::Close, "Close").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| {
                        this.problem = None;
                        cx.notify();
                    }))),
            )
        });
        let sheet = self.sheet.clone().map(|sheet| {
            let content = self.sheet_content(sheet, cx);
            div().id("bots-sheet").absolute().inset_0().flex().items_center().justify_center().p(space::S6).bg(dim(cx)).occlude().child(content)
        });
        div()
            .id("bots-page")
            .debug_selector(|| if matches!(self.shown, Shown::New) { "new-bot-page".into() } else { "bots-page".into() })
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(c.sidebar)
            .child(page)
            .children(problem)
            .children(sheet)
    }
}

/// What's under a sheet is dimmed by.
pub fn dim(cx: &App) -> Hsla {
    if cx.theme().appearance.is_dark() { gpui::hsla(0., 0., 0., 0.45) } else { gpui::hsla(0., 0., 0., 0.12) }
}
