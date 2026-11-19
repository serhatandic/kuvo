//! Set B: Two design drafts for the conversation header's Outputs / Activity popover.
//!
//! Both drafts preserve the agreed Codex structure (Outputs, Subagents, Background processes,
//! plus Schedules) while fixing all four points of user feedback:
//! 1. Clear hierarchy, zero empty-body titles, and unmistakable Wake-ups vs Watches.
//! 2. Image thumbnails open full-screen in the unified image viewer (Set A), not inside the popover.
//! 3. Hundreds of finished subagents/sessions collapse to one summary row with a dedicated
//!    searchable/filterable drill-down instead of listing hundreds inline.
//! 4. Every "+N more" / kind control opens the full Outputs list pre-filtered to that exact kind
//!    (Images -> thumbnail grid, Documents -> .md only, Data -> .csv only, Code -> .json/.ts only).

use gpui::{
    AnyElement, App, Context, Hsla, IntoElement, ObjectFit, ParentElement, Render, Styled,
    StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px,
};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    icon::{Mark, provider_mark, provider_mark_color},
    small::status_dot,
    theme::{radius, space},
};

use super::chrome::{SampleGallery, SampleImage, board_frame, kuvo_window_shell, review_board};
use super::image_viewer::{ViewerCase, ViewerDraft, render_viewer};

/// Which of the two Outputs popover designs to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopoverDraft {
    /// Draft B1: Faithful Codex single-column elevated popover with kind-scoped drill-downs.
    CodexQuiet,
    /// Draft B2: Soft-tinted shell with quiet grouped cards, inline kind-filter chips, and
    /// explicit Wake-ups vs Watches sub-groups.
    GroupedCards,
}

impl PopoverDraft {
    pub const ALL: [PopoverDraft; 2] = [PopoverDraft::CodexQuiet, PopoverDraft::GroupedCards];

