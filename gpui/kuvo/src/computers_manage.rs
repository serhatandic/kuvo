//! What's done with your computers from their pages beyond a button the box offers: a name edited in
//! place, who leads your setup (its main computer and standby, `kuvo/setup`, changed through
//! `kuvo/setup/handover` and `kuvo/setup/standby`), and removing one (`kuvo/computers/<key>/unlink`
//! or `…/unlink-and-wipe`), with what the box says of it after. Every decision is the box's: what
//! it offers (`actions`), what its Kuvo answers (`features`), and how a change went. An action a
//! computer's Kuvo doesn't answer isn't drawn at all.

use gpui::{AnyElement, AppContext as _, Entity, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use kuvo_box::accounts::said;
use kuvo_box::areas::{Computer, ComputerRemoval};
use kuvo_box::setup::{SetupLeadershipReply, SetupOffer, SetupView};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    button::{Button, ButtonSize},
    small::{Chip, ChipTone},
    text_field::{TextField, TextFieldEvent},
    theme::{radius, space},
};

use super::{ComputersEvent, ComputersView, Item, Tone, card_row, standing, tile};

/// What a computer is in your setup, where it's more than a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Main,
    Standby,
}

/// A sheet over the page.
#[derive(Clone, Debug, PartialEq)]
pub enum Sheet {
    /// Change the main computer (`handover`) or choose the standby, `picked` the one chosen.
    Lead { handover: bool, picked: Option<String> },
    /// Remove another computer from your setup: unlinked, or unlinked and wiped.
    Remove { key: String, wipe: bool },
    /// Remove the computer this window shows, the only one in your setup: it's wiped.
    RemoveLast { key: String },
}

/// A row under Role in This Setup: its key, title, chip, words and its one way on.
type RoleRow = (String, String, Option<Role>, String, Option<(String, Sheet)>);

/// One computer a leadership sheet offers.
struct Candidate {
    key: String,
    name: String,
    detail: String,
    badge: Option<&'static str>,
}

/// What a computer's Kuvo answers (`features`): none where it didn't say.
fn has(computer: &Computer, feature: &str) -> bool {
    computer.features.as_ref().is_some_and(|features| features.iter().any(|has| has == feature))
}

/// A role's chip, as rows and heads draw it.
pub fn chip(role: Role, id: &str) -> AnyElement {
    let id = SharedString::from(format!("role-{id}"));
    let selector = format!("role-chip-{}", match role { Role::Main => "main", Role::Standby => "standby" });
    let chip = match role {
        Role::Main => Chip::new(id, "Main computer").tone(ChipTone::Accent).icon(IconName::StarFilled),
        Role::Standby => Chip::new(id, "Standby").tone(ChipTone::Neutral).icon(IconName::Shield),
    };
    div().debug_selector(move || selector).flex_none().child(chip).into_any_element()
}

/// The words a removal comes to, as the box said how it stands: a title, more where there's more,
/// and its tone. `again`: it didn't finish, and asking again goes on with it.
fn removal_words(name: &str, removal: &ComputerRemoval) -> (String, String, Tone, bool) {
    let message = removal.message.clone().unwrap_or_default();
    match removal.state() {
        None | Some("unlinked") => (format!("{name} was removed from your setup"), message, Tone::Info, false),
        Some("wipe-pending") => (format!("{name} was removed from your setup"), if message.is_empty() { "Kuvo there is wiped the next time it comes online.".into() } else { message }, Tone::Waiting, false),
        Some("wiped") => (format!("{name} was removed and wiped"), message, Tone::Info, false),
        Some("saving" | "moving" | "exporting") => (format!("Removing {name}…"), if message.is_empty() { "Your computers are saving the change. It shows here once it’s saved.".into() } else { message }, Tone::Busy, false),
        Some(_) => (format!("Removing {name} didn’t finish"), message, Tone::Attention, true),
    }
}

impl ComputersView {
    /// Reads your setup again (off this thread in the app); a box that doesn't say leaves no roles
    /// drawn.
    pub(super) fn read_setup(&mut self, cx: &mut Context<Self>) {
        // (Asked only of a box that says it answers it: an older one, or one held for its update,
        // is asked nothing more.)
        if !has(&self.this(), "setup") {
            self.setup = None;
            self._setup_read = None;
            return;
        }
        let client = self.client.clone();
        if !self.live {
            self.setup = client.setup_view().ok();
            return;
        }
        self._setup_read = Some(cx.spawn(async move |this, cx| {
            let read = cx.background_executor().spawn(async move { client.setup_view() }).await;
            this.update(cx, |this, cx| {
                let setup = read.ok();
                if this.setup != setup {
                    this.setup = setup;
                    this.rebuild(cx);
                }
            })
            .ok();
        }));
    }

