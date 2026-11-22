//! A process tab: what a command the conversation left running prints, as it prints, read by the
//! box where it runs (`kuvo/work/background/<session>/commands/<id>/output`, followed); Stop stops
//! it there. Once it ends, its last output stays, said to have ended.

use std::time::Duration;

use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::background::{Command, Output};
use kuvo_box::follow::{Follow, Followed};
use kuvo_box::printed::{OUTPUT_STREAM, Printed, PrintedState};
use kuvo_box::BoxClient;
use kuvo_ui::{ActiveTheme, Button, ButtonSize, IconName, StyledType, TypeSize, small::status_dot, theme::space};

use super::{Load, run};

/// How often a box without the output stream is asked again.
const ASKED_AGAIN: Duration = Duration::from_millis(1500);

pub struct ProcessView {
    client: BoxClient,
    live: bool,
    session: String,
    id: String,
    /// The command, as the Outputs popover had it, then as the box says it.
    command: Option<Command>,
    printed: Load<Printed>,
    on_screen: bool,
    stopping: bool,
    stop_failed: Option<String>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _follow: Option<Follow<Followed<Printed>>>,
    _task: Option<Task<()>>,
}

impl ProcessView {
    pub fn new(client: BoxClient, live: bool, session: String, id: String, command: Option<Command>, cx: &mut Context<Self>) -> ProcessView {
        ProcessView { client, live, session, id, command, printed: Load::Loading, on_screen: false, stopping: false, stop_failed: None, scroll: ScrollHandle::new(), focus: cx.focus_handle(), _follow: None, _task: None }
    }

    /// Its command's first line, for its tab.
    pub fn title(&self) -> SharedString {
        let command = self.command.as_ref().map(|command| command.command.as_str()).unwrap_or("Output");
        command.lines().next().unwrap_or(command).to_owned().into()
    }

    /// Whether it still runs, as the box last said (yes until it's said).
    pub fn running(&self) -> bool {
        !matches!(self.printed.ready().map(|printed| printed.state), Some(PrintedState::Ended))
    }

    pub fn printed(&self) -> &Load<Printed> {
        &self.printed
    }

