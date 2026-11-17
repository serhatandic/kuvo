//! Where a menu or popover opens: one rule for the whole app, in [`place`].
//!
//! A popover opens from what opened it, the way macOS and Zed do:
//! - [`Placement::below`]: under its trigger, lined up with the trigger's leading edge (a button,
//!   a title bar menu, a row). [`Placement::align_end`] lines it up with the trailing edge instead,
//!   for a trigger at the end of a bar;
//! - [`Placement::above`]: over its trigger, for a control near the window's foot (the composer's);
//! - [`Placement::beside`]: to the trigger's right, its top level with the trigger's (a rail item);
//! - [`Popover::at`]: at the pointer, for a right-click.
//!
//! Where there isn't room, it flips to the other side (up rather than down, left rather than
//! right) if the window has more room there, and is then shifted to stay fully inside the window,
//! [`MARGIN`] from its edges.
//!
//! The trigger is whatever element the [`Popover`] is a child of: it measures that element as it's
//! drawn and places the popover in the same frame, so a popover follows its trigger as the window
//! scrolls or resizes.
//!
//! Its entry motion moves away from the side asked for; one that flips still moves in that way
//! (a few pixels, while it fades in), as the motion is chosen before the window has its say.

use std::{cell::Cell, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, Display, Element, ElementId, GlobalElementId, InspectorElementId,
    InteractiveElement as _, IntoElement, LayoutId, ParentElement as _, Pixels, Point, Position, RenderOnce, Size, Style,
    Styled as _, Window, canvas, deferred, div, point, px,
};

use crate::{
    motion::{Direction, MotionExt as _},
    theme::space,
};

/// The space between a popover and its trigger.
pub const GAP: Pixels = space::S1;
/// How close a popover comes to the window's edges.
pub const MARGIN: Pixels = px(8.);

/// Which side of its trigger a popover opens on, before the window has its say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Below,
    Above,
    /// To the right, top-aligned.
    Right,
}

/// Which of the trigger's edges a popover lines up with, along the side it opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// The leading edge (left, or top beside it).
    Start,
    /// The trailing edge (right, or bottom beside it).
    End,
}

/// Where a popover opens from its trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub side: Side,
    pub align: Align,
    pub gap: Pixels,
}

impl Placement {
    /// Under the trigger, at its leading edge: a button's menu, a title bar's, a row's.
    pub const fn below() -> Self {
        Self { side: Side::Below, align: Align::Start, gap: GAP }
    }

    /// Over the trigger, at its leading edge: a control near the window's foot.
    pub const fn above() -> Self {
        Self { side: Side::Above, align: Align::Start, gap: GAP }
    }

    /// To the trigger's right, top-aligned: a rail item's card.
    pub const fn beside() -> Self {
        Self { side: Side::Right, align: Align::Start, gap: GAP }
    }

    /// Lined up with the trigger's trailing edge: a trigger at the end of a bar or a row.
    pub const fn align_end(mut self) -> Self {
        self.align = Align::End;
        self
    }

    /// Touching the trigger, for a card that draws its own gap as padding it takes the pointer on
    /// (so moving from the trigger onto it never crosses a gap that would close it).
    pub const fn flush(mut self) -> Self {
        self.gap = px(0.);
        self
    }

    /// The way it moves in: away from the trigger.
    fn motion(self) -> Direction {
        match self.side {
            Side::Below => Direction::FromTop,
            Side::Above => Direction::FromBottom,
            Side::Right => Direction::FromLeft,
        }
    }
}