    /// Your setup where it has more than one computer to lead.
    fn led(&self) -> Option<&SetupView> {
        self.setup.as_ref().filter(|setup| setup.is_set_up() && setup.computers.len() > 1)
    }

    /// What `key` is in your setup.
    pub(super) fn role_of(&self, key: &str) -> Option<Role> {
        let setup = self.led()?;
        if key.is_empty() {
            return None;
        }
        if setup.primary_key() == Some(key) {
            Some(Role::Main)
        } else if setup.standby.as_deref() == Some(key) {
            Some(Role::Standby)
        } else {
            None
        }
    }

    /// A computer of yours by key, as the list has it (the one this window shows, named as it is).
    fn listed(&self, key: &str) -> Option<Computer> {
        let this = self.this();
        if this.key == key {
            return Some(this);
        }
        self.other(key)
    }

    fn setup_name(&self, key: &str) -> String {
        self.listed(key).map(|computer| computer.name).or_else(|| self.led().and_then(|setup| setup.member(key)).map(|member| member.name.clone())).unwrap_or_default()
    }

    /// Whom a sheet offers: your setup's computers but its main one, reached and answering the
    /// change (the main computer's handover needs `setup.handover` of each; a standby,
    /// `setup.shared-saves`). Those that can't be chosen now aren't offered.
    fn candidates(&self, handover: bool) -> Vec<Candidate> {
        let Some(setup) = self.led() else { return Vec::new() };
        let needs = if handover { "setup.handover" } else { "setup.shared-saves" };
        setup
            .computers
            .iter()
            .filter(|member| Some(member.key.as_str()) != setup.primary_key())
            // (A standby is chosen from the others: the one it is now is no choice.)
            .filter(|member| handover || setup.standby.as_deref() != Some(member.key.as_str()))
            .filter_map(|member| {
                let computer = self.listed(&member.key)?;
                let reached = computer.here || computer.reach == "connected";
                (reached && has(&computer, needs)).then(|| Candidate {
                    key: member.key.clone(),
                    name: computer.name.clone(),
                    detail: standing(&computer),
                    badge: (setup.standby.as_deref() == Some(member.key.as_str())).then_some("Current standby"),
                })
            })
            .collect()
    }

    /// Whether the box this window shows changes who leads (`setup.handover`, `setup.takeover`),
    /// and there's someone to change to.
    fn leads(&self, handover: bool) -> bool {
        let this = self.this();
        has(&this, if handover { "setup.handover" } else { "setup.takeover" }) && !self.candidates(handover).is_empty()
    }

    /// Who leads your setup, over all your computers.
    pub(super) fn roles_item(&self, yours: &[Computer]) -> Option<Item> {
        let setup = self.led()?;
        let primary = setup.primary_key()?;
        let name = |key: &str| yours.iter().find(|computer| computer.key == key).map(|computer| computer.name.clone()).unwrap_or_else(|| self.setup_name(key));
        Some(Item::Roles {
            main: name(primary),
            standby: setup.standby.as_deref().map(name).filter(|name| !name.is_empty()),
            warning: setup.leadership_warning.clone(),
            change_main: self.leads(true),
            choose_standby: self.leads(false),
        })
    }

