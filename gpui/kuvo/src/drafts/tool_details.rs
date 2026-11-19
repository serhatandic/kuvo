//! Three stand-in design drafts for tool call details in conversations:
//! - Draft A (**Inline**): quiet evolution of Kuvo's step rows — tool-specific icon + inline
//!   target + chevron on every row; opens along a hairline left guide to `Sent` fields and
//!   `Returned` / `Error` (with native `OutputWell` / `DiffView` for commands and edits).
//! - Draft B (**Ledger**): crisp hairline cards with entity pills (`&Mira`, `#231`, `+2h`) on
//!   closed rows, unfolding into a two-band inspection card (`Sent` top band / `Returned` or
//!   `Error` bottom band) with a `Formatted | JSON` view hint.
//! - Draft C (**Rail**): continuous vertical timeline rail with icon nodes and two-part
//!   at-a-glance headers (target + muted outcome preview even while collapsed), opening into
//!   compact `→ Sent` / `← Returned` drawers.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    output::{DiffView, OutputWell, parse_patch},
    theme::{radius, space},
};

use super::chrome;

/// Which of the three tool-details design drafts to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draft {
    Inline,
    Ledger,
    Rail,
}

impl Draft {
    pub const ALL: [Self; 3] = [Self::Inline, Self::Ledger, Self::Rail];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Inline => "tool-details-inline",
            Self::Ledger => "tool-details-ledger",
            Self::Rail => "tool-details-rail",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Inline => "Draft A · Inline",
            Self::Ledger => "Draft B · Ledger",
            Self::Rail => "Draft C · Rail",
        }
    }

    /// Window size tall enough to show the whole turn cleanly with no vertical clipping.
    pub fn size(self) -> (f32, f32) {
        match self {
            Self::Inline => (1280., 1420.),
            Self::Ledger => (1280., 1460.),
            Self::Rail => (1280., 1380.),
        }
    }
}

/// Stand-in view hosting one tool-details draft inside Kuvo's window chrome.
pub struct ToolDetailsDraft {
    draft: Draft,
    open: HashSet<usize>,
}

impl ToolDetailsDraft {
    pub fn new(draft: Draft) -> Self {
        // Show one collapsed Kuvo row (index 5: Canceled a wake-up) alongside the 6 expanded rows
        // (command, edit, sent message, Work Queue update, scheduled wake-up, failed call) so both
        // at-a-glance scanning and open inspection are visible in the same shot.
        let open = [0, 1, 2, 3, 4, 6].into_iter().collect();
        Self { draft, open }
    }

    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.open.remove(&index) {
            self.open.insert(index);
        }
        cx.notify();
    }
}

struct Field {
    key: &'static str,
    value: &'static str,
    mono: bool,
    long: bool,
}

enum Body {
    Command {
        command: &'static str,
        output: &'static str,
    },
    Edit {
        path: &'static str,
        added: usize,
        removed: usize,
        patch: &'static str,
    },
    Call {
        sent: &'static [Field],
        returned: &'static str,
    },
}

struct SampleStep {
    icon: IconName,
    verb: &'static str,
    /// Primary entity badge/target (e.g. `&Mira`, `#231`, `+2h`, `entries.rs`).
    entity: &'static str,
    entity_mono: bool,
    /// Optional short phrase right beside the entity (e.g. `In review`, `Check release build`).
    context: Option<&'static str>,
    /// Short outcome preview for drafts that surface results on the header line.
    outcome: &'static str,
    duration: &'static str,
    failed: bool,
    body: Body,
}

