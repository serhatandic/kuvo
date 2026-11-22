//! The rail: the areas' buttons at the top, with what waits on you in each on its icon; at its foot
//! a usage ring per account, then Help and Settings. Clicking the area already open folds its
//! sidebar away, or brings it back (Zed's panel buttons).
//!
//! The rings are their own view ([`Rings`]): a limits update redraws only the rings.

use kuvo_ui::anchor::{Placement, Popover};
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Hsla, InteractiveElement, IntoElement, LineCap, ParentElement, PathBuilder, PathStyle, Pixels, Render, SharedString,
    StatefulInteractiveElement, StrokeOptions, Styled, Task, Window, canvas, div, point, prelude::FluentBuilder as _, px,
};
use kuvo_box::areas::{Account, Level};
use kuvo_box::error::Offered;
use kuvo_box::{BoxClient, Refusal, Timing, follow::Followed};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    menu::{Tooltip, card_frame},
    small::{count_badge, spinner},
    theme::{radius, space},
};

use crate::area::{Area, OpenHelp, ShowAccounts, ShowUsage, UsageFilter};
use crate::time;
use crate::workspace::Workspace;

/// How wide it is.
pub const WIDTH: Pixels = px(46.);
/// Its buttons' size.
const BUTTON: Pixels = px(36.);
/// A ring's size and stroke; an account's second ring sits just inside the first.
const RING: Pixels = px(34.);
const STROKE: Pixels = px(2.5);
const INNER_GAP: Pixels = px(1.5);
/// How many rings show before "+N", where the window is tall enough.
const MOST_RINGS: usize = 4;
/// The room a ring takes in the rail, its gap included.
const RING_ROOM: f32 = 48.;
/// The rail's height besides its rings: the title bar, the six areas, Help and Settings, gaps.
const RAIL_REST: f32 = 44. + 6. * 46. + 2. * 46. + 40.;
/// How often the accounts are read again while the app runs (the box reads them on its own clock;
/// this only picks up what it read), and sooner after a read that failed (a box still starting).
const REREAD: Duration = Duration::from_secs(120);
const RETRY: Duration = Duration::from_secs(10);
/// How the rings show while the coding agents restart: the last readings, quieter.
const UPDATING_OPACITY: f32 = 0.5;
/// How long the pointer rests on a ring before its card opens (one already open moves at once).
pub const CARD_DELAY: Duration = Duration::from_millis(300);
/// How long the card stays once the pointer is on neither ring nor card: across the gap between
/// two rings, the open card moves to the next at once rather than closing.
pub const CARD_GRACE: Duration = Duration::from_millis(120);

pub fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let open = workspace.area();
    let waiting = workspace.waiting();
    let top = [Area::Bots, Area::Sessions, Area::Queue, Area::Library, Area::Computers, Area::Insights].into_iter().map(|area| {
        // How many things wait on you there, as the box counts them (`kuvo/waiting`).
        let count = match area {
            Area::Sessions => waiting.sessions,
            Area::Bots => waiting.bots,
            Area::Queue => workspace.queue_badge(cx),
            _ => 0,
        };
        area_button(area, area == open, count, cx)
    });
    div()
        .id("rail")
        .w(WIDTH)
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .pt(space::S1)
        .pb(space::S3)
        .gap(space::S1_5)
        .children(top)
        .child(div().flex_1())
        .child(workspace.rings().clone())
        .child(
            button("rail-help", IconName::Question, false, 0, cx)
                .tooltip(|_, cx| Tooltip::new("Help").build(cx))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenHelp), cx)),
        )
        .child(area_button(Area::Settings, open == Area::Settings, 0, cx))
        .into_any_element()
}

fn area_button(area: Area, selected: bool, waiting: usize, cx: &mut Context<Workspace>) -> AnyElement {
    let (name, keys) = (area.name(), area.keys());
    button(SharedString::from(format!("rail-{}", area.slug())), area.icon(), selected, waiting, cx)
        // (Its id says whether it's open, so a tooltip from before doesn't linger: Zed's.)
        .tooltip(move |_, cx| Tooltip::new(name).shortcut(keys).build(cx))
        .on_click(cx.listener(move |this, _, window, cx| this.rail_pressed(area, window, cx)))
        .into_any_element()
}