    /// A computer's rows under Role in This Setup: its main computer, and its standby.
    pub(super) fn role_items(&self, computer: &Computer) -> Vec<Item> {
        let Some(setup) = self.led() else { return Vec::new() };
        let Some(primary) = setup.primary_key() else { return Vec::new() };
        if setup.member(&computer.key).is_none() {
            return Vec::new();
        }
        let name = &computer.name;
        let main = self.setup_name(primary);
        let is_main = primary == computer.key;
        let is_standby = setup.standby.as_deref() == Some(computer.key.as_str());
        let mut rows: Vec<RoleRow> = Vec::new();
        let hands = self.leads(true);
        let offered = |handover: bool| self.candidates(handover).iter().any(|candidate| candidate.key == computer.key);
        if is_main {
            rows.push(("role-main".into(), "Main computer".into(), None, format!("{name} approves devices, manages membership, and coordinates updates."), hands.then(|| ("Change…".to_owned(), Sheet::Lead { handover: true, picked: None }))));
        } else if hands && offered(true) {
            rows.push(("role-main".into(), "Main computer".into(), None, format!("{main} is currently the main computer. Hand over to make {name} main."), Some(("Make Main Computer…".to_owned(), Sheet::Lead { handover: true, picked: Some(computer.key.clone()) }))));
        }
        if !is_main {
            let chooses = self.leads(false);
            if is_standby {
                rows.push(("role-standby".into(), "Standby".into(), None, format!("{name} takes over if the main computer is lost."), chooses.then(|| ("Change…".to_owned(), Sheet::Lead { handover: false, picked: None }))));
            } else if chooses && offered(false) {
                let words = match setup.standby.as_deref() {
                    Some(standby) => format!("{} takes over if the main computer is lost. Make {name} the standby instead.", self.setup_name(standby)),
                    None => format!("Make {name} the computer that takes over if the main computer is lost."),
                };
                rows.push(("role-standby".into(), "Standby".into(), None, words, Some(("Make Standby…".to_owned(), Sheet::Lead { handover: false, picked: Some(computer.key.clone()) }))));
            }
        }
        if rows.is_empty() {
            return Vec::new();
        }
        let count = rows.len();
        let mut items = vec![Item::Group("Role in this setup".into())];
        items.extend(rows.into_iter().enumerate().map(|(ix, (key, title, chip, words, button))| Item::Role { key, title, chip, words, button, first: ix == 0, last: ix + 1 == count }));
        items
    }

    // MARK: Renaming