    pub fn name(self) -> &'static str {
        match self {
            Self::CodexQuiet => "Popover B1 · Codex Quiet",
            Self::GroupedCards => "Popover B2 · Grouped Cards",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Self::CodexQuiet => "Single-surface Codex hierarchy · collapsed finished sessions · kind-scoped drill-downs",
            Self::GroupedCards => "Quiet pillar cards · one-click kind chips in Outputs · explicit Wake-ups vs Watches",
        }
    }

    pub fn slug_main(self) -> &'static str {
        match self {
            Self::CodexQuiet => "draft-popover-codex",
            Self::GroupedCards => "draft-popover-cards",
        }
    }

    pub fn slug_flows(self) -> &'static str {
        match self {
            Self::CodexQuiet => "draft-popover-codex-flows",
            Self::GroupedCards => "draft-popover-cards-flows",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopoverMode {
    /// Main popover anchored under the Activity button in the Kuvo desktop window.
    Main,
    /// 3-panel review board showing where each "+N more" / disclosure leads + Viewer A handoff.
    Flows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputKind {
    All,
    Images,
    Docs,
    Data,
    Code,
}

struct FileItem {
    name: &'static str,
    ext: &'static str,
    meta: &'static str,
    icon: IconName,
}

struct SubagentItem {
    title: &'static str,
    provider: &'static str,
    status: &'static str,
    asking: bool,
    failed: bool,
    stopped: bool,
    when: &'static str,
}

struct ProcessItem {
    command: &'static str,
    uptime: &'static str,
}

struct ScheduleItem {
    summary: &'static str,
    timing: &'static str,
    is_watch: bool,
}

const RECENT_FILES: &[FileItem] = &[
    FileItem { name: "morning-report-1120.md", ext: "MD", meta: "14 KB · 4m ago", icon: IconName::File },
    FileItem { name: "sysload.csv", ext: "CSV", meta: "48 KB · 6m ago", icon: IconName::Chart },
    FileItem { name: "848-decisions.md", ext: "MD", meta: "9 KB · 18m ago", icon: IconName::File },
];

const DOC_FILES: &[FileItem] = &[
    FileItem { name: "morning-report-1120.md", ext: "MD", meta: "14 KB · 4m ago", icon: IconName::File },
    FileItem { name: "848-decisions.md", ext: "MD", meta: "9 KB · 18m ago", icon: IconName::File },
    FileItem { name: "beta35-rehearsal-notes.md", ext: "MD", meta: "22 KB · 41m ago", icon: IconName::File },
    FileItem { name: "PLAN.md", ext: "MD", meta: "6 KB · 1h ago", icon: IconName::File },
    FileItem { name: "feedback-review-ranking.md", ext: "MD", meta: "11 KB · 2h ago", icon: IconName::File },
    FileItem { name: "machine-scenarios-1118.md", ext: "MD", meta: "18 KB · 3h ago", icon: IconName::File },
    FileItem { name: "brief-template.md", ext: "MD", meta: "4 KB · Yesterday", icon: IconName::File },
    FileItem { name: "release-checklist.md", ext: "MD", meta: "7 KB · Yesterday", icon: IconName::File },
];

const DATA_FILES: &[FileItem] = &[
    FileItem { name: "sysload.csv", ext: "CSV", meta: "48 KB · 420 rows · 6m ago", icon: IconName::Chart },
    FileItem { name: "latency-p99-mini.csv", ext: "CSV", meta: "19 KB · 180 rows · 35m ago", icon: IconName::Chart },
    FileItem { name: "token-spend-7d.csv", ext: "CSV", meta: "12 KB · 96 rows · 2h ago", icon: IconName::Chart },
];

const ACTIVE_SUBAGENTS: &[SubagentItem] = &[
    SubagentItem { title: "Verify beta.35 update rehearsal", provider: "codex-cli", status: "Waiting for you", asking: true, failed: false, stopped: false, when: "2m" },
    SubagentItem { title: "Run browser checks for viewer", provider: "claude-code", status: "Working", asking: false, failed: false, stopped: false, when: "4m" },
    SubagentItem { title: "Compare dark-mode contrast", provider: "codex-cli", status: "Working", asking: false, failed: false, stopped: false, when: "7m" },
];

const FINISHED_SUBAGENTS: &[SubagentItem] = &[
    SubagentItem { title: "beta.35 integration: merge four lines", provider: "claude-code", status: "Finished", asking: false, failed: false, stopped: false, when: "18m ago" },
    SubagentItem { title: "Whole-picture review: #848 + #774", provider: "codex-cli", status: "Finished", asking: false, failed: false, stopped: false, when: "42m ago" },
    SubagentItem { title: "Check flaky bridge reconnect timing", provider: "claude-code", status: "Failed", asking: false, failed: true, stopped: false, when: "1h ago" },
    SubagentItem { title: "Faster combined check + no leaked procs", provider: "codex-cli", status: "Finished", asking: false, failed: false, stopped: false, when: "2h ago" },
    SubagentItem { title: "Daily check 11-19: races + #900 A/B", provider: "claude-code", status: "Stopped", asking: false, failed: false, stopped: true, when: "3h ago" },
    SubagentItem { title: "#900 Batch C groundwork: file info", provider: "codex-cli", status: "Finished", asking: false, failed: false, stopped: false, when: "4h ago" },
    SubagentItem { title: "#848 chunk 4 check: returning files", provider: "claude-code", status: "Finished", asking: false, failed: false, stopped: false, when: "5h ago" },
];

const PROCESSES: &[ProcessItem] = &[
    ProcessItem { command: "npm run dev", uptime: "12m" },
    ProcessItem { command: "npx vitest --watch", uptime: "4m" },
];

const SCHEDULES: &[ScheduleItem] = &[
    ScheduleItem { summary: "Blind review comparison: Opus vs Astra", timing: "In 1 day · Once", is_watch: false },
    ScheduleItem { summary: "Weekly upstream check (opencode, OpenUsage)", timing: "In 3 days · Every week", is_watch: false },
    ScheduleItem { summary: "Mini system load check", timing: "Every 5 min · Next in 4m", is_watch: true },
    ScheduleItem { summary: "Alert if Mini disk < 60 GB free", timing: "Every 5 min · Next in 4m", is_watch: true },
];

pub struct OutputsPopoverDraftView {
    draft: PopoverDraft,
    mode: PopoverMode,
    gallery: SampleGallery,
}

impl OutputsPopoverDraftView {
    pub fn new(draft: PopoverDraft, mode: PopoverMode) -> Self {
        Self { draft, mode, gallery: SampleGallery::build() }
    }
}

impl Render for OutputsPopoverDraftView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.mode {
            PopoverMode::Main => {
                let thumbs: Vec<&SampleImage> = self.gallery.multi.iter().take(3).collect();
                let popover = match self.draft {
                    PopoverDraft::CodexQuiet => render_b1_main(&self.gallery, cx),
                    PopoverDraft::GroupedCards => render_b2_main(&self.gallery, cx),
                };
                // Anchor popover cleanly below the top-right Activity trigger button
                let overlay = div()
                    .absolute()
                    .top(px(48.))
                    .right(px(46.))
                    .child(popover)
                    .into_any_element();
                kuvo_window_shell(self.draft.name(), true, true, &thumbs, Some(overlay), cx)
            }
            PopoverMode::Flows => match self.draft {
                PopoverDraft::CodexQuiet => {
                    let images_view = center_popover_in_well(
                        render_drilldown_images(false, &self.gallery, cx),
                        cx,
                    );
                    let docs_view = center_popover_in_well(
                        render_drilldown_files(false, OutputKind::Docs, DOC_FILES, 10, cx),
                        cx,
                    );
                    let finished_view = center_popover_in_well(
                        render_drilldown_sessions(false, cx),
                        cx,
                    );
                    let body = div()
                        .size_full()
                        .flex()
                        .gap(space::S4)
                        .child(board_frame(
                            "From “+182 images”",
                            "Pre-filtered to Images · 3-col grid · opens Viewer A",
                            images_view,
                            None,
                            cx,
                        ))
                        .child(board_frame(
                            "From “+8 docs”",
                            "Pre-filtered to Documents · .md only, zero PNGs",
                            docs_view,
                            None,
                            cx,
                        ))
                        .child(board_frame(
                            "From “142 finished ›”",
                            "Searchable & filterable history · never dumped inline",
                            finished_view,
                            None,
                            cx,
                        ))
                        .into_any_element();
                    review_board(
                        "Popover B1 · Codex Quiet — Drill-Downs",
                        "Each “+N more” lands directly in its filtered kind; finished sessions stay out of the main popover",
                        body,
                        cx,
                    )
                }
                PopoverDraft::GroupedCards => {
                    let images_view = center_popover_in_well(
                        render_drilldown_images(true, &self.gallery, cx),
                        cx,
                    );
                    let data_view = center_popover_in_well(
                        render_drilldown_files(true, OutputKind::Data, DATA_FILES, 3, cx),
                        cx,
                    );
                    // Panel 3: clicking any thumbnail opens full-screen in unified Viewer A!
                    let viewer_handoff = render_viewer(
                        ViewerDraft::Stage,
                        ViewerCase::Multi { index: 2, total_override: Some(185) },
                        &self.gallery,
                        true,
                        cx,
                    );
                    let body = div()
                        .size_full()
                        .flex()
                        .gap(space::S4)
                        .child(board_frame(
                            "From “Images 185” / “+182”",
                            "Filtered visual gallery · zero .md/.csv clutter",
                            images_view,
                            Some(px(416.)),
                            cx,
                        ))
                        .child(board_frame(
                            "From “Data 3” chip",
                            "Filtered to CSV/data outputs only",
                            data_view,
                            Some(px(416.)),
                            cx,
                        ))
                        .child(board_frame(
                            "Click any image → Viewer A",
                            "Full-screen unified viewer (Outputs · 3 / 185)",
                            viewer_handoff,
                            None,
                            cx,
                        ))
                        .into_any_element();
                    review_board(
                        "Popover B2 · Grouped Cards — Kind Filters & Viewer A Handoff",
                        "Kind chips jump straight to Images or Data; tapping any thumbnail opens full-screen in Viewer A",
                        body,
                        cx,
                    )
                }
            },
        }
    }
}

