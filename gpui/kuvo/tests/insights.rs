//! Insights as someone uses it, against the stand-in box (nothing real is reached): the period's
//! totals, its trend and its ranked lists as the box sums them; a bot opened to its sessions;
//! narrowing to a row and back by the crumbs; a usage ring's account; a first week, nothing at all,
//! a failed read, an older box that only has the overview, and the refresh button that has the box
//! count again. Clicks go through the window.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::stand_in::insights::Sample;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;
const SUMMARY: &str = "kuvo/agents/api/kuvo/insights/summary";

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1280.), px(1800.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

/// The query of the last summary Insights asked for.
fn last_asked(stand_in: &StandIn) -> String {
    stand_in.seen_at("GET", SUMMARY).last().map(|seen| seen.path.clone()).expect("asked for its summary")
}

fn insights(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    let (workspace, cx) = open(cx, stand_in);
    click(cx, "rail-insights");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Insights);
    (workspace, cx)
}

#[gpui::test]
fn insights_opens_on_bots_with_its_totals_trend_and_standouts(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = insights(cx, stand_in.clone());
    let asked = last_asked(&stand_in);
    assert!(asked.contains("period=30") && asked.contains("zone="), "{asked}");
    for part in ["insights-total-cost", "insights-total-tokens", "insights-total-cache", "insights-chart", "insights-on-30d", "insights-on-Cost", "insights-on-Bots"] {
        assert!(shown(cx, part), "{part}");
    }
    // About five rows, then all of them.
    assert!(shown(cx, "insights-row-4") && !shown(cx, "insights-row-5"));
    click(cx, "insights-show-all");
    assert!(shown(cx, "insights-row-9"));
    // At most three standouts.
    assert!(shown(cx, "insights-standout-2") && !shown(cx, "insights-standout-3"));
    // Totals sit side by side, above the trend.
    let (cost, cache, chart) = (bounds(cx, "insights-total-cost").unwrap(), bounds(cx, "insights-total-cache").unwrap(), bounds(cx, "insights-chart").unwrap());
    assert_eq!(cost.top(), cache.top());
    assert!(cost.bottom() < chart.top());
}

#[gpui::test]
fn a_bot_opens_to_the_sessions_it_oversees_and_one_narrows_the_screen(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = insights(cx, stand_in.clone());
    click(cx, "insights-expand-0");
    assert!(shown(cx, "insights-session-ses_f01"));
    // Four, then the rest of its twelve.
    assert!(!shown(cx, "insights-session-ses_f11"));
    click(cx, "insights-sessions-all");
    assert!(shown(cx, "insights-session-ses_f11"));
    click(cx, "insights-session-ses_f01");
    assert!(last_asked(&stand_in).contains("&session=ses_f01"));
    assert!(shown(cx, "insights-narrow-chip"));
    assert!(shown(cx, "insights-on-Sessions"));
}

#[gpui::test]
fn a_row_narrows_everything_to_it_and_the_crumbs_go_back(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = insights(cx, stand_in.clone());
    click(cx, "insights-row-0");
    assert!(last_asked(&stand_in).ends_with("&bot=bot_foreman"));
    assert!(shown(cx, "insights-narrow-chip") && shown(cx, "insights-crumb-root"));
    // Narrowed to the bot, its sessions first.
    assert!(shown(cx, "insights-on-Sessions"));
    click(cx, "insights-row-0");
    assert!(last_asked(&stand_in).ends_with("&bot=bot_foreman&session=ses_f01"));
    assert!(shown(cx, "insights-crumb-0"));
    click(cx, "insights-crumb-0");
    assert!(last_asked(&stand_in).ends_with("&bot=bot_foreman"));
    assert!(!shown(cx, "insights-crumb-0"));
    // The chip's close goes back out.
    click(cx, "insights-narrow-chip");
    let asked = last_asked(&stand_in);
    assert!(!asked.contains("bot=") && !asked.contains("session="), "{asked}");
    assert!(!shown(cx, "insights-narrow-chip"));
    assert!(shown(cx, "insights-on-Bots"));
}

