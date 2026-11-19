//! Three design drafts for `kuvo-browser_show_file` in a conversation:
//!
//! - **Flow** (`show-file-flow`): borderless media directly in the conversation (no redundant
//!   "Showed a file" step line), balanced multi-picture strip, tactile horizontal file cards with
//!   typographic extension tiles, and compact dashed tombstones for missing files.
//! - **Canvas** (`show-file-canvas`): framed editorial figures with unified footers, asymmetric
//!   bento gallery for multi-picture turns, and rich side-by-side preview cards (a miniature page
//!   header for PDFs and a live text excerpt for Markdown) with unavailable-artifact frames.
//! - **Thread** (`show-file-thread`): Zed-inspired quiet tool-step rail (`border_l_1`) tying one
//!   summary line ("Showed 3 images · shots/") to unboxed images and a compact divided file shelf.

use gpui::{
    AnyElement, App, Hsla, IntoElement, ObjectFit, ParentElement, SharedString, Styled, StyledImage as _, div, img,
    prelude::FluentBuilder as _, px,
};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    theme::{radius, space},
};

use super::chrome::{agent_text, user_bubble};
use super::media::{SampleMedia, SamplePicture};

/// Which design draft to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftKind {
    Flow,
    Canvas,
    Thread,
}

impl DraftKind {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Flow => "show-file-flow",
            Self::Canvas => "show-file-canvas",
            Self::Thread => "show-file-thread",
        }
    }

    pub fn badge(self) -> &'static str {
        match self {
            Self::Flow => "Draft A · Flow",
            Self::Canvas => "Draft B · Canvas",
            Self::Thread => "Draft C · Thread",
        }
    }

    /// Tall enough for all four turns to read cleanly above the composer without scrolling.
    pub fn height(self) -> f32 {
        match self {
            Self::Flow => 1420.,
            Self::Canvas => 1560.,
            Self::Thread => 1400.,
        }
    }

    pub const ALL: [DraftKind; 3] = [DraftKind::Flow, DraftKind::Canvas, DraftKind::Thread];
}

#[derive(Clone, Copy)]
enum DocKind {
    Pdf,
    Markdown,
}

impl DocKind {
    fn tag(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Markdown => "MD",
        }
    }

    fn tint(self, cx: &App) -> Hsla {
        let c = &cx.theme().colors;
        match self {
            Self::Pdf => c.danger,
            Self::Markdown => c.accent,
        }
    }
}

struct DocSample {
    name: &'static str,
    folder: &'static str,
    kind: DocKind,
    detail: &'static str,
    short_meta: &'static str,
    caption: &'static str,
    can_preview_in_panel: bool,
}

const SAMPLE_DOCS: [DocSample; 2] = [
    DocSample {
        name: "Q4-launch-report.pdf",
        folder: "docs/release",
        kind: DocKind::Pdf,
        detail: "PDF · 2.4 MB · 14 pages",
        short_meta: "2.4 MB · 14p",
        caption: "Executive summary and rollout timeline for Thursday.",
        can_preview_in_panel: false,
    },
    DocSample {
        name: "migration-plan.md",
        folder: "docs/release",
        kind: DocKind::Markdown,
        detail: "Markdown · 18 KB · docs/release",
        short_meta: "18 KB",
        caption: "Step-by-step schema migration checklist.",
        can_preview_in_panel: true,
    },
];

struct MissingSample {
    name: &'static str,
    folder: &'static str,
    is_picture: bool,
    caption: Option<&'static str>,
}

const SAMPLE_MISSING: [MissingSample; 2] = [
    MissingSample {
        name: "old-wireframe.png",
        folder: "tmp",
        is_picture: true,
        caption: Some("Initial hero wireframe before the copy pass."),
    },
    MissingSample {
        name: "draft-notes.pdf",
        folder: "tmp",
        is_picture: false,
        caption: None,
    },
];

