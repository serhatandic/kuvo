//! Set A: Three design drafts for Kuvo's unified image viewer (used by conversations, Outputs
//! popover thumbnails, the side panel, Work Queue attachments, and composer attachments).
//!
//! - [`ViewerDraft::Stage`]: Full-window dark gallery scrim with floating pills, edge chevrons,
//!   and a compact bottom filmstrip.
//! - [`ViewerDraft::Sheet`]: Theme-native Quick Look sheet floating over a dimmed workspace, with
//!   a structured header bar and zero chrome overlapping image pixels.
//! - [`ViewerDraft::Loupe`]: Full-bleed inspector with a slim named thumbnail rail (hidden on
//!   single images), zoom presets, alpha checkerboard toggle, and Zed-style metadata status bar.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Hsla, Image, IntoElement, ObjectFit, ParentElement, Render, Styled,
    StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px,
};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    theme::{radius, space},
};

use super::chrome::{
    SampleGallery, SampleImage, board_frame, checkerboard_image, keycap, kuvo_window_shell,
    review_board,
};

/// Which of the three unified viewer designs to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerDraft {
    Stage,
    Sheet,
    Loupe,
}

impl ViewerDraft {
    pub const ALL: [ViewerDraft; 3] = [ViewerDraft::Stage, ViewerDraft::Sheet, ViewerDraft::Loupe];

    pub fn name(self) -> &'static str {
        match self {
            Self::Stage => "Viewer A · Stage",
            Self::Sheet => "Viewer B · Sheet",
            Self::Loupe => "Viewer C · Loupe",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Stage => "Floating dark gallery with edge navigation and bottom filmstrip",
            Self::Sheet => "Theme-native Quick Look sheet with zero chrome over the image",
            Self::Loupe => "Full-bleed inspector with named thumbnail rail and status bar",
        }
    }

    pub fn slug_multi(self) -> &'static str {
        match self {
            Self::Stage => "draft-viewer-stage",
            Self::Sheet => "draft-viewer-sheet",
            Self::Loupe => "draft-viewer-loupe",
        }
    }

    pub fn slug_cases(self) -> &'static str {
        match self {
            Self::Stage => "draft-viewer-stage-cases",
            Self::Sheet => "draft-viewer-sheet-cases",
            Self::Loupe => "draft-viewer-loupe-cases",
        }
    }
}

/// Which scenario the viewer is rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerMode {
    /// Full-window multi-image gallery (3 of 6) over the live Kuvo shell.
    Multi,
    /// Review board showing Single, Very Large (3840x5200), and Very Small (32x32) side-by-side.
    Cases,
}

/// Internal viewing case rendered by a single viewer component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerCase {
    Multi { index: usize, total_override: Option<usize> },
    Single,
    VeryLarge,
    VerySmall,
}

pub struct ImageViewerDraftView {
    draft: ViewerDraft,
    mode: ViewerMode,
    gallery: SampleGallery,
}

impl ImageViewerDraftView {
    pub fn new(draft: ViewerDraft, mode: ViewerMode) -> Self {
        Self { draft, mode, gallery: SampleGallery::build() }
    }
}

impl Render for ImageViewerDraftView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.mode {
            ViewerMode::Multi => {
                let thumbs: Vec<&SampleImage> = self.gallery.multi.iter().take(3).collect();
                let overlay = render_viewer(
                    self.draft,
                    ViewerCase::Multi { index: 2, total_override: None },
                    &self.gallery,
                    false,
                    cx,
                );
                kuvo_window_shell(self.draft.name(), false, true, &thumbs, Some(overlay), cx)
            }
            ViewerMode::Cases => {
                let single = render_viewer(self.draft, ViewerCase::Single, &self.gallery, true, cx);
                let large = render_viewer(self.draft, ViewerCase::VeryLarge, &self.gallery, true, cx);
                let small = render_viewer(self.draft, ViewerCase::VerySmall, &self.gallery, true, cx);

                let body = div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(space::S4)
                    .child(
                        div()
                            .h(px(404.))
                            .flex_none()
                            .child(board_frame(
                                "Single image · Conversation",
                                "Multi-image arrows, counter & filmstrip/rail hidden · Esc or backdrop closes",
                                single,
                                None,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .gap(space::S4)
                            .child(board_frame(
                                "Very large · 3840 × 5200 (6.4 MB)",
                                "Zoomed to 100% · Fit toggle & pan cue",
                                large,
                                None,
                                cx,
                            ))
                            .child(board_frame(
                                "Very small · 32 × 32 transparent PNG",
                                "Checkerboard matte · 400% pixel inspection + 1:1 toggle",
                                small,
                                None,
                                cx,
                            )),
                    )
                    .into_any_element();

                review_board(self.draft.name(), self.draft.subtitle(), body, cx)
            }
        }
    }
}

