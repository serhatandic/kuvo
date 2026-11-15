//! The first run as someone meets it, against the stand-in box (nothing real is reached, no key is
//! real): a new computer with nothing on it; one with several agents, some signed in, one ended,
//! one not installed; one where a model runs already; a machine picked; a box that turns out
//! older mid-way. Clicks and keys go through the window; a sign-in is followed on the test's own
//! clock.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::first_run::{FirstRun, Step};
use kuvo::settings::sign_in::POLL;
use kuvo::stand_in::first_run::Scenario;
use kuvo::stand_in::settings::Ending;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_box::models::StartIn;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(1400.)));
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

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn run(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FirstRun> {
    workspace.read_with(cx, |workspace, _| workspace.first_run().cloned()).expect("the first run")
}

fn step(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Step {
    run(workspace, cx).read_with(cx, |run, _| run.step())
}

/// The sign-in's clock moves on: the box is asked how it goes.
fn tick(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(POLL);
    cx.run_until_parked();
}

/// The names of the accounts a step lists, in order.
fn names(accounts: Vec<&kuvo_box::accounts::AccountEntry>) -> Vec<String> {
    accounts.into_iter().map(|entry| entry.integration.name.clone()).collect()
}

#[gpui::test]
fn a_box_youve_had_the_first_run_with_opens_the_app(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    assert!(workspace.read_with(cx, |workspace, _| workspace.first_run().is_none()));
    assert!(shown(cx, "rail-settings") && !shown(cx, "first-run"));
    assert!(stand_in.seen_at("POST", "kuvo/setup/welcomed").is_empty());
}

#[gpui::test]
fn on_a_new_computer_you_install_an_agent_sign_in_choose_a_folder_and_land_on_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Nothing);
    let (workspace, cx) = open(cx, stand_in.clone());
    // The first run has the window to itself: no rail, no sidebar, none of the app's keys.
    assert!(shown(cx, "first-run-card") && !shown(cx, "rail-settings") && !shown(cx, "area-sidebar"));
    cx.simulate_keystrokes("ctrl-5");
    cx.simulate_keystrokes("cmd-n");
    cx.run_until_parked();
    assert_eq!(step(&workspace, cx), Step::Welcome);
    assert!(!shown(cx, "rail-settings"));

    // Only the free models run: the agents and plans come next, not skipped.
    assert!(!shown(cx, "first-run-signed-in"));
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Agents);
    for agent in ["claude-code", "codex-cli", "antigravity", "cursor", "grok-cli"] {
        assert!(shown(cx, &format!("first-run-agent-{agent}-act")), "{agent} offers Install");
    }
    assert!(shown(cx, "first-run-free"), "the free models are there, with no account");

    // Install, then its sign-in opens in place, under its row.
    click(cx, "first-run-agent-claude-code-act");
    assert_eq!(stand_in.seen_at("POST", "kuvo/agents/api/integration/claude-code/install").len(), 1);
    assert!(shown(cx, "sign-in"), "its sign-in, in place");
    assert_eq!(step(&workspace, cx), Step::Agents);
    assert_eq!(stand_in.seen_at("POST", "kuvo/sign-in/start").len(), 1);
    stand_in.end_sign_ins(Ending::Complete, None);
    tick(cx);
    assert!(!shown(cx, "sign-in"));
    assert!(shown(cx, "first-run-agent-claude-code-signed-in"));

    // Where to work: nothing's suggested here, so nothing's picked and Continue waits.
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Where);
    assert!(shown(cx, "first-run-no-folders"));
    assert!(!shown(cx, "first-run-computer-this"), "one computer: nothing to pick between");
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Where);
    assert!(!stand_in.welcomed());

    // A folder chosen by looking through the computer's own, as its box lists them.
    click(cx, "first-run-choose");
    assert_eq!(step(&workspace, cx), Step::Browse);
    click(cx, "first-run-browse-Desktop");
    assert!(shown(cx, "first-run-browse-unreadable"), "a folder the computer won't show says why");
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Browse, "one that can't be read can't be chosen");
    click(cx, "first-run-up");
    click(cx, "first-run-browse-Projects");
    // (A chooser's folders: no files asked for, and none of the hidden ones.)
    let asked = stand_in.seen_at("GET", "kuvo/folders");
    assert!(asked.len() >= 3 && asked.iter().all(|seen| !seen.path.contains("hidden") && !seen.path.contains("files")), "{asked:?}");
    click(cx, "first-run-browse-website");
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Where);
    assert!(shown(cx, "first-run-folder-chosen"));
    assert_eq!(run(&workspace, cx).read_with(cx, |run, _| run.place().cloned()), Some(StartIn::Project("/Users/sam/Projects/website".into())));

    // Done: the box is told, and the window is the app, on its landing view, set to start there.
    click(cx, "first-run-next");
    assert!(workspace.read_with(cx, |workspace, _| workspace.first_run().is_none()));
    assert!(stand_in.welcomed());
    assert_eq!(stand_in.seen_at("POST", "kuvo/setup/welcomed").into_iter().filter_map(|seen| seen.body).collect::<Vec<_>>(), [json!({ "welcomed": true })]);
    assert!(shown(cx, "rail-settings"));
    assert!(workspace.read_with(cx, |workspace, _| workspace.is_landing() && workspace.area() == Area::Sessions));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.landing_place()), StartIn::Project("/Users/sam/Projects/website".into()));
    // The keyboard is in the composer: what's typed and sent starts the first session there.
    cx.simulate_input("Fix the sign-up form");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let started: Vec<_> = stand_in.seen_at("POST", "kuvo/sessions").into_iter().filter_map(|seen| seen.body).collect();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0]["project"], json!("/Users/sam/Projects/website"));
}

