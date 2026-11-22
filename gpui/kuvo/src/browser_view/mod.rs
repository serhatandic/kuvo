//! The browser in the side panel: a computer's browser, live, through its box's socket
//! ([`kuvo_box::browser_view`], PROTOCOL.md "The browser's live view"). A bar with Back, Forward,
//! Reload and the page's address, then the page's picture, as big as fits and keeping its shape on
//! the panel's surface. Clicks, the wheel and keys go to the page, in its own CSS pixels.
//!
//! The box's keeper owns the browser, its pages, which one's in use and their sizes; this draws what
//! it says and sends what's done. On screen it asks for pictures sized to where it's drawn; hidden it
//! asks for none, and keeps its socket. A picture is drawn as it comes, and only once it's drawn is
//! the box told, so it sends the next (an older one never drawn is said dropped).
//!
//! It says plainly when its computer's browser isn't running, can't be shown here yet (a Mac's box,
//! or a computer reached over SSH), or its connection ended.

mod element;
mod keys;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton as GpuiButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, RenderImage, ScrollEvent,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, point, prelude::FluentBuilder as _, px, size,
};
use kuvo_box::BoxClient;
use kuvo_box::browser_view::{
    BrowserViewConnection, BrowserViewError, Command, Dropped, Frame, Go, Input, Mouse, MouseAction, MouseButton, Page, Pages, ViewMessage,
    Viewport, Wheel,
};
use kuvo_ui::menu::{Menu, MenuEntry, PopoverMenu, Tooltip};
use kuvo_ui::text_field::Paste;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TextField, TextFieldEvent, TypeSize, theme::{radius, space}};

use crate::panel::changes::empty;
use crate::panel::run;
use element::PageElement;

/// The bar's height, as the panel's tab strip.
pub const BAR_HEIGHT: Pixels = px(40.);

/// Where a computer's browser stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserState {
    /// Its socket is being opened, or its pages are on their way.
    Connecting,
    /// Its pages are known, and a page is drawn as its pictures come.
    Open,
    /// Its browser isn't running.
    NotRunning,
    /// It can't be shown here yet: a box with no browser of its own (a Mac's), or a computer reached
    /// a way that can't carry its pictures (over SSH).
    Unavailable,
    /// It couldn't be reached: why.
    Failed(String),
    /// Its connection ended: why, where that was said.
    Disconnected(Option<String>),
}

/// What the browser tells the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserEvent {
    /// Its page's name, whether it's loading, or its state changed: its tab draws again.
    Changed,
}

/// A page's picture, ready to draw.
pub(crate) struct Picture {
    pub target_id: String,
    pub frame_id: u64,
    /// Its own pixels, as its header says them.
    pub width: u32,
    pub height: u32,
    pub image: Arc<RenderImage>,
}

/// A picture as it came, decoded off the window's thread: none where its JPEG couldn't be read.
struct Decoded {
    frame: Frame,
    image: Option<Arc<RenderImage>>,
}

/// What the box was last told to show.
#[derive(Clone, Debug, PartialEq)]
enum Told {
    Nothing,
    Viewing { target_id: String, width: u32, height: u32, scale: f32 },
    Stopped,
}

pub struct BrowserView {
    client: BoxClient,
    live: bool,
    state: BrowserState,
    focus: FocusHandle,
    shown: bool,
    /// Each opening's own number: what an older one opened is closed again.
    opening: u64,
    connection: Option<BrowserViewConnection>,
    pages: Pages,
    viewports: HashMap<String, Viewport>,
    /// The page picked from the pages' menu, until the box says it's the one in use.
    picked: Option<String>,
    picture: Option<Picture>,
    /// Pictures replaced since the last frame, for it to take out of the window's atlas.
    pub(crate) retired: Vec<Arc<RenderImage>>,
    /// The picture drawn last, which the box was told of.
    drawn: Option<u64>,
    /// Where the page is drawn, and the display's scale.
    area: Option<Bounds<Pixels>>,
    scale: f32,
    told: Told,
    /// The telling on its way (its id): the newest goes once it's answered.
    telling: Option<u64>,
    /// The mouse button held down in the page.
    pressed: Option<MouseButton>,
    /// A move on its way, and where the mouse went since; the same for the wheel.
    moving: Option<u64>,
    next_move: Option<(f64, f64, MouseButton)>,
    wheeling: Option<u64>,
    next_wheel: Option<Wheel>,
    /// Where you last sent the page (its id), and its refusal, said under the bar.
    going: Option<u64>,
    refused: Option<String>,
    address: Entity<TextField>,
    /// What the address field was last set to: other text in it is yours, kept as pages change.
    address_set: String,
    address_disabled: bool,
    editing: bool,
    /// The field wasn't where you typed when it was clicked: the click selects its address whole.
    selects_address: bool,
    /// Where you type goes back to the page on the next frame (after Return or Escape in the field).
    refocus: bool,
    _reader: Option<Task<()>>,
    _address: Subscription,
}

