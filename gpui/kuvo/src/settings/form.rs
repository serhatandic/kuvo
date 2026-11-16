//! A form a box's settings ask to be filled in before an action (PROTOCOL.md, "A box's settings",
//! `settings.flows`): a sheet over the page with the box's title, its fields in order (a line of
//! text, several, a segmented pick, a switch, a folder, names with values), the box's words under
//! each as you type (`…/check`, a moment after typing stops), and its button. Nothing here knows
//! what any form is for: what's checked, filled in, refused and done is the box's.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder as _, px, svg,
};
use kuvo_box::BoxClient;
use kuvo_box::settings_pages::{Changed, Checked, Field, FieldKind, FieldWords, Folder, Form, Sent, WordsTone};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TextField, TextFieldEvent, TypeSize,
    button::{Button, ButtonSize},
    menu::{Menu, MenuEntry, PopoverMenu},
    small::{Switch, spinner},
    theme::{MONO_FONT, radius, space},
};
use serde_json::{Map, Value, json};

use kuvo_box::accounts::said;

/// The most choices a `choice` field shows side by side; more are a menu.
const MOST_SEGMENTS: usize = 4;

/// How long after the last key the values are checked.
const SETTLE: Duration = Duration::from_millis(300);

/// What the sheet came to, for the page under it.
pub enum FormEvent {
    /// Sent and done: the page as the box has it now.
    Done(Changed),
    /// What it shows changed since it opened: the page as it is now (the sheet stays).
    Stale(kuvo_box::settings_pages::Page),
    Cancelled,
    /// A folder field's Choose…: the page's picker, starting at `start`.
    ChooseFolder { field: String, words: Folder, start: String },
}

/// Where the form's values go: the section, the row and the revision it was read at, the action.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub section: String,
    pub row: String,
    pub was: String,
    pub action: String,
}

/// A `pairs` field's rows: each name's field and its value's.
type Pairs = Vec<(Entity<TextField>, Entity<TextField>)>;

pub struct FormSheet {
    client: BoxClient,
    live: bool,
    target: Target,
    form: Form,
    /// The values of the fields without a text field (a pick, a switch).
    values: Map<String, Value>,
    texts: HashMap<String, Entity<TextField>>,
    pairs: HashMap<String, Pairs>,
    checked: Option<Checked>,
    refused: Option<String>,
    sending: bool,
    checking: Option<Task<()>>,
    work: Option<Task<()>>,
    focus: FocusHandle,
    wants_focus: bool,
    subscriptions: Vec<Subscription>,
}

impl EventEmitter<FormEvent> for FormSheet {}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn pairs_of(value: Option<&Value>) -> Vec<(String, String)> {
    value.and_then(Value::as_array).map(|list| list.iter().map(|each| (text_of(each.get("key")), text_of(each.get("value")))).collect()).unwrap_or_default()
}

impl FormSheet {
    pub fn new(client: BoxClient, live: bool, target: Target, form: Form, cx: &mut Context<Self>) -> FormSheet {
        let mut sheet = FormSheet {
            client,
            live,
            target,
            values: Map::new(),
            texts: HashMap::new(),
            pairs: HashMap::new(),
            checked: None,
            refused: None,
            sending: false,
            checking: None,
            work: None,
            focus: cx.focus_handle(),
            wants_focus: true,
            subscriptions: Vec::new(),
            form: form.clone(),
        };
        for field in &form.fields {
            match field.kind {
                FieldKind::Text | FieldKind::Lines | FieldKind::Folder => {
                    let multi = field.kind == FieldKind::Lines;
                    let entity = sheet.text_field(field.placeholder.clone().unwrap_or_default(), multi, text_of(field.value.as_ref()), field.why_not().is_some(), cx);
                    sheet.texts.insert(field.id.clone(), entity);
                }
                FieldKind::Pairs => {
                    let made = pairs_of(field.value.as_ref()).into_iter().map(|(key, value)| sheet.pair(field, key, value, cx)).collect();
                    sheet.pairs.insert(field.id.clone(), made);
                }
                _ => {
                    sheet.values.insert(field.id.clone(), field.value.clone().unwrap_or(Value::Null));
                }
            }
        }
        sheet
    }