/// Renders any of the three unified viewer designs for a given case. Also reused by Set B
/// (`outputs_popover.rs`) to demonstrate opening an Outputs thumbnail directly in Viewer A.
pub fn render_viewer(
    draft: ViewerDraft,
    case: ViewerCase,
    gallery: &SampleGallery,
    compact: bool,
    cx: &App,
) -> AnyElement {
    match draft {
        ViewerDraft::Stage => render_stage(case, gallery, compact, cx),
        ViewerDraft::Sheet => render_sheet(case, gallery, compact, cx),
        ViewerDraft::Loupe => render_loupe(case, gallery, compact, cx),
    }
}

struct ActiveMeta<'a> {
    sample: &'a SampleImage,
    multi: Option<(usize, usize)>,
    zoom_label: &'static str,
    is_fit: bool,
    is_actual: bool,
    is_panning: bool,
    is_tiny: bool,
}

fn resolve_case<'a>(case: ViewerCase, gallery: &'a SampleGallery) -> ActiveMeta<'a> {
    match case {
        ViewerCase::Multi { index, total_override } => {
            let idx = index.min(gallery.multi.len().saturating_sub(1));
            ActiveMeta {
                sample: &gallery.multi[idx],
                multi: Some((idx, total_override.unwrap_or(gallery.multi.len()))),
                zoom_label: "Fit · 42%",
                is_fit: true,
                is_actual: false,
                is_panning: false,
                is_tiny: false,
            }
        }
        ViewerCase::Single => ActiveMeta {
            sample: &gallery.single,
            multi: None,
            zoom_label: "Fit · 64%",
            is_fit: true,
            is_actual: false,
            is_panning: false,
            is_tiny: false,
        },
        ViewerCase::VeryLarge => ActiveMeta {
            sample: &gallery.very_large,
            multi: None,
            zoom_label: "100%",
            is_fit: false,
            is_actual: true,
            is_panning: true,
            is_tiny: false,
        },
        ViewerCase::VerySmall => ActiveMeta {
            sample: &gallery.very_small,
            multi: None,
            zoom_label: "400%",
            is_fit: false,
            is_actual: false,
            is_panning: false,
            is_tiny: true,
        },
    }
}

// ===========================================================================
// Draft A1: Stage (Floating Dark Gallery)
// ===========================================================================