#[gpui::test]
fn the_lists_switch_and_a_period_reads_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = insights(cx, stand_in.clone());
    click(cx, "insights-tab-providers");
    assert!(shown(cx, "insights-on-Providers"));
    assert!(shown(cx, "insights-row-3") && !shown(cx, "insights-row-4") && !shown(cx, "insights-show-all"));
    click(cx, "insights-row-0");
    assert!(last_asked(&stand_in).ends_with("&provider=claude"));
    // Narrowed: back to the first list.
    assert!(shown(cx, "insights-on-Bots"));
    click(cx, "insights-period-7d");
    assert!(last_asked(&stand_in).starts_with(&format!("{SUMMARY}?period=7")));
    assert!(shown(cx, "insights-on-7d"));
    click(cx, "insights-metric-Tokens");
    assert!(shown(cx, "insights-on-Tokens"));
}

#[gpui::test]
fn another_period_never_shows_the_last_ones_figures_as_its_own(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = insights(cx, stand_in.clone());
    assert!(shown(cx, "insights-total-cost") && !shown(cx, "insights-updating-mark"));

    // 7d chosen while the box is still reading it: the month's figures stay in place, faded and
    // marked as updating, never as the week's; the page doesn't go blank.
    stand_in.set_insights(Sample::Reading);
    click(cx, "insights-period-7d");
    assert!(shown(cx, "insights-on-7d"));
    assert!(shown(cx, "insights-updating") && shown(cx, "insights-updating-mark"));
    assert!(!shown(cx, "insights-said"), "no blank \u{201c}Reading usage\u{2026}\u{201d} page");

    // The box has read it: the week's figures, as they are.
    stand_in.set_insights(Sample::Full);
    cx.executor().advance_clock(std::time::Duration::from_secs(3));
    cx.run_until_parked();
    assert!(last_asked(&stand_in).starts_with(&format!("{SUMMARY}?period=7")));
    assert!(shown(cx, "insights-total-cost") && !shown(cx, "insights-updating") && !shown(cx, "insights-updating-mark"));

    // Back to 30d while the box reads it again: its last figures at once, as its own.
    stand_in.set_insights(Sample::Reading);
    click(cx, "insights-period-30d");
    assert!(shown(cx, "insights-total-cost") && !shown(cx, "insights-updating") && !shown(cx, "insights-updating-mark"));
}

#[gpui::test]
fn insights_opens_at_once_on_the_boxs_last_summary_marked_as_updating(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_insights(Sample::Kept);
    let (_, cx) = insights(cx, stand_in.clone());
    // Its figures as they are (this period's), with a quiet mark while the box reads them afresh.
    assert!(shown(cx, "insights-total-cost") && shown(cx, "insights-updating-mark") && !shown(cx, "insights-updating"));
    stand_in.set_insights(Sample::Full);
    cx.executor().advance_clock(std::time::Duration::from_secs(3));
    cx.run_until_parked();
    assert!(shown(cx, "insights-total-cost") && !shown(cx, "insights-updating-mark"));
}

#[gpui::test]
fn refresh_asks_the_box_to_count_again_and_shows_its_new_numbers_in_place(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = insights(cx, stand_in.clone());
    // A month of work, as the box last counted it five minutes ago.
    assert!(shown(cx, "insights-row-4"));
    assert!(shown(cx, "insights-counted-Updated 5m ago"));
    assert!(!last_asked(&stand_in).contains("refresh="));

    // What the box has changes: until it's asked, the numbers on screen stay what they were.
    stand_in.set_insights(Sample::Little);
    assert!(shown(cx, "insights-row-4"));

    // Refreshed: the box is asked to count again, and its new numbers replace the old in place.
    click(cx, "insights-refresh");
    assert!(last_asked(&stand_in).ends_with("&refresh=1"), "{}", last_asked(&stand_in));
    assert!(shown(cx, "insights-row-1") && !shown(cx, "insights-row-2"));
    assert!(shown(cx, "insights-counted-Updated just now") && !shown(cx, "insights-counted-Updated 5m ago"));
    assert!(!shown(cx, "insights-said"), "never a blank \u{201c}Reading usage\u{2026}\u{201d} page");
}