/// Builds the four shared conversation turns rendered in the chosen draft's style.
pub fn conversation_turns(kind: DraftKind, media: &SampleMedia, cx: &App) -> Vec<AnyElement> {
    let gallery = [
        (&media.checkout_light, "checkout-light.png", "Light mode"),
        (&media.checkout_dark, "checkout-dark.png", "Dark mode"),
        (&media.checkout_narrow, "checkout-narrow.png", "Narrow (390 px)"),
    ];
    let turn = |user: &str, intro: &str, body: AnyElement| {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(space::S3)
            .child(user_bubble(user, cx))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .child(agent_text(intro, cx))
                    .child(body),
            )
            .into_any_element()
    };

    vec![
        turn(
            "Can you show me how the new pricing page hero looks?",
            "Here’s the updated hero from the latest build:",
            match kind {
                DraftKind::Flow => flow_single_picture(&media.pricing_hero, cx),
                DraftKind::Canvas => canvas_single_picture(&media.pricing_hero, cx),
                DraftKind::Thread => thread_single_picture(&media.pricing_hero, cx),
            },
        ),
        turn(
            "Compare light, dark, and the narrow checkout state together.",
            "All three states side by side:",
            match kind {
                DraftKind::Flow => flow_multi_pictures(&gallery, cx),
                DraftKind::Canvas => canvas_multi_pictures(&gallery, cx),
                DraftKind::Thread => thread_multi_pictures(&gallery, cx),
            },
        ),
        turn(
            "Export the release summary as a PDF and show me the migration notes too.",
            "Both files are ready in docs/release:",
            match kind {
                DraftKind::Flow => flow_documents(&SAMPLE_DOCS, cx),
                DraftKind::Canvas => canvas_documents(&SAMPLE_DOCS, cx),
                DraftKind::Thread => thread_documents(&SAMPLE_DOCS, cx),
            },
        ),
        turn(
            "Pull up the earlier wireframe and draft notes from Tuesday.",
            "Here are the two files from earlier:",
            match kind {
                DraftKind::Flow => flow_missing(&SAMPLE_MISSING, cx),
                DraftKind::Canvas => canvas_missing(&SAMPLE_MISSING, cx),
                DraftKind::Thread => thread_missing(&SAMPLE_MISSING, cx),
            },
        ),
    ]
}

// =================================================================================================
// Draft A — Flow: borderless media, balanced strip, tactile file cards, dashed tombstones
// =================================================================================================

fn flow_single_picture(pic: &SamplePicture, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_col()
        .gap(space::S1_5)
        .child(
            div()
                .w(px(520.))
                .h(px(325.))
                .rounded(radius::CARD)
                .overflow_hidden()
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .child(img(pic.image.clone()).size_full().object_fit(ObjectFit::Contain)),
        )
        .child(
            div()
                .max_w(px(520.))
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child("Pricing hero at 1440 px — tighter headline spacing and the annual toggle."),
        )
        .into_any_element()
}

fn flow_multi_pictures(items: &[(&SamplePicture, &'static str, &'static str); 3], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .w_full()
        .flex()
        .gap(space::S3)
        .children(items.iter().map(|(pic, _name, caption)| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(space::S1_5)
                .child(
                    div()
                        .w_full()
                        .h(px(156.))
                        .rounded(radius::POPOVER)
                        .overflow_hidden()
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .child(img(pic.image.clone()).size_full().object_fit(ObjectFit::Contain)),
                )
                .child(
                    div()
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(*caption),
                )
        }))
        .into_any_element()
}

fn flow_documents(docs: &[DocSample], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_col()
        .gap(space::S3)
        .children(docs.iter().map(|doc| {
            let tint = doc.kind.tint(cx);
            let badge = div()
                .size(px(38.))
                .flex_none()
                .rounded(radius::NAV)
                .bg(tint.opacity(0.11))
                .border_1()
                .border_color(tint.opacity(0.24))
                .flex()
                .items_center()
                .justify_center()
                .text_ui(TypeSize::Caption)
                .medium()
                .text_color(tint)
                .child(doc.kind.tag());

            let action = if doc.can_preview_in_panel {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S1)
                    .child(quiet_action_pill("Preview", IconName::PanelRight, cx))
                    .child(
                        div()
                            .size(px(26.))
                            .rounded(radius::NAV)
                            .border_1()
                            .border_color(c.border)
                            .bg(c.surface)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::External).size(IconSize::XSmall).color(c.text_muted)),
                    )
            } else {
                div()
                    .flex_none()
                    .child(quiet_action_pill("Open", IconName::External, cx))
            };

            div()
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(
                    div()
                        .max_w(px(472.))
                        .px(space::S3)
                        .py(space::S2)
                        .rounded(radius::POPOVER)
                        .bg(c.card)
                        .border_1()
                        .border_color(c.border)
                        .shadow(cx.theme().card_shadow())
                        .flex()
                        .items_center()
                        .gap(space::S3)
                        .child(badge)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_ui(TypeSize::Small)
                                        .medium()
                                        .text_color(c.text)
                                        .child(doc.name),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_ui(TypeSize::Caption)
                                        .text_color(c.text_muted)
                                        .child(doc.detail),
                                ),
                        )
                        .child(action),
                )
                .child(
                    div()
                        .max_w(px(472.))
                        .text_ui(TypeSize::Small)
                        .text_color(c.text_muted)
                        .child(doc.caption),
                )
        }))
        .into_any_element()
}

