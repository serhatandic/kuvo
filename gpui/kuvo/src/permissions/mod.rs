//! What macOS allows the program this Mac's agents run under, where it needs you: a heads-up over
//! the window's main view, and, for a permission macOS has you add by hand, a small tile beside
//! System Settings with the program's icon to drag into the list ([`tile`]).
//!
//! Every fact and name is the box's (`kuvo/box/permissions`): which permission to bring up, what
//! System Settings calls it, the address of its list there, the program's name and bundle, and
//! "Not Now" kept so it isn't brought up at each open. This only draws them, and opens what only
//! an app on that Mac can: System Settings, for the box on this computer alone.

pub mod icns;
#[cfg(target_os = "macos")]
mod mac;
pub mod tile;

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Subscription, Task, Window, WindowHandle, div, px};
use kuvo_box::BoxClient;
use kuvo_box::permissions::{Permission, Permissions};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, StyledType, TypeSize, theme::{radius, space}};

use tile::Tile;

/// How often the tile is put beside System Settings' window while it's open, and how many of
/// those go by between asks of the box (once a second).
pub const FOLLOWS_EVERY: Duration = Duration::from_millis(250);
const ASKS_EVERY: u32 = 4;

pub struct HeadsUp {
    client: BoxClient,
    live: bool,
    /// What the box last said.
    facts: Option<Permissions>,
    /// The computer's name, as the window says it.
    box_name: SharedString,
    /// Which read is the latest asked for: an older one's answer is dropped.
    asked: u64,
    /// The permission System Settings was opened for, and the tile beside it.
    asking: Option<(String, Option<(WindowHandle<Tile>, Subscription)>)>,
    /// A change that couldn't be made, in the box's words.
    problem: Option<SharedString>,
    _follow: Option<Task<()>>,
    tasks: Vec<Task<()>>,
    _activation: Subscription,
}

impl HeadsUp {
    pub fn new(client: BoxClient, live: bool, window: &mut Window, cx: &mut Context<Self>) -> HeadsUp {
        // Back in this window (from System Settings, as likely as not): what the box says now.
        let activation = cx.observe_window_activation(window, |this: &mut HeadsUp, window, cx| {
            if window.is_window_active() && this.facts.is_some() {
                this.read(cx);
            }
        });
        HeadsUp { client, live, facts: None, box_name: SharedString::default(), asked: 0, asking: None, problem: None, _follow: None, tasks: Vec::new(), _activation: activation }
    }

    pub fn set_box_name(&mut self, name: SharedString) {
        self.box_name = name;
    }

    /// The permission brought up now: the first the box says to, for the box on this computer
    /// (System Settings can't be opened on another).
    pub fn shown(&self) -> Option<&Permission> {
        if !self.client.here() || self.client.gate().is_held() {
            return None;
        }
        self.facts.as_ref().and_then(Permissions::asked)
    }

    /// The permission System Settings was opened for, while its tile is up.
    pub fn asking(&self) -> Option<&str> {
        self.asking.as_ref().map(|(id, _)| id.as_str())
    }

    /// The tile's window, while it's open.
    pub fn tile(&self) -> Option<WindowHandle<Tile>> {
        self.asking.as_ref().and_then(|(_, tile)| tile.as_ref().map(|(window, _)| *window))
    }