fn render_stage(case: ViewerCase, gallery: &SampleGallery, compact: bool, cx: &App) -> AnyElement {
    let meta = resolve_case(case, gallery);
    let dark = cx.theme().appearance.is_dark();
    let scrim = if dark { gpui::rgba(0x0B0C0EE8) } else { gpui::rgba(0x121318DE) };
    let pill_bg = gpui::rgba(0x1E2026EB);
    let pill_edge = gpui::white().opacity(0.14);
    let fg = gpui::white().opacity(0.94);
    let fg_muted = gpui::white().opacity(0.62);
    let fg_faint = gpui::white().opacity(0.42);
    let accent = cx.theme().colors.accent;
    let pad = if compact { space::S3 } else { space::S5 };

    let top_bar = div()
        .flex_none()
        .w_full()
        .px(pad)
        .pt(pad)
        .flex()
        .items_center()
        .justify_between()
        .gap(space::S3)
        // Left floating identity pill
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(space::S2)
                .px(space::S3)
                .h(px(34.))
                .rounded(radius::PILL)
                .bg(pill_bg)
                .border_1()
                .border_color(pill_edge)
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(radius::PILL)
                        .bg(gpui::white().opacity(0.10))
                        .text_ui(TypeSize::Caption)
                        .text_color(fg_muted)
                        .child(meta.sample.source_tag),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_ui(TypeSize::Small)
                        .medium()
                        .text_color(fg)
                        .child(meta.sample.name),
                )
                .when(!compact, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_ui(TypeSize::Caption)
                            .text_color(fg_faint)
                            .child(format!(
                                "{} × {} · {}",
                                meta.sample.natural_w, meta.sample.natural_h, meta.sample.size_label
                            )),
                    )
                }),
        )
        // Right floating actions + close pill
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(
                    div()
                        .h(px(34.))
                        .px(space::S1_5)
                        .rounded(radius::PILL)
                        .bg(pill_bg)
                        .border_1()
                        .border_color(pill_edge)
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .child(stage_icon_btn(IconName::Copy, fg_muted))
                        .child(stage_icon_btn(IconName::ArrowDown, fg_muted))
                        .child(stage_icon_btn(IconName::External, fg_muted)),
                )
                // Prominent Close button with Esc keycap
                .child(
                    div()
                        .h(px(34.))
                        .px(space::S2)
                        .rounded(radius::PILL)
                        .bg(pill_bg)
                        .border_1()
                        .border_color(gpui::white().opacity(0.22))
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(Icon::new(IconName::Close).size(IconSize::Nav).color(fg))
                        .when(!compact, |d| d.child(keycap("Esc", true, cx))),
                ),
        );

    let stage_canvas = div()
        .flex_1()
        .min_h_0()
        .w_full()
        .px(pad)
        .py(space::S3)
        .flex()
        .items_center()
        .justify_center()
        .gap(space::S4)
        // Left chevron (multi only)
        .when(meta.multi.is_some(), |d| d.child(stage_nav_button(IconName::ChevronLeft, fg)))
        // Main framed image
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(space::S2)
                .child(render_image_stage_item(&meta, true, compact, accent, cx))
                .when_some(meta.sample.caption.filter(|_| !compact), |d, cap| {
                    d.child(
                        div()
                            .max_w(px(640.))
                            .text_ui(TypeSize::Small)
                            .text_color(fg_muted)
                            .text_center()
                            .truncate()
                            .child(cap),
                    )
                }),
        )
        // Right chevron (multi only)
        .when(meta.multi.is_some(), |d| d.child(stage_nav_button(IconName::ChevronRight, fg)));

    let zoom_pill = div()
        .h(px(34.))
        .px(space::S2)
        .rounded(radius::PILL)
        .bg(pill_bg)
        .border_1()
        .border_color(pill_edge)
        .flex()
        .items_center()
        .gap(space::S1_5)
        .child(stage_icon_btn(IconName::Minus, fg_muted))
        .child(div().text_mono(TypeSize::Caption).text_color(fg).child(meta.zoom_label))
        .child(stage_icon_btn(IconName::Plus, fg_muted))
        .child(div().w(px(1.)).h(px(14.)).bg(pill_edge))
        .child(
            div()
                .px(space::S1_5)
                .py(px(2.))
                .rounded(radius::SMALL)
                .when(meta.is_actual, |d| d.bg(gpui::white().opacity(0.14)))
                .text_ui(TypeSize::Caption)
                .text_color(if meta.is_actual { fg } else { fg_muted })
                .child(if meta.is_tiny { "1:1" } else if meta.is_actual { "Fit" } else { "100%" }),
        );

    let bottom_dock = div()
        .flex_none()
        .w_full()
        .px(pad)
        .pb(pad)
        .flex()
        .items_center()
        .justify_center()
        .gap(space::S3)
        .when_some(meta.multi, |d, (active_idx, total)| {
            let thumb_w = if compact { px(34.) } else { px(52.) };
            let thumb_h = if compact { px(24.) } else { px(34.) };
            let visible: Vec<(usize, &SampleImage)> = if compact {
                let start = active_idx.saturating_sub(1).min(gallery.multi.len().saturating_sub(3));
                gallery.multi.iter().enumerate().skip(start).take(3).collect()
            } else {
                gallery.multi.iter().enumerate().collect()
            };
            d.child(
                div()
                    .px(space::S2)
                    .py(space::S1_5)
                    .rounded(radius::POPOVER)
                    .bg(pill_bg)
                    .border_1()
                    .border_color(pill_edge)
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .children(visible.into_iter().map(|(i, item)| {
                        let selected = i == active_idx;
                        div()
                            .w(thumb_w)
                            .h(thumb_h)
                            .rounded(radius::SMALL)
                            .overflow_hidden()
                            .border_2()
                            .border_color(if selected { accent } else { gpui::white().opacity(0.12) })
                            .opacity(if selected { 1.0 } else { 0.62 })
                            .child(img(item.image.clone()).size_full().object_fit(ObjectFit::Cover))
                    }))
                    .child(
                        div()
                            .pl(space::S1_5)
                            .pr(space::S1)
                            .text_mono(TypeSize::Caption)
                            .text_color(fg_muted)
                            .child(format!("{} / {}", active_idx + 1, total)),
                    ),
            )
        })
        .child(zoom_pill);

    div()
        .absolute()
        .inset_0()
        .size_full()
        .bg(scrim)
        .flex()
        .flex_col()
        .child(top_bar)
        .child(stage_canvas)
        .child(bottom_dock)
        .into_any_element()
}