fn flow_missing(items: &[MissingSample], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .children(items.iter().map(|item| {
            let icon = if item.is_picture { IconName::Image } else { IconName::File };
            div()
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(
                    div()
                        .max_w(px(472.))
                        .px(space::S3)
                        .py(space::S2)
                        .rounded(radius::POPOVER)
                        .bg(c.sunken)
                        .border_1()
                        .border_dashed()
                        .border_color(c.border_strong)
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            div()
                                .size(px(32.))
                                .flex_none()
                                .rounded(radius::NAV)
                                .bg(c.surface)
                                .border_1()
                                .border_color(c.border)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(icon).size(IconSize::Nav).color(c.text_faint)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .text_ui(TypeSize::Small)
                                        .text_color(c.text_muted)
                                        .child(item.name),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_ui(TypeSize::Caption)
                                        .text_color(c.text_faint)
                                        .child(format!("No longer on disk · {}/", item.folder)),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px(space::S2)
                                .py(px(2.))
                                .rounded(radius::PILL)
                                .bg(c.surface)
                                .border_1()
                                .border_color(c.border)
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child("Missing"),
                        ),
                )
                .when_some(item.caption, |d, caption| {
                    d.child(
                        div()
                            .max_w(px(472.))
                            .text_ui(TypeSize::Small)
                            .text_color(c.text_faint)
                            .child(caption),
                    )
                })
        }))
        .into_any_element()
}

fn quiet_action_pill(label: &'static str, icon: IconName, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .h(px(26.))
        .px(space::S2)
        .rounded(radius::NAV)
        .bg(c.surface)
        .border_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .gap(space::S1)
        .text_ui(TypeSize::Caption)
        .medium()
        .text_color(c.text)
        .child(label)
        .child(Icon::new(icon).size(IconSize::XSmall).color(c.text_muted))
        .into_any_element()
}

// =================================================================================================
// Draft B — Canvas: framed editorial figures, bento gallery, rich side-by-side preview cards
// =================================================================================================

fn canvas_single_picture(pic: &SamplePicture, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .w(px(544.))
        .rounded(radius::CARD)
        .bg(c.card)
        .border_1()
        .border_color(c.border)
        .shadow(cx.theme().card_shadow())
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .h(px(332.))
                .bg(c.sunken)
                .child(img(pic.image.clone()).size_full().object_fit(ObjectFit::Contain)),
        )
        .child(
            div()
                .px(space::S4)
                .py(space::S2)
                .border_t_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .gap(space::S3)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_ui(TypeSize::Small)
                                .text_color(c.text)
                                .child("Pricing hero at 1440 px — tighter headline spacing and the annual toggle."),
                        )
                        .child(
                            div()
                                .text_mono(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child("shots/pricing-hero.png · 1440 × 900"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(space::S1)
                        .child(canvas_icon_btn(IconName::Copy, cx))
                        .child(canvas_icon_btn(IconName::ExpandEditor, cx)),
                ),
        )
        .into_any_element()
}