fn button(id: impl Into<SharedString>, icon: IconName, selected: bool, waiting: usize, cx: &App) -> gpui::Stateful<gpui::Div> {
    let c = &cx.theme().colors;
    let (hover, selected_bg) = (c.hover, c.selected);
    let id: SharedString = id.into();
    let selector = id.to_string();
    div()
        .id(SharedString::from(format!("{id}-{}", if selected { "open" } else { "shut" })))
        .debug_selector(move || selector)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(BUTTON)
        .rounded(radius::NAV)
        .cursor_pointer()
        .when(selected, |t| t.bg(selected_bg))
        .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
        // The area open is told by its tile, not a heavier icon: its icon only steps up
        // (`icon_on`), with the same line as the rest (`kuvo_ui::icon::STROKE_PX`).
        .child(Icon::new(icon).size(IconSize::Rail).color(if selected { c.icon_on } else { c.icon }))
        .when(waiting > 0, |t| t.child(div().absolute().top(px(-3.)).right(px(-4.)).child(count_badge(waiting, cx))))
}

// MARK: The rings

/// The accounts' rings, in the box's order (its limits' `rows`), as their own view.
pub struct Rings {
    client: BoxClient,
    /// Follow the box's limits (the app); else only read as asked (shots and tests).
    live: bool,
    accounts: Option<Vec<Account>>,
    /// The coding agents' state, where the last read was refused for it (PROTOCOL.md, "The coding
    /// agents' state"): restarting, the readings before stay, marked as updating; down, its
    /// sentence in their place, with the box's Try Again.
    agents: Option<Refusal>,
    /// The box's Try Again under way.
    trying: Option<Task<()>>,
    /// Now, as a window's "resets in" counts from it (fixed for shots).
    now: Option<f64>,
    _following: Option<Task<()>>,
    fallback: Option<Task<()>>,
    again: Option<Task<()>>,
    again_pending: bool,
    /// The ring whose card is open (its place among those drawn), the ring the pointer is on, and
    /// whether it's on the card; the card waiting to open.
    card: Option<usize>,
    on_ring: Option<usize>,
    on_card: bool,
    opening: Option<Task<()>>,
    closing: Option<Task<()>>,
}