fn center_popover_in_well(popover: AnyElement, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .size_full()
        .bg(c.sunken)
        .p(space::S4)
        .flex()
        .items_start()
        .justify_center()
        .overflow_hidden()
        .child(popover)
        .into_any_element()
}

// ===========================================================================
// Draft B1: Codex Quiet (Single-Surface Elevated Popover)
// ===========================================================================

fn render_b1_main(gallery: &SampleGallery, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let c = &theme.colors;

    // 1. Outputs section
    let outputs_section = div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S2)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Outputs"))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("200")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .text_ui(TypeSize::Caption)
                        .text_color(c.accent)
                        .child("All")
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.accent)),
                ),
        )
        // Image strip (3 thumbnails + "+182" tile)
        .child(media_strip(gallery, 90., 56., cx))
        // Recent files (capped at 3)
        .child(
            div()
                .flex()
                .flex_col()
                .children(RECENT_FILES.iter().map(|file| compact_file_row(file, cx))),
        )
        // Kind-specific "+N more" links (never dumps into an unfiltered mixed list!)
        .child(
            div()
                .px(space::S2)
                .pt(px(2.))
                .flex()
                .items_center()
                .gap(space::S1_5)
                .child(kind_more_pill("+8 docs", IconName::File, cx))
                .child(kind_more_pill("+2 data", IconName::Chart, cx))
                .child(kind_more_pill("+2 code", IconName::Terminal, cx)),
        );

    // 2. Subagents section (Codex structure: active rows + ONE collapsed finished row)
    let subagents_section = div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S2)
                .pb(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Subagents"))
                        .child(
                            div()
                                .text_ui(TypeSize::Caption)
                                .text_color(c.attention)
                                .child("2 working · 1 waiting"),
                        ),
                )
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Stop all")),
        )
        .children(ACTIVE_SUBAGENTS.iter().map(|sub| subagent_row(sub, true, cx)))
        // Single collapsed disclosure row for the 142 finished sessions!
        .child(
            div()
                .mt(px(2.))
                .h(px(28.))
                .px(space::S2)
                .rounded(radius::NAV)
                .bg(c.sunken)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(Icon::new(IconName::Archive).size(IconSize::XSmall).color(c.text_faint))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child("142 finished sessions")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("1 failed"))
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.text_faint)),
                ),
        );

    // 3. Background processes section
    let processes_section = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S2)
                .pb(px(2.))
                .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Background processes"))
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("2 running")),
        )
        .children(PROCESSES.iter().map(|proc| process_row(proc, cx)));

    // 4. Schedules section (crystal-clear Wake-ups vs Watches, zero orphan status headers!)
    let schedules_section = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S2)
                .pb(px(2.))
                .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Wake-ups & Watches"))
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("2 wake-ups · 2 watches")),
        )
        .children(SCHEDULES.iter().map(|item| schedule_row_b1(item, cx)));

    div()
        .w(px(392.))
        .p(space::S3)
        .rounded(radius::POPOVER)
        .bg(c.elevated)
        .border_1()
        .border_color(c.border)
        .shadow(theme.elevated_shadow())
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .flex()
        .flex_col()
        .child(outputs_section)
        .child(section_divider(cx))
        .child(subagents_section)
        .child(section_divider(cx))
        .child(processes_section)
        .child(section_divider(cx))
        .child(schedules_section)
        .into_any_element()
}