fn stage_icon_btn(icon: IconName, color: Hsla) -> AnyElement {
    div()
        .size(px(26.))
        .rounded(radius::PILL)
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size(IconSize::Nav).color(color))
        .into_any_element()
}

fn stage_nav_button(icon: IconName, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .size(px(38.))
        .rounded(radius::PILL)
        .bg(gpui::rgba(0x1E2026EB))
        .border_1()
        .border_color(gpui::white().opacity(0.16))
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size(IconSize::Medium).color(color))
        .into_any_element()
}

// ===========================================================================
// Draft A2: Sheet (Theme-Native Quick Look Sheet)
// ===========================================================================

fn render_sheet(case: ViewerCase, gallery: &SampleGallery, compact: bool, cx: &App) -> AnyElement {
    let meta = resolve_case(case, gallery);
    let theme = cx.theme();
    let c = &theme.colors;
    let dark = theme.appearance.is_dark();
    let backdrop = if dark { gpui::black().opacity(0.62) } else { gpui::black().opacity(0.32) };
    let outer_pad = if compact { space::S3 } else { px(34.) };

    let header = div()
        .flex_none()
        .h(px(44.))
        .px(space::S3)
        .bg(c.elevated)
        .border_b_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .justify_between()
        .gap(space::S3)
        // Left: file identity + dimensions
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(Icon::new(IconName::Image).size(IconSize::Nav).color(c.icon))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_ui(TypeSize::Small)
                        .medium()
                        .text_color(c.text)
                        .child(meta.sample.name),
                )
                .when(!compact, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_faint)
                            .child(format!(
                                "{} × {} · {}",
                                meta.sample.natural_w, meta.sample.natural_h, meta.sample.size_label
                            )),
                    )
                }),
        )
        // Center: segmented stepper (only when multiple images)
        .when_some(meta.multi, |d, (active_idx, total)| {
            d.child(
                div()
                    .flex_none()
                    .h(px(28.))
                    .px(px(2.))
                    .rounded(radius::NAV)
                    .bg(c.sunken)
                    .border_1()
                    .border_color(c.border)
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        div()
                            .size(px(24.))
                            .rounded(radius::SMALL)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::ChevronLeft).size(IconSize::Nav).color(c.text_muted)),
                    )
                    .child(
                        div()
                            .px(space::S1_5)
                            .text_mono(TypeSize::Caption)
                            .text_color(c.text)
                            .child(format!("{} of {}", active_idx + 1, total)),
                    )
                    .child(
                        div()
                            .size(px(24.))
                            .rounded(radius::SMALL)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::ChevronRight).size(IconSize::Nav).color(c.text_muted)),
                    ),
            )
        })
        // Right: zoom pill, actions, close
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(space::S1_5)
                .child(
                    div()
                        .h(px(28.))
                        .px(space::S1_5)
                        .rounded(radius::NAV)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .child(Icon::new(IconName::Minus).size(IconSize::XSmall).color(c.icon))
                        .child(div().px(px(2.)).text_mono(TypeSize::Caption).text_color(c.text).child(meta.zoom_label))
                        .child(Icon::new(IconName::Plus).size(IconSize::XSmall).color(c.icon)),
                )
                .when(!compact, |d| {
                    d.child(div().w(px(1.)).h(px(16.)).bg(c.border))
                        .child(sheet_icon_btn(IconName::Copy, c.icon))
                        .child(sheet_icon_btn(IconName::ArrowDown, c.icon))
                        .child(sheet_icon_btn(IconName::External, c.icon))
                })
                .child(div().w(px(1.)).h(px(16.)).bg(c.border))
                .child(
                    div()
                        .h(px(28.))
                        .px(space::S2)
                        .rounded(radius::NAV)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border_strong)
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .child(Icon::new(IconName::Close).size(IconSize::Nav).color(c.text))
                        .when(!compact, |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child("Esc"))),
                ),
        );

    let canvas = div()
        .flex_1()
        .min_h_0()
        .w_full()
        .bg(c.sunken)
        .p(if compact { space::S3 } else { space::S6 })
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .child(render_image_stage_item(&meta, dark, compact, c.accent, cx));

    let footer = div()
        .flex_none()
        .h(px(34.))
        .px(space::S4)
        .bg(c.elevated)
        .border_t_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .justify_between()
        .gap(space::S3)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(radius::SMALL)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(meta.sample.source_tag),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(meta.sample.caption.unwrap_or(meta.sample.name)),
                ),
        )
        .when(!compact, |d| {
            d.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .when(meta.multi.is_some(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(keycap("←", false, cx))
                                .child(keycap("→", false, cx))
                                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Navigate")),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(keycap("⌘0", false, cx))
                            .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Fit")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(keycap("Esc", false, cx))
                            .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Close")),
                    ),
            )
        });

    div()
        .absolute()
        .inset_0()
        .size_full()
        .bg(backdrop)
        .p(outer_pad)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .size_full()
                .rounded(radius::CARD)
                .overflow_hidden()
                .bg(c.surface)
                .border_1()
                .border_color(c.border_strong)
                .shadow(theme.elevated_shadow())
                .flex()
                .flex_col()
                .child(header)
                .child(canvas)
                .child(footer),
        )
        .into_any_element()
}