fn canvas_multi_pictures(items: &[(&SamplePicture, &'static str, &'static str); 3], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let pane_tag = |label: &'static str| {
        div()
            .absolute()
            .left(space::S2)
            .bottom(space::S2)
            .px(space::S2)
            .py(px(2.))
            .rounded(radius::SMALL)
            .bg(c.elevated.opacity(0.92))
            .border_1()
            .border_color(c.border)
            .text_ui(TypeSize::Caption)
            .medium()
            .text_color(c.text)
            .child(label)
    };

    div()
        .w(px(620.))
        .rounded(radius::CARD)
        .bg(c.card)
        .border_1()
        .border_color(c.border)
        .shadow(cx.theme().card_shadow())
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .h(px(244.))
                .bg(c.border)
                .flex()
                .gap(px(1.))
                // Left featured pane.
                .child(
                    div()
                        .relative()
                        .w(px(360.))
                        .h_full()
                        .bg(c.sunken)
                        .overflow_hidden()
                        .child(img(items[0].0.image.clone()).size_full().object_fit(ObjectFit::Cover))
                        .child(pane_tag(items[0].2)),
                )
                // Right two stacked panes.
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .bg(c.sunken)
                                .overflow_hidden()
                                .child(img(items[1].0.image.clone()).size_full().object_fit(ObjectFit::Cover))
                                .child(pane_tag(items[1].2)),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .bg(c.sunken)
                                .overflow_hidden()
                                .child(img(items[2].0.image.clone()).size_full().object_fit(ObjectFit::Contain))
                                .child(pane_tag(items[2].2)),
                        ),
                ),
        )
        .child(
            div()
                .px(space::S3)
                .py(space::S2)
                .border_t_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .gap(space::S2)
                .child(Icon::new(IconName::Image).size(IconSize::Nav).color(c.text_faint))
                .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child("3 images"))
                .child(div().flex_1().truncate().text_mono(TypeSize::Caption).text_color(c.text_faint).child("shots/checkout-{light,dark,narrow}.png"))
                .child(div().flex_none().text_ui(TypeSize::Caption).medium().text_color(c.accent).child("View all")),
        )
        .into_any_element()
}

fn canvas_documents(docs: &[DocSample], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .w_full()
        .flex()
        .gap(space::S3)
        .children(docs.iter().map(|doc| {
            let preview_band: AnyElement = match doc.kind {
                DocKind::Pdf => {
                    // Miniature page silhouette peeking up from the preview header.
                    div()
                        .w_full()
                        .h(px(76.))
                        .bg(c.sunken)
                        .border_b_1()
                        .border_color(c.border)
                        .px(space::S3)
                        .pt(space::S2)
                        .flex()
                        .items_end()
                        .justify_between()
                        .child(
                            div()
                                .w(px(148.))
                                .h(px(62.))
                                .rounded_t(radius::SMALL)
                                .bg(c.surface)
                                .border_t_1()
                                .border_l_1()
                                .border_r_1()
                                .border_color(c.border_strong)
                                .p(space::S2)
                                .flex()
                                .flex_col()
                                .gap(px(5.))
                                .child(div().w(px(36.)).h(px(4.)).rounded(radius::PILL).bg(c.danger.opacity(0.65)))
                                .child(div().w(px(110.)).h(px(5.)).rounded(radius::PILL).bg(c.text_muted.opacity(0.45)))
                                .child(div().w(px(92.)).h(px(4.)).rounded(radius::PILL).bg(c.border_strong))
                                .child(div().w(px(102.)).h(px(4.)).rounded(radius::PILL).bg(c.border_strong)),
                        )
                        .child(
                            div()
                                .mb(space::S2)
                                .px(space::S1_5)
                                .py(px(2.))
                                .rounded(radius::SMALL)
                                .bg(c.danger.opacity(0.12))
                                .text_ui(TypeSize::Caption)
                                .medium()
                                .text_color(c.danger)
                                .child("PDF · 14 pages"),
                        )
                        .into_any_element()
                }
                DocKind::Markdown => {
                    // Live opening excerpt (possible today via `kuvo/file/text`).
                    div()
                        .w_full()
                        .h(px(76.))
                        .bg(c.sunken)
                        .border_b_1()
                        .border_color(c.border)
                        .px(space::S3)
                        .py(space::S2)
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().truncate().text_ui(TypeSize::Caption).medium().text_color(c.text).child("# Q4 Schema Migration"))
                        .child(div().truncate().text_mono(TypeSize::Caption).text_color(c.text_muted).child("1. Backfill sessions.org_id in batches"))
                        .child(div().truncate().text_mono(TypeSize::Caption).text_color(c.text_faint).child("2. Swap NOT NULL constraint zero-downtime"))
                        .into_any_element()
                }
            };

            let (footer_label, footer_icon) = if doc.can_preview_in_panel {
                ("Open in Panel", IconName::PanelRight)
            } else {
                ("Open", IconName::External)
            };

            div()
                .flex_1()
                .min_w_0()
                .rounded(radius::CARD)
                .bg(c.card)
                .border_1()
                .border_color(c.border)
                .shadow(cx.theme().card_shadow())
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(preview_band)
                .child(
                    div()
                        .p(space::S3)
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(space::S1)
                        .child(div().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(doc.name))
                        .child(div().truncate().text_mono(TypeSize::Caption).text_color(c.text_faint).child(format!("{}/ · {}", doc.folder, doc.short_meta)))
                        .child(div().pt(px(2.)).text_ui(TypeSize::Caption).text_color(c.text_muted).child(doc.caption)),
                )
                .child(
                    div()
                        .px(space::S3)
                        .py(space::S2)
                        .border_t_1()
                        .border_color(c.border)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_ui(TypeSize::Caption).medium().text_color(c.accent).child(footer_label))
                        .child(Icon::new(footer_icon).size(IconSize::Nav).color(c.accent)),
                )
        }))
        .into_any_element()
}

