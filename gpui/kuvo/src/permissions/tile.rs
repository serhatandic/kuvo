//! The small window beside System Settings while it's open for a permission macOS has you add by
//! hand: the program's icon, to drag into the list, the way you'd drag an app from Finder. It
//! floats over System Settings, follows its window, and goes once the box says the permission is
//! allowed, or when you close it.
//!
//! The drag is GPUI's own: an element's drag that leaves its window is handed to the system as a
//! file drag (`external_drag_payload`), here the program's bundle.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Bounds, Context, CursorStyle, EventEmitter, ExternalDragPayload, FileDragPaths, Image, ImageFormat, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point, Render, SharedString, Size,
    StatefulInteractiveElement, Styled, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, img, point, px,
};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, StyledType, TypeSize, theme::{radius, space}};

use super::icns;

/// The tile's size, and how far it keeps from System Settings' window.
pub const SIZE: Size<Pixels> = Size { width: px(248.), height: px(214.) };
const GAP: f32 = 14.;
/// How far below the top of System Settings' window the tile sits: beside its list.
const DOWN: f32 = 140.;
const ICON: f32 = 64.;

/// You closed the tile yourself.
pub struct Closed;

/// What's dragged: the program's bundle.
#[derive(Clone)]
struct Dragged(PathBuf);

/// What follows the pointer until the drag leaves the tile's window and macOS draws its own.
struct Preview(Option<Arc<Image>>);

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().children(self.0.clone().map(|icon| img(icon).size(px(ICON))))
    }
}

pub struct Tile {
    /// The program's name, as System Settings lists it, and its bundle.
    name: SharedString,
    bundle: PathBuf,
    icon: Option<Arc<Image>>,
}

impl EventEmitter<Closed> for Tile {}

impl Tile {
    pub fn new(name: SharedString, bundle: PathBuf) -> Tile {
        let icon = icns::of_bundle(&bundle).map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)));
        Tile { name, bundle, icon }
    }

    pub fn bundle(&self) -> &PathBuf {
        &self.bundle
    }

    /// Opens the tile's window: over everything, taking no keyboard, at the right of the main
    /// display until System Settings' window is found.
    pub fn open(name: SharedString, bundle: PathBuf, cx: &mut App) -> Option<WindowHandle<Tile>> {
        let screen = cx.primary_display().map(|display| display.bounds())?;
        let origin = point(screen.size.width - SIZE.width - px(40.), (screen.size.height - SIZE.height) / 2.);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, SIZE))),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        cx.open_window(options, |_, cx| cx.new(|_| Tile::new(name, bundle))).ok()
    }
}

/// Where the tile goes beside System Settings' window (`settings`), on the display that window is
/// on (`screen`): on the side with room, a little below its top, where the list is; kept on
/// screen. All in one coordinate space.
pub fn beside(settings: Bounds<Pixels>, screen: Bounds<Pixels>) -> Point<Pixels> {
    let (gap, width, height) = (px(GAP), SIZE.width, SIZE.height);
    let mut x = settings.right() + gap;
    if x + width > screen.right() {
        x = settings.left() - gap - width;
    }
    if x < screen.left() {
        x = settings.right() - width - px(20.);
    }
    let lowest = (screen.bottom() - height).max(screen.top());
    let y = (settings.top() + px(DOWN)).min(lowest).max(screen.top());
    point(x, y)
}

impl Render for Tile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let (bundle, shown, name) = (self.bundle.clone(), self.icon.clone(), self.name.clone());
        let picture = match &self.icon {
            Some(icon) => img(icon.clone()).size(px(ICON)).into_any_element(),
            // (A bundle with no icon to read: a plain tile, dragged all the same.)
            None => div().size(px(ICON)).flex().items_center().justify_center().rounded(radius::CARD).bg(c.sunken).border_1().border_color(c.border).child(Icon::new(IconName::Shield).color(c.icon)).into_any_element(),
        };
        div()
            .id("permission-tile")
            .debug_selector(|| "permission-tile".into())
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .gap(space::S2)
            .px(space::S4)
            .pt(space::S5)
            .pb(space::S3)
            .rounded(px(18.))
            .bg(c.surface)
            .border_1()
            .border_color(c.border_strong)
            .text_color(c.text)
            // The window moves by its background, as any small panel does.
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 1 {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .id("permission-tile-icon")
                    .debug_selector(|| "permission-tile-icon".into())
                    .cursor(CursorStyle::OpenHand)
                    // (Pressed on the icon, it's the icon that's dragged, not the window moved.)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_drag(Dragged(bundle.clone()), move |_, _, _, cx| cx.new(|_| Preview(shown.clone())))
                    .external_drag_payload(|dragged: &Dragged, _, _| Some(ExternalDragPayload::Files(FileDragPaths::new([(dragged.0.clone(), false)]))))
                    .child(picture),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .child(div().debug_selector(|| "permission-tile-title".into()).text_ui(TypeSize::Small).medium().text_center().child(format!("Drag {name} into the list")))
                    .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).text_center().child("Then switch it on. This window closes by itself.")),
            )
            // Without a pointer to drag with (VoiceOver, a keyboard): the list's + takes it from Finder.
            .child(Button::new("permission-tile-finder", "Show in Finder").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| cx.reveal_path(&this.bundle))))
            .child(
                div()
                    .absolute()
                    .top(space::S1_5)
                    .right(space::S1_5)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Button::icon_only("permission-tile-close", IconName::Close, "Close").ghost().size(ButtonSize::Small).tooltip("Close").on_click(cx.listener(|_, _, _, cx| cx.emit(Closed)))),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn the_tile_sits_beside_system_settings_on_the_side_with_room() {
        let screen = rect(0., 0., 1512., 982.);
        // Room at its right: there, a little below its top.
        assert_eq!(beside(rect(300., 100., 715., 600.), screen), point(px(1029.), px(240.)));
        // None at its right: at its left.
        assert_eq!(beside(rect(700., 100., 715., 600.), screen), point(px(700. - 14. - 248.), px(240.)));
        // None on either side (it fills the display): over its own right edge.
        assert_eq!(beside(rect(100., 40., 1400., 900.), screen), point(px(1500. - 248. - 20.), px(180.)));
        // Low on the display: kept on screen.
        assert_eq!(beside(rect(300., 800., 715., 600.), screen).y, px(982. - 214.));
        // On a second display, to the right of the main one.
        assert_eq!(beside(rect(1600., 50., 715., 600.), rect(1512., 0., 1920., 1080.)), point(px(2329.), px(190.)));
    }
}