// ===========================================================================
// Draft B2: Grouped Cards (Soft Shell + Pillar Cards + Kind Chips)
// ===========================================================================

fn render_b2_main(gallery: &SampleGallery, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let c = &theme.colors;
    let shell_bg = if theme.appearance.is_dark() { c.background } else { c.sidebar };

    // 1. Outputs card with top kind-count chips
    let outputs_card = pillar_card(cx)
        .gap(space::S2)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S1)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Outputs"))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("200")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .text_ui(TypeSize::Caption)
                        .text_color(c.accent)
                        .child("Browse all")
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.accent)),
                ),
        )
        // One-click kind-filter chips right at the top of Outputs
        .child(
            div()
                .px(space::S1)
                .flex()
                .items_center()
                .gap(space::S1)
                .child(kind_count_chip(IconName::Image, "Images", "185", true, cx))
                .child(kind_count_chip(IconName::File, "Docs", "10", false, cx))
                .child(kind_count_chip(IconName::Chart, "Data", "3", false, cx))
                .child(kind_count_chip(IconName::Terminal, "Code", "2", false, cx)),
        )
        // Image strip (thumbnails open Viewer A; "+182" opens filtered Images grid)
        .child(media_strip(gallery, 88., 54., cx))
        // Recent non-image files (3 rows)
        .child(
            div()
                .border_t_1()
                .border_color(c.border)
                .pt(space::S1)
                .flex()
                .flex_col()
                .children(RECENT_FILES.iter().map(|file| compact_file_row(file, cx))),
        );

    // 2. Subagents card
    let subagents_card = pillar_card(cx)
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S1)
                .pb(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Subagents"))
                        .child(
                            div()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(radius::PILL)
                                .bg(c.selection)
                                .text_ui(TypeSize::Caption)
                                .text_color(c.accent)
                                .child("3 active"),
                        ),
                )
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Stop all")),
        )
        .children(ACTIVE_SUBAGENTS.iter().map(|sub| subagent_row(sub, true, cx)))
        .child(
            div()
                .mt(space::S1)
                .pt(space::S1_5)
                .px(space::S1)
                .border_t_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child("142 finished"))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("138 done · 3 stopped · 1 failed")),
                )
                .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.text_faint)),
        );

    // 3. Background processes card
    let processes_card = pillar_card(cx)
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S1)
                .pb(px(2.))
                .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Background processes"))
                .child(div().text_ui(TypeSize::Caption).text_color(c.success).child("2 running")),
        )
        .children(PROCESSES.iter().map(|proc| process_row(proc, cx)));

    // 4. Wake-ups & Watches card (explicitly split into Wake-ups and Watches sub-groups)
    let wakeups: Vec<&ScheduleItem> = SCHEDULES.iter().filter(|s| !s.is_watch).collect();
    let watches: Vec<&ScheduleItem> = SCHEDULES.iter().filter(|s| s.is_watch).collect();

    let schedules_card = pillar_card(cx)
        .gap(space::S2)
        // Wake-ups group
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .px(space::S1)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S1_5)
                                .child(Icon::new(IconName::Clock).size(IconSize::XSmall).color(c.accent))
                                .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text).child("Wake-ups"))
                                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Scheduled prompts")),
                        )
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(wakeups.len().to_string())),
                )
                .children(wakeups.into_iter().map(|item| schedule_row_b2(item, cx))),
        )
        // Watches group
        .child(
            div()
                .pt(space::S2)
                .border_t_1()
                .border_color(c.border)
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .px(space::S1)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S1_5)
                                .child(Icon::new(IconName::Eye).size(IconSize::XSmall).color(c.success))
                                .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text).child("Watches"))
                                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Recurring checks")),
                        )
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(watches.len().to_string())),
                )
                .children(watches.into_iter().map(|item| schedule_row_b2(item, cx))),
        );

    div()
        .w(px(406.))
        .p(space::S2)
        .rounded(radius::POPOVER)
        .bg(shell_bg)
        .border_1()
        .border_color(c.border_strong)
        .shadow(theme.elevated_shadow())
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(outputs_card)
        .child(subagents_card)
        .child(processes_card)
        .child(schedules_card)
        .into_any_element()
}