impl Rings {
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> Rings {
        let mut rings = Rings { client, live, accounts: None, agents: None, trying: None, now: None, _following: None, fallback: None, again: None, again_pending: false, card: None, on_ring: None, on_card: false, opening: None, closing: None };
        if live {
            if rings.client.streams() {
                let follow = rings.client.follow_usage(Timing::default());
                rings._following = Some(cx.spawn(async move |this, cx| {
                    while let Ok(update) = follow.updates().recv().await {
                        if matches!(update, Followed::Now(_))
                            && this.update(cx, |this, cx| this.read_again(cx)).is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            rings.read_again(cx);
        }
        rings
    }

    /// Reads the accounts now, off this thread, and schedules a fallback if no event comes.
    pub fn read_again(&mut self, cx: &mut Context<Self>) {
        if !self.live {
            return self.read_at_once(cx);
        }
        self.fallback = None;
        if self.again.is_some() {
            self.again_pending = true;
            return;
        }
        let client = self.client.clone();
        self.again = Some(cx.spawn(async move |this, cx| {
            let accounts = cx.background_executor().spawn(async move { client.accounts() }).await;
            let delay = if accounts.is_ok() { REREAD } else { RETRY };
            this.update(cx, |this, cx| {
                this.again = None;
                this.take(accounts, cx);
                if std::mem::take(&mut this.again_pending) {
                    this.read_again(cx);
                    return;
                }
                this.fallback = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    this.update(cx, |this, cx| this.read_again(cx)).ok();
                }));
            }).ok();
        }));
    }

    /// Reads the accounts at once, on this thread (shots and tests, against a stand-in).
    pub fn read_at_once(&mut self, cx: &mut Context<Self>) {
        let accounts = self.client.accounts();
        self.take(accounts, cx);
    }

    pub fn set_now(&mut self, now: f64) {
        self.now = Some(now);
    }

    /// What the box said. One that can't be read keeps what was there; where that's for the
    /// coding agents' state, it's kept with it, to say.
    fn take(&mut self, accounts: kuvo_box::Result<Vec<Account>>, cx: &mut Context<Self>) {
        match accounts {
            Ok(accounts) => {
                self.accounts = Some(accounts);
                self.agents = None;
            }
            Err(error) => {
                if let Some(refusal) = error.refusal().filter(|refusal| refusal.about_agents()) {
                    self.agents = Some(refusal.clone());
                }
            }
        }
        cx.notify();
    }

    pub fn accounts(&self) -> Option<&[Account]> {
        self.accounts.as_deref()
    }

    /// The coding agents' state the rings show, where the box said one.
    pub fn agents(&self) -> Option<&Refusal> {
        self.agents.as_ref()
    }

    /// Whether the rings are marked as updating: the last readings, while the agents restart.
    pub fn updating(&self) -> bool {
        self.accounts.is_some() && self.agents.as_ref().is_some_and(Refusal::agents_restarting)
    }

    /// The box's Try Again for its coding agents: answered once they run (then read again), or with
    /// how they stand now.
    pub fn take_action(&mut self, action: Offered, cx: &mut Context<Self>) {
        if self.trying.is_some() {
            return;
        }
        let client = self.client.clone();
        let after = |this: &mut Rings, done: kuvo_box::Result<()>, cx: &mut Context<Rings>| {
            this.trying = None;
            match done {
                Err(error) if error.refusal().is_some_and(Refusal::about_agents) => this.agents = error.refusal().cloned(),
                // (Running, or it can't be asked: read again, which says which.)
                _ => this.read_again(cx),
            }
            cx.notify();
        };
        if !self.live {
            let done = client.take_action(&action);
            after(self, done, cx);
            return;
        }
        self.trying = Some(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { client.take_action(&action) }).await;
            this.update(cx, |this, cx| after(this, done, cx)).ok();
        }));
        cx.notify();
    }

    fn now(&self) -> f64 {
        self.now.unwrap_or_else(time::now_ms)
    }

    /// The accounts drawn as rings: those signed in on this computer, as its box says (`boxes`,
    /// `here`: what its Accounts shows), and never a plan that ended (`ended`).
    fn ringed(&self) -> Vec<&Account> {
        self.accounts.iter().flatten().filter(|account| account.here() && !account.ended).collect()
    }

    /// The pointer came onto a ring, or left it: its card opens once it rests there (at once
    /// where another's is open), and closes once the pointer is on neither ring nor card.
    fn ring_hovered(&mut self, ix: usize, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.on_ring = Some(ix);
            if self.card.is_some() {
                self.card = Some(ix);
                self.opening = None;
            } else {
                self.opening = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(CARD_DELAY).await;
                    this.update(cx, |this, cx| {
                        if this.on_ring == Some(ix) {
                            this.card = Some(ix);
                            cx.notify();
                        }
                    })
                    .ok();
                }));
            }
        } else if self.on_ring == Some(ix) {
            self.on_ring = None;
        }
        self.settle(cx);
    }

    fn card_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.on_card = hovered;
        self.settle(cx);
    }

    /// Off both ring and card: the card closes, after a moment (`CARD_GRACE`) in which reaching
    /// another ring moves it there instead.
    fn settle(&mut self, cx: &mut Context<Self>) {
        if self.on_ring.is_none() && !self.on_card {
            self.opening = None;
            if self.card.is_some() && self.closing.is_none() {
                self.closing = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(CARD_GRACE).await;
                    this.update(cx, |this, cx| {
                        this.closing = None;
                        if this.on_ring.is_none() && !this.on_card {
                            this.card = None;
                            cx.notify();
                        }
                    })
                    .ok();
                }));
            }
        } else {
            self.closing = None;
        }
        cx.notify();
    }

    /// Opens a ring's card as if the pointer rested on it (shots).
    pub fn show_card(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.on_ring = Some(ix);
        self.card = Some(ix);
        cx.notify();
    }

    fn close_card(&mut self, cx: &mut Context<Self>) {
        self.card = None;
        self.opening = None;
        self.closing = None;
        self.on_card = false;
        cx.notify();
    }
}