fn canvas_missing(items: &[MissingSample], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .w_full()
        .flex()
        .gap(space::S3)
        .children(items.iter().map(|item| {
            let icon = if item.is_picture { IconName::Image } else { IconName::File };
            div()
                .flex_1()
                .min_w_0()
                .rounded(radius::CARD)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .p(space::S3)
                .flex()
                .flex_col()
                .gap(space::S1_5)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(Icon::new(icon).size(IconSize::Nav).color(c.text_faint))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_ui(TypeSize::Small)
                                .medium()
                                .text_color(c.text_muted)
                                .child(item.name),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px(space::S1_5)
                                .py(px(1.))
                                .rounded(radius::SMALL)
                                .bg(c.surface)
                                .border_1()
                                .border_color(c.border)
                                .text_ui(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child("Removed"),
                        ),
                )
                .child(
                    div()
                        .truncate()
                        .text_mono(TypeSize::Caption)
                        .text_color(c.text_faint)
                        .child(format!("{}/{}", item.folder, item.name)),
                )
                .child(
                    div()
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(item.caption.unwrap_or("File is no longer on this computer.")),
                )
        }))
        .into_any_element()
}

fn canvas_icon_btn(icon: IconName, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .size(px(28.))
        .rounded(radius::NAV)
        .border_1()
        .border_color(c.border)
        .bg(c.surface)
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size(IconSize::Nav).color(c.text_muted))
        .into_any_element()
}

// =================================================================================================
// Draft C — Thread: Zed-style quiet vertical rail + high-density divided document shelf
// =================================================================================================

fn thread_rail(icon: IconName, label: &str, target: &str, trailing: Option<&str>, body: AnyElement, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let label = SharedString::from(label.to_owned());
    let target = SharedString::from(target.to_owned());
    let trailing = trailing.map(|t| SharedString::from(t.to_owned()));
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S1_5)
                .text_ui(TypeSize::Small)
                .child(
                    div()
                        .w(px(16.))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(Icon::new(icon).size(IconSize::Nav).color(c.text_faint)),
                )
                .child(div().flex_none().text_color(c.text_muted).child(label))
                .child(div().flex_1().min_w_0().truncate().text_mono(TypeSize::Small).text_color(c.text_faint).child(target))
                .when_some(trailing, |d, t| d.child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(t))),
        )
        .child(
            div()
                .ml(px(7.))
                .pl(space::S3)
                .border_l_1()
                .border_color(c.border)
                .child(body),
        )
        .into_any_element()
}

fn thread_single_picture(pic: &SamplePicture, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let body = div()
        .flex()
        .flex_col()
        .gap(space::S1_5)
        .child(
            div()
                .w(px(480.))
                .h(px(300.))
                .rounded(radius::POPOVER)
                .overflow_hidden()
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .child(img(pic.image.clone()).size_full().object_fit(ObjectFit::Contain)),
        )
        .child(
            div()
                .max_w(px(480.))
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child("Pricing hero at 1440 px — tighter headline spacing and the annual toggle."),
        )
        .into_any_element();
    thread_rail(IconName::Image, "Showed", "shots/pricing-hero.png", Some("1440 × 900"), body, cx)
}