// ===========================================================================
// Kind-Filtered Drill-Down Popovers (used by Flows boards for both B1 & B2)
// ===========================================================================

fn render_drilldown_images(cards_style: bool, gallery: &SampleGallery, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let c = &theme.colors;
    let shell_bg = if cards_style {
        if theme.appearance.is_dark() { c.background } else { c.sidebar }
    } else {
        c.elevated
    };

    // 9 tiles in a crisp 3x3 visual grid (cycling the 6 realistic mockups with unique captions)
    let extra_captions = [
        "insights-light",
        "insights-dark",
        "popover-light",
        "pairing-dark",
        "rings-light",
        "bots-dark",
        "queue-light",
        "composer-dark",
        "settings-light",
    ];
    let tiles: Vec<(&SampleImage, &str)> = extra_captions
        .iter()
        .enumerate()
        .map(|(i, cap)| (&gallery.multi[i % gallery.multi.len()], *cap))
        .collect();

    let grid = div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .children(tiles.chunks(3).map(|row_items| {
            div().w_full().flex().gap(space::S2).children(row_items.iter().map(|(sample, cap)| {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(74.))
                            .rounded(radius::NAV)
                            .overflow_hidden()
                            .border_1()
                            .border_color(c.border)
                            .child(img(sample.image.clone()).size_full().object_fit(ObjectFit::Cover)),
                    )
                    .child(
                        div()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_muted)
                            .truncate()
                            .child(*cap),
                    )
            }))
        }));

    let body = div()
        .flex()
        .flex_col()
        .gap(space::S3)
        .child(grid)
        .child(
            div()
                .pt(space::S2)
                .border_t_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_faint)
                        .child(Icon::new(IconName::ExpandEditor).size(IconSize::XSmall).color(c.text_faint))
                        .child("Click any image to open full screen"),
                )
                .child(div().text_ui(TypeSize::Caption).text_color(c.accent).child("Load more…")),
        );

    div()
        .w(px(384.))
        .p(space::S3)
        .rounded(radius::POPOVER)
        .bg(shell_bg)
        .border_1()
        .border_color(if cards_style { c.border_strong } else { c.border })
        .shadow(theme.elevated_shadow())
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(drilldown_header("Outputs", "185 images", cx))
        .child(filter_tabs(OutputKind::Images, cx))
        .child(if cards_style {
            pillar_card(cx).p(space::S2).child(body).into_any_element()
        } else {
            div().pt(space::S1).child(body).into_any_element()
        })
        .into_any_element()
}