const SAMPLE_PATCH: &str = "\
@@ -512,7 +512,10 @@ fn folds(items: &[(Rc<Step>, usize)]) -> bool {
 fn is_completed_show_file(content: &Content) -> bool {
     match content {
         Content::Tool { name, state, .. } if name == \"kuvo-browser_show_file\" => {
-            state_status(state) == Some(\"completed\")
+            let done = state_status(state) == Some(\"completed\");
+            let has_path = input_path(state).is_some();
+            done && has_path
         }
         _ => false,
     }
 }";

const SEND_MESSAGE_FIELDS: &[Field] = &[
    Field { key: "to", value: "&Mira (ses_1ac77df1a001YU3j)", mono: true, long: false },
    Field { key: "wait", value: "false", mono: true, long: false },
    Field {
        key: "text",
        value: "Groundwork is merged and Flow is wired into ConversationView. All 97 unit tests pass.",
        mono: false,
        long: true,
    },
];

const QUEUE_UPDATE_FIELDS: &[Field] = &[
    Field { key: "item", value: "#231", mono: true, long: false },
    Field { key: "status", value: "In review", mono: false, long: false },
    Field {
        key: "note",
        value: "Flow view wired to kuvo/900c-groundwork; light and dark shots rendered.",
        mono: false,
        long: true,
    },
];

const WAKE_SCHEDULE_FIELDS: &[Field] = &[
    Field { key: "when", value: "+2h", mono: true, long: false },
    Field { key: "summary", value: "Check whether the release build passed", mono: false, long: false },
    Field {
        key: "note",
        value: "Check CI for kuvo/900-batch-c-drafts and report any failing checks.",
        mono: false,
        long: true,
    },
];

const WAKE_CANCEL_FIELDS: &[Field] = &[
    Field { key: "id", value: "wake_4b19", mono: true, long: false },
];

const FAILED_QUEUE_FIELDS: &[Field] = &[
    Field { key: "item", value: "#999", mono: true, long: false },
    Field { key: "status", value: "Done", mono: false, long: false },
    Field { key: "note", value: "Close stale follow-up item", mono: false, long: false },
];

fn sample_steps() -> Vec<SampleStep> {
    vec![
        SampleStep {
            icon: IconName::Terminal,
            verb: "Ran",
            entity: "cargo test -p kuvo shown_files",
            entity_mono: true,
            context: None,
            outcome: "7 passed",
            duration: "1.8s",
            failed: false,
            body: Body::Command {
                command: "cargo test -p kuvo shown_files",
                output: "running 7 tests\ntest shown_files::tests::adapt_maps_document_states ... ok\ntest shown_files::tests::adapt_maps_picture_missing_vs_failed ... ok\ntest shown_files::tests::formatting_helpers_produce_clean_labels ... ok\ntest result: ok. 7 passed; 0 failed; 0 ignored",
            },
        },
        SampleStep {
            icon: IconName::Compose,
            verb: "Edited",
            entity: "gpui/kuvo/src/entries.rs",
            entity_mono: true,
            context: Some("+3 −1"),
            outcome: "+3 −1",
            duration: "120ms",
            failed: false,
            body: Body::Edit {
                path: "gpui/kuvo/src/entries.rs",
                added: 3,
                removed: 1,
                patch: SAMPLE_PATCH,
            },
        },
        SampleStep {
            icon: IconName::Send,
            verb: "Sent a message",
            entity: "&Mira",
            entity_mono: false,
            context: Some("“Groundwork is merged and Flow is wired…”"),
            outcome: "Delivered",
            duration: "90ms",
            failed: false,
            body: Body::Call {
                sent: SEND_MESSAGE_FIELDS,
                returned: "Delivered to &Mira (ses_1ac77df1a001YU3j).",
            },
        },
        SampleStep {
            icon: IconName::Queue,
            verb: "Updated Work Queue",
            entity: "#231",
            entity_mono: true,
            context: Some("In review"),
            outcome: "In review",
            duration: "65ms",
            failed: false,
            body: Body::Call {
                sent: QUEUE_UPDATE_FIELDS,
                returned: "Updated #231 “Render show_file cards inline” — status set to In review, note added.",
            },
        },
        SampleStep {
            icon: IconName::Alarm,
            verb: "Scheduled a wake-up",
            entity: "+2h",
            entity_mono: true,
            context: Some("Check whether the release build passed"),
            outcome: "fires 19:30",
            duration: "45ms",
            failed: false,
            body: Body::Call {
                sent: WAKE_SCHEDULE_FIELDS,
                returned: "Scheduled wake_8f2a for today at 19:30 (+03:00).",
            },
        },
        SampleStep {
            icon: IconName::Alarm,
            verb: "Canceled a wake-up",
            entity: "wake_4b19",
            entity_mono: true,
            context: Some("Check groundwork branch"),
            outcome: "Canceled",
            duration: "32ms",
            failed: false,
            body: Body::Call {
                sent: WAKE_CANCEL_FIELDS,
                returned: "Canceled wake_4b19.",
            },
        },
        SampleStep {
            icon: IconName::Warning,
            verb: "Updated Work Queue",
            entity: "#999",
            entity_mono: true,
            context: Some("Done"),
            outcome: "Not found",
            duration: "48ms",
            failed: true,
            body: Body::Call {
                sent: FAILED_QUEUE_FIELDS,
                returned: "Error: Item #999 not found in this workspace.",
            },
        },
    ]
}

impl Render for ToolDetailsDraft {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let steps = sample_steps();
        let body: AnyElement = match self.draft {
            Draft::Inline => self.render_inline(&steps, cx),
            Draft::Ledger => self.render_ledger(&steps, cx),
            Draft::Rail => self.render_rail(&steps, cx),
        };

        let turns = vec![
            chrome::user_bubble(
                "Finish wiring the shown-files view, run the unit tests, let Mira know, update #231, and remind me to check the release build in two hours.",
                cx,
            ),
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(space::S3)
                .child(body)
                .child(chrome::agent_text(
                    "All seven shown-files tests pass, Mira has the update, #231 is now In review, and the 19:30 wake-up is set. I couldn't close #999 because that item doesn't exist in this workspace.",
                    cx,
                ))
                .into_any_element(),
        ];

        chrome::window_shell(self.draft.label(), turns, cx)
    }
}