    fn text_field(&mut self, placeholder: String, multi: bool, text: String, disabled: bool, cx: &mut Context<Self>) -> Entity<TextField> {
        let entity = cx.new(|cx| {
            let mut field = TextField::new(cx).bare().placeholder(placeholder);
            if multi {
                field = field.multi_line(2, 6).submits_on_enter(false);
            }
            field.set_text(text, cx);
            field.set_disabled(disabled, cx);
            field
        });
        self.subscriptions.push(cx.subscribe(&entity, |this: &mut FormSheet, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Changed => this.changed(cx),
            TextFieldEvent::Submit(_) => this.submit(cx),
            TextFieldEvent::Cancel => this.cancel(cx),
        }));
        entity
    }

    fn pair(&mut self, field: &Field, key: String, value: String, cx: &mut Context<Self>) -> (Entity<TextField>, Entity<TextField>) {
        let words = field.pairs.clone();
        let (key_words, value_words) = words.map_or((String::new(), String::new()), |words| (words.key, words.value));
        let disabled = field.why_not().is_some();
        (self.text_field(key_words, false, key, disabled, cx), self.text_field(value_words, false, value, disabled, cx))
    }

    /// Checks the values as they are now, at once (as it opens; and in tests and shots).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.check_now(cx);
    }

    /// The form's values as they are now, every field's, those hidden too (kept while it's open).
    pub fn values(&self, cx: &App) -> Map<String, Value> {
        let mut values = self.values.clone();
        for (id, field) in &self.texts {
            values.insert(id.clone(), Value::String(field.read(cx).text().to_owned()));
        }
        for (id, pairs) in &self.pairs {
            let list = pairs.iter().map(|(key, value)| json!({ "key": key.read(cx).text(), "value": value.read(cx).text() })).collect();
            values.insert(id.clone(), Value::Array(list));
        }
        values
    }

    /// What's typed in a field (for tests and shots).
    pub fn set_text(&mut self, field: &str, text: &str, cx: &mut Context<Self>) {
        if let Some(entity) = self.texts.get(field).cloned() {
            entity.update(cx, |field, cx| field.set_text(text, cx));
        }
        self.check_now(cx);
    }

    pub fn pick(&mut self, field: &str, value: Value, cx: &mut Context<Self>) {
        self.values.insert(field.to_owned(), value);
        self.changed(cx);
        cx.notify();
    }

    /// A folder chosen in the page's picker.
    pub fn chose_folder(&mut self, field: &str, path: &str, cx: &mut Context<Self>) {
        if let Some(entity) = self.texts.get(field).cloned() {
            entity.update(cx, |field, cx| field.set_text(path, cx));
        }
        self.changed(cx);
    }

    pub fn checked(&self) -> Option<&Checked> {
        self.checked.as_ref()
    }

    pub fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    pub fn is_ready(&self) -> bool {
        !self.sending && self.checked.as_ref().is_some_and(|checked| checked.ready)
    }

    /// Something typed: checked a moment after the last key (at once, where there's no waiting).
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.refused = None;
        if !self.live {
            self.check_now(cx);
            return;
        }
        self.checking = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            this.update(cx, |this, cx| this.check_now(cx)).ok();
        }));
    }

    fn check_now(&mut self, cx: &mut Context<Self>) {
        let values = self.values(cx);
        let (client, target) = (self.client.clone(), self.target.clone());
        let work = move || client.check_setting_form(&target.section, &target.row, &target.was, &target.action, &values);
        let then = |this: &mut FormSheet, done: kuvo_box::Result<Checked>, cx: &mut Context<FormSheet>| {
            match done {
                Ok(checked) => {
                    if let Some(fill) = checked.values.clone() {
                        this.fill(fill, cx);
                    }
                    this.checked = Some(checked);
                }
                Err(error) => this.refused = Some(said(&error)),
            }
            cx.notify();
        };
        if !self.live {
            let done = work();
            then(self, done, cx);
            return;
        }
        self.checking = Some(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work() }).await;
            this.update(cx, |this, cx| then(this, done, cx)).ok();
        }));
    }

    /// Values the box filled in (from a setup pasted into a field), put in their fields.
    fn fill(&mut self, fill: Map<String, Value>, cx: &mut Context<Self>) {
        let fields = self.form.fields.clone();
        for (id, value) in fill {
            if let Some(entity) = self.texts.get(&id).cloned() {
                entity.update(cx, |field, cx| field.set_text(text_of(Some(&value)), cx));
            } else if let Some(field) = fields.iter().find(|field| field.id == id && field.kind == FieldKind::Pairs) {
                let made = pairs_of(Some(&value)).into_iter().map(|(key, value)| self.pair(field, key, value, cx)).collect();
                self.pairs.insert(id, made);
            } else {
                self.values.insert(id, value);
            }
        }
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.is_ready() {
            return;
        }
        self.sending = true;
        let values = self.values(cx);
        let (client, target) = (self.client.clone(), self.target.clone());
        let work = move || client.setting_form_at(&target.section, &target.row, &target.was, &target.action, &values);
        let then = |this: &mut FormSheet, done: kuvo_box::Result<Sent>, cx: &mut Context<FormSheet>| {
            this.sending = false;
            match done {
                Ok(Sent::Changed(Changed::Stale { message, page })) => {
                    // Kept open with what was typed, said; sent again, it goes from what's there now.
                    if let Some(revision) = page.row(&this.target.row).and_then(|row| row.actions.iter().chain(row.add.iter()).find(|action| action.id == this.target.action)).and_then(|action| action.form.as_ref()).and_then(|form| form.revision.clone()) {
                        this.target.was = revision;
                    }
                    this.refused = Some(message);
                    cx.emit(FormEvent::Stale(page));
                }
                Ok(Sent::Changed(changed)) => cx.emit(FormEvent::Done(changed)),
                Ok(Sent::Refused { message, fields }) => {
                    this.refused = Some(message);
                    this.checked = Some(Checked { fields, ready: false, values: None });
                }
                Err(error) => this.refused = Some(said(&error)),
            }
            cx.notify();
        };
        if !self.live {
            let done = work();
            then(self, done, cx);
            return;
        }
        self.work = Some(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work() }).await;
            this.update(cx, |this, cx| then(this, done, cx)).ok();
        }));
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(FormEvent::Cancelled);
    }

    fn words_for(&self, id: &str) -> Vec<FieldWords> {
        self.checked.as_ref().map(|checked| checked.fields.iter().filter(|each| each.id == id).cloned().collect()).unwrap_or_default()
    }
}

