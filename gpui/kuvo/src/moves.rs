//! Desktop controls for Move to Computer. A workspace asks the computer that owns the item.

use std::{collections::HashMap, time::Duration};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, Render, Styled, Task, Window, div, px,
    relative,
};
use kuvo_box::moves::{Moves, Moving, Place, Places, Receipt, Target};
use kuvo_box::{
    BoxClient, Error, Timing,
    bots::Elsewhere,
    boxes::Boxes,
    follow::{Follow, Followed},
};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, IconName, StyledType, TypeSize,
    menu::{Menu, MenuEntry, MenuItem, PopoverMenu, PopoverMenuHandle},
    theme::{radius, space},
};

use crate::settings::page::sheet;

pub struct MoveFinished(pub Target);

pub struct OpenMoved {
    pub computer: String,
    pub target: Target,
}

#[derive(Clone)]
pub(crate) struct RemoteSource {
    key: String,
    name: String,
}

impl RemoteSource {
    pub(crate) fn of(bot: &Elsewhere) -> Option<Self> {
        (!bot.unlisted && !bot.computer_key.is_empty()).then(|| Self {
            key: bot.computer_key.clone(),
            name: crate::bots::computer_name(&bot.computer).to_owned(),
        })
    }
}

/// Keep the source of the choice through the request, receipt and any retry.
#[derive(Clone)]
struct Choice {
    client: BoxClient,
    target: Target,
    place: Place,
}

enum Notice {
    Done {
        moved: Moving,
        target: Target,
        message: String,
        note: Option<String>,
    },
    Failed {
        choice: Choice,
        message: String,
    },
}

impl Notice {
    fn target(&self) -> &Target {
        match self {
            Self::Done { target, .. } => target,
            Self::Failed { choice, .. } => &choice.target,
        }
    }
}