#[gpui::test]
fn with_several_agents_each_has_one_row_saying_what_the_box_says(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Crowded);
    let (workspace, cx) = open(cx, stand_in.clone());
    // A sign-in that ended needs you: the agents and plans aren't skipped.
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Agents);
    let first_run = run(&workspace, cx);
    let plans = first_run.read_with(cx, |run, _| names(run.plans()));
    // One row per agent (the account added beside Claude Code's own isn't a second row), then the
    // engine's plans; no account a key is entered for among them.
    assert_eq!(&plans[..5], ["Claude Code", "Codex", "Antigravity", "Cursor", "Grok"]);
    assert!(plans.iter().any(|name| name == "GitHub Copilot"));
    assert!(!plans.iter().any(|name| name.contains("Work") || name == "OpenAI" || name == "DeepSeek" || name == "OpenRouter"), "{plans:?}");
    // Signed in: a check, and nothing to press. Ended, signed out, not installed: the box's words.
    assert!(shown(cx, "first-run-agent-claude-code-signed-in") && !shown(cx, "first-run-agent-claude-code-act"));
    assert!(shown(cx, "first-run-agent-codex-cli-signed-in"));
    for agent in ["antigravity", "cursor", "grok-cli"] {
        assert!(shown(cx, &format!("first-run-agent-{agent}-act")));
    }
    let offers = first_run.read_with(cx, |run, _| {
        run.plans().into_iter().filter(|entry| entry.state != kuvo_box::accounts::State::SignedIn).map(|entry| (entry.integration.id.clone(), entry.actions[0].label.clone())).collect::<Vec<_>>()
    });
    assert!(offers.contains(&("antigravity".into(), "Sign In Again".into())) && offers.contains(&("grok-cli".into(), "Install".into())), "{offers:?}");

    // The keys are apart, and a key is added, never signed in to.
    click(cx, "first-run-keys");
    assert_eq!(step(&workspace, cx), Step::Keys);
    let keys = first_run.read_with(cx, |run, _| run.keys().into_iter().map(|entry| (entry.integration.name.clone(), entry.actions.first().map(|action| action.label.clone()))).collect::<Vec<_>>());
    assert!(keys.contains(&("OpenAI".into(), Some("Add Key".into()))), "{keys:?}");
    assert!(keys.iter().all(|(_, label)| label.as_deref().is_none_or(|label| !label.contains("Sign In"))), "{keys:?}");
    click(cx, "first-run-account-openai-act");
    assert!(shown(cx, "sign-in-key"));
    // A key the provider turns down stays, saying why; a good one connects.
    cx.simulate_input("wrong");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(shown(cx, "sign-in-key") && !shown(cx, "first-run-account-openai-signed-in"));
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("sk-made-up");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(shown(cx, "first-run-account-openai-signed-in") && !shown(cx, "sign-in-key"));
    assert_eq!(stand_in.seen_at("POST", "api/integration/openai/connect/key").len(), 2);
    // Nothing left the flow: Settings never opened.
    assert!(workspace.read_with(cx, |workspace, _| workspace.first_run().is_some() && workspace.area() == Area::Sessions));
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Agents, "Done comes back to the plans");
}