impl Render for Rings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        // The coding agents down (or restarting before anything was read): one ring saying so, in
        // the box's words, where the rings would be.
        let restarting = self.agents.as_ref().is_some_and(Refusal::agents_restarting);
        if let Some(state) = self.agents.clone().filter(|_| !restarting || self.accounts.is_none()) {
            if self.card.is_some_and(|ix| ix > 0) || self.on_ring.is_some_and(|ix| ix > 0) {
                self.card = None;
                self.on_ring = None;
            }
            let ring = state_ring(&state, self.card == Some(0), self.trying.is_some(), cx);
            return div().flex().flex_col().items_center().gap(space::S2).pb(space::S2).child(ring).into_any_element();
        }
        if self.accounts.is_none() {
            return div().into_any_element();
        }
        let accounts: Vec<Account> = self.ringed().into_iter().cloned().collect();
        let now = self.now();
        // As many as the rail has room for (four at most), the rest behind "+N" (which takes a
        // ring's room itself), so Help and Settings stay in reach in a short window.
        let room = ((f32::from(window.viewport_size().height) - RAIL_REST) / RING_ROOM).floor().max(0.) as usize;
        let most = if accounts.len() <= room.min(MOST_RINGS) { accounts.len() } else { room.saturating_sub(1).min(MOST_RINGS) };
        // A ring gone since (its account left the list, or behind "+N" now) takes its card along,
        // and what the pointer was on: its card's or ring's leaving is never heard once it's gone.
        if self.card.is_some_and(|ix| ix >= most) || self.on_ring.is_some_and(|ix| ix >= most) {
            self.card = None;
            self.on_ring = None;
            self.on_card = false;
            self.opening = None;
            self.closing = None;
        }
        let card = self.card;
        // (Restarting: each card says so, in the box's words, under what was last read.)
        let updating = self.agents.as_ref().filter(|_| restarting).map(|state| state.message.clone());
        let mut rings: Vec<AnyElement> = accounts.iter().take(most).enumerate().map(|(ix, account)| ring_button(ix, account, now, card == Some(ix), updating.clone(), cx)).collect();
        // Past the rail's room: "+N", opening Insights.
        if accounts.len() > most {
            let more = accounts.len() - most;
            let hover = c.hover;
            rings.push(
                div()
                    .id("more-rings")
                    .debug_selector(|| "more-rings".into())
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(RING)
                    .rounded(radius::PILL)
                    .border_1()
                    .border_color(c.border)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .text_ui(TypeSize::Caption)
                    .medium()
                    .text_color(c.text_muted)
                    .child(format!("+{more}"))
                    .tooltip(move |_, cx| Tooltip::new(format!("{more} more in Insights")).build(cx))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowUsage), cx))
                    .into_any_element(),
            );
        }
        // None signed in: an empty ring that says where to sign in.
        if accounts.is_empty() && !self.client.gate().is_held() {
            let hover = c.hover;
            rings.push(
                div()
                    .id("no-accounts")
                    .debug_selector(|| "no-accounts".into())
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(RING)
                    .rounded(radius::PILL)
                    .border_1()
                    .border_color(c.border)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(Icon::new(IconName::Plus).color(c.icon))
                    .tooltip(|_, cx| Tooltip::new("Sign in to an account to see its limits here").build(cx))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowAccounts), cx))
                    .into_any_element(),
            );
        }
        div()
            .debug_selector(move || if restarting { "rings-updating".into() } else { "rings".into() })
            .flex()
            .flex_col()
            .items_center()
            .gap(space::S2)
            .pb(space::S2)
            .children(rings)
            .into_any_element()
    }
}

/// The coding agents' state where the rings would be: a ring with a warning while they're down, a
/// spinner while they restart. Resting on it opens its card, with the box's sentence and its Try
/// Again; clicking it opens Insights.
fn state_ring(state: &Refusal, open: bool, trying: bool, cx: &mut Context<Rings>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let hover = c.hover;
    let down = !state.agents_restarting();
    let inside: AnyElement = if down { Icon::new(IconName::Warning).color(c.warning).into_any_element() } else { spinner("rings-restarting-spinner", IconSize::Nav, c.text_muted).into_any_element() };
    let selector = if down { "rings-agents-down" } else { "rings-agents-restarting" };
    div()
        .id(selector)
        .debug_selector(move || selector.into())
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(RING + px(6.))
        .rounded(radius::PILL)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(div().flex().flex_none().items_center().justify_center().size(RING).rounded(radius::PILL).border_1().border_color(c.border).child(inside))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.ring_hovered(0, *hovered, cx)))
        .on_click(cx.listener(|this, _, window, cx| {
            this.close_card(cx);
            window.dispatch_action(Box::new(ShowUsage), cx);
        }))
        .when(open, |ring| ring.child(state_card_beside(state, trying, cx)))
        .into_any_element()
}