fn render_drilldown_files(
    cards_style: bool,
    active: OutputKind,
    items: &[FileItem],
    total: usize,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let c = &theme.colors;
    let shell_bg = if cards_style {
        if theme.appearance.is_dark() { c.background } else { c.sidebar }
    } else {
        c.elevated
    };
    let count_label = match active {
        OutputKind::Docs => format!("{total} documents"),
        OutputKind::Data => format!("{total} data files"),
        OutputKind::Code => format!("{total} code files"),
        _ => format!("{total} items"),
    };

    let list = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .children(items.iter().map(|file| detailed_file_row(file, cx)));

    div()
        .w(px(384.))
        .p(space::S3)
        .rounded(radius::POPOVER)
        .bg(shell_bg)
        .border_1()
        .border_color(if cards_style { c.border_strong } else { c.border })
        .shadow(theme.elevated_shadow())
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(drilldown_header("Outputs", &count_label, cx))
        .child(filter_tabs(active, cx))
        .child(if cards_style {
            pillar_card(cx).p(space::S2).child(list).into_any_element()
        } else {
            div().pt(space::S1).child(list).into_any_element()
        })
        .into_any_element()
}

fn render_drilldown_sessions(cards_style: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let c = &theme.colors;
    let shell_bg = if cards_style {
        if theme.appearance.is_dark() { c.background } else { c.sidebar }
    } else {
        c.elevated
    };

    let search = div()
        .h(px(30.))
        .px(space::S2)
        .rounded(radius::NAV)
        .bg(c.sunken)
        .border_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .gap(space::S1_5)
        .child(Icon::new(IconName::Search).size(IconSize::XSmall).color(c.text_faint))
        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Filter 142 finished sessions…"));

    let filters = div()
        .flex()
        .items_center()
        .gap(space::S1)
        .child(filter_pill("All", "142", true, cx))
        .child(filter_pill("Failed", "1", false, cx))
        .child(filter_pill("Stopped", "3", false, cx));

    let list = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .children(FINISHED_SUBAGENTS.iter().map(|sub| subagent_row(sub, false, cx)))
        .child(
            div()
                .pt(space::S2)
                .px(space::S2)
                .flex()
                .items_center()
                .justify_between()
                .text_ui(TypeSize::Caption)
                .text_color(c.text_faint)
                .child("Showing 7 of 142")
                .child(div().text_color(c.accent).child("Show older…")),
        );

    div()
        .w(px(384.))
        .p(space::S3)
        .rounded(radius::POPOVER)
        .bg(shell_bg)
        .border_1()
        .border_color(if cards_style { c.border_strong } else { c.border })
        .shadow(theme.elevated_shadow())
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(drilldown_header("Finished sessions", "142", cx))
        .child(search)
        .child(filters)
        .child(if cards_style {
            pillar_card(cx).p(space::S2).child(list).into_any_element()
        } else {
            div().pt(px(2.)).child(list).into_any_element()
        })
        .into_any_element()
}

// ===========================================================================
// Reusable Popover Primitives
// ===========================================================================

fn pillar_card(cx: &App) -> gpui::Div {
    let theme = cx.theme();
    let c = &theme.colors;
    div()
        .p(px(10.))
        .rounded(px(10.))
        .bg(c.card)
        .border_1()
        .border_color(c.border)
        .shadow(theme.card_shadow())
        .flex()
        .flex_col()
}

fn section_divider(cx: &App) -> AnyElement {
    div().my(space::S2).h(px(1.)).w_full().bg(cx.theme().colors.border).into_any_element()
}

