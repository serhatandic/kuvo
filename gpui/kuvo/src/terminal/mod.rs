//! A terminal in the side panel: a shell on the box, in its engine's PTY, drawn here as a real VT
//! (alacritty_terminal's model of the screen, its scrollback and selection), in the app's mono font
//! and its theme's colours, on the panel's own surface.
//!
//! The box runs the shell and keeps what it printed ([`kuvo_box::pty`]); this draws it, sends your
//! keys, pastes and the size it's drawn at, and says plainly when it ended or when this computer
//! can't be reached for one yet. It keeps its shell while it's hidden (only painting stops), and
//! ends it on the box when it's dropped, as closing a terminal's tab does.
//!
//! What the shell prints is read as it comes and drawn once a frame, however much came between.

mod element;
pub(crate) mod keys;
mod palette;

use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Config, Term, TermMode, viewport_to_point};
use alacritty_terminal::vte::ansi::Processor;
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollEvent, SharedString, Styled, Task, Window,
    div, prelude::FluentBuilder,
};
use kuvo_box::BoxClient;
use kuvo_box::pty::{PtyConnection, PtyError, PtyInfo, PtyOutput};
use kuvo_ui::text_field::{Copy, Paste};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, StyledType, TypeSize, theme::space};

use crate::panel::run;
use element::TerminalElement;
use palette::Palette;

/// ⌘C and ⌘V in a terminal: its selection, and pasting into its shell, as Edit's Copy and Paste
/// (the same actions, so the menu's items work in it too). Every other key is the shell's
/// ([`keys`]), but ⌘'s, which stay the app's.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-c", Copy, Some("Terminal")), KeyBinding::new("cmd-v", Paste, Some("Terminal"))]);
}

/// Lines kept above the screen, for a long build's or test run's first lines (as 0.7's had).
const SCROLLBACK: usize = 10_000;

/// Where a terminal stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalState {
    /// Its shell is being made, and its socket opened.
    Starting,
    Running,
    /// Its shell exited, its connection went, or it couldn't start: why, in words.
    Ended(String),
    /// This computer can't be reached for a terminal yet, the way the app reaches it: why.
    Unavailable(String),
}

/// What a terminal tells the panel it's in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// New Terminal, from one that ended (its button, or Return in it): the panel opens another.
    Restart,
    /// Its title or state changed: its tab draws again.
    Changed,
}

/// What the VT asks of the view as it reads the shell's output (answers it writes back, the title,
/// a copy), kept until the view acts on them.
#[derive(Clone, Default)]
struct Asked(Rc<RefCell<Vec<Event>>>);

impl EventListener for Asked {
    fn send_event(&self, event: Event) {
        self.0.borrow_mut().push(event);
    }
}

/// A grid's size, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cells {
    rows: usize,
    cols: usize,
}

impl Dimensions for Cells {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// Where the grid was laid out last: its first cell's corner, a cell's size, and how many fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    pub origin: Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub rows: usize,
    pub cols: usize,
}