impl ToolDetailsDraft {
    /// Draft A — Inline: quiet step rows with tool-specific icons and inline targets; opening
    /// reveals a left-ruled block with `SENT` key/value lines and `RETURNED` / `ERROR` below.
    fn render_inline(&self, steps: &[SampleStep], cx: &mut Context<Self>) -> AnyElement {
        let c = &cx.theme().colors;
        let mut rows = Vec::with_capacity(steps.len());

        for (ix, step) in steps.iter().enumerate() {
            let opened = self.open.contains(&ix);
            let icon_color = if step.failed { c.danger } else { c.text_faint };
            let verb_color = if step.failed { c.danger } else { c.text_muted };

            let header = div()
                .id(SharedString::from(format!("inline-step-{ix}")))
                .flex()
                .items_center()
                .gap(space::S1_5)
                .min_w_0()
                .cursor_pointer()
                .hover(|s| s.opacity(0.82))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                .child(
                    div()
                        .w(px(16.))
                        .flex()
                        .flex_none()
                        .justify_center()
                        .child(Icon::new(step.icon).size(IconSize::Nav).color(icon_color)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_ui(TypeSize::Small)
                        .text_color(verb_color)
                        .child(step.verb),
                )
                .child(
                    div()
                        .flex_none()
                        .when(step.entity_mono, |d| d.text_mono(TypeSize::Small))
                        .when(!step.entity_mono, |d| d.text_ui(TypeSize::Small).medium())
                        .text_color(if step.failed { c.danger } else { c.text })
                        .child(step.entity),
                )
                .when_some(step.context, |d, ctx| {
                    d.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_ui(TypeSize::Small)
                            .text_color(c.text_faint)
                            .child(format!("· {ctx}")),
                    )
                })
                .when(step.failed, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .px(space::S1_5)
                            .py(px(1.))
                            .rounded(radius::SMALL)
                            .bg(c.danger.opacity(0.12))
                            .text_ui(TypeSize::Caption)
                            .medium()
                            .text_color(c.danger)
                            .child("Failed"),
                    )
                })
                .child(
                    Icon::new(if opened { IconName::ChevronDown } else { IconName::ChevronRight })
                        .size(IconSize::Nav)
                        .color(c.text_faint),
                );