/// The rule: where a popover of `size` opens from `trigger`, inside `window` (both in window
/// coordinates). It goes on the placement's side, lined up with the placement's edge; on either
/// axis, if it would leave the window and the other side (or edge) leaves it less, it flips; then
/// it's shifted to stay [`MARGIN`] inside the window (pinned to the top left if it's larger).
pub fn place(trigger: Bounds<Pixels>, size: Size<Pixels>, window: Bounds<Pixels>, placement: Placement) -> Bounds<Pixels> {
    let Placement { side, align, gap } = placement;
    let (left, right) = (window.left() + MARGIN, window.right() - MARGIN);
    let (top, bottom) = (window.top() + MARGIN, window.bottom() - MARGIN);
    let start_first = align == Align::Start;
    let (x, y) = match side {
        Side::Below | Side::Above => {
            let (below, above) = (trigger.bottom() + gap, trigger.top() - gap - size.height);
            let y = if side == Side::Below { pick(below, above, size.height, top, bottom) } else { pick(above, below, size.height, top, bottom) };
            let (leading, trailing) = (trigger.left(), trigger.right() - size.width);
            let x = if start_first { pick(leading, trailing, size.width, left, right) } else { pick(trailing, leading, size.width, left, right) };
            (x, y)
        }
        Side::Right => {
            let x = pick(trigger.right() + gap, trigger.left() - gap - size.width, size.width, left, right);
            let (level, raised) = (trigger.top(), trigger.bottom() - size.height);
            let y = if start_first { pick(level, raised, size.height, top, bottom) } else { pick(raised, level, size.height, top, bottom) };
            (x, y)
        }
    };
    let x = x.min(right - size.width).max(left);
    let y = y.min(bottom - size.height).max(top);
    Bounds { origin: point(x, y), size }
}

/// `first`, unless it leaves `lo..hi` and `other` leaves it less.
fn pick(first: Pixels, other: Pixels, len: Pixels, lo: Pixels, hi: Pixels) -> Pixels {
    let outside = |at: Pixels| (lo - at).max(px(0.)) + (at + len - hi).max(px(0.));
    if outside(first) > px(0.) && outside(other) < outside(first) { other } else { first }
}

/// What a popover opens from.
#[derive(Clone)]
enum Target {
    /// The element the popover is a child of, as measured this frame.
    Parent(Rc<Cell<Option<Bounds<Pixels>>>>),
    /// A point in the window (the pointer, for a right-click).
    Point(Point<Pixels>),
    /// An area in the window (the end of a selection's line), measured by the caller.
    Area(Bounds<Pixels>),
}

/// A popover over everything else, opened from its trigger by [`place`]. Make it a child of its
/// trigger (or of an element around it), or open it at a point with [`Popover::at`].
#[derive(IntoElement)]
pub struct Popover {
    id: ElementId,
    target: Target,
    placement: Placement,
    priority: usize,
    content: AnyElement,
}

impl Popover {
    /// `content`, opened from the element this is a child of. `id` names its motion in: give a new
    /// one to each thing it shows, so each moves in afresh.
    pub fn new(id: impl Into<ElementId>, placement: Placement, content: impl IntoElement) -> Self {
        Self { id: id.into(), target: Target::Parent(Rc::default()), placement, priority: 1, content: content.into_any_element() }
    }

    /// `content`, opened at `position` in the window (a right-click's).
    pub fn at(id: impl Into<ElementId>, position: Point<Pixels>, content: impl IntoElement) -> Self {
        Self { id: id.into(), target: Target::Point(position), placement: Placement::below().flush(), priority: 1, content: content.into_any_element() }
    }

    /// `content`, opened from `area` in the window (the end of a selection, say), placed as
    /// `placement` says: the one rule, whatever it opens from.
    pub fn from_area(id: impl Into<ElementId>, area: Bounds<Pixels>, placement: Placement, content: impl IntoElement) -> Self {
        Self { id: id.into(), target: Target::Area(area), placement, priority: 1, content: content.into_any_element() }
    }

    /// Drawn over popovers of a lower priority (and what they draw, like a scrim).
    pub fn priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }
}

impl RenderOnce for Popover {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let content = div().id(self.id).child(self.content).animate_in(self.placement.motion(), true).into_any_element();
        let placed = deferred(Placed { target: self.target.clone(), placement: self.placement, content }).with_priority(self.priority);
        match self.target {
            // (Absolute, so it takes no room in what holds it.)
            Target::Point(_) | Target::Area(_) => div().absolute().child(placed),
            // Measures the trigger as it's prepainted; the popover, deferred, is placed after.
            Target::Parent(measured) => div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(canvas(move |bounds, _, _| measured.set(Some(bounds)), |_, _, _, _| {}).size_full())
                .child(placed),
        }
    }
}