impl EventEmitter<BrowserEvent> for BrowserView {}

impl BrowserView {
    /// The browser of `client`'s computer, its socket opened at once. `live`: off the window's thread,
    /// as the app runs; otherwise each request is answered at once (tests and shots, against the
    /// stand-in), and nothing waits on a clock.
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> BrowserView {
        let address = cx.new(|cx| TextField::new(cx).bare().placeholder("Search or enter address"));
        let watch = cx.subscribe(&address, |this: &mut BrowserView, _, event: &TextFieldEvent, cx| this.address_said(event, cx));
        cx.on_release(|this: &mut BrowserView, cx| {
            if let Some(connection) = this.connection.take() {
                connection.close();
            }
            for image in this.retired.drain(..).chain(this.picture.take().map(|picture| picture.image)) {
                cx.drop_image(image, None);
            }
        })
        .detach();
        let mut view = BrowserView {
            client,
            live,
            state: BrowserState::Connecting,
            focus: cx.focus_handle(),
            shown: true,
            opening: 0,
            connection: None,
            pages: Pages::default(),
            viewports: HashMap::new(),
            picked: None,
            picture: None,
            retired: Vec::new(),
            drawn: None,
            area: None,
            scale: 1.,
            told: Told::Nothing,
            telling: None,
            pressed: None,
            moving: None,
            next_move: None,
            wheeling: None,
            next_wheel: None,
            going: None,
            refused: None,
            address,
            address_set: String::new(),
            address_disabled: false,
            editing: false,
            selects_address: false,
            refocus: false,
            _reader: None,
            _address: watch,
        };
        view.connect(cx);
        view
    }

    /// Whether it's on screen: on screen it asks for pictures sized to where it's drawn; hidden, for
    /// none (its socket stays open).
    pub fn set_shown(&mut self, on_screen: bool, cx: &mut Context<Self>) {
        if self.shown != on_screen {
            self.shown = on_screen;
            self.tell_view();
            cx.notify();
        }
    }

    /// Its tab's name: the page's, as the protocol names a tab ("Browser" until a page is known).
    pub fn title(&self) -> SharedString {
        self.page().map_or_else(|| "Browser".into(), |page| page.name().into())
    }

    /// Whether the page shown is loading.
    pub fn loading(&self) -> bool {
        self.page().is_some_and(|page| page.loading)
    }

    pub fn state(&self) -> BrowserState {
        self.state.clone()
    }

    /// The browser's pages, as its box last said.
    pub fn pages(&self) -> &Pages {
        &self.pages
    }

    /// The page shown: the one picked here, else the one the box says is in use, else the first.
    pub fn page(&self) -> Option<&Page> {
        let pages = &self.pages;
        self.picked.as_deref().and_then(|id| pages.page(id)).or_else(|| pages.active.as_deref().and_then(|id| pages.page(id))).or_else(|| pages.pages.first())
    }

    /// The picture drawn last (its frame's id).
    pub fn drawn_frame(&self) -> Option<u64> {
        self.drawn
    }

    /// The address field.
    pub fn address(&self) -> &Entity<TextField> {
        &self.address
    }

