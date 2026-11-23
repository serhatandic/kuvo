//! The Marketplace as someone uses it, against the stand-in box (nothing real is reached; it
//! serves the catalog the box ships, logos and all): opened from the rail, an app's own logo, an
//! app connected with its token, an agent added as a bot with its routines off, a skill from a
//! GitHub link previewed and then installed; and where it took over, Settings and New Bot. Clicks
//! and keys go through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::stand_in::marketplace::Sample;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

/// 2026-11-14, 09:53 UTC.
const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, sample: Sample) -> (StandIn, Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.marketplace_sample(sample);
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1320.), px(1600.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (stand_in, workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn bodies(stand_in: &StandIn, method: &str, route: &str) -> Vec<serde_json::Value> {
    stand_in.seen_at(method, route).into_iter().filter_map(|seen| seen.body).collect()
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (Option<String>, Option<String>) {
    workspace.read_with(cx, |workspace, cx| {
        let (title, beside) = workspace.title(cx);
        (title.map(|title| title.to_string()), beside.map(|beside| beside.to_string()))
    })
}

#[gpui::test]
fn apps_show_each_apps_own_logo_as_the_box_serves_it(cx: &mut TestAppContext) {
    let (stand_in, workspace, cx) = open(cx, Sample::InUse);
    click(cx, "rail-marketplace");
    assert_eq!(title(&workspace, cx), (Some("Apps".into()), Some("Marketplace".into())));
    // Each catalog app's own mark, read from the box's logo route, drawn as a picture.
    for app in ["github", "linear", "notion", "sentry"] {
        assert!(shown(cx, &format!("market-item-{app}")), "{app} is listed");
        assert!(shown(cx, &format!("market-logo-{app}")), "{app} shows its own logo");
    }
    assert!(!stand_in.seen_at("GET", "kuvo/marketplace/apps/github/logo").is_empty());
    // An app whose sign-in needs Kuvo's own key isn't offered: no dead ends.
    assert!(!shown(cx, "market-item-slack"));
    // What's connected says so in place of a button; what isn't offers the box's words.
    assert!(shown(cx, "market-status-notion") && !shown(cx, "market-button-notion"));
    assert!(shown(cx, "market-button-github") && !shown(cx, "market-status-github"));
}

#[gpui::test]
fn connecting_an_app_sends_its_token_and_the_app_shows_connected(cx: &mut TestAppContext) {
    let (stand_in, _, cx) = open(cx, Sample::Fresh);
    click(cx, "rail-marketplace");
    click(cx, "market-button-github");
    assert!(shown(cx, "market-app-sheet"));
    click(cx, "market-field-token");
    cx.simulate_input("ghp_marketplace_token");
    click(cx, "market-app-add");
    assert_eq!(stand_in.seen_at("POST", "kuvo/marketplace/apps/github/add").len(), 1, "added for the setup first");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/marketplace/apps/github/connect"), vec![json!({ "token": "ghp_marketplace_token" })]);
    // Done: the sheet goes, and the row says it's connected.
    assert!(!shown(cx, "market-app-sheet"));
    assert!(shown(cx, "market-status-github") && !shown(cx, "market-button-github"));
}

#[gpui::test]
fn adding_an_agent_makes_its_bot_with_the_routines_off(cx: &mut TestAppContext) {
    let (stand_in, workspace, cx) = open(cx, Sample::Fresh);
    click(cx, "rail-marketplace");
    click(cx, "market-section-agents");
    click(cx, "market-button-coding-reviewer");
    assert!(shown(cx, "market-agent-sheet"));
    // Where it runs, the account and model, and its routine, off.
    assert!(shown(cx, "market-computer") && shown(cx, "market-model") && shown(cx, "market-routine-weekly"));
    click(cx, "market-add-agent");
    let sent = bodies(&stand_in, "POST", "kuvo/marketplace/agents/coding-reviewer/add");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["routines"], json!([]), "no routine turned on");
    assert_eq!(sent[0]["computer"], json!("here-key"));
    assert_eq!(sent[0]["name"], json!("Coding Reviewer"));
    assert!(sent[0]["model"]["id"].is_string());
    // The bot is made, and its conversation opens, as New Bot's does.
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Bots);
    let bot = workspace.read_with(cx, |workspace, _| workspace.bot_chat().map(|(bot, _)| bot.to_owned())).expect("its conversation");
    assert!(shown(cx, &format!("bot-{bot}")), "listed in Bots");
    assert_eq!(title(&workspace, cx).0.as_deref(), Some("Coding Reviewer"));
}

#[gpui::test]
fn a_skill_from_a_github_link_shows_its_files_then_installs(cx: &mut TestAppContext) {
    let (stand_in, _, cx) = open(cx, Sample::Fresh);
    click(cx, "rail-marketplace");
    click(cx, "market-section-skills");
    let link = "https://github.com/acme/agent-skills/tree/main/pr-checklist";
    click(cx, "market-search");
    cx.simulate_input(link);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(bodies(&stand_in, "POST", "kuvo/marketplace/skills/preview"), vec![json!({ "from": link })]);
    // Its files, as the box read them, before anything is installed.
    assert!(shown(cx, "market-preview-sheet") && shown(cx, "market-skill-files") && shown(cx, "market-file-text"));
    assert!(stand_in.seen_at("POST", "kuvo/marketplace/skills/install").is_empty());
    click(cx, "market-install-skill");
    let installed = bodies(&stand_in, "POST", "kuvo/marketplace/skills/install");
    assert_eq!(installed.len(), 1);
    assert!(installed[0]["preview"].is_string(), "the bytes previewed, by their preview");
    assert!(!shown(cx, "market-preview-sheet"));
    // It's one of your skills now, on, with its switch.
    assert!(shown(cx, "market-item-pr-checklist") && shown(cx, "market-switch-pr-checklist"));
}

#[gpui::test]
fn settings_no_longer_has_integrations_or_skills(cx: &mut TestAppContext) {
    let (_, _, cx) = open(cx, Sample::Fresh);
    click(cx, "rail-settings");
    click(cx, "settings-foot");
    if shown(cx, "settings-later") {
        click(cx, "settings-later");
    }
    // What's still to come is listed, but Integrations and Skills are the Marketplace's now.
    assert!(shown(cx, "setting-mcp"));
    assert!(!shown(cx, "setting-integrations") && !shown(cx, "setting-skills"));
}

#[gpui::test]
fn new_bot_points_to_the_marketplaces_agents(cx: &mut TestAppContext) {
    let (_, workspace, cx) = open(cx, Sample::Fresh);
    click(cx, "rail-bots");
    click(cx, "new-bot");
    assert!(shown(cx, "new-marketplace"));
    click(cx, "new-marketplace-open");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Marketplace);
    assert_eq!(title(&workspace, cx), (Some("Agents".into()), Some("Marketplace".into())));
    assert!(shown(cx, "market-item-coding-reviewer"));
}