/// A text field's frame, as Settings' rows draw theirs.
fn framed(selector: String, field: Entity<TextField>, multi: bool, mono: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .debug_selector(move || selector)
        .w_full()
        .min_h(px(if multi { 56. } else { 32. }))
        .px(space::S3)
        .when(multi, |d| d.py(space::S2))
        .flex()
        .when(!multi, |d| d.items_center())
        .rounded(radius::NAV)
        .border_1()
        .border_color(c.border_strong)
        .bg(c.card)
        .when(mono, |d| d.font_family(MONO_FONT))
        .child(div().flex_1().min_w_0().child(field))
        .into_any_element()
}

impl Render for FormSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        if std::mem::take(&mut self.wants_focus) {
            let first = self.form.fields.iter().find(|field| self.texts.contains_key(&field.id) && field.why_not().is_none() && field.is_shown(&self.values(cx))).and_then(|field| self.texts.get(&field.id)).cloned();
            match first {
                Some(field) => window.focus(&field.focus_handle(cx), cx),
                None => window.focus(&self.focus, cx),
            }
        }
        let values = self.values(cx);
        let fields: Vec<Field> = self.form.fields.iter().filter(|field| field.is_shown(&values)).cloned().collect();
        let mut body: Vec<AnyElement> = Vec::new();
        for field in fields {
            body.push(self.field(&field, &values, cx));
        }
        let ready = self.is_ready();
        let footer_note = self.form.note.clone();
        div()
            .id("settings-form")
            .debug_selector(|| "settings-form".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.cancel(cx);
                }
            }))
            .w(px(540.))
            .max_w_full()
            .max_h(px(640.))
            .flex()
            .flex_col()
            .rounded(radius::CARD)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(cx.theme().elevated_shadow())
            .occlude()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .px(space::S6)
                    .pt(space::S6)
                    .pb(space::S3)
                    .child(div().debug_selector(|| "settings-form-title".into()).flex_1().min_w_0().truncate().text_ui(TypeSize::Body).medium().text_color(c.text).child(self.form.title.clone()))
                    .child(Button::icon_only("settings-form-x", IconName::Close, "Close").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))),
            )
            .child(div().id("settings-form-fields").flex_1().min_h_0().overflow_y_scroll().px(space::S6).py(space::S2).flex().flex_col().gap(space::S4).children(body))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S3)
                    .px(space::S6)
                    .pt(space::S3)
                    .pb(space::S6)
                    .border_t_1()
                    .border_color(c.border)
                    .children(self.refused.clone().map(|why| {
                        div()
                            .debug_selector(|| "settings-form-refused".into())
                            .flex()
                            .items_start()
                            .gap(space::S2)
                            .text_ui(TypeSize::Small)
                            .text_color(c.text)
                            .child(div().pt(px(2.)).child(Icon::new(IconName::Warning).size(IconSize::Nav).color(c.danger)))
                            .child(div().flex_1().min_w_0().child(why))
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.text_muted).children(footer_note))
                            .when(self.sending, |d| d.child(spinner("settings-form-sending", IconSize::Nav, c.text_muted)))
                            .child(Button::new("settings-form-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| this.cancel(cx))))
                            .child(Button::new("settings-form-submit", self.form.submit.clone()).primary().disabled(!ready).on_click(cx.listener(|this, _, _, cx| this.submit(cx)))),
                    ),
            )
    }
}