    /// Whether it's the tab on screen: followed while it is (the app), read once as it shows (tests
    /// and shots).
    pub fn set_shown(&mut self, on_screen: bool, cx: &mut Context<Self>) {
        if self.on_screen == on_screen {
            return;
        }
        self.on_screen = on_screen;
        self._follow = None;
        self._task = None;
        if !on_screen {
            return;
        }
        if !self.client.gate().has(OUTPUT_STREAM) {
            self.ask_older_box(cx);
            return;
        }
        if !self.live {
            self.read_once(cx);
            return;
        }
        let follow = self.client.follow_printed(&self.session, &self.id);
        let updates = follow.updates().clone();
        self._follow = Some(follow);
        self._task = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let taken = this.update(cx, |this, cx| match update {
                    Followed::Now(printed) => this.take(Ok(printed), cx),
                    Followed::Failed(error) => this.take(Err(error.to_string()), cx),
                });
                if taken.is_err() {
                    break;
                }
            }
        }));
    }

    fn read_once(&mut self, cx: &mut Context<Self>) {
        let (client, session, id) = (self.client.clone(), self.session.clone(), self.id.clone());
        run(self, false, cx, move || client.printed(&session, &id), |this, read, cx| this.take(read.map_err(|error| error.to_string()), cx));
    }

    /// A box without the output stream: its command's output read as the popover read it before.
    // BOX-NEEDED: gone once every box has `work.background.output`.
    fn ask_older_box(&mut self, cx: &mut Context<Self>) {
        let Some(command) = self.command.clone() else {
            self.printed = Load::Failed("This computer’s box needs its update to show this here.".into());
            cx.notify();
            return;
        };
        let read = |client: &BoxClient, command: &Command| {
            let output = client.command_output(command);
            let (output, state) = match output {
                Output::Printed(text) => (text, PrintedState::Running),
                Output::Ended => (String::new(), PrintedState::Ended),
                Output::Unread => (String::new(), PrintedState::Unread),
            };
            Printed { command: Some(command.clone()), output, cut: false, state }
        };
        if !self.live {
            let printed = read(&self.client, &command);
            self.take(Ok(printed), cx);
            return;
        }
        let client = self.client.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            loop {
                let (asked, command) = (client.clone(), command.clone());
                let printed = cx.background_executor().spawn(async move { read(&asked, &command) }).await;
                let ended = printed.state == PrintedState::Ended;
                if this.update(cx, |this, cx| this.take(Ok(printed), cx)).is_err() || ended {
                    break;
                }
                cx.background_executor().timer(ASKED_AGAIN).await;
            }
        }));
    }

    /// What the box said: the output, kept through a read that failed or found it ended with
    /// nothing; scrolled along to its end where it was at its end.
    fn take(&mut self, read: Result<Printed, String>, cx: &mut Context<Self>) {
        let at_end = self.at_end();
        match read {
            Ok(mut printed) => {
                if let Some(before) = self.printed.ready() {
                    if printed.output.is_empty() && printed.state != PrintedState::Running {
                        printed.output = before.output.clone();
                    }
                    if printed.command.is_none() {
                        printed.command = before.command.clone();
                    }
                }
                if let Some(command) = &printed.command {
                    self.command = Some(command.clone());
                }
                self.printed = Load::Ready(printed);
            }
            Err(why) => {
                if self.printed.ready().is_none() {
                    self.printed = Load::Failed(why);
                }
            }
        }
        if at_end {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn at_end(&self) -> bool {
        let offset = self.scroll.offset().y;
        let most = self.scroll.max_offset().y;
        most <= px(0.) || -offset >= most - px(24.)
    }

    /// Stops the command where it runs.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(command) = self.command.clone() else { return };
        self.stopping = true;
        self.stop_failed = None;
        let client = self.client.clone();
        run(self, self.live, cx, move || client.stop_command(&command), |this, done, cx| {
            this.stopping = false;
            if let Err(error) = done {
                this.stop_failed = Some(error.to_string());
            }
            // (The follow says it ended; read again where nothing follows.)
            if !this.live && this.client.gate().has(OUTPUT_STREAM) {
                this.read_once(cx);
            }
            cx.notify();
        });
        cx.notify();
    }

    fn bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let running = self.running();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S2)
            .h(px(36.))
            .pl(space::S3)
            .pr(space::S2)
            .border_b_1()
            .border_color(c.border)
            .child(status_dot(if running { c.success } else { c.text_faint }))
            .child(div().flex_1().min_w_0().truncate().text_mono(TypeSize::Small).text_color(c.text).child(self.title()))
            .when(running, |d| {
                d.child(
                    Button::new("process-stop", "Stop")
                        .icon(IconName::Stop)
                        .ghost()
                        .quiet()
                        .size(ButtonSize::Small)
                        .loading(self.stopping)
                        .disabled(self.command.is_none())
                        .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                )
            })
            .when(!running, |d| d.child(div().pr(space::S1).text_ui(TypeSize::Caption).text_color(c.text_faint).child("Ended")))
            .into_any_element()
    }
}

/// `text` without the colour and cursor codes a terminal would act on.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                // CSI: up to its final byte.
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ST.
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' || (next == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if next == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
            continue;
        }
        if ch == '\r' {
            continue;
        }
        out.push(ch);
    }
    out
}

impl Focusable for ProcessView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ProcessView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let body: AnyElement = match &self.printed {
            Load::Loading => kuvo_ui::small::LoadingLabel::new("process-loading", "Reading its output…").fill().into_any_element(),
            Load::Failed(why) => super::changes::empty("Couldn’t Read Its Output", why.clone(), None, cx),
            Load::Ready(printed) => {
                let text = plain(&printed.output);
                let note = match printed.state {
                    PrintedState::Ended => Some("It ended."),
                    PrintedState::Unread => Some("Couldn’t read it just now; it’s read again in a moment."),
                    _ => None,
                };
                div()
                    .id("process-output")
                    .debug_selector(|| "process-output".into())
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(space::S4)
                    .py(space::S3)
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .when(printed.cut, |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Its latest output")))
                    .child(if text.trim().is_empty() {
                        div().text_ui(TypeSize::Small).text_color(c.text_faint).child("Nothing printed yet.").into_any_element()
                    } else {
                        div().debug_selector(|| "process-text".into()).text_mono(TypeSize::Small).text_color(c.text).child(text).into_any_element()
                    })
                    .children(note.map(|note| div().debug_selector(|| "process-note".into()).text_ui(TypeSize::Caption).text_color(c.text_faint).child(note)))
                    .children(self.stop_failed.clone().map(|why| div().text_ui(TypeSize::Caption).text_color(c.danger).child(format!("It couldn’t be stopped: {why}"))))
                    .into_any_element()
            }
        };
        div().track_focus(&self.focus).size_full().flex().flex_col().child(self.bar(cx)).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_codes_are_left_out_of_what_shows() {
        assert_eq!(plain("\u{1b}[32m✓\u{1b}[0m Ready\r\n"), "✓ Ready\n");
        assert_eq!(plain("\u{1b}]0;title\u{7}done"), "done");
    }
}
