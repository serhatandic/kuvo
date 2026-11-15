//! Selecting text in a conversation: a short exchange, with a selection across a paragraph, in a
//! code block, across two messages, and one set aside while another field has the keyboard. Each
//! can be dragged in, double and triple clicked, and copied with ⌘C.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, px, relative,
};
use kuvo_ui::{
    ActiveTheme, StyledType, TypeSize,
    markdown::{self, Markdown, Selectable},
    selection::{self, Lead, SelectableText, SelectionArea, SelectionState, Unit},
    theme::{radius, space},
};

use crate::{Gallery, page as frame, section};

const ASKED: &str = "The sign-up form clears everything when the email is taken. Can you look?";

const REPLY: &str = "Found it. When the server answers **409**, the error path calls `reset()` before it shows the message:\n\n```ts\n} catch (error) {\n  reset();\n  setError(error.message);\n}\n```\n\nMoving `reset()` into the success path keeps what was typed.";

/// The exchange's text, as copying sees it.
fn units() -> Vec<(SharedString, Vec<Unit>)> {
    vec![("asked".into(), vec![Unit::new(&[0], ASKED, Lead::Paragraph)]), ("reply".into(), markdown::units(REPLY, &[0]))]
}

/// One exchange with its own selection.
pub struct Demo {
    state: SelectionState,
    focus: FocusHandle,
    in_use: bool,
}

impl Demo {
    fn new(from: &str, to: &str, in_use: bool, cx: &mut Context<Self>) -> Self {
        let state = SelectionState::new();
        state.set_rows(["asked", "reply"]);
        state.select_text(&units(), from, to);
        Self { state, focus: cx.focus_handle(), in_use }
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let state = self.state.clone();
        let asked = div().flex().justify_end().child(
            div()
                .max_w(relative(0.82))
                .px(space::S4)
                .py(px(10.))
                .rounded(radius::CARD)
                .bg(c.sunken)
                .text_ui(TypeSize::Body)
                .text_color(c.text)
                .child(SelectableText::new("asked", ASKED).selectable(Some((state.clone(), selection::UnitId::new("asked", &[0]))))),
        );
        let reply = Markdown::new("reply", REPLY).selectable(Selectable::new(state.clone(), "reply", &[0]));
        let column = div().flex().flex_col().gap(space::S5).p(space::S4).child(asked).child(reply);
        div()
            .key_context(selection::CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &selection::Copy, _, cx| {
                let order = this.state.order();
                if let Some((start, end)) = this.state.range() {
                    selection::copy_to_clipboard(selection::copy_text(&order, units(), &start, &end), cx);
                }
            }))
            .on_action(cx.listener(|this, _: &selection::SelectAll, _, cx| {
                this.state.select_all("asked", "reply");
                cx.notify();
            }))
            .child(SelectionArea::new(state, self.focus.clone(), column).shown_in_use(self.in_use))
    }
}

pub struct Demos {
    paragraph: Entity<Demo>,
    code: Entity<Demo>,
    across: Entity<Demo>,
    aside: Entity<Demo>,
}

impl Demos {
    pub fn new(cx: &mut Context<Gallery>) -> Self {
        Self {
            paragraph: cx.new(|cx| Demo::new("the error path", "the message", true, cx)),
            code: cx.new(|cx| Demo::new("reset();", "error.message", true, cx)),
            across: cx.new(|cx| Demo::new("when the email", "Found it. When the server", true, cx)),
            aside: cx.new(|cx| Demo::new("Moving", "success path", false, cx)),
        }
    }
}

pub fn page(demos: &Demos, _window: &mut Window, cx: &mut Context<Gallery>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let pane = |title: &'static str, demo: &Entity<Demo>| {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(space::S2)
            .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(title))
            .child(div().id(title).rounded(radius::CARD).bg(c.surface).border_1().border_color(c.border).child(demo.clone()))
    };
    frame(
        "Selecting text",
        "Drag across any messages, double-click a word, triple-click a line, shift-click to extend; ⌘A takes the message, again the conversation; ⌘C copies plain text.",
        vec![
            section(
                "Selected, while the conversation has the keyboard",
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S5)
                    .child(div().flex().gap(space::S5).child(pane("Across a paragraph", &demos.paragraph)).child(pane("Inside a code block", &demos.code)))
                    .child(div().flex().gap(space::S5).child(pane("Across two messages", &demos.across)).child(pane("Set aside: another field has the keyboard", &demos.aside))),
                cx,
            ),
        ],
        cx,
    )
}