    fn run<T: Send + 'static>(&mut self, work: impl FnOnce(&BoxClient) -> T + Send + 'static, then: impl FnOnce(&mut HeadsUp, T, &mut Context<HeadsUp>) + 'static, cx: &mut Context<Self>) {
        let client = self.client.clone();
        if !self.live {
            let done = work(&client);
            then(self, done, cx);
            cx.notify();
            return;
        }
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work(&client) }).await;
            this.update(cx, |this, cx| {
                then(this, done, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Asks the box what macOS allows (only the box on this computer: nothing's drawn of another's).
    pub fn read(&mut self, cx: &mut Context<Self>) {
        if !self.client.here() {
            return;
        }
        self.asked += 1;
        let asked = self.asked;
        self.run(
            |client| client.permissions(),
            move |this, read, cx| {
                // (One it couldn't say leaves what's shown: nothing's brought up on a guess.)
                if asked == this.asked
                    && let Ok(facts) = read
                {
                    this.take(facts, cx);
                }
            },
            cx,
        );
    }

    fn take(&mut self, facts: Permissions, cx: &mut Context<Self>) {
        // Allowed, the one System Settings was opened for: its tile goes, and Kuvo comes back to
        // the front.
        let allowed = self.asking.as_ref().is_some_and(|(id, _)| facts.of(id).is_none_or(|permission| permission.allowed == Some(true)));
        self.facts = Some(facts);
        if allowed {
            self.stop_asking(cx);
            if self.live {
                cx.activate(true);
            }
        }
        cx.notify();
    }

    /// Opens System Settings on the list for the permission shown, with the tile beside it where
    /// macOS has you add the program by hand.
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        let Some(permission) = self.shown().cloned() else { return };
        self.stop_asking(cx);
        self.problem = None;
        cx.open_url(&permission.settings);
        let (name, bundle) = match &self.facts {
            Some(facts) => (facts.name.clone().unwrap_or_default(), facts.bundle.clone()),
            None => (String::new(), None),
        };
        let tile = bundle.filter(|_| permission.by_hand).and_then(|bundle| Tile::open(name.into(), PathBuf::from(bundle), cx)).map(|window| {
            let closed = window.entity(cx).ok().map(|tile| cx.subscribe(&tile, |this: &mut HeadsUp, _, _: &tile::Closed, cx| this.stop_asking(cx)));
            (window, closed)
        });
        let tile = tile.and_then(|(window, closed)| closed.map(|closed| (window, closed)));
        self.asking = Some((permission.id.clone(), tile));
        // While it's asked for: the tile kept beside System Settings, and the box asked each
        // second whether it's allowed yet (macOS says nothing when Screen Recording changes).
        self._follow = Some(cx.spawn(async move |this, cx| {
            let mut ticks = 0u32;
            loop {
                cx.background_executor().timer(FOLLOWS_EVERY).await;
                ticks += 1;
                let going = this.update(cx, |this, cx| {
                    if this.asking.is_none() {
                        return false;
                    }
                    this.place(cx);
                    if ticks % ASKS_EVERY == 0 {
                        this.read(cx);
                    }
                    true
                });
                if !going.unwrap_or(false) {
                    return;
                }
            }
        }));
        cx.notify();
    }

    /// Puts the tile beside System Settings' window, where it has one on screen.
    fn place(&mut self, cx: &mut Context<Self>) {
        // (Only the app follows a real window; tests and shots have none to follow.)
        #[cfg(target_os = "macos")]
        if self.live
            && let Some(tile) = self.tile()
            && let Some(settings) = mac::settings_window()
        {
            let displays: Vec<_> = cx.displays().iter().map(|display| display.bounds()).collect();
            let Some(main) = cx.primary_display().map(|display| display.bounds()) else { return };
            let screen = displays.into_iter().find(|display| display.intersects(&settings)).unwrap_or(main);
            let to = tile::beside(settings, screen);
            tile.update(cx, |_, window, _| mac::move_window(window, to, main.size.height)).ok();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
    }

    /// The tile goes, and the box is no longer asked each second.
    pub fn stop_asking(&mut self, cx: &mut Context<Self>) {
        if let Some((_, Some((tile, _)))) = self.asking.take() {
            tile.update(cx, |_, window, _| window.remove_window()).ok();
        }
        self.asking = None;
        self._follow = None;
        cx.notify();
    }

    /// Not Now: the box keeps it, so no client brings it up again at each open.
    pub fn not_now(&mut self, cx: &mut Context<Self>) {
        let Some(permission) = self.shown().cloned() else { return };
        self.stop_asking(cx);
        self.asked += 1;
        self.run(
            move |client| client.permission_later(&permission.id),
            |this, said, cx| match said {
                Ok(facts) => {
                    this.problem = None;
                    this.take(facts, cx);
                }
                Err(error) => this.problem = Some(kuvo_box::accounts::said(&error).into()),
            },
            cx,
        );
    }

    /// The heads-up's card, where there's a permission to bring up.
    pub fn card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let permission = self.shown()?.clone();
        let c = cx.theme().colors.clone();
        let program: SharedString = self.facts.as_ref().and_then(|facts| facts.name.clone()).unwrap_or_else(|| "its helper".into()).into();
        let place = if self.box_name.is_empty() { "this Mac".into() } else { self.box_name.clone() };
        let detail = if self.asking() == Some(permission.id.as_str()) && permission.by_hand {
            format!("Drag {program} into the list in System Settings, then switch it on.")
        } else {
            format!("Agents on {place} run as {program}. Only you can allow it, in System Settings.")
        };
        Some(
            div()
                .debug_selector(|| "permission-heads-up".into())
                .w_full()
                .max_w(px(520.))
                .flex()
                .flex_col()
                .gap(space::S2)
                .p(space::S4)
                .rounded(radius::CARD)
                .bg(c.surface)
                .border_1()
                .border_color(c.border)
                .shadow(cx.theme().elevated_shadow())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(Icon::new(IconName::Shield).color(c.warning))
                        .child(div().debug_selector(|| "permission-heads-up-title".into()).flex_1().min_w_0().text_ui(TypeSize::Body).medium().text_color(c.text).child(format!("Allow {} for {program}?", permission.name))),
                )
                .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(detail))
                .children(self.problem.clone().map(|why| div().debug_selector(|| "permission-heads-up-problem".into()).text_ui(TypeSize::Small).text_color(c.danger).child(why)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .pt(space::S1)
                        .child(Button::new("permission-open", "Open System Settings").primary().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))))
                        .child(Button::new("permission-later", "Not Now").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.not_now(cx)))),
                )
                .into_any_element(),
        )
    }
}

impl gpui::Render for HeadsUp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Over the main view's top, in its middle, taking no room of its own.
        match self.card(cx) {
            Some(card) => div().absolute().top(space::S4).left_0().right_0().flex().justify_center().px(space::S4).child(card),
            None => div(),
        }
    }
}