fn sheet_icon_btn(icon: IconName, color: Hsla) -> AnyElement {
    div()
        .size(px(28.))
        .rounded(radius::NAV)
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size(IconSize::Nav).color(color))
        .into_any_element()
}

// ===========================================================================
// Draft A3: Loupe (Full-Bleed Inspector with Named Thumbnail Rail)
// ===========================================================================

fn render_loupe(case: ViewerCase, gallery: &SampleGallery, compact: bool, cx: &App) -> AnyElement {
    let meta = resolve_case(case, gallery);
    let theme = cx.theme();
    let c = &theme.colors;
    let dark = theme.appearance.is_dark();

    let zoom_preset = |label: &'static str, active: bool| {
        div()
            .px(space::S1_5)
            .py(px(2.))
            .rounded(radius::SMALL)
            .when(active, |d| d.bg(c.selected_card).border_1().border_color(c.selected_edge))
            .text_mono(TypeSize::Caption)
            .text_color(if active { c.text } else { c.text_muted })
            .child(label)
    };

    let toolbar = div()
        .flex_none()
        .h(px(42.))
        .px(space::S3)
        .bg(c.sidebar)
        .border_b_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .justify_between()
        .gap(space::S3)
        // Left: source + filename
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(radius::SMALL)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(meta.sample.source_tag),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_ui(TypeSize::Small)
                        .medium()
                        .text_color(c.text)
                        .child(meta.sample.name),
                )
                .when_some(meta.multi, |d, (idx, total)| {
                    d.child(
                        div()
                            .flex_none()
                            .text_mono(TypeSize::Caption)
                            .text_color(c.text_faint)
                            .child(format!("{} / {}", idx + 1, total)),
                    )
                }),
        )
        // Center: zoom presets (Fit / 1:1 / 4×)
        .child(
            div()
                .flex_none()
                .h(px(28.))
                .px(px(3.))
                .rounded(radius::NAV)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .gap(px(2.))
                .child(zoom_preset("Fit", meta.is_fit))
                .child(zoom_preset("1:1", meta.is_actual))
                .child(zoom_preset(if meta.is_tiny { "4×" } else { "2×" }, meta.is_tiny)),
        )
        // Right: actions + Close
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(space::S1)
                .when(!compact, |d| {
                    d.child(sheet_icon_btn(IconName::Copy, c.icon))
                        .child(sheet_icon_btn(IconName::ArrowDown, c.icon))
                        .child(sheet_icon_btn(IconName::External, c.icon))
                        .child(div().w(px(1.)).h(px(16.)).mx(space::S1).bg(c.border))
                })
                .child(
                    div()
                        .h(px(28.))
                        .px(space::S2)
                        .rounded(radius::NAV)
                        .bg(c.elevated)
                        .border_1()
                        .border_color(c.border_strong)
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .child(Icon::new(IconName::Close).size(IconSize::Nav).color(c.text))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child("Close")),
                ),
        );

    let left_rail = meta.multi.map(|(active_idx, total)| {
        div()
            .w(px(138.))
            .flex_none()
            .h_full()
            .bg(c.sidebar)
            .border_r_1()
            .border_color(c.border)
            .p(space::S2)
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(
                div()
                    .px(space::S1)
                    .pb(px(2.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_ui(TypeSize::Caption)
                    .text_color(c.text_faint)
                    .child("Gallery")
                    .child(format!("{total}")),
            )
            .children(gallery.multi.iter().enumerate().map(|(i, item)| {
                let selected = i == active_idx;
                div()
                    .p(space::S1)
                    .rounded(radius::NAV)
                    .when(selected, |d| {
                        d.bg(c.selected_card)
                            .border_1()
                            .border_color(c.accent)
                            .shadow(theme.selected_shadow())
                    })
                    .when(!selected, |d| d.border_1().border_color(gpui::transparent_black()))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .w_full()
                            .h(px(64.))
                            .rounded(radius::SMALL)
                            .overflow_hidden()
                            .border_1()
                            .border_color(c.border)
                            .child(img(item.image.clone()).size_full().object_fit(ObjectFit::Cover)),
                    )
                    .child(
                        div()
                            .px(px(2.))
                            .text_ui(TypeSize::Caption)
                            .text_color(if selected { c.text } else { c.text_muted })
                            .truncate()
                            .child(item.short_label),
                    )
            }))
    });

    let body = div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .children(left_rail)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(c.background)
                .p(if compact { space::S3 } else { space::S6 })
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(render_image_stage_item(&meta, dark, compact, c.accent, cx)),
        );

    let status_bar = div()
        .flex_none()
        .h(px(28.))
        .px(space::S3)
        .bg(c.sidebar)
        .border_t_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .justify_between()
        .gap(space::S3)
        .text_ui(TypeSize::Caption)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(c.text_muted)
                .child(meta.sample.caption.unwrap_or(meta.sample.name)),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(space::S3)
                .text_mono(TypeSize::Caption)
                .text_color(c.text_faint)
                .child(format!(
                    "{} · {} × {} · {}",
                    meta.sample.format_label,
                    meta.sample.natural_w,
                    meta.sample.natural_h,
                    meta.sample.size_label
                ))
                .when(!compact, |d| d.child("←/→  +/−  0 Fit  Esc")),
        );

    div()
        .absolute()
        .inset_0()
        .size_full()
        .bg(c.background)
        .flex()
        .flex_col()
        .child(toolbar)
        .child(body)
        .child(status_bar)
        .into_any_element()
}

