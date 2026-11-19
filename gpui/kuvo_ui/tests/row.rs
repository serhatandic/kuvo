//! Text-row containment: single-line rows truncate with an ellipsis inside their container while
//! keeping leading and trailing slots visible; wrapping rows wrap onto multiple lines inside their
//! container instead of running past the right edge.

use gpui::{
    Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use kuvo_ui::{
    Appearance, Theme,
    button::keycap,
    icon::{Icon, IconName, IconSize},
    row::{ListRow, NavRow, SectionHeader, TextRow},
    theme::{ActiveTheme as _, space},
};

const CONTAINER_W: f32 = 320.;
const LONG_SUMMARY: &str = "Listed the Work Queue, ran 2 commands, added \"Fix text overflow in long step summaries and question options\" to the Work Queue, started a session";
const LONG_OPTION: &str = "Rebuild with a shared TextRow primitive · Fix single-line truncation and multi-line option wrapping once across conversations, question cards, and popovers";
const LONG_COMMAND: &str = "cargo test -p kuvo_ui --test row -- --nocapture && cargo test -p kuvo text_overflow_bounds_stay_within_container";

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        div()
            .size_full()
            .p(space::S4)
            .child(
                div()
                    .debug_selector(|| "container".into())
                    .w(px(CONTAINER_W))
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .child(
                        TextRow::truncated("summary-hug", LONG_SUMMARY)
                            .hug()
                            .debug_selector("summary-hug")
                            .leading_slot(px(16.), Icon::new(IconName::Check).size(IconSize::Nav))
                            .trailing(
                                div()
                                    .debug_selector(|| "summary-hug-chevron".into())
                                    .child(Icon::new(IconName::ChevronRight).size(IconSize::Nav)),
                            ),
                    )
                    .child(
                        TextRow::truncated("step-hug", "Ran")
                            .hug()
                            .debug_selector("step-hug")
                            .leading_slot(px(16.), Icon::new(IconName::Terminal).size(IconSize::Nav))
                            .secondary(LONG_COMMAND, true, None)
                            .extra("· read 4 files, searched twice, updated plan", None)
                            .trailing(
                                div()
                                    .debug_selector(|| "step-hug-chevron".into())
                                    .child(Icon::new(IconName::ChevronRight).size(IconSize::Nav)),
                            ),
                    )
                    .child(
                        TextRow::truncated("popover-fill", LONG_SUMMARY)
                            .fill()
                            .debug_selector("popover-fill")
                            .leading(Icon::new(IconName::File).size(IconSize::Nav))
                            .trailing(
                                div()
                                    .debug_selector(|| "popover-fill-badge".into())
                                    .child("2m ago"),
                            ),
                    )
                    .child(
                        TextRow::wrapping("option-short", "Short option")
                            .debug_selector("option-short")
                            .leading(keycap("1".into(), c.border_strong, c.text_muted)),
                    )
                    .child(
                        TextRow::wrapping("option-wrap", LONG_OPTION)
                            .debug_selector("option-wrap")
                            .leading(keycap("2".into(), c.border_strong, c.text_muted)),
                    )
                    .child(
                        div().debug_selector(|| "nav-row-wrap".into()).w_full().min_w_0().child(
                            NavRow::new("nav-long", LONG_SUMMARY)
                                .icon(IconName::Folder)
                                .trailing(div().debug_selector(|| "nav-row-end".into()).child("12")),
                        ),
                    )
                    .child(
                        div().debug_selector(|| "section-wrap".into()).w_full().min_w_0().child(
                            SectionHeader::new("sec-long", LONG_SUMMARY),
                        ),
                    )
                    .child(
                        div().debug_selector(|| "list-row-wrap".into()).w_full().min_w_0().child(
                            ListRow::new("list-long", LONG_SUMMARY)
                                .detail(LONG_OPTION)
                                .trailing(div().debug_selector(|| "list-row-end".into()).child("Open")),
                        ),
                    ),
            )
    }
}

fn host(cx: &mut TestAppContext) -> &mut VisualTestContext {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo_ui::bind_keys(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Host);
    cx.simulate_resize(size(px(800.), px(900.)));
    cx.run_until_parked();
    cx
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is drawn"))
}

fn assert_within(container: Bounds<Pixels>, item: Bounds<Pixels>, name: &str) {
    assert!(
        item.left() >= container.left() - px(0.5) && item.right() <= container.right() + px(0.5),
        "{name} overflowed container: item={item:?}, container={container:?}"
    );
}

#[gpui::test]
fn truncated_text_rows_stay_within_container_with_trailing_visible(cx: &mut TestAppContext) {
    let cx = host(cx);
    let container = bounds(cx, "container");

    for (row_sel, text_sel, end_sel) in [
        ("summary-hug", "summary-hug-text", "summary-hug-chevron"),
        ("step-hug", "step-hug-text", "step-hug-chevron"),
        ("popover-fill", "popover-fill-text", "popover-fill-badge"),
    ] {
        let row = bounds(cx, row_sel);
        let text = bounds(cx, text_sel);
        let end = bounds(cx, end_sel);
        assert_within(container, row, row_sel);
        assert_within(container, text, text_sel);
        assert_within(container, end, end_sel);
        assert!(end.size.width > px(0.), "{end_sel} should remain visible: {end:?}");
        assert!(
            row.size.height <= px(28.),
            "{row_sel} should stay on a single line, got height {:?}",
            row.size.height
        );
    }
}

#[gpui::test]
fn wrapping_text_rows_stay_within_container_and_wrap_vertically(cx: &mut TestAppContext) {
    let cx = host(cx);
    let container = bounds(cx, "container");

    let short = bounds(cx, "option-short");
    let wrapped = bounds(cx, "option-wrap");
    let wrapped_text = bounds(cx, "option-wrap-text");

    assert_within(container, short, "option-short");
    assert_within(container, wrapped, "option-wrap");
    assert_within(container, wrapped_text, "option-wrap-text");
    assert!(
        wrapped.size.height > short.size.height,
        "long option should wrap to multiple lines: wrapped={wrapped:?}, short={short:?}"
    );
}

#[gpui::test]
fn list_nav_and_section_rows_stay_within_container(cx: &mut TestAppContext) {
    let cx = host(cx);
    let container = bounds(cx, "container");

    for sel in ["nav-row-wrap", "nav-row-end", "section-wrap", "list-row-wrap", "list-row-end"] {
        let item = bounds(cx, sel);
        assert_within(container, item, sel);
        assert!(item.size.width > px(0.), "{sel} should remain visible: {item:?}");
    }
}
