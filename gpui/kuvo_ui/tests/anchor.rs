//! Where menus and popovers open, as people see them: from their trigger, by the one rule in
//! `kuvo_ui::anchor`, flipping or shifting at the window's edges.

use gpui::{
    AppContext as _, Bounds, Context, Entity, InteractiveElement, IntoElement, Modifiers, MouseButton, ParentElement, Pixels,
    Render, Styled, TestAppContext, prelude::FluentBuilder as _, VisualTestContext, Window, div, point, px, size,
};
use kuvo_ui::{
    Appearance, Theme,
    anchor::{GAP, MARGIN, Placement, Popover},
    menu::{ContextMenuArea, Menu, MenuEntry, PopoverMenu},
};

fn menu(cx: &mut gpui::App) -> Entity<Menu> {
    cx.new(|cx| Menu::new(cx).header("Models").entry(MenuEntry::new("Claude Opus 5.5")).entry(MenuEntry::new("GPT-6")).separator().entry(MenuEntry::new("Add a model…")))
}

/// A window of 800 × 600 with triggers where the app has them: a toolbar's buttons at the top, one
/// at the end of the bar, a composer's control at the foot, a rail item at the left, and an area
/// to right-click.
struct Host {
    rail_open: bool,
    /// How far the first button has moved right from where it starts.
    moved: f32,
}

/// A trigger, `w` × `h`.
fn spot(w: f32, h: f32, id: &'static str) -> gpui::Div {
    div().debug_selector(move || id.into()).w(px(w)).h(px(h))
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let card = div().debug_selector(|| "card".into()).w(px(200.)).h(px(150.));
        div()
            .size_full()
            .child(ContextMenuArea::new("area", div().size_full(), |_, cx| menu(cx)).fill())
            .child(div().absolute().left(px(100. + self.moved)).top(px(40.)).child(PopoverMenu::new("button", spot(120., 30., "button"), |_, cx| menu(cx))))
            .child(div().absolute().left(px(700.)).top(px(40.)).child(PopoverMenu::new("bar-end", spot(80., 30., "bar-end"), |_, cx| menu(cx)).align_right()))
            .child(div().absolute().left(px(740.)).top(px(80.)).child(PopoverMenu::new("edge", spot(50., 30., "edge"), |_, cx| menu(cx))))
            .child(div().absolute().left(px(100.)).top(px(540.)).child(PopoverMenu::new("composer", spot(120., 30., "composer"), |_, cx| menu(cx)).above()))
            .child(div().absolute().left(px(300.)).top(px(540.)).child(PopoverMenu::new("foot", spot(120., 30., "foot"), |_, cx| menu(cx))))
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(200.))
                    .debug_selector(|| "rail-item".into())
                    .w(px(32.))
                    .h(px(32.))
                    .when(self.rail_open, |d| d.child(Popover::new("rail-card", Placement::beside(), card))),
            )
    }
}

fn host(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo_ui::bind_keys(cx);
    });
    let (host, cx) = cx.add_window_view(|_, _| Host { rail_open: false, moved: 0. });
    cx.simulate_resize(size(px(800.), px(600.)));
    cx.run_until_parked();
    (host, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is drawn"))
}

/// Lets the entry motion (a few pixels' slide) finish, so what's measured is where it rests.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..12 {
        cx.executor().advance_clock(std::time::Duration::from_micros(16_667));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

/// Clicks a trigger, and gives the menu that opens.
fn open(cx: &mut VisualTestContext, trigger: &'static str) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let at = bounds(cx, trigger);
    cx.simulate_click(at.center(), Modifiers::default());
    settle(cx);
    (at, bounds(cx, "menu"))
}

fn inside_window(menu: Bounds<Pixels>) {
    assert!(menu.left() >= MARGIN && menu.top() >= MARGIN && menu.right() <= px(800.) - MARGIN && menu.bottom() <= px(600.) - MARGIN, "inside the window: {menu:?}");
}