pub struct TerminalView {
    client: BoxClient,
    live: bool,
    directory: Option<String>,
    state: TerminalState,
    /// It ran: an end is "This terminal ended", not "couldn't start".
    started: bool,
    title: SharedString,
    focus: FocusHandle,
    shown: bool,
    term: Term<Asked>,
    parser: Processor,
    asked: Asked,
    /// Its PTY on the box, and its socket.
    pty: Option<String>,
    connection: Option<PtyConnection>,
    layout: Option<Layout>,
    /// The size the box was last told, and whether a telling is on its way.
    told: Option<(u16, u16)>,
    telling: bool,
    /// A drag selecting, and the part of a line the wheel has moved so far.
    selecting: bool,
    scrolled: f32,
    _reader: Option<Task<()>>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl TerminalView {
    /// A new terminal on `client`'s box, in `directory` (the engine's own folder when none), its
    /// shell made at once. `live`: off the window's thread, as the app runs; otherwise each request
    /// is answered at once (tests and shots, against the stand-in), and nothing waits on a clock.
    pub fn new(client: BoxClient, live: bool, directory: Option<String>, cx: &mut Context<Self>) -> TerminalView {
        let asked = Asked::default();
        let config = Config { scrolling_history: SCROLLBACK, ..Config::default() };
        let term = Term::new(config, &Cells { rows: 24, cols: 80 }, asked.clone());
        let mut view = TerminalView {
            client,
            live,
            directory,
            state: TerminalState::Starting,
            started: false,
            title: "Terminal".into(),
            focus: cx.focus_handle(),
            shown: true,
            term,
            parser: Processor::new(),
            asked,
            pty: None,
            connection: None,
            layout: None,
            told: None,
            telling: false,
            selecting: false,
            scrolled: 0.,
            _reader: None,
        };
        view.start(cx);
        view
    }

    /// Whether it's on screen: hidden, it keeps its shell and reads what it prints, only not drawn.
    pub fn set_shown(&mut self, on_screen: bool, cx: &mut Context<Self>) {
        if self.shown != on_screen {
            self.shown = on_screen;
            cx.notify();
        }
    }

    /// Its tab's name: the shell's, as the engine says what runs in it ("zsh"); "Terminal" until
    /// it's known.
    pub fn title(&self) -> SharedString {
        self.title.clone()
    }

    pub fn state(&self) -> TerminalState {
        self.state.clone()
    }

    /// Its screen as text, a line a row, its trailing blanks left out: what's drawn on it.
    pub fn screen_text(&self) -> Vec<String> {
        element::screen_text(&self.term)
    }

    /// The colour the character at `row` and `column` of its screen is drawn in.
    pub fn color_at(&self, row: usize, column: usize, cx: &App) -> Option<Hsla> {
        element::color_at(&self.term, &Palette::of(cx.theme()), row, column)
    }

    // MARK: Its shell

    fn start(&mut self, cx: &mut Context<Self>) {
        self.state = TerminalState::Starting;
        let client = self.client.clone();
        let directory = self.directory.clone();
        run(self, self.live, cx, move || open(&client, directory.as_deref()), |this, opened, cx| this.opened(opened, cx));
    }

    fn opened(&mut self, opened: Result<(PtyInfo, PtyConnection), PtyError>, cx: &mut Context<Self>) {
        match opened {
            Ok((info, connection)) => {
                self.title = info.shell().map(SharedString::from).unwrap_or_else(|| "Terminal".into());
                self.pty = Some(info.id);
                self.connection = Some(connection.clone());
                self.state = TerminalState::Running;
                self.started = true;
                self.told = None;
                self.tell_size(cx);
                self.read(connection, cx);
            }
            Err(PtyError::NotHereYet(why)) => self.state = TerminalState::Unavailable(why),
            Err(PtyError::Failed(why)) => self.state = TerminalState::Ended(why),
        }
        cx.emit(TerminalEvent::Changed);
        cx.notify();
    }

    /// Reads what the shell prints as it comes: all that's come by then, a batch at a time.
    fn read(&mut self, connection: PtyConnection, cx: &mut Context<Self>) {
        self._reader = Some(cx.spawn(async move |this, cx| {
            while let Some(first) = connection.next().await {
                let mut batch = vec![first];
                while let Some(more) = connection.try_next() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| this.take(batch, cx)).is_err() {
                    return;
                }
            }
        }));
    }

    fn take(&mut self, batch: Vec<PtyOutput>, cx: &mut Context<Self>) {
        let mut ended = None;
        for output in batch {
            match output {
                PtyOutput::Bytes(bytes) => self.parser.advance(&mut self.term, &bytes),
                PtyOutput::Ended(why) => ended = Some(why),
            }
        }
        self.answer_asked(cx);
        if let Some(why) = ended {
            self.ended(why, cx);
        }
        // Drawn on the next frame, however many batches came before it; hidden, not at all.
        if self.shown {
            cx.notify();
        }
    }

    /// Its socket ended: how its shell ended, as the box says. One whose shell runs on with no way
    /// to it is ended on the box, so it doesn't run on there unseen.
    fn ended(&mut self, why: Option<String>, cx: &mut Context<Self>) {
        self.connection = None;
        let Some(id) = self.pty.clone() else { return };
        let client = self.client.clone();
        let directory = self.directory.clone();
        let place = self.client.describe();
        run(self, self.live, cx, move || {
            let info = client.pty(&id, directory.as_deref());
            match info {
                Ok(info) if !info.running() => (info.ended(), false),
                _ => {
                    let _ = client.remove_pty(&id, directory.as_deref());
                    (why.unwrap_or_else(|| format!("The connection to {place} closed.")), true)
                }
            }
        }, |this, (why, removed), cx| {
            if removed {
                this.pty = None;
            }
            this.state = TerminalState::Ended(why);
            this.selecting = false;
            cx.emit(TerminalEvent::Changed);
            cx.notify();
        });
    }

    /// What the VT asked as it read: answers to the shell's questions (where the cursor is, a
    /// colour, the size), a copy to the clipboard.
    fn answer_asked(&mut self, cx: &mut Context<Self>) {
        let asked = std::mem::take(&mut *self.asked.0.borrow_mut());
        for event in asked {
            match event {
                Event::PtyWrite(text) => self.send(&text),
                Event::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Event::ColorRequest(index, format) => {
                    let rgb = Palette::of(cx.theme()).rgb_of(index, self.term.colors());
                    self.send(&format(rgb));
                }
                Event::TextAreaSizeRequest(format) => {
                    if let Some(layout) = self.layout {
                        let size = WindowSize { num_lines: layout.rows as u16, num_cols: layout.cols as u16, cell_width: f32::from(layout.cell_width) as u16, cell_height: f32::from(layout.line_height) as u16 };
                        self.send(&format(size));
                    }
                }
                _ => {}
            }
        }
    }