pub struct MovesView {
    client: BoxClient,
    computers: Option<Boxes>,
    live: bool,
    said: Moves,
    pending: HashMap<Target, Option<Moving>>,
    confirm: Option<Choice>,
    notices: Vec<Notice>,
    focus: FocusHandle,
    return_to: Option<FocusHandle>,
    head_menu: PopoverMenuHandle,
    _follow: Option<Follow<Followed<Moves>>>,
    retrying: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<OpenMoved> for MovesView {}
impl EventEmitter<MoveFinished> for MovesView {}

impl MovesView {
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            client,
            computers: None,
            live,
            said: Moves::default(),
            pending: HashMap::new(),
            confirm: None,
            notices: Vec::new(),
            focus: cx.focus_handle(),
            return_to: None,
            head_menu: PopoverMenuHandle::default(),
            _follow: None,
            retrying: None,
            _tasks: Vec::new(),
        };
        if live {
            let follow = view.client.follow_moves(Timing::default());
            let updates = follow.updates().clone();
            view._tasks.push(cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    if let Followed::Now(said) = update
                        && this.update(cx, |view, cx| view.take(said, cx)).is_err()
                    {
                        break;
                    }
                }
            }));
            view._follow = Some(follow);
        } else if let Ok(said) = view.client.moves() {
            view.said = said;
        }
        view
    }

    pub(crate) fn set_computers(&mut self, computers: Boxes) {
        self.computers = Some(computers);
    }

    fn take(&mut self, said: Moves, cx: &mut Context<Self>) {
        if self.said != said {
            self.said = said;
            cx.notify();
        }
    }

    pub fn busy(&self, target: &Target) -> bool {
        self.pending.contains_key(target) || self.moving(target).is_some()
    }

    pub fn moving(&self, target: &Target) -> Option<&Moving> {
        self.pending
            .get(target)
            .and_then(Option::as_ref)
            .or_else(|| self.said.of(target))
    }

    pub fn row_status(&self, target: &Target, cx: &App) -> Option<AnyElement> {
        if let Some(moving) = self.moving(target) {
            return Some(progress(moving, cx));
        }
        self.pending.contains_key(target).then(|| {
            div()
                .text_ui(TypeSize::Caption)
                .text_color(cx.theme().colors.text_muted)
                .child("Starting the move…")
                .into_any_element()
        })
    }

    pub fn head_button(view: &Entity<Self>, target: Target, cx: &App) -> AnyElement {
        Self::head_button_from(view, target, None, cx)
    }

    pub(crate) fn head_button_from(
        view: &Entity<Self>,
        target: Target,
        source: Option<RemoteSource>,
        cx: &App,
    ) -> AnyElement {
        let state = view.read(cx);
        let trigger = Button::new("bot-move-trigger", "Move to…")
            .ghost()
            .disabled(state.busy(&target));
        let handle = state.head_menu.clone();
        let view = view.clone();
        PopoverMenu::new("bot-move-menu", trigger, move |window, cx| {
            Self::menu_from(&view, target.clone(), source.clone(), window, cx)
        })
        .with_handle(handle)
        .align_right()
        .into_any_element()
    }

    #[cfg(feature = "shots")]
    pub fn open_head_menu(view: &Entity<Self>, target: Target, window: &mut Window, cx: &mut App) {
        let handle = view.read(cx).head_menu.clone();
        handle.open(Self::menu(view, target, window, cx), window, cx);
    }

    pub fn menu(
        view: &Entity<Self>,
        target: Target,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Menu> {
        Self::menu_from(view, target, None, window, cx)
    }

    pub(crate) fn menu_from(
        view: &Entity<Self>,
        target: Target,
        source: Option<RemoteSource>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Menu> {
        let state = view.read(cx);
        let (live, busy) = (state.live, state.busy(&target));
        let busy_words = state
            .moving(&target)
            .and_then(|moving| moving.progress.clone());
        let client = match &source {
            Some(source) => state
                .computers
                .as_ref()
                .and_then(|computers| computers.client(&source.key)),
            None => Some(state.client.clone()),
        };
        let source_error = source
            .map(|source| {
                format!(
                    "Kuvo on {} couldn’t be reached. Check the connection and try again.",
                    source.name
                )
            })
            .unwrap_or_else(|| "Couldn’t load your computers. Open Move to… to try again.".into());
        let target_for_read = target.clone();
        let return_to = window.focused(cx);
        let menu = cx.new(|cx| Menu::new(cx).width(px(320.)).header("Move to…"));
        if busy {
            let message = busy_words.unwrap_or_else(|| "Starting the move…".into());
            menu.update(cx, |menu, cx| {
                menu.set_items(vec![MenuItem::Header(message.into())], cx)
            });
            return menu;
        }
        let Some(client) = client else {
            menu.update(cx, |menu, cx| {
                menu.set_items(vec![MenuItem::Header(source_error.into())], cx)
            });
            return menu;
        };
        let source_client = client.clone();
        let view = view.downgrade();
        let fill = move |result: kuvo_box::Result<Places>,
                         menu: &mut Menu,
                         cx: &mut Context<Menu>| {
            let mut items = vec![MenuItem::Header("Move to…".into())];
            match result {
                Ok(list) if list.places.is_empty() => {
                    items.push(MenuItem::Header("No other computers yet.".into()))
                }
                Ok(list) => {
                    for place in list.places {
                        let mut entry = MenuEntry::new(place.name.clone()).icon(IconName::Computer);
                        if place.ok && place.confirm.is_some() {
                            let (view, return_to) = (view.clone(), return_to.clone());
                            let choice = Choice {
                                client: source_client.clone(),
                                target: target.clone(),
                                place,
                            };
                            entry = entry.on_select(move |window, cx| {
                                view.update(cx, |view, cx| {
                                    view.ask_choice(choice.clone(), return_to.clone(), window, cx)
                                })
                                .ok();
                            });
                            items.push(MenuItem::Entry(entry));
                        } else {
                            let why = place.why.unwrap_or_else(|| {
                                "Update Kuvo on this computer to move from the desktop app.".into()
                            });
                            items.push(MenuItem::Entry(entry.disabled(true)));
                            items.push(MenuItem::Header(why.into()));
                        }
                    }
                }
                Err(error) => items.push(MenuItem::Header(words(&error, &source_error).into())),
            }
            menu.set_items(items, cx);
        };
        if live {
            menu.update(cx, |menu, cx| {
                menu.set_items(
                    vec![MenuItem::Header("Looking for your computers…".into())],
                    cx,
                )
            });
            let weak = menu.downgrade();
            window
                .spawn(cx, async move |cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { client.move_places(&target_for_read) })
                        .await;
                    weak.update(cx, |menu, cx| fill(result, menu, cx)).ok();
                })
                .detach();
        } else {
            let result = client.move_places(&target_for_read);
            menu.update(cx, |menu, cx| fill(result, menu, cx));
        }
        menu
    }

    #[cfg(feature = "shots")]
    pub fn ask(
        &mut self,
        target: Target,
        place: Place,
        return_to: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let choice = Choice {
            client: self.client.clone(),
            target,
            place,
        };
        self.ask_choice(choice, return_to, window, cx);
    }

    fn ask_choice(
        &mut self,
        choice: Choice,
        return_to: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy(&choice.target) || !choice.place.ok || choice.place.confirm.is_none() {
            return;
        }
        self.retrying = None;
        self.confirm = Some(choice);
        self.return_to = return_to;
        let focus = self.focus.clone();
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
        cx.notify();
    }

    fn close_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        if let Some(focus) = self.return_to.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = self.confirm.take() else {
            return;
        };
        self.close_confirm(window, cx);
        let target = &choice.target;
        if self.busy(target) {
            return;
        }
        self.notices.retain(|notice| notice.target() != target);
        self.retrying = None;
        self.pending.insert(target.clone(), None);
        let (client, to) = (choice.client.clone(), choice.place.key.clone());
        if !self.live {
            match client
                .begin_move(target, &to)
                .and_then(|accepted| client.move_receipt(&accepted.moving.id))
            {
                Ok(receipt) => {
                    self.take_receipt(&choice, receipt, cx);
                }
                Err(error) => self.failed(
                    choice,
                    words(
                        &error,
                        "Couldn’t start the move. Check the connection and try again.",
                    ),
                    cx,
                ),
            }
            return;
        }
        let target = target.clone();
        self._tasks.retain(|task| !task.is_ready());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let worker = cx.background_executor().clone();
            let (source, item) = (client.clone(), target.clone());
            let id = match worker
                .spawn(async move { source.begin_move(&item, &to) })
                .await
            {
                Ok(accepted) => {
                    let id = accepted.moving.id.clone();
                    if this
                        .update(cx, |view, cx| {
                            view.pending.insert(target.clone(), Some(accepted.moving));
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                    id
                }
                Err(error) => {
                    let message = words(
                        &error,
                        "Couldn’t confirm the move started. Check the connection and try again.",
                    );
                    this.update(cx, |view, cx| view.failed(choice, message, cx))
                        .ok();
                    return;
                }
            };
            let mut missing = 0;
            loop {
                let (source, id) = (client.clone(), id.clone());
                match worker.spawn(async move { source.move_receipt(&id) }).await {
                    Ok(receipt) => {
                        missing = 0;
                        if !matches!(
                            this.update(cx, |view, cx| view.take_receipt(&choice, receipt, cx)),
                            Ok(true)
                        ) {
                            break;
                        }
                    }
                    Err(error) if error.status() == Some(404) => {
                        // The source can be restoring a held group between removing the move and
                        // writing its result. Give it a few reads before reporting a lost receipt.
                        missing += 1;
                        if missing == 5 {
                            let message = words(
                                &error,
                                "Couldn’t read the move’s result. Check where it is and try again.",
                            );
                            this.update(cx, |view, cx| view.failed(choice, message, cx))
                                .ok();
                            break;
                        }
                    }
                    // A lost connection doesn't change the outcome. Keep looking for the receipt.
                    Err(_) => {}
                }
                worker.timer(Duration::from_secs(2)).await;
            }
        }));
        cx.notify();
    }

    fn take_receipt(&mut self, choice: &Choice, receipt: Receipt, cx: &mut Context<Self>) -> bool {
        let target = &choice.target;
        match receipt {
            Receipt::Moving { moving } => {
                if let Some(last) = self.pending.get_mut(target)
                    && last.as_ref() != Some(&moving)
                {
                    *last = Some(moving);
                    cx.notify();
                }
                true
            }
            Receipt::Done {
                moved,
                message,
                note,
            } => {
                self.pending.remove(target);
                self.remove_progress(&moved.id);
                self.notices.push(Notice::Done {
                    moved,
                    target: target.clone(),
                    message,
                    note,
                });
                cx.emit(MoveFinished(target.clone()));
                cx.notify();
                false
            }
            Receipt::Failed { message } => {
                if let Some(Some(moving)) = self.pending.remove(target) {
                    self.remove_progress(&moving.id);
                }
                self.failed(choice.clone(), message, cx);
                false
            }
        }
    }

    fn remove_progress(&mut self, id: &str) {
        self.said.moves.retain(|moving| moving.id != id);
        for peer in &mut self.said.elsewhere {
            peer.moves.retain(|moving| moving.id != id);
        }
    }

    fn failed(&mut self, choice: Choice, message: String, cx: &mut Context<Self>) {
        self.pending.remove(&choice.target);
        self.notices
            .retain(|notice| notice.target() != &choice.target);
        self.notices.push(Notice::Failed { choice, message });
        cx.notify();
    }

    fn retry(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        if self.retrying.is_some() {
            return;
        }
        let (client, item) = (choice.client.clone(), choice.target.clone());
        let done = move |view: &mut Self,
                         answer: kuvo_box::Result<Places>,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
            view.retrying = None;
            if !matches!(view.notices.last(), Some(Notice::Failed { choice: failed, .. })
                if failed.target == choice.target && failed.place.key == choice.place.key)
            {
                cx.notify();
                return;
            }
            let fresh = match answer {
                Ok(list) => list
                    .places
                    .into_iter()
                    .find(|fresh| fresh.key == choice.place.key),
                Err(error) => {
                    view.failed(
                        choice,
                        words(
                            &error,
                            "Couldn’t check this computer. Check the connection and try again.",
                        ),
                        cx,
                    );
                    return;
                }
            };
            match fresh {
                Some(place) if place.ok && place.confirm.is_some() => {
                    view.notices.pop();
                    view.ask_choice(Choice { place, ..choice }, window.focused(cx), window, cx);
                }
                Some(place) => {
                    let message = place
                        .why
                        .clone()
                        .unwrap_or_else(|| "Update Kuvo on this computer to try again.".into());
                    view.failed(Choice { place, ..choice }, message, cx);
                }
                None => view.failed(
                    choice,
                    "That computer isn’t on the list anymore. Open Move to… to choose another."
                        .into(),
                    cx,
                ),
            }
        };
        if self.live {
            self.retrying = Some(cx.spawn_in(window, async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { client.move_places(&item) })
                    .await;
                this.update_in(cx, |view, window, cx| done(view, result, window, cx))
                    .ok();
            }));
            cx.notify();
        } else {
            done(self, client.move_places(&item), window, cx);
        }
    }
}