#[gpui::test]
fn a_sign_in_that_fails_says_why_in_place_and_cancel_gives_one_up(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Crowded);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "first-run-next");
    // Its page turns the sign-in down: the box's words, under its row, with a way to try again.
    click(cx, "first-run-account-poe-act");
    tick(cx);
    assert!(shown(cx, "sign-in-failure") && shown(cx, "sign-in-again"));
    click(cx, "first-run-account-poe-cancel");
    assert!(!shown(cx, "sign-in"));
    // One under way is given up on its engine as another opens, and by Cancel.
    click(cx, "first-run-agent-antigravity-act");
    assert!(shown(cx, "sign-in-waiting"));
    click(cx, "first-run-agent-antigravity-cancel");
    assert!(!shown(cx, "sign-in"));
    assert_eq!(stand_in.seen().iter().filter(|seen| seen.method == "DELETE" && seen.path.contains("/integration/antigravity/connect/oauth/")).count(), 1);
    assert!(shown(cx, "first-run-agent-antigravity-act"), "still to sign in again");
    assert_eq!(step(&workspace, cx), Step::Agents);
    // Going on with one open gives it up too.
    click(cx, "first-run-agent-antigravity-act");
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Where);
    assert_eq!(stand_in.seen().iter().filter(|seen| seen.method == "DELETE" && seen.path.contains("/integration/antigravity/connect/oauth/")).count(), 2);
}

#[gpui::test]
fn where_a_model_runs_already_the_welcome_goes_straight_to_where_to_work(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Running);
    let (workspace, cx) = open(cx, stand_in.clone());
    assert!(shown(cx, "first-run-signed-in"), "the welcome says what's signed in");
    // Return is the way on.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(step(&workspace, cx), Step::Where);
    // The agents and plans are a step back, not gone.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(step(&workspace, cx), Step::Agents);
    assert!(shown(cx, "first-run-agent-claude-code-signed-in"));
    click(cx, "first-run-next");
    // The folder worked in latest is picked to begin with; another is a click.
    assert_eq!(run(&workspace, cx).read_with(cx, |run, _| run.place().cloned()), Some(StartIn::Project("/Users/sam/Projects/website".into())));
    click(cx, "first-run-folder-1");
    assert_eq!(run(&workspace, cx).read_with(cx, |run, _| run.place().cloned()), Some(StartIn::Project("/Users/sam/Projects/api".into())));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.welcomed());
    assert!(shown(cx, "rail-settings"));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.landing_place()), StartIn::Project("/Users/sam/Projects/api".into()));
    let landing = workspace.read_with(cx, |workspace, _| workspace.landing().clone());
    assert!(cx.update(|window, cx| landing.focus_handle(cx).contains_focused(window, cx)), "the keyboard is in the composer");
}

#[gpui::test]
fn no_project_starts_the_first_session_as_a_chat(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Running);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "first-run-next");
    click(cx, "first-run-no-project");
    click(cx, "first-run-next");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.landing_place()), StartIn::Chat);
    cx.simulate_input("What can you do?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let started: Vec<_> = stand_in.seen_at("POST", "kuvo/sessions").into_iter().filter_map(|seen| seen.body).collect();
    assert_eq!(started[0]["chat"], json!(true));
}

#[gpui::test]
fn a_machine_picked_says_it_cant_be_worked_on_from_here_yet(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Running);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "first-run-next");
    assert!(shown(cx, "first-run-computer-this") && shown(cx, "first-run-computer-key_devbox"));
    assert!(!shown(cx, "first-run-computer-key_build"), "a machine that isn't yours isn't offered");
    click(cx, "first-run-computer-key_devbox");
    // Said plainly, with no folders of this Mac's passed off as the machine's, and no way on.
    assert!(shown(cx, "first-run-elsewhere") && !shown(cx, "first-run-folder-0"));
    click(cx, "first-run-next");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(step(&workspace, cx), Step::Where);
    assert!(!stand_in.welcomed());
    // Back on this Mac, its folders and the way on are there again.
    click(cx, "first-run-computer-this");
    assert!(shown(cx, "first-run-folder-0") && !shown(cx, "first-run-elsewhere"));
    click(cx, "first-run-next");
    assert!(stand_in.welcomed());
}

#[gpui::test]
fn a_box_that_turns_out_older_holds_the_first_run_until_it_has_updated(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Crowded);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Agents);
    // Its server started again on an older Kuvo: the first run says so, with nothing to go on to.
    stand_in.set_older(true);
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "first-run-held"));
    assert!(!shown(cx, "first-run-next") && !shown(cx, "first-run-agent-cursor-act") && !shown(cx, "rail-settings"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(step(&workspace, cx), Step::Agents);
    assert!(!stand_in.welcomed());
    // Updated: it carries on where it was.
    stand_in.set_older(false);
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(!shown(cx, "first-run-held") && shown(cx, "first-run-agent-cursor-act"));
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Where);
}