    fn send(&self, text: &str) {
        if let Some(connection) = &self.connection {
            // (A socket that's ended says so on its own, and the end is drawn then.)
            let _ = connection.write(text);
        }
    }

    // MARK: Its size

    /// Laid out at `layout` (from its element, as it's drawn): the grid takes the size, and the box
    /// is told, once each time it changes.
    pub(crate) fn laid_out(&mut self, layout: Layout, cx: &mut Context<Self>) {
        let resized = self.layout.is_none_or(|before| (before.rows, before.cols) != (layout.rows, layout.cols));
        self.layout = Some(layout);
        if resized {
            self.term.resize(Cells { rows: layout.rows, cols: layout.cols });
            self.tell_size(cx);
        }
    }

    /// Tells the box the size it's drawn at: one telling at a time, then the newest size if it
    /// changed meanwhile.
    fn tell_size(&mut self, cx: &mut Context<Self>) {
        let (Some(id), Some(layout)) = (self.pty.clone(), self.layout) else { return };
        let size = (layout.rows.min(u16::MAX as usize) as u16, layout.cols.min(u16::MAX as usize) as u16);
        if self.state != TerminalState::Running || self.telling || self.told == Some(size) {
            return;
        }
        self.telling = true;
        let client = self.client.clone();
        let directory = self.directory.clone();
        run(self, self.live, cx, move || client.resize_pty(&id, directory.as_deref(), size.0, size.1), move |this, _told, cx| {
            this.telling = false;
            this.told = Some(size);
            this.tell_size(cx);
        });
    }

    // MARK: What's typed

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        // Return in one that ended starts another, as 0.7's terminal did.
        if matches!(self.state, TerminalState::Ended(_)) && self.started && event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() {
            cx.emit(TerminalEvent::Restart);
            cx.stop_propagation();
            return;
        }
        if self.state != TerminalState::Running {
            return;
        }
        let Some(text) = keys::sent(&event.keystroke, *self.term.mode()) else { return };
        cx.stop_propagation();
        self.typed(&text, cx);
    }

    /// What's typed goes to the shell, and the screen comes back to where it's typed.
    fn typed(&mut self, text: &str, cx: &mut Context<Self>) {
        self.send(text);
        self.term.selection = None;
        self.term.scroll_display(Scroll::Bottom);
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        match self.term.selection_to_string().filter(|text| !text.is_empty()) {
            Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            None => cx.propagate(),
        }
    }

    /// Pastes the clipboard's text into the shell: marked as a paste where the shell asked for that
    /// (bracketed paste, so a pasted line doesn't run as it lands), with its line ends as Return.
    fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        if self.state != TerminalState::Running {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let pasted = pasted(&text, self.term.mode().contains(TermMode::BRACKETED_PASTE));
        self.typed(&pasted, cx);
    }

    // MARK: The mouse

    /// The cell under `position`, and which half of it.
    fn cell_at(&self, position: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let layout = self.layout?;
        let x = f32::from(position.x - layout.origin.x).max(0.) / f32::from(layout.cell_width);
        let y = f32::from(position.y - layout.origin.y).max(0.) / f32::from(layout.line_height);
        let column = (x as usize).min(layout.cols.saturating_sub(1));
        let row = (y as usize).min(layout.rows.saturating_sub(1));
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        let offset = self.term.grid().display_offset();
        Some((viewport_to_point(offset, GridPoint::new(row, Column(column))), side))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((point, side)) = self.cell_at(event.position) else { return };
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.term.selection = Some(Selection::new(kind, point, side));
        self.selecting = true;
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some((point, side)) = self.cell_at(event.position) else { return };
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        self.selecting = false;
        // A click that didn't drag selects nothing.
        if self.term.selection.as_ref().is_some_and(|selection| selection.is_empty()) {
            self.term.selection = None;
        }
        cx.notify();
    }

    /// The wheel scrolls back through what it printed; in a full-screen program (less, vim) that
    /// takes its own scrolling, it's arrows to that program, as Terminal does.
    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let Some(layout) = self.layout else { return };
        self.scrolled += f32::from(event.delta.pixel_delta(layout.line_height).y) / f32::from(layout.line_height);
        let lines = self.scrolled.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scrolled -= lines as f32;
        let mode = *self.term.mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !mode.intersects(TermMode::MOUSE_MODE) {
            if self.state == TerminalState::Running {
                let arrow = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                    (true, true) => "\x1bOA",
                    (true, false) => "\x1b[A",
                    (false, true) => "\x1bOB",
                    (false, false) => "\x1b[B",
                };
                self.send(&arrow.repeat(lines.unsigned_abs() as usize));
            }
            return;
        }
        self.term.scroll_display(Scroll::Delta(lines));
        cx.notify();
    }

    // MARK: Its states, drawn

    fn restart(&mut self, cx: &mut Context<Self>) {
        cx.emit(TerminalEvent::Restart);
    }

    /// Asks for a shell again in place (one that couldn't start, or a computer that can't be
    /// reached for one yet, which may have been updated since).
    fn try_again(&mut self, cx: &mut Context<Self>) {
        self.start(cx);
        cx.notify();
    }

    fn ended_bar(&self, why: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        div()
            .debug_selector(|| "terminal-ended".into())
            .flex()
            .items_center()
            .gap(space::S2)
            .px(space::S3)
            .py(space::S2)
            .border_t_1()
            .border_color(c.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_ui(TypeSize::Small).medium().text_color(c.text_muted).child("This terminal ended."))
                    .child(div().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(why.to_owned())),
            )
            .child(Button::new("terminal-new", "New Terminal").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.restart(cx))))
    }
}