#[gpui::test]
fn a_buttons_menu_opens_below_it_at_its_leading_edge(cx: &mut TestAppContext) {
    let (_, cx) = host(cx);
    let (button, menu) = open(cx, "button");
    assert_eq!(menu.top(), button.bottom() + GAP, "below it: {menu:?} from {button:?}");
    assert_eq!(menu.left(), button.left(), "at its leading edge");
}

#[gpui::test]
fn a_menu_at_the_end_of_a_bar_lines_up_with_its_trailing_edge(cx: &mut TestAppContext) {
    let (_, cx) = host(cx);
    let (button, menu) = open(cx, "bar-end");
    assert_eq!(menu.top(), button.bottom() + GAP);
    assert_eq!(menu.right(), button.right());
}

#[gpui::test]
fn at_the_right_edge_a_menu_flips_to_end_at_its_buttons_trailing_edge(cx: &mut TestAppContext) {
    let (_, cx) = host(cx);
    let (button, menu) = open(cx, "edge");
    assert_eq!(menu.top(), button.bottom() + GAP);
    assert_eq!(menu.right(), button.right(), "flipped left, not cut off: {menu:?} from {button:?}");
    inside_window(menu);
}

#[gpui::test]
fn a_composer_control_opens_its_menu_above_it(cx: &mut TestAppContext) {
    let (_, cx) = host(cx);
    let (control, menu) = open(cx, "composer");
    assert_eq!(menu.bottom(), control.top() - GAP, "above it: {menu:?} from {control:?}");
    assert_eq!(menu.left(), control.left());
}

#[gpui::test]
fn near_the_foot_a_menu_below_flips_up(cx: &mut TestAppContext) {
    let (_, cx) = host(cx);
    let (button, menu) = open(cx, "foot");
    assert_eq!(menu.bottom(), button.top() - GAP, "over the button, not over it: {menu:?} from {button:?}");
    inside_window(menu);
}

#[gpui::test]
fn a_rail_items_card_opens_to_its_right_top_aligned(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    host.update(cx, |host, cx| {
        host.rail_open = true;
        cx.notify();
    });
    settle(cx);
    let (item, card) = (bounds(cx, "rail-item"), bounds(cx, "card"));
    assert_eq!(card.left(), item.right() + GAP, "to its right: {card:?} from {item:?}");
    assert_eq!(card.top(), item.top(), "top-aligned");
}

#[gpui::test]
fn a_right_click_menu_opens_at_the_pointer_and_flips_at_the_corner(cx: &mut TestAppContext) {
    let (_, cx) = host(cx);
    let right_click = |cx: &mut VisualTestContext, at| {
        cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
        settle(cx);
    };
    let at = point(px(400.), px(250.));
    right_click(cx, at);
    let menu = bounds(cx, "menu");
    assert_eq!(menu.origin, at, "its top left at the pointer");

    // Near the bottom right corner, it opens up and to the left of the pointer instead.
    cx.simulate_keystrokes("escape");
    let corner = point(px(760.), px(560.));
    right_click(cx, corner);
    let menu = bounds(cx, "menu");
    assert_eq!((menu.right(), menu.bottom()), (corner.x, corner.y), "its bottom right at the pointer: {menu:?}");
    inside_window(menu);
}

/// The trigger is measured in the frame that draws the menu: with no motion to settle, the menu is
/// against its button on the first frame it's drawn, and when the button moves (a list scrolling
/// under it), the menu moves with it in the next frame.
#[gpui::test]
fn a_menu_is_in_place_on_its_first_frame_and_follows_its_trigger(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    let button = bounds(cx, "button");
    cx.simulate_click(button.center(), Modifiers::default());
    cx.run_until_parked();
    let menu = bounds(cx, "menu");
    assert_eq!(menu.origin, point(button.left(), button.bottom() + GAP), "in place at once: {menu:?} from {button:?}");

    host.update(cx, |host, cx| {
        host.moved = 50.;
        cx.notify();
    });
    cx.run_until_parked();
    let moved = bounds(cx, "button");
    assert_eq!(moved.left(), button.left() + px(50.));
    assert_eq!(bounds(cx, "menu").origin, point(moved.left(), moved.bottom() + GAP), "it moved with its button");
}
