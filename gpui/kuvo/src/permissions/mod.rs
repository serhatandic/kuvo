//! What macOS allows the program this Mac's agents run under, where it needs you: a step of
//! setting up (the first run, and after a move from an earlier Kuvo Code), listing each permission
//! with its own Open System Settings, and, for a permission macOS has you add by hand, a small tile
//! beside System Settings with the program's icon to drag into the list ([`tile`]). Nothing is
//! brought up over the main window: Settings › This Mac shows how each stands afterwards, and
//! opening one from there gets the same tile ([`HeadsUp::opened`]).
//!
//! Every fact and name is the box's (`kuvo/box/permissions`): each permission's name and line, what
//! System Settings calls it, the address of its list there, the program's name and bundle, whether
//! to ask about it (`asks`), and "Not Now" kept so it isn't asked about again. This only draws
//! them, and opens what only an app on that Mac can: System Settings, for the box on this computer
//! alone.

pub mod icns;
#[cfg(target_os = "macos")]
mod mac;
pub mod tile;

use std::path::PathBuf;
use std::time::Duration;

use gpui::{prelude::FluentBuilder as _, AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Subscription, Task, Window, WindowHandle, div};
use kuvo_box::BoxClient;
use kuvo_box::permissions::{Permission, Permissions};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, StyledType, TypeSize, theme::space};

use tile::Tile;

/// How often the tile is put beside System Settings' window while it's open, and how many of
/// those go by between asks of the box (once a second).
pub const FOLLOWS_EVERY: Duration = Duration::from_millis(250);
const ASKS_EVERY: u32 = 4;
/// How many of those System Settings' window may be gone before the tile goes too (two seconds),
/// and after how many the tile is given up on whatever happens (ten minutes).
const GONE_AFTER: u32 = 8;
const GIVEN_UP_AFTER: u32 = 2400;
/// How long a tile that's gone is kept, hidden, before its window is let go: well past AppKit's
/// next look at the app's windows (see [`HeadsUp::close_tile`]).
const LET_GO_AFTER: Duration = Duration::from_secs(1);

/// The tile: one window from the first permission added by hand until none is left to add, hidden
/// in between and shown again where it was.
struct KeptTile {
    window: WindowHandle<Tile>,
    shown: bool,
    /// Whether it's been put beside System Settings yet: only once, so it stays where it is, or
    /// where you moved it, from one permission to the next.
    placed: bool,
    /// Hearing it closed with its own button.
    _closed: Subscription,
}