/// The state ring's card: the box's sentence, and its Try Again where it offers one (busy while
/// it goes).
fn state_card_beside(state: &Refusal, trying: bool, cx: &mut Context<Rings>) -> impl IntoElement {
    let c = cx.theme().colors.clone();
    let again = state.action.clone().map(|action| {
        let label = action.label.clone();
        div().debug_selector(|| "rings-try-again".into()).child(
            Button::new("rings-try-again", label).size(ButtonSize::Small).disabled(trying).on_click(cx.listener(move |this, _, _, cx| this.take_action(action.clone(), cx))),
        )
    });
    let card = div()
        .flex()
        .flex_col()
        .items_start()
        .gap(space::S3)
        .child(div().debug_selector(|| "rings-agents-words".into()).text_ui(TypeSize::Small).text_color(c.text).child(state.message.clone()))
        .children(again);
    let content = div()
        .id("rings-agents-card")
        .debug_selector(|| "ring-card".into())
        .occlude()
        .pl(space::S2)
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.card_hovered(*hovered, cx)))
        .child(card_frame(card, cx));
    Popover::new("rings-agents-card-motion", Placement::beside().flush(), content)
}

/// The colour of a level, as the box says it: never a threshold of this app's.
pub fn level_color(level: Level, cx: &App) -> Hsla {
    let c = &cx.theme().colors;
    match level {
        Level::Over => c.danger,
        Level::Near => c.warning,
        Level::Calm => c.success,
        // A word this app doesn't know: quiet, never a colour of its own choosing.
        Level::Unknown => c.text_muted,
    }
}

fn ring_button(ix: usize, account: &Account, now: f64, open: bool, updating: Option<String>, cx: &mut Context<Rings>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let hover = c.hover;
    // The box's rings (`rows[].rings`): its short window outside and its long one inside, one
    // where it says one; each filled with what's left of it (a full ring, all of it), coloured by
    // the box's level. A plan with no window has an empty track.
    let rings = account.rings();
    let arcs_drawn: Vec<AnyElement> = if rings.is_empty() {
        vec![arcs(RING, STROKE, 0., c.text_muted, c.border).into_any_element()]
    } else {
        rings
            .iter()
            .enumerate()
            .map(|(ix, (used, level))| {
                let size = RING - (STROKE + INNER_GAP) * 2. * ix as f32;
                let inset = (STROKE + INNER_GAP) * ix as f32;
                let selector = format!("ring-{}-arc-{ix}", account_key(account));
                div().debug_selector(move || selector).absolute().top(inset).left(inset).size(size).child(arcs(size, STROKE, 1. - *used, level_color(*level, cx), c.border)).into_any_element()
            })
            .collect()
    };
    // The mark sits in the circle inside the innermost ring: the standard's share of it.
    let innermost = RING - (STROKE + INNER_GAP) * 2. * rings.len().saturating_sub(1) as f32;
    let mark: AnyElement = match kuvo_ui::icon::Mark::of_service(&account.service) {
        Some(mark) => mark.in_tile(innermost - STROKE * 2.).into_any_element(),
        // A service a newer box names, with no mark here: its first letter.
        None => div().text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child(account.service.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default()).into_any_element(),
    };
    let selector = format!("ring-{ix}");
    let filter = UsageFilter { title: account.title.clone(), account: account.id.clone().filter(|id| !id.is_empty()), provider: account.service.clone() };
    div()
        .id(SharedString::from(format!("ring-{ix}")))
        .debug_selector(move || selector)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(RING + px(6.))
        .rounded(radius::PILL)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(
            div()
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(RING)
                // (The last readings while the coding agents restart: quieter, its card says why.)
                .when(updating.is_some(), |d| d.opacity(UPDATING_OPACITY))
                .children(arcs_drawn)
                .child(mark),
        )
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.ring_hovered(ix, *hovered, cx)))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.close_card(cx);
            window.dispatch_action(Box::new(filter.clone()), cx);
        }))
        .when(open, |ring| ring.child(card_beside(account, now, updating, cx)))
        .into_any_element()
}

/// A ring's card: to its right, top-aligned with it (by the rule in `kuvo_ui::anchor`), over
/// everything else and taking the pointer's clicks itself, so what's under it isn't reached and the
/// rings below stay in view. It starts where the ring ends (its gap is padding it takes the pointer
/// on), so the pointer moves onto it without closing it.
fn card_beside(account: &Account, now: f64, updating: Option<String>, cx: &mut Context<Rings>) -> impl IntoElement {
    let card = account_card(account, now, updating, cx);
    let key = account_key(account);
    let content = div()
        .id(SharedString::from(format!("ring-card-{key}")))
        .debug_selector(|| "ring-card".into())
        .occlude()
        .pl(space::S2)
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.card_hovered(*hovered, cx)))
        .child(card_frame(card, cx));
    Popover::new(SharedString::from(format!("ring-card-motion-{key}")), Placement::beside().flush(), content)
}