fn media_strip(gallery: &SampleGallery, thumb_w: f32, thumb_h: f32, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .px(space::S1)
        .flex()
        .items_start()
        .gap(space::S1_5)
        .children(gallery.multi.iter().take(3).enumerate().map(|(i, sample)| {
            div()
                .w(px(thumb_w))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .relative()
                        .w(px(thumb_w))
                        .h(px(thumb_h))
                        .rounded(radius::NAV)
                        .overflow_hidden()
                        .border_1()
                        .border_color(c.border)
                        .child(img(sample.image.clone()).size_full().object_fit(ObjectFit::Cover))
                        // Subtle full-screen viewer cue on the first hovered tile
                        .when(i == 0, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top(px(4.))
                                    .right(px(4.))
                                    .size(px(18.))
                                    .rounded(radius::SMALL)
                                    .bg(gpui::black().opacity(0.64))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::ExpandEditor).size(IconSize::Indicator).color(gpui::white())),
                            )
                        }),
                )
                .child(
                    div()
                        .w(px(thumb_w))
                        .truncate()
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(sample.short_label),
                )
        }))
        // "+182 images" tile -> opens Outputs filtered to Images
        .child(
            div()
                .flex_1()
                .min_w(px(54.))
                .h(px(thumb_h))
                .rounded(radius::NAV)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("+182"))
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("images")),
        )
        .into_any_element()
}

fn compact_file_row(file: &FileItem, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .h(px(28.))
        .px(space::S2)
        .rounded(radius::NAV)
        .flex()
        .items_center()
        .gap(space::S2)
        .child(Icon::new(file.icon).size(IconSize::Nav).color(c.icon))
        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(file.name))
        .child(div().flex_none().text_mono(TypeSize::Caption).text_color(c.text_faint).child(file.ext))
        .into_any_element()
}

fn detailed_file_row(file: &FileItem, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .px(space::S2)
        .py(space::S1_5)
        .rounded(radius::NAV)
        .flex()
        .items_center()
        .gap(space::S2)
        .child(
            div()
                .size(px(26.))
                .flex_none()
                .rounded(radius::SMALL)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(file.icon).size(IconSize::Nav).color(c.icon)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(div().truncate().text_ui(TypeSize::Small).text_color(c.text).child(file.name))
                .child(div().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(file.meta)),
        )
        .child(div().flex_none().text_mono(TypeSize::Caption).text_color(c.text_faint).child(file.ext))
        .into_any_element()
}

fn kind_more_pill(label: &'static str, icon: IconName, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .px(space::S2)
        .py(px(3.))
        .rounded(radius::PILL)
        .bg(c.sunken)
        .border_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .gap(px(4.))
        .text_ui(TypeSize::Caption)
        .text_color(c.text_muted)
        .child(Icon::new(icon).size(IconSize::Indicator).color(c.icon))
        .child(label)
        .into_any_element()
}

fn kind_count_chip(icon: IconName, label: &'static str, count: &'static str, highlight: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .px(space::S1_5)
        .py(px(3.))
        .rounded(radius::SMALL)
        .bg(if highlight { c.selection } else { c.sunken })
        .border_1()
        .border_color(if highlight { c.accent.opacity(0.35) } else { c.border })
        .flex()
        .items_center()
        .gap(px(4.))
        .text_ui(TypeSize::Caption)
        .child(Icon::new(icon).size(IconSize::Indicator).color(if highlight { c.accent } else { c.icon }))
        .child(div().text_color(if highlight { c.text } else { c.text_muted }).child(label))
        .child(div().text_mono(TypeSize::Caption).text_color(if highlight { c.accent } else { c.text_faint }).child(count))
        .into_any_element()
}

fn provider_avatar(provider: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let mark = provider_mark(provider).map(|(service, path)| {
        let color = provider_mark_color(provider)
            .or_else(|| kuvo_ui::icon::mark_color(service))
            .unwrap_or(c.text);
        Mark::new(path).size(px(11.)).color(color).into_any_element()
    });
    div()
        .flex_none()
        .size(px(20.))
        .rounded(radius::PILL)
        .bg(c.sunken)
        .border_1()
        .border_color(c.border)
        .flex()
        .items_center()
        .justify_center()
        .child(mark.unwrap_or_else(|| Icon::new(IconName::Bot).size(IconSize::XSmall).color(c.icon).into_any_element()))
        .into_any_element()
}