fn thread_multi_pictures(items: &[(&SamplePicture, &'static str, &'static str); 3], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let body = div()
        .w_full()
        .flex()
        .gap(space::S2)
        .children(items.iter().map(|(pic, name, caption)| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(
                    div()
                        .w_full()
                        .h(px(140.))
                        .rounded(radius::NAV)
                        .overflow_hidden()
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .child(img(pic.image.clone()).size_full().object_fit(ObjectFit::Contain)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().truncate().text_ui(TypeSize::Caption).text_color(c.text_muted).child(*caption))
                        .child(div().truncate().text_mono(TypeSize::Caption).text_color(c.text_faint).child(*name)),
                )
        }))
        .into_any_element();
    thread_rail(IconName::Image, "Showed 3 images", "shots/", None, body, cx)
}

fn thread_documents(docs: &[DocSample], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let shelf = div()
        .w_full()
        .rounded(radius::POPOVER)
        .bg(c.sunken)
        .border_1()
        .border_color(c.border)
        .overflow_hidden()
        .flex()
        .flex_col()
        .children(docs.iter().enumerate().map(|(idx, doc)| {
            let tint = doc.kind.tint(cx);
            let action = if doc.can_preview_in_panel {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S1)
                            .text_ui(TypeSize::Caption)
                            .medium()
                            .text_color(c.accent)
                            .child("Panel")
                            .child(Icon::new(IconName::PanelRight).size(IconSize::XSmall).color(c.accent)),
                    )
                    .child(Icon::new(IconName::External).size(IconSize::XSmall).color(c.text_faint))
            } else {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S1)
                    .text_ui(TypeSize::Caption)
                    .medium()
                    .text_color(c.accent)
                    .child("Open")
                    .child(Icon::new(IconName::External).size(IconSize::XSmall).color(c.accent))
            };

            div()
                .when(idx > 0, |d| d.border_t_1().border_color(c.border))
                .px(space::S3)
                .py(space::S2)
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            div()
                                .w(px(32.))
                                .h(px(20.))
                                .flex_none()
                                .rounded(radius::SMALL)
                                .bg(tint.opacity(0.12))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_ui(TypeSize::Caption)
                                .medium()
                                .text_color(tint)
                                .child(doc.kind.tag()),
                        )
                        .child(div().flex_none().text_ui(TypeSize::Small).medium().text_color(c.text).child(doc.name))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_mono(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child(format!("{}/", doc.folder)),
                        )
                        .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(doc.short_meta))
                        .child(action),
                )
                .child(
                    div()
                        .pl(px(40.))
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(doc.caption),
                )
        }))
        .into_any_element();
    thread_rail(IconName::File, "Showed 2 files", "docs/release/", None, shelf, cx)
}

fn thread_missing(items: &[MissingSample], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let shelf = div()
        .w_full()
        .rounded(radius::POPOVER)
        .bg(c.sunken)
        .border_1()
        .border_color(c.border)
        .overflow_hidden()
        .flex()
        .flex_col()
        .children(items.iter().enumerate().map(|(idx, item)| {
            let icon = if item.is_picture { IconName::Image } else { IconName::File };
            div()
                .when(idx > 0, |d| d.border_t_1().border_color(c.border))
                .px(space::S3)
                .py(space::S2)
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(
                            div()
                                .w(px(20.))
                                .flex_none()
                                .flex()
                                .justify_center()
                                .child(Icon::new(icon).size(IconSize::Nav).color(c.text_faint)),
                        )
                        .child(div().flex_none().text_ui(TypeSize::Small).text_color(c.text_muted).child(item.name))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_mono(TypeSize::Caption)
                                .text_color(c.text_faint)
                                .child(format!("{}/{}", item.folder, item.name)),
                        )
                        .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child("File not found")),
                )
                .when_some(item.caption, |d, caption| {
                    d.child(
                        div()
                            .pl(px(28.))
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_faint)
                            .child(caption),
                    )
                })
        }))
        .into_any_element();
    thread_rail(IconName::File, "Showed 2 files", "tmp/", None, shelf, cx)
}