    // MARK: Its socket

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.close();
        self.state = BrowserState::Connecting;
        self.opening += 1;
        let opening = self.opening;
        let client = self.client.clone();
        run(self, self.live, cx, move || client.open_browser_view(), move |this, opened, cx| this.opened(opening, opened, cx));
    }

    fn opened(&mut self, opening: u64, opened: Result<BrowserViewConnection, BrowserViewError>, cx: &mut Context<Self>) {
        if opening != self.opening {
            if let Ok(connection) = opened {
                connection.close();
            }
            return;
        }
        match opened {
            Ok(connection) => {
                self.connection = Some(connection.clone());
                self.read(connection, cx);
            }
            Err(BrowserViewError::NoBrowser(_) | BrowserViewError::NotThisWay(_)) => self.state = BrowserState::Unavailable,
            Err(BrowserViewError::NotRunning(_)) => self.state = BrowserState::NotRunning,
            Err(BrowserViewError::Failed(why)) => self.state = BrowserState::Failed(why),
        }
        cx.emit(BrowserEvent::Changed);
        cx.notify();
    }

    /// Reads what the box says as it comes, all that's come by then at a time: of its pictures only
    /// the newest is decoded (off the window's thread) and drawn; those it replaced are said dropped.
    fn read(&mut self, connection: BrowserViewConnection, cx: &mut Context<Self>) {
        self._reader = Some(cx.spawn(async move |this, cx| {
            while let Some(mut batch) = connection.next().await {
                while let Some(more) = connection.try_next() {
                    batch.extend(more);
                }
                let mut frames = Vec::new();
                let mut said = Vec::new();
                for message in batch {
                    match message {
                        ViewMessage::Frame(frame) => frames.push(frame),
                        other => said.push(other),
                    }
                }
                let newest = frames.pop();
                for frame in frames {
                    let _ = connection.send(&Command::FrameDropped { frame_id: frame.frame_id, reason: Dropped::Backpressure });
                }
                let decoded = match newest {
                    Some(frame) => Some(cx.background_executor().spawn(async move { decode(frame) }).await),
                    None => None,
                };
                if this.update(cx, |this, cx| this.take(said, decoded, cx)).is_err() {
                    return;
                }
            }
        }));
    }

    fn take(&mut self, said: Vec<ViewMessage>, decoded: Option<Decoded>, cx: &mut Context<Self>) {
        let before = (self.title(), self.loading(), self.state.clone());
        for message in said {
            match message {
                ViewMessage::Pages(pages) => self.took_pages(pages),
                ViewMessage::Viewport(viewport) => {
                    self.viewports.insert(viewport.target_id.clone(), viewport);
                }
                ViewMessage::Answer { id, refused } => self.answered(id, refused),
                ViewMessage::Ended(why) => self.ended(why),
                ViewMessage::Frame(_) | ViewMessage::External { .. } | ViewMessage::Other { .. } => {}
            }
        }
        if let Some(decoded) = decoded {
            self.took_picture(decoded);
        }
        self.tell_view();
        self.send_move();
        self.send_wheel();
        self.show_address(cx);
        if (self.title(), self.loading(), self.state.clone()) != before {
            cx.emit(BrowserEvent::Changed);
        }
        cx.notify();
    }

    fn took_pages(&mut self, pages: Pages) {
        self.pages = pages;
        if self.picked.as_deref().is_some_and(|picked| self.pages.active.as_deref() == Some(picked) || self.pages.page(picked).is_none()) {
            self.picked = None;
        }
        if self.state == BrowserState::Connecting && self.connection.is_some() {
            self.state = BrowserState::Open;
        }
        self.viewports.retain(|target, _| self.pages.page(target).is_some());
        self.forget_other_pages_picture();
    }

    fn took_picture(&mut self, decoded: Decoded) {
        let Decoded { frame, image } = decoded;
        let showing = self.page().map(|page| page.target_id.as_str()) == Some(frame.target_id.as_str());
        let (true, true, Some(image)) = (self.shown, showing, image) else {
            self.dropped(frame.frame_id, Dropped::Unsent);
            return;
        };
        let picture = Picture { target_id: frame.target_id, frame_id: frame.frame_id, width: frame.width, height: frame.height, image };
        if let Some(older) = self.picture.replace(picture) {
            self.retire(older, Dropped::Backpressure);
        }
    }

    /// A picture no longer drawn: out of the atlas, and said dropped where it never was drawn.
    fn retire(&mut self, picture: Picture, reason: Dropped) {
        if self.drawn != Some(picture.frame_id) {
            self.dropped(picture.frame_id, reason);
        }
        self.retired.push(picture.image);
    }

    fn forget_other_pages_picture(&mut self) {
        let showing = self.page().map(|page| page.target_id.clone());
        if self.picture.as_ref().is_some_and(|picture| Some(&picture.target_id) != showing.as_ref())
            && let Some(picture) = self.picture.take()
        {
            self.retire(picture, Dropped::Unsent);
        }
    }

    fn dropped(&self, frame_id: u64, reason: Dropped) {
        if let Some(connection) = &self.connection {
            let _ = connection.send(&Command::FrameDropped { frame_id, reason });
        }
    }

    fn answered(&mut self, id: u64, refused: Option<String>) {
        if self.telling == Some(id) {
            self.telling = None;
        }
        if self.moving == Some(id) {
            self.moving = None;
        }
        if self.wheeling == Some(id) {
            self.wheeling = None;
        }
        if self.going == Some(id) {
            self.going = None;
            self.refused = refused;
        }
    }

    /// Its socket ended: the browser stopped, or the connection went.
    fn ended(&mut self, why: Option<String>) {
        self.connection = None;
        self.reset();
        self.state = BrowserState::Disconnected(why.filter(|why| !why.trim().is_empty()));
    }

    /// Closes its socket, and forgets what was on its way.
    fn close(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
        self._reader = None;
        self.reset();
    }

    fn reset(&mut self) {
        self.told = Told::Nothing;
        self.telling = None;
        self.moving = None;
        self.next_move = None;
        self.wheeling = None;
        self.next_wheel = None;
        self.going = None;
        self.pressed = None;
        if let Some(picture) = self.picture.take() {
            self.retired.push(picture.image);
        }
    }

    // MARK: What the box is asked to show

    /// Laid out at `bounds` (from its element, as it's drawn), on a display of `scale`: the box is
    /// asked for pictures that size.
    pub(crate) fn laid_out(&mut self, bounds: Bounds<Pixels>, scale: f32, _cx: &mut Context<Self>) {
        if self.area != Some(bounds) || self.scale != scale {
            self.area = Some(bounds);
            self.scale = scale;
            self.tell_view();
        }
    }

    /// Tells the box what to show: the page shown, sized to where it's drawn, while it's on screen;
    /// nothing while it's hidden. One telling at a time, then the newest if it changed meanwhile.
    fn tell_view(&mut self) {
        let Some(connection) = self.connection.clone() else { return };
        if self.telling.is_some() {
            return;
        }
        let wanted = match (self.shown && self.state == BrowserState::Open, self.page(), self.area) {
            (true, Some(page), Some(area)) => Told::Viewing {
                target_id: page.target_id.clone(),
                width: f32::from(area.size.width).round().max(1.) as u32,
                height: f32::from(area.size.height).round().max(1.) as u32,
                scale: self.scale,
            },
            _ if self.told == Told::Nothing => return,
            _ => Told::Stopped,
        };
        if wanted == self.told {
            return;
        }
        let command = match &wanted {
            Told::Viewing { target_id, width, height, scale } => Command::View { target_id: target_id.clone(), width: *width, height: *height, scale: *scale },
            _ => Command::StopView,
        };
        if let Ok(id) = connection.ask(&command) {
            self.telling = Some(id);
            self.told = wanted;
        }
    }

    /// The picture to draw: the page shown's, while it's open and on screen.
    pub(crate) fn shown_picture(&self) -> Option<&Picture> {
        let picture = self.picture.as_ref()?;
        let showing = self.page()?.target_id == picture.target_id;
        (self.shown && showing && self.state == BrowserState::Open).then_some(picture)
    }

    /// Its picture was drawn: the box is told, once, so it sends the next.
    pub(crate) fn drawn(&mut self, frame_id: u64) {
        if self.drawn == Some(frame_id) {
            return;
        }
        self.drawn = Some(frame_id);
        if let Some(connection) = &self.connection {
            let _ = connection.send(&Command::FrameAck(frame_id));
        }
    }

    // MARK: Where the page goes

    fn go(&mut self, go: Go, cx: &mut Context<Self>) {
        let (Some(connection), Some(target_id)) = (self.connection.clone(), self.page().map(|page| page.target_id.clone())) else { return };
        self.refused = None;
        if let Ok(id) = connection.ask(&Command::Go { target_id, go }) {
            self.going = Some(id);
        }
        cx.notify();
    }

    /// Shows a page picked from the pages' menu: the box makes it the one in use.
    fn pick(&mut self, target_id: String, cx: &mut Context<Self>) {
        if self.page().map(|page| page.target_id.as_str()) == Some(target_id.as_str()) {
            return;
        }
        self.picked = Some(target_id);
        self.forget_other_pages_picture();
        self.editing = false;
        self.tell_view();
        self.show_address(cx);
        cx.emit(BrowserEvent::Changed);
        cx.notify();
    }

    fn address_said(&mut self, event: &TextFieldEvent, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Changed => self.editing = self.address.read(cx).text() != self.address_set,
            TextFieldEvent::Submit(text) => {
                let typed = text.trim();
                let Some(page) = self.page() else { return };
                if typed.is_empty() {
                    return;
                }
                // The address as it's shown goes as the page has it (`http:` and all).
                let input = if typed == shown_address(&page.url) { page.url.clone() } else { typed.to_owned() };
                self.go(Go::Address(input), cx);
                self.editing = false;
                self.refocus = true;
            }
            TextFieldEvent::Cancel => {
                self.editing = false;
                self.show_address(cx);
                self.refocus = true;
                cx.notify();
            }
        }
    }

    /// The page's address in its field, unless you're writing there; the field takes nothing while
    /// there's no page to send it to.
    fn show_address(&mut self, cx: &mut Context<Self>) {
        let page = self.page();
        let disabled = page.is_none();
        let shown = page.map(|page| shown_address(&page.url)).unwrap_or_default();
        if !self.editing && self.address.read(cx).text() != shown {
            self.address_set = shown.clone();
            self.address.update(cx, |field, cx| field.set_text(shown, cx));
        }
        if self.address_disabled != disabled {
            self.address_disabled = disabled;
            self.address.update(cx, |field, cx| field.set_disabled(disabled, cx));
        }
    }

    // MARK: The mouse and the keys

    /// Where `position` is in the page, in its CSS pixels: none outside its picture (unless
    /// `clamped`, to its nearest edge, for a drag that left it).
    fn page_point(&self, position: Point<Pixels>, clamped: bool) -> Option<(f64, f64)> {
        let picture = self.shown_picture()?;
        let rect = fitted(self.area?, picture.width, picture.height);
        let (width, height) = (f32::from(rect.size.width), f32::from(rect.size.height));
        if width <= 0. || height <= 0. {
            return None;
        }
        let mut across = f32::from(position.x - rect.origin.x) / width;
        let mut down = f32::from(position.y - rect.origin.y) / height;
        if clamped {
            across = across.clamp(0., 1.);
            down = down.clamp(0., 1.);
        } else if !(0.0..=1.0).contains(&across) || !(0.0..=1.0).contains(&down) {
            return None;
        }
        let (css_width, css_height) = self.css_size(picture);
        Some((hundredths(across as f64 * css_width), hundredths(down as f64 * css_height)))
    }

    /// The page's CSS viewport, which its pictures show whole: as its box last said, else the size
    /// asked for where the picture has its shape, else the browser's usual width.
    fn css_size(&self, picture: &Picture) -> (f64, f64) {
        if let Some(viewport) = self.viewports.get(&picture.target_id).filter(|viewport| viewport.width > 0. && viewport.height > 0.) {
            return (viewport.width, viewport.height);
        }
        let shape = picture.width.max(1) as f64 / picture.height.max(1) as f64;
        if let Told::Viewing { width, height, .. } = &self.told
            && ((*width as f64 / (*height).max(1) as f64) - shape).abs() < 0.02
        {
            return (*width as f64, *height as f64);
        }
        (1280., (1280. / shape).round())
    }

    fn send_input(&self, input: Input) {
        if let (Some(connection), Some(page)) = (&self.connection, self.page()) {
            let _ = connection.send(&Command::Input { target_id: page.target_id.clone(), input });
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(button) = button(event.button) else { return };
        let Some((x, y)) = self.page_point(event.position, false) else { return };
        self.next_move = None;
        self.pressed = Some(button);
        self.send_input(Input::Mouse(Mouse { action: MouseAction::Pressed, x, y, button, click_count: event.click_count as u32, modifiers: modifiers(event.modifiers) }));
    }

    fn mouse_up(&mut self, event: &MouseUpEvent) {
        if self.pressed != button(event.button) {
            return;
        }
        let Some(button) = self.pressed.take() else { return };
        let Some((x, y)) = self.page_point(event.position, true) else { return };
        self.next_move = None;
        self.send_input(Input::Mouse(Mouse { action: MouseAction::Released, x, y, button, click_count: event.click_count as u32, modifiers: modifiers(event.modifiers) }));
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent) {
        let Some((x, y)) = self.page_point(event.position, self.pressed.is_some()) else { return };
        self.next_move = Some((x, y, self.pressed.unwrap_or(MouseButton::None)));
        self.send_move();
    }

    /// The mouse's newest place, one move on its way at a time.
    fn send_move(&mut self) {
        if self.moving.is_some() {
            return;
        }
        let (Some(connection), Some(page), Some((x, y, button))) = (self.connection.clone(), self.page().map(|page| page.target_id.clone()), self.next_move.take()) else { return };
        let mouse = Mouse { action: MouseAction::Moved, x, y, button, click_count: 0, modifiers: Default::default() };
        self.moving = connection.ask(&Command::Input { target_id: page, input: Input::Mouse(mouse) }).ok();
    }

    fn scroll(&mut self, event: &ScrollEvent) {
        let Some((x, y)) = self.page_point(event.position, false) else { return };
        let (Some(picture), Some(area)) = (self.shown_picture(), self.area) else { return };
        // Moved as far in the page as on screen.
        let rect = fitted(area, picture.width, picture.height);
        let ratio = self.css_size(picture).0 / f32::from(rect.size.width).max(1.) as f64;
        let delta = event.delta.pixel_delta(px(40.));
        let (delta_x, delta_y) = (-f32::from(delta.x) as f64 * ratio, -f32::from(delta.y) as f64 * ratio);
        let wheel = match self.next_wheel.take() {
            Some(before) => Wheel { x, y, delta_x: before.delta_x + delta_x, delta_y: before.delta_y + delta_y, modifiers: modifiers(event.modifiers) },
            None => Wheel { x, y, delta_x, delta_y, modifiers: modifiers(event.modifiers) },
        };
        self.next_wheel = Some(wheel);
        self.send_wheel();
    }

    /// The wheel's turn since, one on its way at a time.
    fn send_wheel(&mut self) {
        if self.wheeling.is_some() {
            return;
        }
        let (Some(connection), Some(page), Some(wheel)) = (self.connection.clone(), self.page().map(|page| page.target_id.clone()), self.next_wheel.take()) else { return };
        let wheel = Wheel { delta_x: hundredths(wheel.delta_x), delta_y: hundredths(wheel.delta_y), ..wheel };
        self.wheeling = connection.ask(&Command::Input { target_id: page, input: Input::Wheel(wheel) }).ok();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.state != BrowserState::Open || self.page().is_none() {
            return;
        }
        let keystroke = &event.keystroke;
        let m = keystroke.modifiers;
        if m.platform && keystroke.key == "v" && !m.shift && !m.alt && !m.control {
            cx.stop_propagation();
            self.paste(cx);
            return;
        }
        let Some(keys) = keys::sent(keystroke) else { return };
        cx.stop_propagation();
        for key in keys {
            self.send_input(Input::Key(key));
        }
    }

    /// Puts this Mac's clipboard's text where the page is typed into (Chrome's own clipboard is on
    /// its computer).
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()).filter(|text| !text.is_empty()) else { return };
        self.send_input(Input::Text(text));
    }

    // MARK: Drawn

    fn try_again(&mut self, cx: &mut Context<Self>) {
        self.connect(cx);
        cx.emit(BrowserEvent::Changed);
        cx.notify();
    }

    fn nav_button(&self, id: &'static str, icon: IconName, tip: &'static str, enabled: bool, go: Go, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(radius::NAV)
            .tooltip(move |_, cx| Tooltip::new(tip).build(cx))
            .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c.hover)).on_click(cx.listener(move |this, _, _, cx| this.go(go.clone(), cx))))
            .child(Icon::new(icon).size(IconSize::XSmall).color(if enabled { c.icon } else { c.text_faint }))
    }

    fn bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let c = theme.colors.clone();
        let field_fill = if theme.appearance.is_dark() { c.sunken } else { c.sidebar };
        let page = self.page();
        let (back, forward, loading) = page.map_or((false, false, false), |page| (page.can_go_back, page.can_go_forward, page.loading));
        let has_page = page.is_some();
        div()
            .debug_selector(|| "browser-bar".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S1)
            .h(BAR_HEIGHT)
            .px(space::S2)
            .border_b_1()
            .border_color(c.border)
            .child(self.nav_button("browser-back", IconName::ArrowLeft, "Back", back, Go::Back, cx))
            .child(self.nav_button("browser-forward", IconName::ArrowRight, "Forward", forward, Go::Forward, cx))
            .child(if loading {
                self.nav_button("browser-stop", IconName::Close, "Stop", true, Go::Stop, cx).into_any_element()
            } else {
                self.nav_button("browser-reload", IconName::Refresh, "Reload", has_page, Go::Reload, cx).into_any_element()
            })
            .child(
                div()
                    .id("browser-address")
                    .debug_selector(|| "browser-address".into())
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(space::S1)
                    .h(px(28.))
                    .pl(space::S2)
                    .pr(space::S1)
                    .rounded(radius::NAV)
                    .border_1()
                    .border_color(c.border)
                    .bg(field_fill)
                    .cursor_text()
                    .capture_any_mouse_down(cx.listener(|this, _, window, cx| this.selects_address = !this.address.focus_handle(cx).is_focused(window)))
                    .on_mouse_down(GpuiButton::Left, cx.listener(|this, _, window, cx| {
                        let field = this.address.focus_handle(cx);
                        if !field.is_focused(window) {
                            window.focus(&field, cx);
                        }
                    }))
                    // A first click takes the whole address, as a browser's does.
                    .on_mouse_up(GpuiButton::Left, cx.listener(|this, _, _, cx| {
                        if std::mem::take(&mut this.selects_address) {
                            this.address.update(cx, |field, cx| {
                                let all = field.text().len();
                                field.select_range(0..all, cx);
                            });
                        }
                    }))
                    .child(div().flex_1().min_w_0().text_ui(TypeSize::Caption).child(self.address.clone()))
                    .children(self.pages_menu(cx)),
            )
    }

    /// The pages' menu, where there's more than one: which one shows.
    fn pages_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let count = self.pages.pages.len();
        if count < 2 {
            return None;
        }
        let c = cx.theme().colors.clone();
        let trigger = div()
            .id("browser-pages")
            .debug_selector(|| "browser-pages".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(20.))
            .px(space::S1)
            .rounded(radius::SMALL)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover))
            .tooltip(|_, cx| Tooltip::new("Pages").build(cx))
            .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(count.to_string()))
            .child(Icon::new(IconName::ChevronDown).size(IconSize::Indicator).color(c.text_faint));
        let pages = self.pages.pages.clone();
        let showing = self.page().map(|page| page.target_id.clone());
        let this = cx.entity().downgrade();
        Some(
            PopoverMenu::new("browser-pages-menu", trigger, move |_, cx| {
                let (pages, showing, this) = (pages.clone(), showing.clone(), this.clone());
                cx.new(|cx| {
                    let mut menu = Menu::new(cx).width(px(300.));
                    for page in pages {
                        let this = this.clone();
                        let target_id = page.target_id.clone();
                        menu = menu.entry(
                            MenuEntry::new(page.name())
                                .detail(shown_address(&page.url))
                                .checked(Some(&page.target_id) == showing.as_ref())
                                .on_select(move |_, cx| {
                                    this.update(cx, |this, cx| this.pick(target_id.clone(), cx)).ok();
                                }),
                        );
                    }
                    menu
                })
            })
            .align_right()
            .centered(),
        )
    }

    fn page_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("browser-page")
            .debug_selector(|| "browser-page".into())
            .flex_1()
            .min_h_0()
            .key_context("BrowserPage")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key_down(event, cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| this.mouse_down(event, window, cx)))
            .on_mouse_up(GpuiButton::Left, cx.listener(|this, event: &MouseUpEvent, _, _| this.mouse_up(event)))
            .on_mouse_up(GpuiButton::Right, cx.listener(|this, event: &MouseUpEvent, _, _| this.mouse_up(event)))
            .on_mouse_up(GpuiButton::Middle, cx.listener(|this, event: &MouseUpEvent, _, _| this.mouse_up(event)))
            .on_mouse_up_out(GpuiButton::Left, cx.listener(|this, event: &MouseUpEvent, _, _| this.mouse_up(event)))
            .on_mouse_up_out(GpuiButton::Right, cx.listener(|this, event: &MouseUpEvent, _, _| this.mouse_up(event)))
            .on_mouse_up_out(GpuiButton::Middle, cx.listener(|this, event: &MouseUpEvent, _, _| this.mouse_up(event)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| this.mouse_move(event)))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, _| this.scroll(event)))
            .child(PageElement::new(cx.entity()))
    }

    fn state_page(&self, selector: &'static str, title: &'static str, detail: String, action: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let again = Button::new("browser-try-again", action).size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.try_again(cx))).into_any_element();
        div().debug_selector(move || selector.into()).flex().flex_col().flex_1().child(empty(title, detail, Some(again), cx))
    }
}