    /// Its name, as a field in its page's head, all of it picked.
    pub(super) fn start_rename(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(computer) = self.listed(key) else { return };
        let field = cx.new(|cx| {
            let mut field = TextField::new(cx).placeholder("Name").submits_on_enter(true);
            field.set_text(computer.name.clone(), cx);
            let all = 0..field.text().len();
            field.select_range(all, cx);
            field
        });
        self._rename_typed = Some(cx.subscribe(&field, |this: &mut ComputersView, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submit(_) => this.save_rename(cx),
            TextFieldEvent::Cancel => this.cancel_rename(cx),
            TextFieldEvent::Changed => {}
        }));
        self.focus_rename = true;
        self.problem = None;
        self.renaming = Some((key.to_owned(), field));
        self.rebuild(cx);
    }

    pub(super) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming = None;
        self.rename_busy = None;
        self._rename_typed = None;
        self.rebuild(cx);
    }

    /// Asks the box to name it so (it hands it to the box that writes its record); a refusal is
    /// shown in its words, the field left as typed.
    pub(super) fn save_rename(&mut self, cx: &mut Context<Self>) {
        let Some((key, field)) = self.renaming.clone() else { return };
        if self.rename_busy.is_some() {
            return;
        }
        let name = field.read(cx).text().trim().to_owned();
        let unchanged = self.listed(&key).is_some_and(|computer| computer.name == name);
        if name.is_empty() || unchanged {
            return self.cancel_rename(cx);
        }
        let client = self.client.clone();
        let on = self.shown.clone();
        let then = move |this: &mut ComputersView, done: kuvo_box::Result<()>, cx: &mut Context<ComputersView>| {
            this.rename_busy = None;
            match done {
                Ok(()) => {
                    this.renaming = None;
                    this._rename_typed = None;
                    this.read_setup(cx);
                    cx.emit(ComputersEvent::ReadAgain);
                }
                Err(error) => this.problem = Some((on, said(&error))),
            }
            this.rebuild(cx);
        };
        let work = move || client.rename_computer_with_receipt(&key, &name).map(|_| ());
        if !self.live {
            let done = work();
            return then(self, done, cx);
        }
        self.rename_busy = Some(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work() }).await;
            this.update(cx, |this, cx| then(this, done, cx)).ok();
        }));
        self.rebuild(cx);
    }

    // MARK: Sheets

    /// Remove…: the sheet for another computer, or for the only one, which is wiped.
    pub(super) fn open_removal(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(computer) = self.listed(key) else { return };
        // (The box offers removing the computer it runs on only where it's the last of your setup.)
        self.sheet = Some(if computer.here { Sheet::RemoveLast { key: key.to_owned() } } else { Sheet::Remove { key: key.to_owned(), wipe: false } });
        self.open_sheet(cx);
    }

    pub(super) fn open_lead(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        let Sheet::Lead { handover, picked } = sheet else { return };
        let candidates = self.candidates(handover);
        // (Picked: the one asked for, else the standby, else the first.)
        let standby = self.led().and_then(|setup| setup.standby.clone());
        let picked = picked
            .filter(|key| candidates.iter().any(|candidate| &candidate.key == key))
            .or_else(|| (handover).then(|| standby.filter(|key| candidates.iter().any(|candidate| &candidate.key == key))).flatten())
            .or_else(|| candidates.first().map(|candidate| candidate.key.clone()));
        self.sheet = Some(Sheet::Lead { handover, picked });
        self.open_sheet(cx);
    }

    fn open_sheet(&mut self, cx: &mut Context<Self>) {
        self.sheet_problem = None;
        self.sheet_busy = None;
        self.focus_sheet = true;
        cx.notify();
    }

    fn close_sheet(&mut self, cx: &mut Context<Self>) {
        self.sheet = None;
        self.sheet_busy = None;
        self.sheet_problem = None;
        cx.notify();
    }

    /// Whether the box this window shows takes a removal's choice (`computers.removal`): unlinked,
    /// or unlinked and wiped. Without it, a removal is what its box did before (`DELETE`).
    fn chooses_removal(&self) -> bool {
        has(&self.this(), "computers.removal")
    }

    /// Goes ahead with the sheet open: asks the box, and says what it said.
    fn go(&mut self, cx: &mut Context<Self>) {
        if self.sheet_busy.is_some() {
            return;
        }
        let Some(sheet) = self.sheet.clone() else { return };
        let client = self.client.clone();
        match sheet {
            Sheet::Lead { handover, picked: Some(picked) } => {
                let Some(epoch) = self.setup.as_ref().and_then(|setup| setup.epoch) else { return };
                let work = move || if handover { client.hand_over_setup(&picked, epoch) } else { client.select_setup_standby(&picked, epoch) };
                self.run_sheet(work, |this, _: SetupLeadershipReply, cx| {
                    this.close_sheet(cx);
                    this.read_setup(cx);
                    cx.emit(ComputersEvent::ReadAgain);
                }, cx);
            }
            Sheet::Lead { picked: None, .. } => {}
            Sheet::Remove { ref key, .. } | Sheet::RemoveLast { ref key } if !key.is_empty() => {
                let key = key.clone();
                let wipe = match &sheet { Sheet::Remove { wipe, .. } => *wipe, _ => true };
                let choose = self.chooses_removal();
                let name = self.setup_name(&key);
                let asked = key.clone();
                let work = move || if choose { client.unlink_computer(&asked, wipe) } else { client.remove_computer_said(&asked) };
                self.run_sheet(work, move |this, removal: ComputerRemoval, cx| {
                    this.sheet = None;
                    let (title, words, tone, again) = removal_words(&name, &removal);
                    // (One that didn't finish stays open, in the box's words: its button asks again.)
                    if again {
                        this.sheet = Some(sheet.clone());
                        this.sheet_problem = Some([title, words].into_iter().filter(|words| !words.is_empty()).collect::<Vec<_>>().join(": "));
                        cx.notify();
                        return;
                    }
                    this.removed = Some((title, words, tone));
                    this.read_setup(cx);
                    cx.emit(ComputersEvent::ReadAgain);
                    cx.emit(ComputersEvent::Select(None));
                    if removal.offer == Some(SetupOffer::ReinstallLocal) {
                        cx.emit(ComputersEvent::Reinstall);
                    }
                    this.rebuild(cx);
                }, cx);
            }
            _ => {}
        }
    }

    fn run_sheet<T: Send + 'static>(&mut self, work: impl FnOnce() -> kuvo_box::Result<T> + Send + 'static, ok: impl FnOnce(&mut ComputersView, T, &mut Context<ComputersView>) + 'static, cx: &mut Context<Self>) {
        self.sheet_problem = None;
        let then = move |this: &mut ComputersView, done: kuvo_box::Result<T>, cx: &mut Context<ComputersView>| {
            this.sheet_busy = None;
            match done {
                Ok(value) => ok(this, value, cx),
                Err(error) => {
                    this.sheet_problem = Some(said(&error));
                    cx.notify();
                }
            }
        };
        if !self.live {
            let done = work();
            return then(self, done, cx);
        }
        self.sheet_busy = Some(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work() }).await;
            this.update(cx, |this, cx| then(this, done, cx)).ok();
        }));
        cx.notify();
    }

    /// The sheet open, over a dimmed page.
    pub(super) fn sheet_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sheet = self.sheet.clone()?;
        if std::mem::take(&mut self.focus_sheet) {
            window.focus(&self.sheet_focus, cx);
        }
        let card = match &sheet {
            Sheet::Lead { handover, picked } => self.lead_sheet(*handover, picked.clone(), cx),
            Sheet::Remove { key, wipe } => self.remove_sheet(key, *wipe, cx),
            Sheet::RemoveLast { key } => self.remove_last_sheet(key, cx),
        };
        let card = card
            .id("computers-sheet")
            .debug_selector(|| "computers-sheet".into())
            .track_focus(&self.sheet_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.sheet_busy.is_none() {
                    this.close_sheet(cx);
                }
            }))
            .into_any_element();
        Some(crate::settings::page::sheet(card, cx))
    }

    fn sheet_frame(&self, width: f32, cx: &Context<Self>) -> gpui::Div {
        let theme = cx.theme();
        let c = &theme.colors;
        div().w(px(width)).max_w_full().rounded(radius::CARD).bg(c.elevated).border_1().border_color(c.border_strong).shadow(theme.elevated_shadow()).p(space::S5).flex().flex_col().gap(space::S4)
    }

    /// What the box refused in a sheet, in its words.
    fn sheet_problem_view(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let c = &cx.theme().colors;
        self.sheet_problem.clone().map(|problem| {
            div()
                .debug_selector(|| "sheet-problem".into())
                .flex()
                .items_start()
                .gap(space::S2)
                .text_ui(TypeSize::Small)
                .text_color(c.text)
                .child(div().pt(px(2.)).child(Icon::new(IconName::Warning).size(IconSize::Nav).color(c.warning)))
                .child(div().flex_1().min_w_0().child(problem))
                .into_any_element()
        })
    }

    /// Cancel, and the sheet's button.
    fn sheet_foot(&self, label: String, danger: bool, ready: bool, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.sheet_busy.is_some();
        let go = Button::new("computers-sheet-go", label).disabled(!ready || busy).loading(busy).on_click(cx.listener(|this, _, _, cx| this.go(cx)));
        let go = if danger { go.destructive() } else { go.primary() };
        div()
            .pt(space::S1)
            .flex()
            .items_center()
            .justify_end()
            .gap(space::S2)
            .child(div().debug_selector(|| "computers-sheet-cancel".into()).child(Button::new("computers-sheet-cancel", "Cancel").disabled(busy).on_click(cx.listener(|this, _, _, cx| this.close_sheet(cx)))))
            .child(div().debug_selector(|| "computers-sheet-go".into()).child(go))
            .into_any_element()
    }

    fn radio(&self, selected: bool, danger: bool, cx: &Context<Self>) -> AnyElement {
        let c = &cx.theme().colors;
        let fill = if danger { c.danger } else { c.accent };
        div()
            .size(px(16.))
            .flex_none()
            .rounded_full()
            .border_1()
            .border_color(if selected { fill } else { c.border_strong })
            .bg(if selected { fill } else { c.surface })
            .flex()
            .items_center()
            .justify_center()
            .when(selected, |d| d.child(div().size(px(6.)).rounded_full().bg(c.text_on_accent)))
            .into_any_element()
    }

    fn heading(title: String, words: String, cx: &Context<Self>) -> AnyElement {
        let c = &cx.theme().colors;
        div()
            .flex()
            .flex_col()
            .gap(space::S1)
            .child(div().debug_selector(|| "sheet-title".into()).text_ui(TypeSize::Title).medium().text_color(c.text).child(title))
            .when(!words.is_empty(), |d| d.child(div().text_ui(TypeSize::Body).text_color(c.text_muted).child(words)))
            .into_any_element()
    }

    /// Change Main Computer, or Choose Standby: your computers that can be, one picked.
    fn lead_sheet(&self, handover: bool, picked: Option<String>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let c = cx.theme().colors.clone();
        let main = self.led().and_then(|setup| setup.primary_key().map(str::to_owned)).map(|key| self.setup_name(&key)).unwrap_or_default();
        let candidates = self.candidates(handover);
        let count = candidates.len();
        let rows = candidates.into_iter().enumerate().map(|(ix, candidate)| {
            let selected = picked.as_deref() == Some(candidate.key.as_str());
            let key = candidate.key.clone();
            let selector = format!("sheet-candidate-{}", candidate.key);
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .w_full()
                .flex()
                .items_center()
                .gap(space::S3)
                .px(space::S4)
                .py(space::S3)
                .cursor_pointer()
                .when(selected, |d| d.bg(c.accent.opacity(0.08)))
                .when(ix + 1 < count, |d| d.border_b_1().border_color(c.border))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(Sheet::Lead { picked, .. }) = &mut this.sheet {
                        *picked = Some(key.clone());
                        this.sheet_problem = None;
                        cx.notify();
                    }
                }))
                .child(self.radio(selected, false, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::S0_5)
                        .child(div().flex().items_center().gap(space::S2).child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(candidate.name.clone())).children(candidate.badge.map(|badge| Chip::new(SharedString::from(format!("sheet-badge-{}", candidate.key)), badge))))
                        .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(candidate.detail)),
                )
        });
        let (title, words, label) = if handover {
            ("Change main computer".to_owned(), format!("Hand over device approvals, membership, and release coordination from {main} to another connected computer."), "Hand Over Main Computer")
        } else {
            ("Choose standby".to_owned(), format!("The standby takes over if {main} is lost."), "Make Standby")
        };
        self.sheet_frame(500., cx)
            .child(Self::heading(title, words, cx))
            .child(div().rounded(radius::POPOVER).border_1().border_color(c.border).bg(c.card).overflow_hidden().children(rows))
            .children(self.sheet_problem_view(cx))
            .child(self.sheet_foot(label.into(), false, picked.is_some(), cx))
            .id("computers-sheet-card")
    }

    /// Remove another computer: unlink it, or unlink and wipe it (now, or once it's back).
    fn remove_sheet(&self, key: &str, wipe: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let c = cx.theme().colors.clone();
        let computer = self.listed(key);
        let name = computer.as_ref().map(|computer| computer.name.clone()).unwrap_or_default();
        let away = computer.as_ref().is_some_and(|computer| computer.reach == "away");
        let choose = self.chooses_removal();
        let option = |id: &'static str, selected: bool, danger: bool, title: &'static str, body: String, wipe: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .w_full()
                .p(space::S3)
                .rounded(radius::POPOVER)
                .border_1()
                .border_color(if selected { if danger { c.danger } else { c.accent } } else { c.border })
                .bg(if selected { if danger { c.danger.opacity(0.06) } else { c.accent.opacity(0.06) } } else { c.card })
                .cursor_pointer()
                .flex()
                .items_start()
                .gap(space::S3)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(Sheet::Remove { wipe: picked, .. }) = &mut this.sheet {
                        *picked = wipe;
                        this.sheet_problem = None;
                        cx.notify();
                    }
                }))
                .child(div().pt(px(2.)).child(self.radio(selected, danger, cx)))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(space::S0_5).child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title)).child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(body)))
        };
        let wipe = wipe && choose;
        let offline = (away && wipe).then(|| {
            div()
                .debug_selector(|| "remove-offline".into())
                .px(space::S3)
                .py(space::S2)
                .rounded(radius::NAV)
                .bg(c.warning.opacity(0.10))
                .border_1()
                .border_color(c.warning.opacity(0.35))
                .flex()
                .items_start()
                .gap(space::S2)
                .child(div().pt(px(2.)).child(Icon::new(IconName::Clock).size(IconSize::Nav).color(c.warning)))
                .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.text).child(format!("{name} is offline. It will be unlinked from your setup right now, and wiped automatically the next time it comes online.")))
        });
        let unlink = option("remove-unlink", !wipe, false, "Unlink", format!("{name} leaves your setup. Its sessions and bots stay on it as its own standalone setup."), false, cx);
        // (Wiping is drawn where the box this window shows takes it.)
        let wiping = choose.then(|| option("remove-wipe", wipe, true, "Unlink + wipe", format!("Removes {name} from your setup now and erases Kuvo there once it reconnects."), true, cx));
        let label = match (wipe, away) {
            (false, _) => "Unlink",
            (true, true) => "Unlink & Queue Wipe",
            (true, false) => "Unlink & Wipe",
        };
        self.sheet_frame(540., cx)
            .child(Self::heading(format!("Remove {name}"), String::new(), cx))
            .children(offline)
            .child(div().flex().flex_col().gap(space::S2).child(unlink).children(wiping))
            .children(self.sheet_problem_view(cx))
            .child(self.sheet_foot(label.into(), true, true, cx))
            .id("computers-sheet-card")
    }

    /// Remove the last computer: it's wiped, and your setup ends.
    fn remove_last_sheet(&self, key: &str, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let c = cx.theme().colors.clone();
        let name = self.setup_name(key);
        self.sheet_frame(520., cx)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(space::S3)
                    .child(div().size(px(36.)).flex_none().rounded(radius::NAV).bg(c.danger.opacity(0.12)).flex().items_center().justify_center().child(Icon::new(IconName::Warning).size(IconSize::Medium).color(c.danger)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::S1)
                            .child(div().debug_selector(|| "sheet-title".into()).text_ui(TypeSize::Title).medium().text_color(c.text).child("Remove your last computer?"))
                            .child(div().text_ui(TypeSize::Body).text_color(c.text_muted).child(format!("{name} is the only computer in your setup. Removing the last computer wipes Kuvo on this computer and ends your setup — Unlink alone isn’t available."))),
                    ),
            )
            .child(div().text_ui(TypeSize::Small).text_color(c.text_faint).child("After wiping, Kuvo will offer to reinstall cleanly on this computer."))
            .children(self.sheet_problem_view(cx))
            .child(self.sheet_foot("Wipe This Computer".into(), true, true, cx))
            .id("computers-sheet-card")
    }

    // MARK: Drawing on the page

    /// A computer's page head: its tile, its name (or the field editing it) with its role, and how
    /// it stands.
    pub(super) fn head(&mut self, computer: &Computer, role: Option<Role>, renaming: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let line = standing(computer);
        let field = self.renaming.as_ref().filter(|_| renaming).map(|(_, field)| field.clone());
        let id = if computer.here { "this".to_owned() } else { computer.key.clone() };
        let top: AnyElement = match field {
            Some(field) => {
                let busy = self.rename_busy.is_some();
                div()
                    .debug_selector(|| "rename-editor".into())
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(div().w(px(280.)).child(field))
                    .child(div().debug_selector(|| "rename-save".into()).child(Button::new("rename-save", "Save").primary().size(ButtonSize::Small).loading(busy).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.save_rename(cx)))))
                    .child(div().debug_selector(|| "rename-cancel".into()).child(Button::new("rename-cancel", "Cancel").size(ButtonSize::Small).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.cancel_rename(cx)))))
                    .into_any_element()
            }
            None => div()
                .flex()
                .items_center()
                .gap(space::S2)
                .min_w_0()
                .child(div().debug_selector(|| "computer-head-name".into()).truncate().text_ui(TypeSize::Title).medium().text_color(c.text).child(computer.name.clone()))
                .children(role.filter(|role| *role == Role::Main).map(|role| chip(role, &format!("head-{id}"))))
                .into_any_element(),
        };
        let under: AnyElement = if renaming {
            div()
                .debug_selector(|| "computer-head-line".into())
                .flex()
                .items_center()
                .gap(space::S2)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(Icon::new(IconName::Globe).size(IconSize::XSmall).color(c.text_faint))
                .child(format!("Shown on every app in your setup · {line}"))
                .into_any_element()
        } else {
            div().debug_selector(|| "computer-head-line".into()).text_ui(TypeSize::Body).text_color(c.text_muted).child(line).into_any_element()
        };
        div()
            .debug_selector(|| "computer-head".into())
            .pt(space::S8)
            .pb(space::S2)
            .flex()
            .items_center()
            .gap(space::S4)
            .child(tile(computer, px(56.), cx))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(if renaming { space::S1_5 } else { space::S1 }).child(top).child(under))
            .into_any_element()
    }

    /// Who leads your setup: its main computer and its standby, each with its change.
    pub(super) fn roles_card(&mut self, main: String, standby: Option<String>, warning: Option<String>, change_main: bool, choose_standby: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let row = |key: &'static str, icon: IconName, accent: bool, title: &'static str, who: String, words: String, button: Option<(&'static str, Sheet)>, last: bool, cx: &mut Context<Self>| {
            let button = button.map(|(label, sheet)| {
                let selector = format!("{key}-button");
                div().debug_selector(move || selector).flex_none().child(Button::new(SharedString::from(format!("{key}-button")), label).size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.open_lead(sheet.clone(), cx))))
            });
            card_row(key.into(), key == "roles-main", last, cx)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(space::S0_5)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S2)
                                .child(Icon::new(icon).size(IconSize::Nav).color(if accent { c.accent } else { c.icon_on }))
                                .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                                .child(div().text_ui(TypeSize::Body).text_color(c.text_muted).child(format!("· {who}"))),
                        )
                        .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words)),
                )
                .children(button)
        };
        // (No standby and none to choose: its row would say nothing you can act on, so it isn't drawn.)
        let standby_drawn = standby.is_some() || choose_standby;
        let main_row = row("roles-main", IconName::StarFilled, true, "Main computer", main, warning.unwrap_or_else(|| "Approves devices, manages membership, and coordinates updates.".into()), change_main.then_some(("Change…", Sheet::Lead { handover: true, picked: None })), !standby_drawn, cx);
        let standby_row = standby_drawn.then(|| row("roles-standby", IconName::Shield, false, "Standby", standby.unwrap_or_else(|| "None yet".into()), "Saved standby for older Kuvo versions.".into(), choose_standby.then_some(("Choose…", Sheet::Lead { handover: false, picked: None })), true, cx));
        div().debug_selector(|| "setup-roles".into()).rounded(radius::POPOVER).shadow(cx.theme().card_shadow()).child(main_row).children(standby_row).into_any_element()
    }

    /// One of a computer's rows under Role in This Setup.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn role_row(&mut self, key: String, title: String, role: Option<Role>, words: String, button: Option<(String, Sheet)>, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let button = button.map(|(label, sheet)| {
            let selector = format!("{key}-button");
            div().debug_selector(move || selector).flex_none().child(Button::new(SharedString::from(format!("{key}-button")), label).size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.open_lead(sheet.clone(), cx))))
        });
        let id = key.clone();
        card_row(key, first, last, cx)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::S0_5)
                    .child(div().flex().items_center().gap(space::S2).child(div().text_ui(TypeSize::Body).text_color(c.text).child(title)).children(role.map(|role| chip(role, &id))))
                    .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words)),
            )
            .children(button)
            .into_any_element()
    }
}