pub struct HeadsUp {
    client: BoxClient,
    live: bool,
    /// What the box last said.
    facts: Option<Permissions>,
    /// Which read is the latest asked for: an older one's answer is dropped.
    asked: u64,
    /// The permission System Settings was opened for.
    asking: Option<String>,
    /// The tile to drag the program from, where one's added by hand.
    tile: Option<KeptTile>,
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
        HeadsUp { client, live, facts: None, asked: 0, asking: None, tile: None, problem: None, _follow: None, tasks: Vec::new(), _activation: activation }
    }

    /// Whether this is the box on this computer, and can be asked (System Settings can't be
    /// opened on another, and one held for its update says nothing).
    fn reachable(&self) -> bool {
        self.client.here() && !self.client.gate().is_held()
    }

    /// The permissions the box lists, where they apply to this computer.
    pub fn list(&self) -> &[Permission] {
        match &self.facts {
            Some(facts) if facts.applies && self.reachable() => &facts.list,
            _ => &[],
        }
    }

    /// Whether the box says a permission is to be asked about now (known not to be allowed, and
    /// not put off): what brings the step up while setting up.
    pub fn asks(&self) -> bool {
        self.list().iter().any(|permission| permission.asks)
    }

    /// The first permission the box says to ask about now.
    pub fn shown(&self) -> Option<&Permission> {
        self.list().iter().find(|permission| permission.asks)
    }

    /// The program agents run under, as the box names it.
    pub fn program(&self) -> SharedString {
        self.facts.as_ref().and_then(|facts| facts.name.clone()).unwrap_or_else(|| "Kuvo Helper".into()).into()
    }

    /// Whether the box has answered at all.
    pub fn read_yet(&self) -> bool {
        self.facts.is_some()
    }

    /// The permission System Settings was opened for, while it's asked for.
    pub fn asking(&self) -> Option<&str> {
        self.asking.as_deref()
    }

    /// The tile's window, while it's shown.
    pub fn tile(&self) -> Option<WindowHandle<Tile>> {
        self.tile.as_ref().filter(|kept| kept.shown).map(|kept| kept.window)
    }

    /// The tile's window, shown or kept hidden for the next permission.
    pub fn kept_tile(&self) -> Option<WindowHandle<Tile>> {
        self.tile.as_ref().map(|kept| kept.window)
    }

    /// Whether one still asked about is added by hand, with the tile.
    fn asks_by_hand(&self) -> bool {
        self.list().iter().any(|permission| permission.asks && permission.by_hand)
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
        // Allowed, the one System Settings was opened for: its tile is put away, and Kuvo comes
        // back to the front.
        let allowed = self.asking.as_ref().is_some_and(|id| facts.of(id).is_some_and(|permission| permission.allowed == Some(true)));
        // (One the box no longer lists at all isn't asked for any more, with nothing claimed.)
        let gone = self.asking.as_ref().is_some_and(|id| facts.of(id).is_none());
        self.facts = Some(facts);
        if allowed || gone {
            self.asking = None;
        }
        // Between permissions the tile waits, hidden, for the next one added by hand; with none
        // left, it goes.
        if self.asking.is_none() {
            if self.asks_by_hand() {
                self.hide_tile(cx);
            } else {
                self.close_tile(cx);
            }
        }
        if allowed && self.live {
            cx.activate(true);
        }
        cx.notify();
    }

    /// Opens System Settings on the list for one permission, with the tile beside it where macOS
    /// has you add the program by hand.
    pub fn open_settings(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(permission) = self.list().iter().find(|permission| permission.id == id).cloned() else { return };
        // (Only an address of System Settings' own is ever opened, whatever a box says.)
        if !permission.settings.starts_with("x-apple.systempreferences:") {
            return;
        }
        cx.open_url(&permission.settings);
        self.ask_for(permission, cx);
    }

    /// System Settings was opened at `address` from somewhere else in the window (Settings › This
    /// Mac, where the permissions are looked up after "Not Now"): where that's the list of a
    /// permission this Mac's Kuvo says isn't allowed, it's helped along as the heads-up's own
    /// button does, with the tile for one added by hand. Says whether it was one.
    pub fn opened(&mut self, address: &str, cx: &mut Context<Self>) -> bool {
        if !self.reachable() {
            return false;
        }
        let Some(permission) = self.facts.as_ref().and_then(|facts| facts.list.iter().find(|permission| permission.settings == address && permission.allowed == Some(false))).cloned() else { return false };
        self.ask_for(permission, cx);
        true
    }

    /// System Settings is open on `permission`'s list: the tile beside it where macOS has you
    /// add the program by hand, and the box asked until it's allowed.
    fn ask_for(&mut self, permission: Permission, cx: &mut Context<Self>) {
        self.problem = None;
        let (name, bundle) = match &self.facts {
            Some(facts) => (facts.name.clone().unwrap_or_default(), facts.bundle.clone()),
            None => (String::new(), None),
        };
        // (Only a program that's really there is offered to drag; one that isn't is said, with
        // System Settings open all the same.)
        let bundle = bundle.filter(|_| permission.by_hand).map(PathBuf::from);
        if let Some(missing) = bundle.as_ref().filter(|bundle| !(bundle.extension().is_some_and(|extension| extension == "app") && bundle.is_dir())) {
            self.problem = Some(format!("{name} isn’t where it should be ({}), so there’s nothing to drag. Try again once Kuvo has finished starting.", missing.display()).into());
        }
        match bundle.filter(|_| self.problem.is_none()) {
            Some(bundle) => self.show_tile(name.into(), bundle, cx),
            None => self.hide_tile(cx),
        }
        self.asking = Some(permission.id.clone());
        self.follow(cx);
        cx.notify();
    }

    /// While it's asked for, or the tile waits for the next: the tile put beside System Settings,
    /// and the box asked each second whether it's allowed yet (macOS says nothing when Screen
    /// Recording changes).
    fn follow(&mut self, cx: &mut Context<Self>) {
        self._follow = Some(cx.spawn(async move |this, cx| {
            let mut ticks = 0u32;
            // System Settings' window seen, and how long it's been gone since.
            let (mut seen, mut gone) = (false, 0u32);
            loop {
                cx.background_executor().timer(FOLLOWS_EVERY).await;
                ticks += 1;
                let going = this.update(cx, |this, cx| {
                    if this.asking.is_none() && this.tile.is_none() {
                        return false;
                    }
                    // System Settings closed without it (its window gone two seconds), or left
                    // open and forgotten: the tile goes, and the box is asked no more.
                    match this.place(cx) {
                        Some(true) => (seen, gone) = (true, 0),
                        Some(false) if seen => gone += 1,
                        _ => {}
                    }
                    if gone >= GONE_AFTER || ticks >= GIVEN_UP_AFTER {
                        this.stop_asking(cx);
                        return false;
                    }
                    if ticks.is_multiple_of(ASKS_EVERY) {
                        this.read(cx);
                    }
                    true
                });
                if !going.unwrap_or(false) {
                    return;
                }
            }
        }));
    }

    /// The tile shown for the program to drag: the one kept from the permission before, where
    /// it was, or a new one.
    fn show_tile(&mut self, name: SharedString, bundle: PathBuf, cx: &mut Context<Self>) {
        let live = self.live;
        if let Some(kept) = &mut self.tile {
            let shown = kept
                .window
                .update(cx, |tile, window, cx| {
                    tile.set(name.clone(), bundle.clone(), cx);
                    #[cfg(target_os = "macos")]
                    if live {
                        mac::set_shown(window, true);
                    }
                    let _ = (window, live);
                })
                .is_ok();
            if shown {
                kept.shown = true;
                return;
            }
            // (Its window went some other way: a new one.)
            self.tile = None;
        }
        let Some(window) = Tile::open(name, bundle, cx) else { return };
        let Ok(tile) = window.entity(cx) else { return };
        let closed = cx.subscribe(&tile, |this: &mut HeadsUp, _, _: &tile::Closed, cx| this.stop_asking(cx));
        #[cfg(target_os = "macos")]
        if live {
            window.update(cx, |_, window, _| mac::never_key(window)).ok();
        }
        self.tile = Some(KeptTile { window, shown: true, placed: false, _closed: closed });
    }

    /// The tile hidden where it is, kept for the next permission.
    fn hide_tile(&mut self, cx: &mut Context<Self>) {
        let live = self.live;
        let Some(kept) = self.tile.as_mut().filter(|kept| kept.shown) else { return };
        kept.shown = false;
        #[cfg(target_os = "macos")]
        if live {
            kept.window.update(cx, |_, window, _| mac::set_shown(window, false)).ok();
        }
        let _ = (cx, live);
    }

    /// The tile gone: hidden at once, and its window let go a moment later. (A window AppKit
    /// still watched as it went, for the Touch Bar, stopped the app; this one never takes the
    /// keyboard, so isn't watched, and is out of sight well before it's let go all the same.)
    fn close_tile(&mut self, cx: &mut Context<Self>) {
        let Some(kept) = self.tile.take() else { return };
        let window = kept.window;
        #[cfg(target_os = "macos")]
        if self.live {
            window.update(cx, |_, window, _| mac::set_shown(window, false)).ok();
        }
        // (Let go whatever becomes of this: the step may be over by then.)
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(LET_GO_AFTER).await;
            window.update(cx, |_, window, _| window.remove_window()).ok();
        })
        .detach();
    }

    /// Puts the tile beside System Settings' window the first time it has one on screen, and
    /// leaves it be after that. Says whether that window is there (none where that isn't looked
    /// for: tests and shots follow no real window).
    fn place(&mut self, cx: &mut Context<Self>) -> Option<bool> {
        #[cfg(target_os = "macos")]
        if self.live {
            let Some(settings) = mac::settings_window() else { return Some(false) };
            if let Some(kept) = self.tile.as_mut().filter(|kept| kept.shown && !kept.placed) {
                let displays: Vec<_> = cx.displays().iter().map(|display| display.bounds()).collect();
                let main = cx.primary_display().map(|display| display.bounds())?;
                let screen = displays.into_iter().find(|display| display.intersects(&settings)).unwrap_or(main);
                let to = tile::beside(settings, screen);
                kept.placed = true;
                kept.window.update(cx, |_, window, _| mac::move_window(window, to, main.size.height)).ok();
            }
            return Some(true);
        }
        let _ = cx;
        None
    }

    /// The tile goes, and the box is no longer asked each second.
    pub fn stop_asking(&mut self, cx: &mut Context<Self>) {
        self.asking = None;
        self.close_tile(cx);
        self._follow = None;
        cx.notify();
    }

    /// Not Now: every one still asked about is put off, and the box keeps that, so no client asks
    /// again until it's been allowed and taken away. Nothing waits on it: what couldn't be kept is
    /// only asked about again the next time you set up.
    pub fn not_now(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.list().iter().filter(|permission| permission.asks).map(|permission| permission.id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        self.stop_asking(cx);
        self.asked += 1;
        self.run(
            move |client| {
                // (The first one refused stops it, and says so: none is put off unseen.)
                let mut said = None;
                for id in &ids {
                    let answer = client.permission_later(id);
                    let refused = answer.is_err();
                    said = Some(answer);
                    if refused {
                        break;
                    }
                }
                said
            },
            |this, said, cx| match said {
                Some(Ok(facts)) => {
                    this.problem = None;
                    this.take(facts, cx);
                }
                Some(Err(error)) => this.problem = Some(kuvo_box::accounts::said(&error).into()),
                None => {}
            },
            cx,
        );
    }

    /// One permission's row: its name, the box's line (or, while System Settings is open on a
    /// list it's added to by hand, what to do there), and at its end Allowed, or its own Open
    /// System Settings.
    fn row(&self, permission: &Permission, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let program = self.program();
        let asking = self.asking() == Some(permission.id.as_str());
        let (detail, color) = match permission.allowed {
            Some(true) => (permission.line.clone().unwrap_or_default(), c.text_muted),
            _ if asking && permission.by_hand => (format!("Drag {program} into the list in System Settings, then switch it on."), c.text),
            Some(false) => (permission.line.clone().unwrap_or_else(|| "Not allowed yet.".into()), c.text_muted),
            None => (format!("{program} hasn’t said yet."), c.text_muted),
        };
        let icon = if permission.id == "screenRecording" { IconName::Computer } else { IconName::Return };
        let id = permission.id.clone();
        let end: AnyElement = match permission.allowed {
            Some(true) => div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("permission-{id}-allowed")
                })
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1_5)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(Icon::new(IconName::Check).color(c.success))
                .child("Allowed")
                .into_any_element(),
            _ => Button::new(SharedString::from(format!("permission-{id}-open")), "Open System Settings")
                .size(ButtonSize::Small)
                .when(asking, |button| button.primary())
                .on_click(cx.listener(move |this, _, _, cx| this.open_settings(&id, cx)))
                .into_any_element(),
        };
        let selector = format!("permission-{}", permission.id);
        div()
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S4)
            .py(space::S3)
            .child(crate::first_run::icon_tile(icon, cx))
            // (Its line wraps: what to do in System Settings is read whole.)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).truncate().child(permission.name.clone()))
                    .child(div().text_ui(TypeSize::Caption).text_color(color).child(detail)),
            )
            .child(end)
            .into_any_element()
    }

    /// The step's heading words: its title, and the line under it.
    pub fn heading(&self) -> (String, String) {
        let program = self.program();
        // (Only ever this Mac's own: System Settings opens on no other.)
        let place = "this Mac";
        (format!("Let {program} work on {place}"), format!("Agents on {place} run as {program}. Only you can allow these, in System Settings. You can change them later in Settings › This Mac."))
    }
}

/// The step's body: each permission's row, and what went wrong, where something did.
impl gpui::Render for HeadsUp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let list: Vec<Permission> = self.list().to_vec();
        let rows: Vec<AnyElement> = list.iter().map(|permission| self.row(permission, cx)).collect();
        div()
            .debug_selector(|| "permissions-step".into())
            .flex()
            .flex_col()
            .flex_none()
            .gap(space::S3)
            .child(crate::first_run::list(rows, cx))
            .children(self.problem.clone().map(|why| div().debug_selector(|| "permission-problem".into()).px(space::S1).text_ui(TypeSize::Small).text_color(c.danger).child(why)))
    }
}