/// `rect`'s part where a picture `width` by `height` is drawn: as big as fits, keeping its shape,
/// centred (the rest is the panel's surface).
pub(crate) fn fitted(rect: Bounds<Pixels>, width: u32, height: u32) -> Bounds<Pixels> {
    let (room_x, room_y) = (f32::from(rect.size.width), f32::from(rect.size.height));
    if width == 0 || height == 0 {
        return Bounds::new(rect.origin, size(px(0.), px(0.)));
    }
    let fit = (room_x / width as f32).min(room_y / height as f32);
    let (drawn_x, drawn_y) = (width as f32 * fit, height as f32 * fit);
    Bounds::new(point(rect.origin.x + px((room_x - drawn_x) / 2.), rect.origin.y + px((room_y - drawn_y) / 2.)), size(px(drawn_x), px(drawn_y)))
}

/// An address as its field shows it: without `https://` or `http://`, nor a lone `/` after its
/// host; nothing for a blank page.
pub fn shown_address(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() || url == "about:blank" {
        return String::new();
    }
    let bare = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    match bare.strip_suffix('/') {
        Some(host) if !host.contains('/') && url != bare => host.to_owned(),
        _ => bare.to_owned(),
    }
}

/// Decodes a picture's JPEG into the pixels a window draws (BGRA).
fn decode(frame: Frame) -> Decoded {
    let image = image::load_from_memory_with_format(&frame.jpeg, image::ImageFormat::Jpeg).ok().map(|decoded| {
        let mut pixels = decoded.into_rgba8();
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
    });
    Decoded { frame, image }
}