            let detail = opened.then(|| {
                let content: AnyElement = match &step.body {
                    Body::Command { command, output } => OutputWell::new(format!("inline-cmd-{ix}"), *output)
                        .command(*command)
                        .into_any_element(),
                    Body::Edit { path, added, removed, patch } => {
                        let lines = Rc::new(parse_patch(patch));
                        DiffView::new(format!("inline-diff-{ix}"), lines)
                            .title(*path)
                            .counts(*added, *removed)
                            .into_any_element()
                    }
                    Body::Call { sent, returned } => {
                        let sent_rows = sent.iter().map(|f| {
                            div()
                                .flex()
                                .items_baseline()
                                .gap(space::S2)
                                .min_w_0()
                                .child(
                                    div()
                                        .w(px(62.))
                                        .flex_none()
                                        .text_mono(TypeSize::Caption)
                                        .text_color(c.text_faint)
                                        .child(f.key),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .when(f.mono, |d| d.text_mono(TypeSize::Caption))
                                        .when(!f.mono, |d| d.text_ui(TypeSize::Small))
                                        .text_color(c.text)
                                        .child(f.value),
                                )
                        });

                        let result_block: AnyElement = if step.failed {
                            div()
                                .mt(space::S1_5)
                                .px(space::S2)
                                .py(space::S1_5)
                                .rounded(radius::SMALL)
                                .bg(c.danger.opacity(0.08))
                                .border_l_2()
                                .border_color(c.danger)
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_ui(TypeSize::Caption)
                                        .medium()
                                        .text_color(c.danger)
                                        .child("Error"),
                                )
                                .child(
                                    div()
                                        .text_ui(TypeSize::Small)
                                        .text_color(c.text)
                                        .child(*returned),
                                )
                                .into_any_element()
                        } else {
                            div()
                                .mt(space::S1_5)
                                .pt(space::S1_5)
                                .border_t_1()
                                .border_color(c.border)
                                .flex()
                                .items_baseline()
                                .gap(space::S2)
                                .child(
                                    div()
                                        .w(px(62.))
                                        .flex_none()
                                        .text_ui(TypeSize::Caption)
                                        .medium()
                                        .text_color(c.text_faint)
                                        .child("Returned"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_ui(TypeSize::Small)
                                        .text_color(c.text_muted)
                                        .child(*returned),
                                )
                                .into_any_element()
                        };

                        div()
                            .flex()
                            .flex_col()
                            .gap(space::S1)
                            .py(px(2.))
                            .child(
                                div()
                                    .text_ui(TypeSize::Caption)
                                    .medium()
                                    .text_color(c.text_faint)
                                    .child("Sent"),
                            )
                            .children(sent_rows)
                            .child(result_block)
                            .into_any_element()
                    }
                };

                div()
                    .ml(px(7.))
                    .pl(space::S3)
                    .border_l_1()
                    .border_color(if step.failed { c.danger.opacity(0.45) } else { c.border })
                    .child(content)
            });

            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S1_5)
                    .child(header)
                    .children(detail)
                    .into_any_element(),
            );
        }

        div().w_full().flex().flex_col().gap(space::S2).children(rows).into_any_element()
    }

    /// Draft B — Ledger: structured hairline rows/cards with entity pills; opening unfolds a
    /// two-band inspection card (`Sent` on surface, `Returned` / `Error` on sunken/tinted foot).
    fn render_ledger(&self, steps: &[SampleStep], cx: &mut Context<Self>) -> AnyElement {
        let c = &cx.theme().colors;
        let mut cards = Vec::with_capacity(steps.len());

        for (ix, step) in steps.iter().enumerate() {
            let opened = self.open.contains(&ix);
            let card_border = if step.failed { c.danger.opacity(0.42) } else { c.border };
            let icon_color = if step.failed { c.danger } else { c.icon };

            let pill = div()
                .flex_none()
                .px(space::S1_5)
                .py(px(1.))
                .rounded(radius::SMALL)
                .bg(if step.failed { c.danger.opacity(0.10) } else { c.sunken })
                .when(step.entity_mono, |d| d.text_mono(TypeSize::Caption))
                .when(!step.entity_mono, |d| d.text_ui(TypeSize::Caption).medium())
                .text_color(if step.failed { c.danger } else { c.text })
                .child(step.entity);

            let header = div()
                .id(SharedString::from(format!("ledger-step-{ix}")))
                .px(space::S3)
                .py(px(7.))
                .flex()
                .items_center()
                .gap(space::S2)
                .min_w_0()
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                .child(Icon::new(step.icon).size(IconSize::Nav).color(icon_color))
                .child(
                    div()
                        .flex_none()
                        .text_ui(TypeSize::Small)
                        .medium()
                        .text_color(if step.failed { c.danger } else { c.text })
                        .child(step.verb),
                )
                .child(pill)
                .when_some(step.context, |d, ctx| {
                    d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_ui(TypeSize::Small)
                            .text_color(c.text_muted)
                            .child(ctx),
                    )
                })
                .when(step.context.is_none(), |d| d.child(div().flex_1()))
                .when(opened && matches!(step.body, Body::Call { .. }), |d| {
                    d.child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .rounded(radius::SMALL)
                            .bg(c.sunken)
                            .p(px(2.))
                            .gap(px(2.))
                            .child(
                                div()
                                    .px(space::S1_5)
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(c.surface)
                                    .text_ui(TypeSize::Caption)
                                    .medium()
                                    .text_color(c.text)
                                    .child("Formatted"),
                            )
                            .child(
                                div()
                                    .px(space::S1_5)
                                    .py(px(1.))
                                    .text_ui(TypeSize::Caption)
                                    .text_color(c.text_faint)
                                    .child("Raw"),
                            ),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .text_mono(TypeSize::Caption)
                        .text_color(c.text_faint)
                        .child(step.duration),
                )
                .child(
                    Icon::new(if opened { IconName::ChevronDown } else { IconName::ChevronRight })
                        .size(IconSize::Nav)
                        .color(c.text_faint),
                );

            let body = opened.then(|| match &step.body {
                Body::Command { command, output } => div()
                    .border_t_1()
                    .border_color(c.border)
                    .p(space::S2)
                    .bg(c.sunken)
                    .child(OutputWell::new(format!("ledger-cmd-{ix}"), *output).command(*command))
                    .into_any_element(),
                Body::Edit { path, added, removed, patch } => {
                    let lines = Rc::new(parse_patch(patch));
                    div()
                        .border_t_1()
                        .border_color(c.border)
                        .p(space::S2)
                        .bg(c.sunken)
                        .child(DiffView::new(format!("ledger-diff-{ix}"), lines).title(*path).counts(*added, *removed))
                        .into_any_element()
                }
                Body::Call { sent, returned } => {
                    let sent_band = div()
                        .border_t_1()
                        .border_color(c.border)
                        .px(space::S3)
                        .py(space::S2)
                        .bg(c.surface)
                        .flex()
                        .gap(space::S3)
                        .child(
                            div()
                                .w(px(64.))
                                .flex_none()
                                .pt(px(1.))
                                .text_ui(TypeSize::Caption)
                                .medium()
                                .text_color(c.text_faint)
                                .child("Sent"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(space::S1)
                                .children(sent.iter().map(|f| {
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap(space::S2)
                                        .min_w_0()
                                        .child(
                                            div()
                                                .w(px(62.))
                                                .flex_none()
                                                .text_mono(TypeSize::Caption)
                                                .text_color(c.text_faint)
                                                .child(f.key),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .when(f.mono, |d| d.text_mono(TypeSize::Caption))
                                                .when(!f.mono, |d| d.text_ui(TypeSize::Small))
                                                .text_color(c.text)
                                                .child(f.value),
                                        )
                                })),
                        );

                    let returned_band = div()
                        .border_t_1()
                        .border_color(if step.failed { c.danger.opacity(0.32) } else { c.border })
                        .px(space::S3)
                        .py(space::S2)
                        .bg(if step.failed { c.danger.opacity(0.08) } else { c.sunken })
                        .flex()
                        .items_baseline()
                        .gap(space::S3)
                        .child(
                            div()
                                .w(px(64.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(space::S1)
                                .child(
                                    Icon::new(if step.failed { IconName::Warning } else { IconName::Check })
                                        .size(IconSize::XSmall)
                                        .color(if step.failed { c.danger } else { c.success }),
                                )
                                .child(
                                    div()
                                        .text_ui(TypeSize::Caption)
                                        .medium()
                                        .text_color(if step.failed { c.danger } else { c.text_muted })
                                        .child(if step.failed { "Error" } else { "Returned" }),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ui(TypeSize::Small)
                                .text_color(if step.failed { c.danger } else { c.text })
                                .child(*returned),
                        );

                    div().flex().flex_col().child(sent_band).child(returned_band).into_any_element()
                }
            });

            cards.push(
                div()
                    .w_full()
                    .rounded(if opened { radius::POPOVER } else { radius::NAV })
                    .bg(c.surface)
                    .border_1()
                    .border_color(card_border)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(header)
                    .children(body)
                    .into_any_element(),
            );
        }

        div().w_full().flex().flex_col().gap(space::S2).children(cards).into_any_element()
    }

    /// Draft C — Rail: continuous timeline rail with circular icon nodes and two-part header
    /// summaries (target + right-aligned outcome preview even when closed); opening reveals a
    /// compact sunken `→ Sent` / `← Returned` drawer beside the rail.
    fn render_rail(&self, steps: &[SampleStep], cx: &mut Context<Self>) -> AnyElement {
        let c = &cx.theme().colors;
        let last_ix = steps.len().saturating_sub(1);
        let mut items = Vec::with_capacity(steps.len());

        for (ix, step) in steps.iter().enumerate() {
            let opened = self.open.contains(&ix);
            let is_last = ix == last_ix;

            let node = div()
                .size(px(22.))
                .flex_none()
                .rounded(radius::PILL)
                .bg(if step.failed { c.danger.opacity(0.12) } else { c.sunken })
                .border_1()
                .border_color(if step.failed { c.danger.opacity(0.45) } else { c.border })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(step.icon)
                        .size(IconSize::XSmall)
                        .color(if step.failed { c.danger } else { c.icon }),
                );

            let rail_col = div()
                .w(px(22.))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .child(node)
                .when(!is_last, |d| {
                    d.child(div().flex_1().w(px(1.)).min_h(space::S2).bg(c.border))
                });

            let header = div()
                .id(SharedString::from(format!("rail-step-{ix}")))
                .h(px(22.))
                .flex()
                .items_center()
                .gap(space::S1_5)
                .min_w_0()
                .cursor_pointer()
                .hover(|s| s.opacity(0.82))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                .child(
                    div()
                        .flex_none()
                        .text_ui(TypeSize::Small)
                        .medium()
                        .text_color(if step.failed { c.danger } else { c.text })
                        .child(step.verb),
                )
                .child(
                    div()
                        .flex_none()
                        .when(step.entity_mono, |d| d.text_mono(TypeSize::Small))
                        .when(!step.entity_mono, |d| d.text_ui(TypeSize::Small))
                        .text_color(c.text_muted)
                        .child(step.entity),
                )
                .when_some(step.context.filter(|ctx| *ctx != step.outcome), |d, ctx| {
                    d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_ui(TypeSize::Small)
                            .text_color(c.text_faint)
                            .child(format!("· {ctx}")),
                    )
                })
                .when(step.context.filter(|ctx| *ctx != step.outcome).is_none(), |d| d.child(div().flex_1()))
                .child(
                    div()
                        .flex_none()
                        .text_ui(TypeSize::Caption)
                        .when(step.failed, |d| d.medium())
                        .text_color(if step.failed { c.danger } else { c.text_faint })
                        .child(step.outcome),
                )
                .child(
                    Icon::new(if opened { IconName::ChevronDown } else { IconName::ChevronRight })
                        .size(IconSize::Nav)
                        .color(c.text_faint),
                );

            let drawer = opened.then(|| match &step.body {
                Body::Command { command, output } => OutputWell::new(format!("rail-cmd-{ix}"), *output)
                    .command(*command)
                    .into_any_element(),
                Body::Edit { path, added, removed, patch } => {
                    let lines = Rc::new(parse_patch(patch));
                    DiffView::new(format!("rail-diff-{ix}"), lines)
                        .title(*path)
                        .counts(*added, *removed)
                        .into_any_element()
                }
                Body::Call { sent, returned } => {
                    let short_fields: Vec<&Field> = sent.iter().filter(|f| !f.long).collect();
                    let long_fields: Vec<&Field> = sent.iter().filter(|f| f.long).collect();

                    div()
                        .w_full()
                        .rounded(radius::NAV)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(if step.failed { c.danger.opacity(0.38) } else { c.border })
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px(space::S3)
                                .py(space::S2)
                                .flex()
                                .flex_col()
                                .gap(space::S1_5)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .flex_wrap()
                                        .gap(space::S1_5)
                                        .child(
                                            div()
                                                .text_ui(TypeSize::Caption)
                                                .medium()
                                                .text_color(c.text_faint)
                                                .child("→ Sent"),
                                        )
                                        .children(short_fields.into_iter().map(|f| {
                                            div()
                                                .px(space::S1_5)
                                                .py(px(1.))
                                                .rounded(radius::SMALL)
                                                .bg(c.surface)
                                                .border_1()
                                                .border_color(c.border)
                                                .flex()
                                                .items_baseline()
                                                .gap(space::S1)
                                                .child(
                                                    div()
                                                        .text_mono(TypeSize::Caption)
                                                        .text_color(c.text_faint)
                                                        .child(format!("{}:", f.key)),
                                                )
                                                .child(
                                                    div()
                                                        .when(f.mono, |d| d.text_mono(TypeSize::Caption))
                                                        .when(!f.mono, |d| d.text_ui(TypeSize::Caption))
                                                        .text_color(c.text)
                                                        .child(f.value),
                                                )
                                        })),
                                )
                                .children(long_fields.into_iter().map(|f| {
                                    div()
                                        .pl(space::S2)
                                        .border_l_2()
                                        .border_color(c.border_strong)
                                        .flex()
                                        .flex_col()
                                        .gap(px(1.))
                                        .child(
                                            div()
                                                .text_mono(TypeSize::Caption)
                                                .text_color(c.text_faint)
                                                .child(f.key),
                                        )
                                        .child(
                                            div()
                                                .text_ui(TypeSize::Small)
                                                .text_color(c.text)
                                                .child(f.value),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .border_t_1()
                                .border_color(if step.failed { c.danger.opacity(0.32) } else { c.border })
                                .px(space::S3)
                                .py(space::S2)
                                .when(step.failed, |d| d.bg(c.danger.opacity(0.08)))
                                .flex()
                                .items_baseline()
                                .gap(space::S2)
                                .child(
                                    div()
                                        .flex_none()
                                        .text_ui(TypeSize::Caption)
                                        .medium()
                                        .text_color(if step.failed { c.danger } else { c.text_faint })
                                        .child(if step.failed { "✕ Error" } else { "← Returned" }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_ui(TypeSize::Small)
                                        .text_color(if step.failed { c.danger } else { c.text_muted })
                                        .child(*returned),
                                ),
                        )
                        .into_any_element()
                }
            });

            items.push(
                div()
                    .w_full()
                    .flex()
                    .gap(space::S2)
                    .child(rail_col)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!is_last, |d| d.pb(space::S3))
                            .flex()
                            .flex_col()
                            .gap(space::S1_5)
                            .child(header)
                            .children(drawer),
                    )
                    .into_any_element(),
            );
        }

        div().w_full().flex().flex_col().children(items).into_any_element()
    }
}