// ===========================================================================
// Shared Stage Item Renderer (handles normal, very large 100%, very small 32x32)
// ===========================================================================

fn render_image_stage_item(
    meta: &ActiveMeta<'_>,
    dark_stage: bool,
    compact: bool,
    accent: Hsla,
    cx: &App,
) -> AnyElement {
    let c = &cx.theme().colors;
    let border = if dark_stage { gpui::white().opacity(0.14) } else { c.border_strong };

    if meta.is_tiny {
        // Very small 32x32 transparent PNG: stage inside a calm checkerboard matte card showing
        // crisp 400% magnified preview beside an actual-size 1:1 (32x32) reference swatch!
        let checker: Arc<Image> = checkerboard_image(dark_stage);
        let card_bg: Hsla = if dark_stage { gpui::rgba(0x1B1D22FF).into() } else { c.elevated };
        let label_fg = if dark_stage { gpui::white().opacity(0.75) } else { c.text_muted };
        let faint_fg = if dark_stage { gpui::white().opacity(0.48) } else { c.text_faint };
        let zoom_px = if compact { px(128.) } else { px(160.) };
        return div()
            .flex()
            .items_center()
            .gap(space::S5)
            // Primary 400% checkerboard loupe card
            .child(
                div()
                    .p(space::S3)
                    .rounded(radius::CARD)
                    .bg(card_bg)
                    .border_1()
                    .border_color(border)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .relative()
                            .size(zoom_px)
                            .rounded(radius::POPOVER)
                            .overflow_hidden()
                            .border_1()
                            .border_color(border)
                            .child(img(checker.clone()).size_full().object_fit(ObjectFit::Cover))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        img(meta.sample.image.clone())
                                            .size(px(if compact { 96. } else { 128. }))
                                            .object_fit(ObjectFit::Contain),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .text_mono(TypeSize::Caption)
                            .text_color(label_fg)
                            .child("400% · 32 × 32 px"),
                    ),
            )
            // Actual-size 1:1 swatch beside it for instant scale reality check
            .child(
                div()
                    .p(space::S3)
                    .rounded(radius::POPOVER)
                    .bg(card_bg)
                    .border_1()
                    .border_color(border)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .relative()
                            .size(px(56.))
                            .rounded(radius::NAV)
                            .overflow_hidden()
                            .border_1()
                            .border_color(border)
                            .child(img(checker).size_full().object_fit(ObjectFit::Cover))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(img(meta.sample.image.clone()).size(px(32.)).object_fit(ObjectFit::Contain)),
                            ),
                    )
                    .child(div().text_mono(TypeSize::Caption).text_color(faint_fg).child("Actual 1:1")),
            )
            .into_any_element();
    }

    if meta.is_panning {
        // Very large 3840x5200 image at 100% zoom: edge-to-edge cropped viewport with a subtle
        // corner pan/minimap badge showing where the viewport sits inside the full 5K sheet.
        let (w, h) = if compact { (px(572.), px(284.)) } else { (px(880.), px(520.)) };
        return div()
            .relative()
            .w(w)
            .h(h)
            .max_w_full()
            .max_h_full()
            .rounded(radius::NAV)
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .child(img(meta.sample.image.clone()).size_full().object_fit(ObjectFit::Cover))
            // Subtle bottom-right minimap showing viewport region of 3840x5200
            .child(
                div()
                    .absolute()
                    .bottom(space::S2)
                    .right(space::S2)
                    .p(px(6.))
                    .rounded(radius::NAV)
                    .bg(gpui::rgba(0x14161CE6))
                    .border_1()
                    .border_color(gpui::white().opacity(0.18))
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        // Mini aspect rectangle with active viewport box
                        div()
                            .relative()
                            .w(px(28.))
                            .h(px(38.))
                            .rounded(px(3.))
                            .bg(gpui::white().opacity(0.12))
                            .border_1()
                            .border_color(gpui::white().opacity(0.20))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(5.))
                                    .left(px(4.))
                                    .w(px(16.))
                                    .h(px(11.))
                                    .rounded(px(2.))
                                    .border_1()
                                    .border_color(accent)
                                    .bg(accent.opacity(0.28)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(div().text_mono(TypeSize::Caption).text_color(gpui::white().opacity(0.92)).child("100% · Pan"))
                            .child(div().text_mono(TypeSize::Caption).text_color(gpui::white().opacity(0.56)).child("3840 × 5200")),
                    ),
            )
            .into_any_element();
    }

    // Standard fitted screenshot (multi or single)
    let (w, h) = if compact { (px(440.), px(268.)) } else { (px(780.), px(492.)) };
    div()
        .w(w)
        .h(h)
        .max_w_full()
        .max_h_full()
        .rounded(radius::POPOVER)
        .overflow_hidden()
        .border_1()
        .border_color(border)
        .child(img(meta.sample.image.clone()).size_full().object_fit(ObjectFit::Contain))
        .into_any_element()
}