/// What tests and shots read and open.
impl ComputersView {
    /// The sheet open, if any: its title as drawn.
    pub fn sheet_open(&self) -> Option<&'static str> {
        self.sheet.as_ref().map(|sheet| match sheet {
            Sheet::Lead { handover: true, .. } => "handover",
            Sheet::Lead { handover: false, .. } => "standby",
            Sheet::Remove { .. } => "remove",
            Sheet::RemoveLast { .. } => "remove-last",
        })
    }

    /// What the box refused in the sheet open, in its words.
    pub fn sheet_refused(&self) -> Option<&str> {
        self.sheet_problem.as_deref()
    }

    /// The field editing a computer's name, where one is open.
    pub fn rename_field(&self) -> Option<Entity<TextField>> {
        self.renaming.as_ref().map(|(_, field)| field.clone())
    }

    /// Opens Remove… for `key` as its page's row does (shots).
    #[cfg(feature = "stand-in")]
    pub fn show_removal(&mut self, key: &str, wipe: bool, cx: &mut Context<Self>) {
        self.open_removal(key, cx);
        if let Some(Sheet::Remove { wipe: picked, .. }) = &mut self.sheet {
            *picked = wipe;
        }
        cx.notify();
    }

    /// Goes ahead with the removal sheet open, wiped or not, as its button does (shots).
    #[cfg(feature = "stand-in")]
    pub fn go_now(&mut self, wipe: bool, cx: &mut Context<Self>) {
        if let Some(Sheet::Remove { wipe: picked, .. }) = &mut self.sheet {
            *picked = wipe;
        }
        self.go(cx);
    }

    /// Opens a leadership sheet as its button does (shots).
    #[cfg(feature = "stand-in")]
    pub fn show_lead(&mut self, handover: bool, picked: Option<&str>, cx: &mut Context<Self>) {
        self.open_lead(Sheet::Lead { handover, picked: picked.map(str::to_owned) }, cx);
    }

    /// Starts renaming `key` as its row does (shots).
    #[cfg(feature = "stand-in")]
    pub fn show_rename(&mut self, key: &str, cx: &mut Context<Self>) {
        self.start_rename(key, cx);
    }

}