#[gpui::test]
fn what_the_box_cant_say_is_said_with_a_way_to_ask_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Running);
    stand_in.set_first_run(|first_run| first_run.folders_fail = true);
    stand_in.agents_running(false);
    let (workspace, cx) = open(cx, stand_in.clone());
    // The coding agents' bridge isn't running: not skipped past, and said in the box's words,
    // with the engine's plans still there.
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Agents);
    assert!(shown(cx, "first-run-down") && shown(cx, "first-run-account-github-copilot-signed-in"));
    click(cx, "first-run-next");
    // The folders couldn't be read: No project can still be picked, and Try Again reads them.
    assert!(shown(cx, "first-run-folders-failed") && shown(cx, "first-run-no-project"));
    stand_in.set_first_run(|first_run| first_run.folders_fail = false);
    click(cx, "first-run-folders-failed-again");
    assert!(shown(cx, "first-run-folder-0") && !shown(cx, "first-run-folders-failed"));
}

/// The window as the app starts it: the box not reached yet, nothing asked.
fn starting(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(1400.)));
    workspace.update_in(cx, |workspace, _, cx| workspace.await_box(cx));
    cx.run_until_parked();
    (workspace, cx)
}

#[gpui::test]
fn until_the_box_has_said_the_window_waits_rather_than_show_the_app_and_take_it_away(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Nothing);
    let (workspace, cx) = starting(cx, stand_in.clone());
    // Being reached: neither the app nor the welcome yet.
    assert!(shown(cx, "window-waiting") && !shown(cx, "rail-settings") && !shown(cx, "first-run"));
    // It can't be reached: said, still without the app a first run would then take away.
    workspace.update_in(cx, |workspace, _, cx| workspace.connected(Err(kuvo_box::Error::NotRunning("Kuvo Code isn’t answering on this Mac.".into())), cx));
    cx.run_until_parked();
    assert!(shown(cx, "window-waiting-why") && !shown(cx, "rail-settings") && !shown(cx, "first-run"));
    // Reached: it says the first run hasn't been had, and the welcome is what shows.
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "first-run-card") && !shown(cx, "window-waiting") && !shown(cx, "rail-settings"));
}

#[gpui::test]
fn a_box_youve_had_it_with_opens_the_app_once_reached(cx: &mut TestAppContext) {
    let (workspace, cx) = starting(cx, StandIn::at(Replies::Never, NOW));
    assert!(shown(cx, "window-waiting"));
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "rail-settings") && !shown(cx, "window-waiting") && !shown(cx, "first-run"));
}

#[gpui::test]
fn a_box_held_for_its_update_at_the_start_says_so_in_the_app(cx: &mut TestAppContext) {
    // It can't say whether the first run's been had: the window's own notice, not a welcome.
    let (workspace, cx) = starting(cx, StandIn::held_older(Replies::Never));
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "held-box-notice") && !shown(cx, "window-waiting") && !shown(cx, "first-run"));
}

#[gpui::test]
fn a_box_that_couldnt_say_once_is_asked_again_without_the_window_going_empty(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Running);
    stand_in.set_first_run(|first_run| first_run.setup_fails = true);
    let (workspace, cx) = starting(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    // It answered, though not what was asked: the app, as it would be.
    assert!(shown(cx, "rail-settings") && !shown(cx, "window-waiting"));
    // Asked again as it's next read, the window never emptied meanwhile; it says the first run
    // hasn't been had, and that's shown.
    stand_in.set_first_run(|first_run| first_run.setup_fails = false);
    assert!(!workspace.read_with(cx, |workspace, _| workspace.awaits_box()));
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "first-run-card"));
}

#[gpui::test]
fn signed_in_with_a_key_alone_the_welcome_lists_it_and_goes_straight_on(cx: &mut TestAppContext) {
    // No plan signed in, but a key whose model runs: nothing to draw an empty row for.
    let stand_in = StandIn::first_run(Replies::Never, NOW, Scenario::Running);
    for agent in ["claude-code", "codex-cli", "antigravity"] {
        stand_in.set_account_said(agent, json!({ "state": "signedOut", "line": "Not signed in", "actions": [{ "id": "signIn", "label": "Sign In" }] }));
    }
    for account in ["github-copilot", "gitlab"] {
        stand_in.set_account_said(account, json!({ "state": "signedOut", "line": "Sign in", "actions": [{ "id": "signIn", "label": "Sign In" }] }));
    }
    let (workspace, cx) = open(cx, stand_in);
    assert!(shown(cx, "first-run-signed-in"), "OpenRouter's key is what's ready");
    click(cx, "first-run-next");
    assert_eq!(step(&workspace, cx), Step::Where);
}