/// Lays its content out at its own size and places it by the rule once the trigger is measured.
struct Placed {
    target: Target,
    placement: Placement,
    content: AnyElement,
}

impl Element for Placed {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, LayoutId) {
        let content = self.content.request_layout(window, cx);
        let style = Style { position: Position::Absolute, display: Display::Flex, ..Style::default() };
        (window.request_layout(style, [content], cx), content)
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, content: &mut LayoutId, window: &mut Window, cx: &mut App) {
        let size = window.layout_bounds(*content).size;
        let trigger = match &self.target {
            Target::Parent(measured) => measured.get().unwrap_or(Bounds { origin: bounds.origin, size: Size::default() }),
            Target::Point(at) => Bounds { origin: *at, size: Size::default() },
            Target::Area(area) => *area,
        };
        let area = Bounds { origin: Point::default(), size: window.viewport_size() }.inset(window.client_inset().unwrap_or(px(0.)));
        let placed = place(trigger, size, area, self.placement);
        let offset = placed.origin - bounds.origin;
        window.with_element_offset(point(offset.x.round(), offset.y.round()), |window| self.content.prepaint(window, cx));
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut LayoutId, _: &mut (), window: &mut Window, cx: &mut App) {
        self.content.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn window() -> Bounds<Pixels> {
        Bounds { origin: Point::default(), size: size(px(1000.), px(700.)) }
    }

    fn at(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
    }

    const MENU: Size<Pixels> = Size { width: px(240.), height: px(200.) };

    #[test]
    fn a_buttons_menu_opens_below_it_at_its_leading_edge() {
        let button = at(100., 50., 80., 28.);
        assert_eq!(place(button, MENU, window(), Placement::below()).origin, point(px(100.), px(82.)));
        let at_the_end = at(700., 50., 80., 28.);
        assert_eq!(place(at_the_end, MENU, window(), Placement::below().align_end()).origin, point(px(540.), px(82.)));
        // Lined up with its trailing edge, a menu that would leave the window's left side flips
        // to its leading edge.
        assert_eq!(place(button, MENU, window(), Placement::below().align_end()).origin, point(px(100.), px(82.)));
    }

    #[test]
    fn near_the_foot_it_flips_up_and_near_the_right_edge_left() {
        let button = at(900., 600., 60., 28.);
        let placed = place(button, MENU, window(), Placement::below());
        assert_eq!(placed.bottom(), px(596.), "over the button");
        assert_eq!(placed.right(), px(960.), "lined up with its trailing edge");
    }

    #[test]
    fn a_composer_control_opens_above_and_flips_down_only_without_room_above() {
        let control = at(300., 620., 100., 28.);
        assert_eq!(place(control, MENU, window(), Placement::above()).bottom(), px(616.));
        let high = at(300., 60., 100., 28.);
        assert_eq!(place(high, MENU, window(), Placement::above()).top(), px(92.));
    }

    #[test]
    fn a_rail_items_card_opens_to_its_right_top_aligned_and_flips_left_at_the_edge() {
        let ring = at(10., 300., 32., 32.);
        let card = place(ring, MENU, window(), Placement::beside().flush());
        assert_eq!(card.origin, point(px(42.), px(300.)));
        let low = at(10., 600., 32., 32.);
        assert_eq!(place(low, MENU, window(), Placement::beside()).bottom(), px(632.), "raised to end level with it");
        let far = at(900., 300., 32., 32.);
        assert_eq!(place(far, MENU, window(), Placement::beside()).right(), px(896.));
    }

    #[test]
    fn what_fits_nowhere_is_shifted_inside_the_window() {
        let tall = size(px(240.), px(650.));
        let button = at(100., 300., 80., 28.);
        let placed = place(button, tall, window(), Placement::below());
        assert_eq!(placed.bottom(), px(692.), "8 px from the foot");
        let huge = size(px(1200.), px(900.));
        assert_eq!(place(button, huge, window(), Placement::below()).origin, point(px(8.), px(8.)), "pinned to the top left");
    }
}