fn hundredths(value: f64) -> f64 {
    (value * 100.).round() / 100.
}

fn button(button: GpuiButton) -> Option<MouseButton> {
    match button {
        GpuiButton::Left => Some(MouseButton::Left),
        GpuiButton::Right => Some(MouseButton::Right),
        GpuiButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

/// The modifiers held with the mouse: ⌘ as Control, as the keys send it.
fn modifiers(held: gpui::Modifiers) -> kuvo_box::browser_view::Modifiers {
    kuvo_box::browser_view::Modifiers { alt: held.alt, control: held.platform, meta: false, shift: held.shift }
}

impl Focusable for BrowserView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refocus) {
            window.focus(&self.focus, cx);
        }
        let root = div().id("browser").debug_selector(|| "browser".into()).size_full().flex().flex_col();
        if !self.shown {
            return root.track_focus(&self.focus);
        }
        match self.state.clone() {
            BrowserState::Connecting => root.track_focus(&self.focus).child(kuvo_ui::small::LoadingLabel::new("browser-connecting", "Connecting…").fill()),
            BrowserState::NotRunning => {
                let page = self.state_page("browser-not-running", "Browser Not Running", "This computer’s browser isn’t running. It starts when an agent browses.".into(), "Try Again", cx);
                root.track_focus(&self.focus).child(page)
            }
            BrowserState::Unavailable => {
                let page = self.state_page("browser-unavailable", "No Browser Here Yet", "This computer’s browser can’t be shown here yet.".into(), "Try Again", cx);
                root.track_focus(&self.focus).child(page)
            }
            BrowserState::Failed(why) => {
                let page = self.state_page("browser-failed", "Couldn’t Reach the Browser", why, "Try Again", cx);
                root.track_focus(&self.focus).child(page)
            }
            BrowserState::Disconnected(why) => {
                let page = self.state_page("browser-disconnected", "Disconnected from the Browser", why.unwrap_or_else(|| "Its connection closed.".into()), "Reconnect", cx);
                root.track_focus(&self.focus).child(page)
            }
            BrowserState::Open => {
                let c = cx.theme().colors.clone();
                let refused = self.refused.clone();
                let body = if self.pages.pages.is_empty() {
                    div()
                        .debug_selector(|| "browser-no-pages".into())
                        .flex()
                        .flex_col()
                        .flex_1()
                        .track_focus(&self.focus)
                        .child(empty("No Pages Open", "No page is open in this computer’s browser. Pages open here as agents browse.", None, cx))
                        .into_any_element()
                } else {
                    self.page_area(cx).into_any_element()
                };
                root.child(self.bar(cx))
                    .when_some(refused, |root, why| {
                        root.child(div().debug_selector(|| "browser-refused".into()).flex_none().px(space::S3).py(space::S1).border_b_1().border_color(c.border).text_ui(TypeSize::Caption).text_color(c.danger).child(why))
                    })
                    .child(body)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_drawn_as_big_as_fits_keeping_its_shape_centred() {
        let room = Bounds::new(point(px(10.), px(50.)), size(px(600.), px(400.)));
        // Wider than the room: bands above and below.
        assert_eq!(fitted(room, 1280, 800), Bounds::new(point(px(10.), px(62.5)), size(px(600.), px(375.))));
        // Taller: bands beside it.
        assert_eq!(fitted(room, 400, 800), Bounds::new(point(px(210.), px(50.)), size(px(200.), px(400.))));
    }

    #[test]
    fn an_address_is_shown_as_browsers_show_it() {
        assert_eq!(shown_address("https://example.com/agent"), "example.com/agent");
        assert_eq!(shown_address("https://example.com/"), "example.com");
        assert_eq!(shown_address("http://localhost:3000/signup"), "localhost:3000/signup");
        assert_eq!(shown_address("file:///home/sam/report.html"), "file:///home/sam/report.html");
        assert_eq!(shown_address("about:blank"), "");
    }
}