fn words(error: &Error, fallback: &str) -> String {
    error
        .refusal()
        .filter(|refusal| !refusal.message.starts_with("It said no ("))
        .map(|refusal| refusal.message.clone())
        .unwrap_or_else(|| fallback.to_owned())
}

/// Render the source's status line and its byte progress when given.
fn progress(moving: &Moving, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .debug_selector(|| "move-progress".into())
        .flex()
        .flex_col()
        .gap(space::S1)
        .min_w_0()
        .child(
            div()
                .truncate()
                .text_ui(TypeSize::Caption)
                .text_color(c.text_muted)
                .child(moving.progress.clone().unwrap_or_else(|| "Moving…".into())),
        )
        .children(moving.fraction().map(|fraction| {
            div()
                .h(px(3.))
                .w_full()
                .rounded(radius::PILL)
                .bg(c.sunken)
                .child(
                    div()
                        .h_full()
                        .w(relative(fraction))
                        .rounded(radius::PILL)
                        .bg(c.accent),
                )
        }))
        .into_any_element()
}

impl Render for MovesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let prompt = self
            .confirm
            .as_ref()
            .and_then(|choice| choice.place.confirm.clone())
            .map(|confirm| {
                let card = div()
                    .id("move-confirm")
                    .debug_selector(|| "move-confirm".into())
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            view.close_confirm(window, cx);
                        }
                    }))
                    .w(px(440.))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .gap(space::S3)
                    .p(space::S6)
                    .rounded(radius::CARD)
                    .border_1()
                    .border_color(c.border)
                    .bg(c.elevated)
                    .shadow(cx.theme().elevated_shadow())
                    .child(div().text_ui(TypeSize::Body).medium().child(confirm.title))
                    .child(
                        div()
                            .text_ui(TypeSize::Small)
                            .text_color(c.text_muted)
                            .child(confirm.message),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(space::S2)
                            .child(Button::new("move-cancel", "Cancel").on_click(
                                cx.listener(|view, _, window, cx| view.close_confirm(window, cx)),
                            ))
                            .child(Button::new("move-go", confirm.label).primary().on_click(
                                cx.listener(|view, _, window, cx| view.submit(window, cx)),
                            )),
                    );
                sheet(card.into_any_element(), cx)
            });

        let notice = self.notices.last().map(|notice| {
            let (id, message, note, action) = match notice {
                Notice::Done {
                    moved,
                    target,
                    message,
                    note,
                } => {
                    let (computer, target) = (moved.to.clone(), target.clone());
                    let label = moved
                        .place
                        .as_ref()
                        .map(|name| format!("Open on {name}"))
                        .unwrap_or_else(|| "Open on the other computer".into());
                    (
                        "move-done",
                        message.clone(),
                        note.clone(),
                        Button::new("move-open", label).on_click(cx.listener(
                            move |_, _, _, cx| {
                                cx.emit(OpenMoved {
                                    computer: computer.clone(),
                                    target: target.clone(),
                                });
                            },
                        )),
                    )
                }
                Notice::Failed { choice, message } => {
                    let choice = choice.clone();
                    (
                        "move-failed",
                        "Couldn’t move".into(),
                        Some(message.clone()),
                        Button::new("move-again", "Try Again")
                            .loading(self.retrying.is_some())
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.retry(choice.clone(), window, cx);
                            })),
                    )
                }
            };
            div()
                .absolute()
                .top(px(56.))
                .right(space::S4)
                .w(px(350.))
                .max_w_full()
                .debug_selector(move || id.into())
                .occlude()
                .flex()
                .flex_col()
                .gap(space::S2)
                .p(space::S4)
                .rounded(radius::CARD)
                .border_1()
                .border_color(c.border)
                .bg(c.elevated)
                .shadow(cx.theme().elevated_shadow())
                .child(div().text_ui(TypeSize::Small).medium().child(message))
                .children(note.map(|note| {
                    div()
                        .text_ui(TypeSize::Small)
                        .text_color(c.text_muted)
                        .child(note)
                }))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .child(
                            Button::new("move-dismiss", "Dismiss")
                                .ghost()
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.notices.pop();
                                    view.retrying = None;
                                    cx.notify();
                                })),
                        )
                        .child(action.size(ButtonSize::Small)),
                )
        });

        div().absolute().inset_0().children(notice).children(prompt)
    }
}