impl FormSheet {
    fn field(&mut self, field: &Field, values: &Map<String, Value>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let id = field.id.clone();
        let selector = format!("settings-form-{id}");
        let why = field.why_not().map(str::to_owned).filter(|why| !why.is_empty());
        let disabled = why.is_some();
        let control: AnyElement = match field.kind {
            FieldKind::Text | FieldKind::Lines => match self.texts.get(&id).cloned() {
                Some(entity) => framed(format!("settings-form-{id}-field"), entity, field.kind == FieldKind::Lines, field.mono, cx),
                None => div().into_any_element(),
            },
            FieldKind::Folder => {
                let entity = self.texts.get(&id).cloned();
                let words = field.folder.clone();
                let has = values.get(&id).and_then(Value::as_str).is_some_and(|path| !path.is_empty());
                let start = values.get(&id).and_then(Value::as_str).filter(|path| !path.is_empty()).unwrap_or("~").to_owned();
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .children(entity.map(|entity| div().flex_1().min_w_0().child(framed(format!("settings-form-{id}-field"), entity, false, false, cx))))
                    .children(words.clone().filter(|_| !disabled).map(|words| {
                        let (id, start) = (id.clone(), start.clone());
                        Button::new(SharedString::from(format!("settings-form-{id}-choose")), words.choose.clone()).size(ButtonSize::Small).on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(FormEvent::ChooseFolder { field: id.clone(), words: words.clone(), start: start.clone() });
                        }))
                    }))
                    .children(words.filter(|_| has && !disabled).map(|words| {
                        let id = id.clone();
                        Button::new(SharedString::from(format!("settings-form-{id}-clear")), words.clear).ghost().size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.chose_folder(&id, "", cx)))
                    }))
                    .into_any_element()
            }
            // More choices than fit side by side (a model): a menu, as a choice row's.
            FieldKind::Choice if field.choices.len() > MOST_SEGMENTS => {
                let chosen = values.get(&id).and_then(Value::as_str).unwrap_or_default().to_owned();
                let shown = field.choices.iter().find(|choice| choice.id == chosen).map(|choice| choice.title.clone()).unwrap_or_else(|| chosen.clone());
                let trigger_id = SharedString::from(format!("settings-form-{id}-menu"));
                let trigger = div()
                    .id(trigger_id.clone())
                    .debug_selector({
                        let id = trigger_id.to_string();
                        move || id
                    })
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .h(px(32.))
                    .px(space::S3)
                    .rounded(radius::NAV)
                    .border_1()
                    .border_color(c.border_strong)
                    .bg(c.card)
                    .when(!disabled, |d| d.cursor_pointer())
                    .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).text_color(if disabled { c.text_muted } else { c.text }).child(if shown.is_empty() { "—".to_owned() } else { shown }))
                    .child(div().flex().flex_none().flex_col().child(svg().path(IconName::ChevronUp.path()).size(px(10.)).text_color(c.icon)).child(svg().path(IconName::ChevronDown.path()).size(px(10.)).text_color(c.icon)));
                if disabled {
                    trigger.into_any_element()
                } else {
                    let (choices, this) = (field.choices.clone(), cx.entity().downgrade());
                    let wide = choices.iter().any(|choice| choice.line.is_some());
                    PopoverMenu::new(SharedString::from(format!("settings-form-{id}-popover")), trigger, move |_, cx| {
                        let (choices, this, id, chosen) = (choices.clone(), this.clone(), id.clone(), chosen.clone());
                        cx.new(|cx| {
                            let mut menu = Menu::new(cx).width(px(if wide { 360. } else { 240. }));
                            for choice in choices {
                                let (this, id, pick) = (this.clone(), id.clone(), choice.id.clone());
                                let mut entry = MenuEntry::new(choice.title.clone()).checked(choice.id == chosen).on_select(move |_, cx| {
                                    this.update(cx, |this, cx| this.pick(&id, Value::String(pick.clone()), cx)).ok();
                                });
                                if let Some(line) = choice.line.clone() {
                                    entry = entry.detail(line);
                                }
                                menu = menu.entry(entry);
                            }
                            menu
                        })
                    })
                    .into_any_element()
                }
            }
            FieldKind::Choice => {
                let chosen = values.get(&id).and_then(Value::as_str).unwrap_or_default().to_owned();
                div()
                    .flex()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(radius::NAV)
                    .bg(c.sunken)
                    .children(field.choices.iter().map(|choice| {
                        let on = choice.id == chosen;
                        let (id, pick) = (id.clone(), choice.id.clone());
                        let selector = format!("settings-form-{id}-{pick}");
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector)
                            .flex_1()
                            .flex()
                            .justify_center()
                            .py(space::S1)
                            .px(space::S3)
                            .rounded(radius::NAV)
                            .text_ui(TypeSize::Small)
                            .when(on, |d| d.bg(c.card).shadow(cx.theme().card_shadow()).text_color(c.text).medium())
                            .when(!on, |d| d.text_color(c.text_muted).when(!disabled, |d| d.cursor_pointer().hover(|s| s.text_color(c.text))))
                            .child(choice.title.clone())
                            .when(!disabled && !on, |d| d.on_click(cx.listener(move |this, _, _, cx| this.pick(&id, Value::String(pick.clone()), cx))))
                    }))
                    .into_any_element()
            }
            FieldKind::Switch => {
                let on = values.get(&id).and_then(Value::as_bool).unwrap_or(false);
                let this = cx.entity().downgrade();
                let switch = Switch::new(SharedString::from(format!("settings-form-{id}-switch")), on).disabled(disabled);
                let switch = if disabled {
                    switch
                } else {
                    switch.on_change(move |now, _, cx| {
                        let id = id.clone();
                        this.update(cx, |this, cx| this.pick(&id, Value::Bool(now), cx)).ok();
                    })
                };
                // A switch sits beside its title, as Settings' rows draw one.
                return div()
                    .debug_selector(move || selector)
                    .flex()
                    .items_start()
                    .gap(space::S3)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::S0_5)
                            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(field.title.clone()))
                            .children(field.line.clone().map(|line| div().text_ui(TypeSize::Small).text_color(c.text_muted).child(line)))
                            .children(why.map(|why| div().text_ui(TypeSize::Small).text_color(c.text_faint).child(why)))
                            .children(self.words_under(&field.id, cx)),
                    )
                    .child(div().flex_none().child(switch))
                    .into_any_element();
            }
            FieldKind::Pairs => {
                let rows: Vec<AnyElement> = self
                    .pairs
                    .get(&id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (key, value))| {
                        let id = id.clone();
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .child(div().w(px(180.)).flex_none().child(framed(format!("settings-form-{id}-key-{ix}"), key, false, true, cx)))
                            .child(div().flex_1().min_w_0().child(framed(format!("settings-form-{id}-value-{ix}"), value, false, false, cx)))
                            .when(!disabled, |d| {
                                d.child(Button::icon_only(SharedString::from(format!("settings-form-{id}-remove-{ix}")), IconName::Close, "Remove").ghost().size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(list) = this.pairs.get_mut(&id)
                                        && ix < list.len()
                                    {
                                        list.remove(ix);
                                    }
                                    this.changed(cx);
                                    cx.notify();
                                })))
                            })
                            .into_any_element()
                    })
                    .collect();
                let add_field = field.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .children(rows)
                    .when(!disabled, |d| {
                        d.child(div().flex().child(Button::new(SharedString::from(format!("settings-form-{id}-add")), "Add").size(ButtonSize::Small).on_click(cx.listener(move |this, _, window, cx| {
                            let made = this.pair(&add_field, String::new(), String::new(), cx);
                            window.focus(&made.0.focus_handle(cx), cx);
                            this.pairs.entry(add_field.id.clone()).or_default().push(made);
                            cx.notify();
                        }))))
                    })
                    .into_any_element()
            }
            FieldKind::Unknown => div().text_ui(TypeSize::Small).text_color(c.text_muted).child("A newer Kuvo changes this").into_any_element(),
        };
        div()
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(field.title.clone()))
            .child(control)
            .children(field.line.clone().map(|line| div().text_ui(TypeSize::Small).text_color(c.text_muted).child(line)))
            .children(why.map(|why| div().text_ui(TypeSize::Small).text_color(c.text_faint).child(why)))
            .children(self.words_under(&field.id, cx))
            .into_any_element()
    }

    /// The box's words under a field: what it'll do, that it's there, or what's wrong.
    fn words_under(&self, id: &str, cx: &App) -> Vec<AnyElement> {
        let c = &cx.theme().colors;
        self.words_for(id)
            .into_iter()
            .enumerate()
            .map(|(ix, words)| {
                let selector = format!("settings-form-{id}-words-{ix}");
                let (icon, color) = match words.tone {
                    WordsTone::Good => (Some(IconName::Check), c.success),
                    WordsTone::Warning => (Some(IconName::Warning), c.warning),
                    WordsTone::Quiet => (None, c.text_faint),
                };
                div()
                    .debug_selector(move || selector)
                    .flex()
                    .items_start()
                    .gap(space::S1_5)
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_muted)
                    .children(icon.map(|icon| div().pt(px(2.)).child(Icon::new(icon).size(IconSize::Nav).color(color))))
                    .child(div().flex_1().min_w_0().child(words.words))
                    .into_any_element()
            })
            .collect()
    }
}
