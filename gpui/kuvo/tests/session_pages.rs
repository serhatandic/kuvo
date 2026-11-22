//! The sessions list as someone uses it, against the stand-in box: a long project's list shows its
//! first sessions, those at work or waiting on you first, and "Show more" adds the box's next
//! page, 10, then 20, then what's left, with "Show fewer" back (#951); a subagent sits under the
//! session that started it, and opened, says the model and approval it really runs with, or, if it
//! takes no messages, says so with the way to its conversation (#950). Clicks go through the window.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::sidebar::Line;
use kuvo::stand_in::{HOME, Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_box::transport::{Request, Transport};
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

fn website() -> String {
    format!("{HOME}/Projects/website")
}

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(4000.)));
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

/// What shows under the Website project: its sessions (a subagent marked `sub:`), then "more" and
/// "fewer" where they show.
fn website_rows(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
    let lines = workspace.read_with(cx, |workspace, cx| kuvo::sidebar::lines(workspace, cx));
    let at = lines.iter().position(|line| *line == Line::Project(website())).expect("Website");
    lines[at + 1..]
        .iter()
        .take_while(|line| !matches!(line, Line::Project(_) | Line::Header(_)))
        .map(|line| match line {
            Line::Session { id, section: "subagent", .. } => format!("sub:{id}"),
            Line::Session { id, .. } => id.clone(),
            Line::More(_) => "more".into(),
            Line::Fewer(_) => "fewer".into(),
            other => format!("{other:?}"),
        })
        .collect()
}

fn sessions(rows: &[String]) -> usize {
    rows.iter().filter(|row| row.starts_with("ses_")).count()
}

#[gpui::test]
fn a_long_list_shows_those_at_work_first_and_more_in_growing_pages(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    // Website: its four, and forty newer.
    for _ in 0..40 {
        stand_in.send(&Request::post("kuvo/sessions", json!({ "project": website() }))).unwrap();
    }
    let (workspace, cx) = open(cx, stand_in.clone());

    // Its first six: the one at work and the one waiting on you first, though forty are newer.
    let first = website_rows(&workspace, cx);
    assert_eq!(&first[..2], ["ses_signup", "ses_cms"]);
    assert_eq!(sessions(&first), 6);
    assert_eq!(first.last().map(String::as_str), Some("more"));
    let label = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| workspace.read_with(cx, |workspace, _| workspace.more_of(&website()).map(|more| more.count));
    assert_eq!(label(&workspace, cx), Some(10));

    // Show more: 10, then 20, then the 8 left; never the whole list at once.
    click(cx, &format!("more-{}", website()));
    let rows = website_rows(&workspace, cx);
    assert_eq!(sessions(&rows), 16);
    assert_eq!(&rows[..6], &first[..6], "the first stay where they were");
    assert_eq!(rows[rows.len() - 2..], ["more", "fewer"]);
    assert_eq!(label(&workspace, cx), Some(20));
    click(cx, &format!("more-{}", website()));
    assert_eq!(sessions(&website_rows(&workspace, cx)), 36);
    assert_eq!(label(&workspace, cx), Some(8));
    click(cx, &format!("more-{}", website()));
    let all = website_rows(&workspace, cx);
    assert_eq!(sessions(&all), 44);
    assert_eq!(all.last().map(String::as_str), Some("fewer"), "nothing more to show");
    // Each asked of the box with the cursor it gave.
    let asked = stand_in.seen_at("GET", "kuvo/sessions/page");
    assert_eq!(asked.len(), 3);
    assert!(asked.iter().all(|seen| seen.path.contains("project=") && seen.path.contains("cursor=")));

    // As the list changes, it's read again as far as it was: one at work now comes first.
    stand_in.set_running("ses_lighthouse", true);
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    let now = website_rows(&workspace, cx);
    assert_eq!(sessions(&now), 44);
    assert!(now[..3].contains(&"ses_lighthouse".to_owned()), "{now:?}");

    // Show fewer: its first six again.
    click(cx, &format!("fewer-{}", website()));
    let fewer = website_rows(&workspace, cx);
    assert_eq!(sessions(&fewer), 6);
    assert_eq!(fewer.last().map(String::as_str), Some("more"));
}

#[gpui::test]
fn a_subagent_sits_under_its_session_and_says_what_it_runs_with(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.subagent_of("ses_sub", "ses_signup");
    stand_in.set_model_state("ses_sub", json!({ "model": { "providerID": "anthropic", "id": "claude-haiku-4-5" }, "name": "Haiku 4.5", "runs": true, "fixed": true }));
    let (workspace, cx) = open(cx, stand_in.clone());

    // Not at work, it shows only while its session (or it) is open.
    assert!(!website_rows(&workspace, cx).contains(&"sub:ses_sub".to_owned()));
    click(cx, "session-project-ses_signup");
    let rows = website_rows(&workspace, cx);
    let at = rows.iter().position(|row| row == "ses_signup").unwrap();
    assert_eq!(rows[at + 1], "sub:ses_sub", "right under the session that started it");

    // Opened: its real model, named with no choice, and its conversation's approval.
    click(cx, "session-subagent-ses_sub");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_sub".to_owned()));
    // (What its composer says, read now rather than as the box streams it.)
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_sub", window, cx));
    cx.run_until_parked();
    let composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
    assert_eq!(composer.read_with(cx, |composer, _| composer.chip_label()), Some("Haiku 4.5".to_owned()));
    assert_eq!(composer.read_with(cx, |composer, _| composer.approval().map(|shown| shown.parent.clone())), Some(Some("ses_signup".to_owned())));
    click(cx, "model-chip");
    assert!(bounds(cx, "model-picker").is_none(), "no model to choose");
    assert!(bounds(cx, "composer-card").is_some(), "it takes messages, through its session");

    // One that takes none says so in its box's words, with the way to its session.
    stand_in.set_facts("ses_sub", |facts| facts["messages"] = json!({ "via": "none", "note": "Claude Code's Explore agents can't take messages." }));
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(bounds(cx, "subagent-no-messages").is_some());
    assert!(bounds(cx, "composer-card").is_none(), "no composer that can't send");
    click(cx, "open-parent");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_signup".to_owned()));
}