fn subagent_row(sub: &SubagentItem, active: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let status_color: Hsla = if sub.asking {
        c.attention
    } else if sub.failed {
        c.danger
    } else if sub.stopped {
        c.warning
    } else {
        c.text_faint
    };
    div()
        .h(px(28.))
        .px(space::S1_5)
        .rounded(radius::NAV)
        .flex()
        .items_center()
        .gap(space::S2)
        .child(provider_avatar(sub.provider, cx))
        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(sub.title))
        .when(sub.asking, |d| d.child(status_dot(c.accent)))
        .child(
            div()
                .flex_none()
                .text_ui(TypeSize::Caption)
                .text_color(status_color)
                .child(if active { sub.status } else if sub.failed || sub.stopped { sub.status } else { sub.when }),
        )
        .when(active, |d| {
            d.child(Icon::new(IconName::Stop).size(IconSize::XSmall).color(c.text_faint))
        })
        .into_any_element()
}

fn process_row(proc: &ProcessItem, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .h(px(28.))
        .px(space::S2)
        .rounded(radius::NAV)
        .flex()
        .items_center()
        .gap(space::S2)
        .child(Icon::new(IconName::TerminalWindow).size(IconSize::Nav).color(c.icon))
        .child(div().flex_1().min_w_0().truncate().text_mono(TypeSize::Small).text_color(c.text).child(proc.command))
        .child(div().flex_none().text_mono(TypeSize::Caption).text_color(c.text_faint).child(proc.uptime))
        .child(status_dot(c.success))
        .into_any_element()
}

fn schedule_row_b1(item: &ScheduleItem, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let (icon, tint, kind_tag) = if item.is_watch {
        (IconName::Eye, c.success, "Watch")
    } else {
        (IconName::Clock, c.accent, "Wake-up")
    };
    div()
        .px(space::S2)
        .py(px(5.))
        .rounded(radius::NAV)
        .flex()
        .items_start()
        .gap(space::S2)
        .child(
            div()
                .mt(px(2.))
                .size(px(22.))
                .flex_none()
                .rounded(radius::SMALL)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon).size(IconSize::XSmall).color(tint)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(div().truncate().text_ui(TypeSize::Small).text_color(c.text).child(item.summary))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .text_ui(TypeSize::Caption)
                        .child(div().text_color(tint).child(kind_tag))
                        .child(div().text_color(c.text_faint).child("·"))
                        .child(div().truncate().text_color(c.text_muted).child(item.timing)),
                ),
        )
        .child(div().mt(px(3.)).child(Icon::new(IconName::Close).size(IconSize::XSmall).color(c.text_faint)))
        .into_any_element()
}

fn schedule_row_b2(item: &ScheduleItem, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .px(space::S1_5)
        .py(px(4.))
        .rounded(radius::SMALL)
        .flex()
        .items_center()
        .justify_between()
        .gap(space::S2)
        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(item.summary))
        .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_muted).child(item.timing))
        .child(Icon::new(IconName::Close).size(IconSize::XSmall).color(c.text_faint))
        .into_any_element()
}

fn drilldown_header(title: &str, count: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S1_5)
                .child(
                    div()
                        .size(px(24.))
                        .rounded(radius::SMALL)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::ChevronLeft).size(IconSize::Nav).color(c.text_muted)),
                )
                .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(title.to_owned())),
        )
        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(count.to_owned()))
        .into_any_element()
}

fn filter_tabs(active: OutputKind, cx: &App) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(space::S1)
        .child(filter_pill("All", "200", active == OutputKind::All, cx))
        .child(filter_pill("Images", "185", active == OutputKind::Images, cx))
        .child(filter_pill("Docs", "10", active == OutputKind::Docs, cx))
        .child(filter_pill("Data", "3", active == OutputKind::Data, cx))
        .child(filter_pill("Code", "2", active == OutputKind::Code, cx))
        .into_any_element()
}

fn filter_pill(label: &'static str, count: &'static str, active: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .px(space::S1_5)
        .py(px(3.))
        .rounded(radius::PILL)
        .bg(if active { c.primary } else { c.sunken })
        .border_1()
        .border_color(if active { c.primary } else { c.border })
        .flex()
        .items_center()
        .gap(px(4.))
        .text_ui(TypeSize::Caption)
        .child(div().text_color(if active { c.text_on_primary } else { c.text_muted }).child(label))
        .child(
            div()
                .text_mono(TypeSize::Caption)
                .text_color(if active { c.text_on_primary.opacity(0.72) } else { c.text_faint })
                .child(count),
        )
        .into_any_element()
}