/// Makes a PTY and opens its socket; one whose socket won't open is removed again, so nothing runs
/// on the box that can't be reached.
fn open(client: &BoxClient, directory: Option<&str>) -> Result<(PtyInfo, PtyConnection), PtyError> {
    let info = client.create_pty(directory)?;
    match client.connect_pty(&info.id, directory) {
        Ok(connection) => Ok((info, connection)),
        Err(error) => {
            let _ = client.remove_pty(&info.id, directory);
            Err(error)
        }
    }
}

/// Text pasted into a shell: its line ends as Return; marked as a paste where the shell asked
/// (without anything in it that could end the mark early).
fn pasted(text: &str, bracketed: bool) -> String {
    let returns = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed { format!("\x1b[200~{}\x1b[201~", returns.replace('\x1b', "")) } else { returns }
}

impl Drop for TerminalView {
    /// Gone from the panel: its shell is ended on the box, as closing a terminal's tab does.
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
        let Some(id) = self.pty.take() else { return };
        let client = self.client.clone();
        let directory = self.directory.clone();
        let remove = move || {
            let _ = client.remove_pty(&id, directory.as_deref());
        };
        if self.live {
            let _ = std::thread::Builder::new().name("kuvo-terminal-end".into()).spawn(remove);
        } else {
            remove();
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div()
            .key_context("Terminal")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key_down(event, cx)))
            .size_full()
            .flex()
            .flex_col();
        if !self.shown {
            return root;
        }
        let try_again = |id: &'static str, cx: &mut Context<Self>| Button::new(id, "Try Again").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.try_again(cx))).into_any_element();
        match self.state.clone() {
            TerminalState::Starting => root.child(kuvo_ui::small::LoadingLabel::new("terminal-starting", "Starting…").fill()),
            TerminalState::Unavailable(why) => {
                let action = try_again("terminal-try-again", cx);
                root.child(crate::panel::changes::empty("No Terminal Here Yet", why, Some(action), cx))
            }
            TerminalState::Ended(why) if !self.started => {
                let action = try_again("terminal-start-again", cx);
                root.child(crate::panel::changes::empty("The Terminal Couldn’t Start", why, Some(action), cx))
            }
            state => {
                let ended = match &state {
                    TerminalState::Ended(why) => Some(why.clone()),
                    _ => None,
                };
                root.child(
                    div()
                        .id("terminal-grid")
                        .debug_selector(|| "terminal-grid".into())
                        .flex_1()
                        .min_h_0()
                        .pl(space::S3)
                        .pr(space::S2)
                        .pt(space::S1)
                        .pb(space::S2)
                        .cursor_text()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| this.mouse_down(event, window, cx)))
                        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.mouse_move(event, cx)))
                        .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.mouse_up(cx)))
                        .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| this.scroll(event, cx)))
                        .child(TerminalElement::new(cx.entity(), self.state == TerminalState::Running)),
                )
                .when_some(ended, |root, why| root.child(self.ended_bar(&why, cx)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paste_is_marked_where_the_shell_asked_and_its_line_ends_are_returns() {
        assert_eq!(pasted("echo one\necho two\r\n", false), "echo one\recho two\r");
        assert_eq!(pasted("rm -rf build\n", true), "\x1b[200~rm -rf build\r\x1b[201~");
        // Nothing pasted can end the mark early and run the rest.
        assert_eq!(pasted("a\x1b[201~b", true), "\x1b[200~a[201~b\x1b[201~");
    }
}