/// How a ring's arcs are named in tests: the account's service and label.
fn account_key(account: &Account) -> String {
    format!("{}{}", account.service, account.label.as_deref().filter(|label| !label.is_empty()).map(|label| format!("-{label}")).unwrap_or_default())
}

/// Paints a ring: a faint track, and an arc from the top, clockwise, for `share` (0 to 1).
fn arcs(size: Pixels, stroke: Pixels, share: f32, color: Hsla, track: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let r = (size - stroke) / 2.;
            let center = point(bounds.origin.x + size / 2., bounds.origin.y + size / 2.);
            let style = || PathStyle::Stroke(StrokeOptions { line_width: f32::from(stroke), line_cap: LineCap::Round, ..Default::default() });
            let top = point(center.x, center.y - r);
            let bottom = point(center.x, center.y + r);
            let mut full = PathBuilder::stroke(stroke).with_style(style());
            full.move_to(top);
            full.arc_to(point(r, r), px(0.), false, true, bottom);
            full.arc_to(point(r, r), px(0.), false, true, top);
            if let Ok(path) = full.build() {
                window.paint_path(path, track);
            }
            let share = share.clamp(0., 1.);
            if share <= 0.005 {
                return;
            }
            let mut arc = PathBuilder::stroke(stroke).with_style(style());
            arc.move_to(top);
            if share >= 0.995 {
                arc.arc_to(point(r, r), px(0.), false, true, bottom);
                arc.arc_to(point(r, r), px(0.), false, true, top);
            } else {
                let angle = share * std::f32::consts::TAU;
                let end = point(center.x + r * angle.sin(), center.y - r * angle.cos());
                arc.arc_to(point(r, r), px(0.), share > 0.5, true, end);
            }
            if let Ok(path) = arc.build() {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size(size)
}

/// What resting on a ring shows: the account, its plan, and what's left of each window with when
/// it resets; while it's being updated (the coding agents restarting), the box's words for that
/// at its foot.
fn account_card(account: &Account, now: f64, updating: Option<String>, cx: &App) -> AnyElement {
    let c = cx.theme().colors.clone();
    let title = account.title.clone();
    let windows = account.windows.iter().map(|window| {
        // A window that started over, as the box says: not read since, not used (PROTOCOL.md).
        let over = window.expired;
        let color = level_color(window.level, cx);
        // (A level this app doesn't know is drawn as quietly as a calm one.)
        let calm = matches!(window.level, Level::Calm | Level::Unknown);
        let left = 1. - window.used.clamp(0., 1.);
        let said = if over { "Not read since it reset".to_owned() } else { format!("{}% left", (left * 100.).round() as u32) };
        let resets = window.resets_at.filter(|_| !over).map(|at| format!("Resets in {}", time::until(at, now)));
        div()
            .flex()
            .flex_col()
            .gap(space::S1)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::S2)
                    .text_ui(TypeSize::Small)
                    .child(div().min_w_0().truncate().text_color(c.text).child(window.title.clone()))
                    .child(div().flex_none().text_color(if calm { c.text_muted } else { color }).when(!calm, |d| d.medium()).child(said)),
            )
            .child(
                // (What's left, as the ring has it; one that reset since it was read, all of it.)
                div().h(px(4.)).w_full().rounded(radius::PILL).bg(c.border).child(div().h_full().w(gpui::relative(left)).rounded(radius::PILL).bg(color)),
            )
            .children(resets.map(|resets| div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(resets)))
    });
    div()
        .flex()
        .flex_col()
        .gap(space::S3)
        .child(
            div()
                .flex()
                .flex_col()
                .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(title))
                .children(account.plan.clone().map(|plan| div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(format!("{plan} plan")))),
        )
        .children(windows)
        .children(updating.map(|words| div().debug_selector(|| "ring-card-updating".into()).flex().items_start().gap(space::S1_5).text_ui(TypeSize::Caption).text_color(c.text_muted).child(div().pt(px(1.)).child(spinner("ring-card-updating-spinner", IconSize::Nav, c.text_faint))).child(div().flex_1().min_w_0().child(words))))
        .into_any_element()
}