#[gpui::test]
fn a_usage_ring_opens_insights_narrowed_to_its_account(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "ring-0");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Insights);
    assert!(last_asked(&stand_in).contains("&account="));
    assert!(shown(cx, "insights-narrow-chip"));
    click(cx, "insights-narrow-chip");
    assert!(!last_asked(&stand_in).contains("&account="));
    assert!(!shown(cx, "insights-narrow-chip"));
    assert_eq!(workspace.read_with(cx, |workspace, cx| workspace.title(cx).0).as_deref(), Some("Insights"));
}

#[gpui::test]
fn a_first_week_shows_what_there_is_and_nothing_stands_out(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_insights(Sample::Little);
    let (_, cx) = insights(cx, stand_in);
    assert!(shown(cx, "insights-row-1") && !shown(cx, "insights-row-2"));
    assert!(!shown(cx, "insights-show-all"));
    assert!(!shown(cx, "insights-standouts"));
}

#[gpui::test]
fn with_nothing_recorded_it_says_so_and_another_period_is_a_click_away(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_insights(Sample::Nothing);
    let (_, cx) = insights(cx, stand_in.clone());
    assert!(shown(cx, "insights-said"));
    assert!(!shown(cx, "insights-ranked"));
    click(cx, "insights-period-90d");
    assert!(last_asked(&stand_in).contains("period=90"));
}

#[gpui::test]
fn a_failed_read_says_why_and_tries_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_insights(Sample::Failing);
    let (_, cx) = insights(cx, stand_in.clone());
    assert!(shown(cx, "insights-said") && shown(cx, "insights-try-again"));
    stand_in.set_insights(Sample::Full);
    click(cx, "insights-try-again");
    assert!(shown(cx, "insights-ranked"));
    assert!(!shown(cx, "insights-try-again"));
}

#[gpui::test]
fn an_older_box_shows_its_totals_trend_and_providers(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_lacks(&[kuvo::insights::SUMMARY]);
    let (_, cx) = insights(cx, stand_in.clone());
    assert!(stand_in.seen_at("GET", SUMMARY).is_empty());
    assert!(!stand_in.seen_at("GET", "kuvo/agents/api/kuvo/insights").is_empty());
    for part in ["insights-total-cost", "insights-chart", "insights-on-Providers", "insights-row-0"] {
        assert!(shown(cx, part), "{part}");
    }
    assert!(!shown(cx, "insights-tab-bots") && !shown(cx, "insights-standouts"));
    // It doesn't say when it counted, and refresh reads it again.
    assert!(!shown(cx, "insights-counted"));
    let reads = stand_in.seen_at("GET", "kuvo/agents/api/kuvo/insights").len();
    click(cx, "insights-refresh");
    assert_eq!(stand_in.seen_at("GET", "kuvo/agents/api/kuvo/insights").len(), reads + 1);
    // Its providers still narrow, as it can.
    click(cx, "insights-row-0");
    assert!(stand_in.seen_at("GET", "kuvo/agents/api/kuvo/insights").last().unwrap().path.contains("&provider="));
    assert!(shown(cx, "insights-narrow-chip"));
}

#[gpui::test]
fn a_narrow_window_keeps_the_figures_in_line(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = insights(cx, stand_in);
    cx.simulate_resize(size(px(700.), px(1400.)));
    cx.run_until_parked();
    let (row, next) = (bounds(cx, "insights-row-0").unwrap(), bounds(cx, "insights-row-1").unwrap());
    let view = bounds(cx, "insights").unwrap();
    assert!(row.right() <= view.right() && row.bottom() <= next.top());
}
