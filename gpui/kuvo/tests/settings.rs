//! Settings as someone uses it, against the stand-in box (nothing real is reached, no key is
//! real): your own settings and a computer's, each with its sidebar; the computer picker (a
//! machine, one that's away); pages drawn from the box's description, each change going to the
//! box from the row's revision; the search, on its side and across; and
//! signing in each way the box offers (in the browser, with a device code, a code pasted back, a
//! key, credentials with a form, an agent installed first), failing, giving up, and signing out.
//! Clicks and keys go through the window; a sign-in is followed on the test's own clock.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::settings::{SettingsView, Side};
use kuvo::settings::sign_in::{POLL, Step};
use kuvo::stand_in::settings::Ending;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
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
    // (Active, as the app's window is when you use it: a field is only left in an active one.)
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

fn settings(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<SettingsView> {
    workspace.read_with(cx, |workspace, cx| workspace.area_view(Area::Settings).and_then(|view| view.read(cx).settings().cloned()).expect("Settings"))
}

/// Settings, on the side `side`, over the row at its sidebar's foot where it's on the other.
fn side(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, side: Side) -> Entity<SettingsView> {
    if workspace.read_with(cx, |workspace, _| workspace.area()) != Area::Settings {
        click(cx, "rail-settings");
    }
    let view = settings(workspace, cx);
    if view.read_with(cx, |view, _| view.side()) != side {
        click(cx, "settings-foot");
    }
    view
}

/// The settings of the computer shown, on `name`, picked in its sidebar.
fn section(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, name: &str) -> Entity<SettingsView> {
    let view = side(workspace, cx, Side::Computer);
    let row = format!("setting-{}", name.to_lowercase());
    // (One that isn't in the desktop app yet is under their one row.)
    if !shown(cx, &row) {
        click(cx, "settings-later");
    }
    click(cx, &row);
    view
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.title(cx).0.map(|title| title.to_string()))
}

fn step(view: &Entity<SettingsView>, cx: &mut VisualTestContext) -> Option<Step> {
    view.read_with(cx, |view, cx| view.sign_in().map(|sheet| sheet.read(cx).step().clone()))
}

/// The sign-in's clock moves on: the box is asked how it goes.
fn tick(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(POLL);
    cx.run_until_parked();
}

fn bodies(stand_in: &StandIn, method: &str, route: &str) -> Vec<serde_json::Value> {
    stand_in.seen_at(method, route).into_iter().filter_map(|seen| seen.body).collect()
}

#[gpui::test]
fn settings_opens_on_your_own_and_the_foot_row_goes_to_the_computers(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-settings");
    // Your settings first: this app's own, which ask no box anything.
    assert!(shown(cx, "settings-yours-title") && shown(cx, "settings-search"));
    assert!(shown(cx, "row-editor"), "General first");
    assert_eq!(title(&workspace, cx).as_deref(), Some("General"));
    assert!(stand_in.seen_at("GET", "kuvo/settings").is_empty());
    // One of your own that isn't in the desktop app yet says so.
    // What isn't in the desktop app yet is one row, opening its short list; each says so.
    assert!(!shown(cx, "setting-voice"));
    click(cx, "settings-later");
    click(cx, "setting-voice");
    assert!(shown(cx, "placeholder"));
    // Folded again, the one shown stays listed.
    click(cx, "settings-later");
    assert!(shown(cx, "setting-voice"));
    // The foot row goes to the computer's: its picker as the title, its sections as its box says them.
    click(cx, "settings-foot");
    assert!(shown(cx, "settings-computer") && !shown(cx, "settings-yours-title"));
    assert_eq!(stand_in.seen_at("GET", "kuvo/settings").len(), 1);
    assert!(shown(cx, "row-time-zone"), "its Clock");
    // Only the page on screen asks the box anything.
    assert!(stand_in.seen_at("GET", "kuvo/settings/browser").is_empty());
    for (name, row) in [("Accounts", "account-claude-code"), ("Usage", "usage-0"), ("Models", "model-0"), ("Permissions", "row-sandbox"), ("Browser", "row-search-engine")] {
        section(&workspace, cx, name);
        assert!(shown(cx, row), "{name} shows {row}");
        assert_eq!(title(&workspace, cx).as_deref(), Some(name));
    }
    // A section neither the box describes nor this app has yet says so.
    section(&workspace, cx, "Git");
    assert!(shown(cx, "placeholder"));
    // Back goes to the section before; and the foot row back to your own, where you were.
    cx.simulate_keystrokes("cmd-[");
    cx.run_until_parked();
    assert!(shown(cx, "row-search-engine"));
    click(cx, "settings-foot");
    assert!(shown(cx, "settings-yours-title") && shown(cx, "placeholder"), "Keyboard Shortcuts, as it was left");
}

#[gpui::test]
fn the_picker_shows_another_computers_settings_and_leaves_the_window(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Permissions");
    assert!(shown(cx, "row-allowed-alw_1"));
    click(cx, "settings-computer");
    click(cx, "menu-item-devbox");
    // The same section, read from the machine's own box: its folders, its name in its words.
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    assert_eq!(devbox.seen_at("GET", "kuvo/settings/permissions").len(), 1);
    assert!(shown(cx, "row-allowed-alw_srv") && !shown(cx, "row-allowed-alw_1"));
    let view = settings(&workspace, cx);
    assert_eq!(view.read_with(cx, |view, _| view.computer_name()), "devbox");
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-other-computers")).as_deref(), Some("Their bots and agents can start sessions on devbox."));
    // Only Settings changed: the window still shows this Mac.
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.box_name().to_string()), "This Mac");
    // A change there goes to the machine's box, never this Mac's.
    click(cx, "row-other-computers-switch");
    assert_eq!(bodies(&devbox, "PATCH", "kuvo/settings/permissions").len(), 1);
    assert!(stand_in.seen_at("PATCH", "kuvo/settings/permissions").is_empty());
    // Its own pages (Accounts) are that box's too.
    section(&workspace, cx, "Accounts");
    assert_eq!(devbox.seen_at("GET", "kuvo/sign-in/accounts").len(), 2, "each side's, from devbox");
    // Back goes to this Mac's settings again.
    section(&workspace, cx, "Permissions");
    click(cx, "settings-computer");
    click(cx, "menu-item-This Mac");
    assert!(shown(cx, "row-allowed-alw_1"));
}

#[gpui::test]
fn a_computer_thats_away_or_not_reached_says_so_and_is_asked_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Browser");
    click(cx, "settings-computer");
    click(cx, "menu-item-studio-mini");
    assert!(shown(cx, "settings-away") && !shown(cx, "row-search-engine"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("studio-mini"));
    // In the box's words about its settings, not the Computers area's about what it runs.
    let view = settings(&workspace, cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.away_words()),
        Some(("studio-mini can’t be reached".to_owned(), "Its settings are kept on studio-mini, so they can’t be read or changed until Studio hears from it again. They show here by themselves once it’s back.".to_owned()))
    );
    assert!(!shown(cx, "setting-browser") && !shown(cx, "settings-search"), "none of its sections are listed");
    // One on an older Kuvo says what brings it up to date.
    click(cx, "settings-computer");
    click(cx, "menu-item-old-mini");
    assert!(shown(cx, "settings-away"));
    // One this app can't reach from here yet says that plainly, and nothing is read through this Mac.
    stand_in.forget_peers();
    let before = stand_in.seen().len();
    click(cx, "settings-computer");
    click(cx, "menu-item-devbox");
    assert!(shown(cx, "settings-away"));
    assert_eq!(stand_in.seen().len(), before);
    // Add a Machine… goes to the Computers area's.
    click(cx, "settings-computer");
    click(cx, "menu-item-Add a Machine…");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Computers);
    assert!(shown(cx, "add-machine-page"));
}

#[gpui::test]
fn the_rails_accounts_and_usage_are_the_windows_computers_whichever_settings_shows(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Browser");
    click(cx, "settings-computer");
    click(cx, "menu-item-studio-mini");
    assert!(shown(cx, "settings-away"));
    click(cx, "rail-sessions");
    // Accounts, from anywhere in the window, is this window's computer's.
    cx.dispatch_action(kuvo::area::ShowAccounts);
    cx.run_until_parked();
    assert!(shown(cx, "account-claude-code") && !shown(cx, "settings-away"));
    assert_eq!(settings(&workspace, cx).read_with(cx, |view, _| view.computer_name()), "This Mac");
}

#[gpui::test]
fn a_computer_removed_while_its_settings_show_gives_way_to_the_windows(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Permissions");
    click(cx, "settings-computer");
    click(cx, "menu-item-devbox");
    assert!(shown(cx, "row-allowed-alw_srv"));
    // Removed (from another of your computers, say): the list comes back without it.
    stand_in.computers_alone();
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    let view = settings(&workspace, cx);
    assert_eq!(view.read_with(cx, |view, _| view.computer_name()), "This Mac");
    assert!(shown(cx, "row-allowed-alw_1") && !shown(cx, "row-allowed-alw_srv"), "this Mac's own page, read afresh");
    // A change now is this Mac's, never the one that left's.
    click(cx, "row-other-computers-switch");
    assert_eq!(stand_in.seen_at("PATCH", "kuvo/settings/permissions").len(), 1);
    assert!(stand_in.peer_box("key_devbox").expect("devbox").seen_at("PATCH", "kuvo/settings/permissions").is_empty());
    // Back, to the place kept from when it was there, is the window's too: nothing waits forever.
    cx.simulate_keystrokes("cmd-[");
    cx.run_until_parked();
    assert!(!shown(cx, "settings-away") && !shown(cx, "note-sections"));
}

#[gpui::test]
fn sections_that_couldnt_be_read_say_so_and_are_asked_for_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.settings_readable(false);
    let (workspace, cx) = open(cx, stand_in.clone());
    side(&workspace, cx, Side::Computer);
    assert!(shown(cx, "note-sections") && shown(cx, "settings-sections-problem"));
    // What the desktop app draws itself still opens.
    click(cx, "setting-accounts");
    assert!(shown(cx, "account-claude-code"));
    // The box answers again: coming back to Settings reads them.
    stand_in.settings_readable(true);
    click(cx, "rail-sessions");
    click(cx, "rail-settings");
    assert!(shown(cx, "setting-permissions") && !shown(cx, "settings-sections-problem"));
}

#[gpui::test]
fn a_computer_without_the_open_section_shows_its_clock(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (added, page) = kuvo::stand_in::settings::every_kind();
    stand_in.add_settings_section(added, page);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "samples");
    click(cx, "settings-computer");
    click(cx, "menu-item-devbox");
    // devbox's box has no such section: its Clock, and no page that never comes.
    assert_eq!(title(&workspace, cx).as_deref(), Some("Clock"));
    assert!(shown(cx, "row-time-zone"));
}

#[gpui::test]
fn a_row_that_cant_be_changed_here_says_why_and_sends_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Permissions");
    // Full Access is turned on only in a conversation: drawn, held, with the box's why.
    assert!(shown(cx, "row-full-access-why"));
    assert_eq!(view.read_with(cx, |view, _| view.row_why("row-full-access")).as_deref(), Some("Turn it on in a conversation: pick Full Access in its mode menu, and confirm it there."));
    click(cx, "row-full-access-switch");
    assert!(stand_in.seen_at("PATCH", "kuvo/settings/permissions").is_empty());
}

#[gpui::test]
fn a_row_changed_since_it_was_read_is_turned_down_and_the_page_shows_what_it_is_now(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Browser");
    // Another app turned it off meanwhile.
    stand_in.set_browser_setting("tidiesAgentTabs", json!(false));
    click(cx, "row-tidies-switch");
    assert!(shown(cx, "settings-problem"));
    assert_eq!(view.read_with(cx, |view, _| view.problem().map(str::to_owned)).as_deref(), Some("It changed since you read it."));
    // The page is the box's now: the next flip is from its revision, and turns it on.
    click(cx, "row-tidies-switch");
    let sent = bodies(&stand_in, "PATCH", "kuvo/settings/browser");
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0]["value"].clone(), sent[1]["value"].clone()), (json!(false), json!(true)));
    assert_ne!(sent[0]["was"], sent[1]["was"]);
    assert!(!shown(cx, "settings-problem"));
    // A change to one row never turns down another's.
    stand_in.set_browser_setting("offersToSave", json!(false));
    click(cx, "row-search-engine-menu");
    click(cx, "menu-item-Kagi");
    assert!(!shown(cx, "settings-problem"));
}

/// A row of every kind a box may describe, drawn and changed with nothing known of what it is.
#[gpui::test]
fn every_kind_of_row_is_drawn_from_the_boxs_description(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (added, page) = kuvo::stand_in::settings::every_kind();
    stand_in.add_settings_section(added, page);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "samples");
    for row in ["row-proxy", "row-keep", "row-projects", "row-token", "row-key", "row-reads", "row-rules", "row-docs", "row-pair", "row-background", "row-later", "row-cache", "row-hosts", "row-hosts-empty"] {
        assert!(shown(cx, row), "{row}");
    }
    let last = |stand_in: &StandIn| bodies(stand_in, "PATCH", "kuvo/settings/samples").last().cloned().unwrap_or_default();
    // A field: typed, and saved with Return, from the row's revision.
    click(cx, "row-proxy-field");
    cx.simulate_input("  http://proxy.example:8080  ");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!((last(&stand_in)["row"].clone(), last(&stand_in)["value"].clone()), (json!("proxy"), json!("http://proxy.example:8080")));
    assert!(last(&stand_in)["was"].as_str().is_some_and(|was| !was.is_empty()));
    // A number's field takes a number: words that aren't one are said, and not sent.
    click(cx, "row-keep-field");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("soon");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.problem().map(str::to_owned)).as_deref(), Some("Keep sessions for takes a number."));
    click(cx, "row-keep-field");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("14");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(last(&stand_in)["value"], json!(14.0));
    // A replacement typed, then Cancel (or leaving it): nothing's sent, and it isn't kept.
    click(cx, "row-token-replace");
    click(cx, "row-token-field");
    cx.simulate_input("wrong-paste");
    click(cx, "settings-whose");
    click(cx, "row-token-cancel");
    assert!(bodies(&stand_in, "PATCH", "kuvo/settings/samples").iter().all(|sent| sent["row"] != json!("token")));
    // A secret that's set shows no field until Replace; what's typed is sent and never drawn again.
    assert!(!shown(cx, "row-token-field"));
    click(cx, "row-token-replace");
    click(cx, "row-token-field");
    cx.simulate_input("tok-123");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!((last(&stand_in)["row"].clone(), last(&stand_in)["value"].clone()), (json!("token"), json!("tok-123")));
    assert!(!shown(cx, "row-token-field") && shown(cx, "row-token-replace"));
    click(cx, "row-token-remove");
    assert_eq!(last(&stand_in)["value"], json!(null));
    assert!(shown(cx, "row-token-field"), "none set: its field");
    // An action that asks first, in the box's words; one that can't be done from here is drawn, with why.
    click(cx, "row-cache-clear");
    assert_eq!(view.read_with(cx, |view, _| view.confirm_words()), Some(("Clear the cache?".into(), "The next build takes longer.".into(), "Clear".into())));
    assert!(stand_in.seen_at("POST", "kuvo/settings/samples/cache/clear").is_empty());
    click(cx, "confirm-go");
    assert_eq!(stand_in.seen_at("POST", "kuvo/settings/samples/cache/clear").len(), 1);
    click(cx, "row-cache-show");
    assert!(stand_in.seen_at("POST", "kuvo/settings/samples/cache/show").is_empty());
    assert_eq!(view.read_with(cx, |view, _| view.row_why("row-cache")).as_deref(), Some("Only on that computer itself."));
    // What only an installed app does says the box's why; a page opens; a section opens in Settings.
    assert_eq!(view.read_with(cx, |view, _| view.row_why("row-background")).as_deref(), Some("Turn it on in Kuvo Code on that computer."));
    click(cx, "row-docs");
    assert_eq!(cx.opened_url().as_deref(), Some("https://www.kuvo.run/code/docs/settings/"));
    click(cx, "row-rules");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Permissions"));
}


#[gpui::test]
fn the_search_finds_sections_and_rows(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    section(&workspace, cx, "Models");
    click(cx, "settings-search");
    cx.simulate_input("search engine");
    cx.run_until_parked();
    assert!(shown(cx, "setting-browser"));
    assert!(!shown(cx, "setting-models") && !shown(cx, "setting-accounts"), "sections with nothing found leave");
    // The page open had nothing: the first that has is shown, with only what was found.
    assert!(shown(cx, "row-search-engine"));
    assert!(!shown(cx, "row-tidies"));
    // An account's name finds it too.
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("copilot");
    cx.run_until_parked();
    assert!(shown(cx, "setting-accounts"));
    assert!(shown(cx, "account-github-copilot") && !shown(cx, "account-openrouter"));
    // A word the box gave for a section (not on its page) finds it whole.
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("passwords");
    cx.run_until_parked();
    assert!(shown(cx, "setting-browser") && shown(cx, "row-tidies"));
    // Escape clears it: every section again.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(shown(cx, "setting-models") && shown(cx, "settings-later"));
}

#[gpui::test]
fn a_search_lists_what_it_finds_on_the_other_side_and_goes_there(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    // In your settings, something that's the computer's: listed under it, with where.
    let view = side(&workspace, cx, Side::Yours);
    click(cx, "settings-search");
    cx.simulate_input("sandbox");
    cx.run_until_parked();
    assert!(!shown(cx, "setting-appearance"), "nothing of your own matches");
    assert!(shown(cx, "settings-hit-0"));
    click(cx, "settings-hit-0");
    assert_eq!(view.read_with(cx, |view, _| view.side()), Side::Computer);
    assert_eq!(title(&workspace, cx).as_deref(), Some("Permissions"));
    assert!(shown(cx, "row-sandbox") && shown(cx, "row-default-mode"), "the whole page: the search was left behind");
    // In the computer's, something that's yours.
    click(cx, "settings-search");
    cx.simulate_input("dark");
    cx.run_until_parked();
    click(cx, "settings-hit-0");
    assert_eq!(view.read_with(cx, |view, _| view.side()), Side::Yours);
    assert!(shown(cx, "row-theme"));
}

// MARK: Accounts

#[gpui::test]
fn accounts_lists_the_agents_and_providers_as_the_box_says_them(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Accounts");
    for row in ["account-claude-code", "account-claude-code@w0rk1a2b", "account-codex-cli", "account-cursor", "account-grok-cli", "account-github-copilot", "account-anthropic", "account-groq"] {
        assert!(shown(cx, row), "{row}");
    }
    // Only those the box says Accounts lists (`listed`): a web search service, an MCP server's
    // sign-in and the engine's own way to Claude's plan are left out, as it says.
    for left_out in ["account-tavily", "account-linear", "account-claude-agent"] {
        assert!(!shown(cx, left_out), "{left_out}");
    }
    // Each one's mark is the box's `service`; one it names none for shows its letter.
    for marked in ["account-claude-code", "account-codex-cli", "account-github-copilot", "account-openrouter", "account-anthropic"] {
        assert!(shown(cx, &format!("{marked}-mark")), "{marked}");
    }
    assert!(shown(cx, "account-gitlab-initial") && !shown(cx, "account-gitlab-mark"));
    // Signed in, ended, never: each says so, with the box's ways.
    assert!(shown(cx, "account-antigravity-sign-in-again"), "an agent whose sign-in ended");
    assert!(shown(cx, "account-gitlab-sign-in-again") && shown(cx, "account-gitlab-disconnect"), "an engine plan the box says ended");
    assert!(shown(cx, "account-cursor-sign-in"));
    assert!(shown(cx, "account-grok-cli-install"));
    assert!(shown(cx, "account-anthropic-sign-in"));
    assert!(!shown(cx, "account-groq-sign-in"), "no way in to offer: only its variable");
    // Keys are entered, never signed into (#733): a key-only account's button is the box's Add Key.
    for key_only in ["openai", "deepseek", "mistral"] {
        assert!(shown(cx, &format!("account-{key_only}-add-key")) && !shown(cx, &format!("account-{key_only}-sign-in")), "{key_only}");
    }
    // Nor are the computer's credentials (#734): Amazon Bedrock's button is the box's Connect.
    assert!(shown(cx, "account-amazon-bedrock-connect") && !shown(cx, "account-amazon-bedrock-sign-in"));
    // What each offers is the box's `actions`, no more: the agent's own sign-in has no Remove
    // Account (only one you added does), and one signed in from where nothing's kept, no Disconnect.
    click(cx, "account-claude-code-menu");
    assert!(shown(cx, "menu-item-Sign In Again…") && !shown(cx, "menu-item-Remove Account…"));
    cx.simulate_keystrokes("escape");
    assert!(shown(cx, "account-github-copilot-disconnect") && !shown(cx, "account-github-copilot-sign-in-again"));
    // The coding agents not running: the box's words, and the engine's accounts still.
    stand_in.agents_running(false);
    section(&workspace, cx, "Usage");
    section(&workspace, cx, "Accounts");
    assert!(shown(cx, "agents-down"));
    assert!(shown(cx, "account-github-copilot"));
}

/// The coding agents not running on a computer (PROTOCOL.md, "The coding agents' state"): Accounts
/// and Usage say the box's one sentence, once, with no words of the app's in front of it; Try
/// Again posts the box's route and reads the page again. Restarting is a quiet line, not a warning.
/// While the box says the coding agents restart, Accounts reads again a moment on by itself, and
/// lists them once they're back: the restarting line never stays after they are.
#[gpui::test]
fn accounts_read_again_while_the_agents_restart_and_list_them_once_back(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.agents_restarting();
    let client = BoxClient::new(stand_in.clone());
    // Live: read on threads of their own, as the app reads, with its timers.
    let window = cx.add_window(move |window, cx| Workspace::new(client, true, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.executor().allow_parking();
    cx.simulate_resize(size(px(1280.), px(1800.)));
    cx.run_until_parked();
    side(&workspace, cx, Side::Computer);
    cx.run_until_parked();
    click(cx, "setting-accounts");
    cx.run_until_parked();
    assert!(shown(cx, "note-agents-restarting") && !shown(cx, "account-claude-code"));
    // Back: read again a moment on, by itself.
    stand_in.agents_running(true);
    cx.executor().advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    assert!(shown(cx, "account-claude-code") && !shown(cx, "note-agents-restarting"));
}

#[gpui::test]
fn the_coding_agents_down_say_the_boxs_sentence_once_with_try_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let words = "The coding agents keep stopping. Kuvo tries again in about 5 minutes.";
    stand_in.agents_down(words, true);
    stand_in.agents_start_works(false);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    let page = view.read_with(cx, |view, _| view.page_words());
    assert_eq!(page.matches(words).count(), 1, "said once: {page}");
    assert!(!page.contains("aren’t running on") && !page.contains("isn’t running on"), "no words of the app's own: {page}");
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("agents-down")).as_deref(), Some(words));
    // The engine's accounts are still there.
    assert!(shown(cx, "account-github-copilot") && !shown(cx, "account-claude-code"));

    // Try Again goes to the box's route; still down, its sentence as the box says it now.
    let later = "The coding agents didn't finish starting.";
    stand_in.agents_down(later, true);
    click(cx, "agents-down-try-again");
    assert_eq!(stand_in.seen_at("POST", "kuvo/agents/start").len(), 1);
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("agents-down")).as_deref(), Some(later));
    assert_eq!(view.read_with(cx, |view, _| view.problem().map(str::to_owned)), None, "its state is the page's, not a problem over it");

    // Usage says the same in the limits' place, with its Try Again, and no notices' error.
    let usage = section(&workspace, cx, "Usage");
    let page = usage.read_with(cx, |view, _| view.page_words());
    assert_eq!(page.matches(later).count(), 1, "said once: {page}");
    assert!(!page.contains("couldn’t be read"), "{page}");
    assert!(shown(cx, "agents-down-try-again"));

    // This time it starts them: the page is read again, the coding agents' accounts listed.
    stand_in.agents_start_works(true);
    let view = section(&workspace, cx, "Accounts");
    click(cx, "agents-down-try-again");
    assert_eq!(stand_in.seen_at("POST", "kuvo/agents/start").len(), 2);
    assert!(!shown(cx, "agents-down") && shown(cx, "account-claude-code"));

    // Down with nothing to try: the sentence alone, no button.
    stand_in.agents_down("The coding agents aren't installed here yet. Kuvo's next update brings them.", false);
    section(&workspace, cx, "Usage");
    section(&workspace, cx, "Accounts");
    assert!(shown(cx, "agents-down") && !shown(cx, "agents-down-try-again"));

    // Restarting: a quiet line in the box's words, not a warning, with no Try Again.
    stand_in.agents_restarting();
    section(&workspace, cx, "Usage");
    section(&workspace, cx, "Accounts");
    assert!(shown(cx, "note-agents-restarting") && !shown(cx, "agents-down"));
    assert_eq!(view.read_with(cx, |view, _| view.page_words()).matches("The coding agents are restarting. This takes a few seconds.").count(), 1);
}

#[gpui::test]
fn signing_in_in_the_browser_follows_the_box_until_it_goes_through(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-gitlab-sign-in-again");
    // Its one way starts at once: the page opens, and it waits.
    assert!(matches!(step(&view, cx), Some(Step::Waiting { .. })));
    // Started through the box, which starts it on its engine.
    let started = stand_in.seen_at("POST", "kuvo/sign-in/start");
    assert_eq!(started.iter().map(|seen| (seen.path.as_str(), seen.body.clone())).collect::<Vec<_>>(), vec![("kuvo/sign-in/start?integration=gitlab", Some(json!({ "methodID": "browser" })))]);
    assert!(stand_in.seen_at("POST", "api/integration/gitlab/connect/oauth").is_empty());
    assert!(cx.opened_url().is_some_and(|url| url.starts_with("https://sign-in.stand-in.invalid/gitlab")));
    assert!(shown(cx, "sign-in-waiting"));
    tick(cx);
    assert!(stand_in.seen_at("GET", "kuvo/sign-in/status").len() == 1, "asked how it goes");
    assert!(matches!(step(&view, cx), Some(Step::Waiting { .. })));
    stand_in.end_sign_ins(Ending::Complete, None);
    tick(cx);
    assert_eq!(step(&view, cx), Some(Step::Done));
    click(cx, "sign-in-done");
    assert!(!shown(cx, "sign-in"));
    // Read again: signed in, no longer ended.
    assert!(!shown(cx, "account-gitlab-sign-in-again"));
    assert!(shown(cx, "account-gitlab-disconnect"));
}

#[gpui::test]
fn a_device_code_is_shown_and_copied_after_disconnecting(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Accounts");
    // Disconnect is asked first, then goes to the box's route.
    click(cx, "account-github-copilot-disconnect");
    assert!(shown(cx, "settings-confirm"));
    assert!(stand_in.seen_at("POST", "kuvo/accounts/github-copilot/forget").is_empty());
    click(cx, "confirm-go");
    assert_eq!(stand_in.seen_at("POST", "kuvo/accounts/github-copilot/forget").len(), 1);
    // Now under Add a Provider: Sign In, and its device code, as the box says it apart when it
    // starts the sign-in (the engine leaves it in its page's address, which this never reads).
    let view = settings(&workspace, cx);
    click(cx, "account-github-copilot-sign-in");
    assert_eq!(stand_in.seen_at("POST", "kuvo/sign-in/start").len(), 1);
    assert!(matches!(step(&view, cx), Some(Step::Waiting { attempt, .. }) if attempt.user_code.as_deref() == Some("WDJB-MJHT")));
    assert!(shown(cx, "sign-in-user-code"));
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("WDJB-MJHT"), "copied for the page");
}

#[gpui::test]
fn a_code_pasted_back_finishes_an_agents_sign_in(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-claude-code-menu");
    click(cx, "menu-item-Sign In Again…");
    assert!(matches!(step(&view, cx), Some(Step::Waiting { .. })));
    let started: Vec<String> = stand_in.seen_at("POST", "kuvo/sign-in/start").into_iter().map(|seen| seen.path).collect();
    assert_eq!(started, vec!["kuvo/sign-in/start?integration=claude-code&agents=1".to_owned()], "through the box, on the agents' bridge");
    click(cx, "sign-in-paste");
    cx.simulate_input("wrong-code");
    click(cx, "sign-in-send-code");
    assert!(shown(cx, "sign-in-code-error"), "turned down, in the box's words");
    assert!(matches!(step(&view, cx), Some(Step::Waiting { .. })));
    click(cx, "sign-in-code");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("stand-in-code");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(step(&view, cx), Some(Step::Done));
}

#[gpui::test]
fn a_key_turned_down_stays_with_why_and_a_good_one_connects(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-openai-add-key");
    assert_eq!(view.read_with(cx, |view, cx| view.sign_in().map(|sheet| sheet.read(cx).title())), Some("Add Key · OpenAI".into()), "the box's title, with the account's own name");
    assert!(matches!(step(&view, cx), Some(Step::Key(_))), "its only way is a key");
    cx.simulate_input("not-a-real-key");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(matches!(step(&view, cx), Some(Step::Key(_))));
    let sheet = view.read_with(cx, |view, _| view.sign_in().cloned().unwrap());
    let error = sheet.read_with(cx, |sheet, cx| sheet.key_field().read(cx).error().map(|error| error.to_string()));
    assert_eq!(error.as_deref(), Some("That key wasn’t accepted (401 Unauthorized)."));
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("sk-stand-in-key");
    click(cx, "sign-in-connect");
    assert_eq!(step(&view, cx), Some(Step::Done));
    assert_eq!(bodies(&stand_in, "POST", "api/integration/openai/connect/key").last(), Some(&json!({ "key": "sk-stand-in-key" })));
}

#[gpui::test]
fn credentials_on_the_computer_take_their_forms_answer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-amazon-bedrock-connect");
    assert_eq!(view.read_with(cx, |view, cx| view.sign_in().map(|sheet| sheet.read(cx).title())), Some("Connect · Amazon Bedrock".into()), "the box's title, with the account's own name");
    assert_eq!(step(&view, cx), Some(Step::Choose), "two ways: chosen");
    assert!(shown(cx, "sign-in-way-0") && shown(cx, "sign-in-way-1"));
    click(cx, "sign-in-way-0");
    assert!(matches!(step(&view, cx), Some(Step::Form(_))), "its region first");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("eu-west-1");
    click(cx, "sign-in-continue");
    assert_eq!(step(&view, cx), Some(Step::Done));
    assert_eq!(bodies(&stand_in, "POST", "api/integration/amazon-bedrock/connect/external"), vec![json!({ "methodID": "aws", "answer": { "region": "eu-west-1" } })]);
}

#[gpui::test]
fn the_ways_in_come_in_the_boxs_order_with_legacy_ones_apart(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-anthropic-sign-in");
    let labels = view.read_with(cx, |view, cx| view.sign_in().unwrap().read(cx).ways().primary().map(|way| way.label.clone()).collect::<Vec<_>>());
    assert_eq!(labels, ["Claude Pro/Max", "Create an API Key", "Manually enter API Key"]);
    assert!(!shown(cx, "sign-in-way-3"), "the legacy way waits under Other ways");
    click(cx, "sign-in-others");
    assert!(shown(cx, "sign-in-way-3"));
}

#[gpui::test]
fn a_sign_in_turned_down_says_why_in_the_boxs_words(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-poe-sign-in");
    tick(cx);
    assert_eq!(step(&view, cx), Some(Step::Failed("The sign-in was turned down on its page. Try again to sign in.".into())));
    assert!(shown(cx, "sign-in-failure"));
    // Try Again starts over from its ways.
    click(cx, "sign-in-again");
    assert!(matches!(step(&view, cx), Some(Step::Waiting { .. })));
}

#[gpui::test]
fn cancelling_a_sign_in_gives_it_up_on_the_engine(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-cursor-sign-in");
    click(cx, "sign-in-way-0");
    assert!(matches!(step(&view, cx), Some(Step::Waiting { .. })));
    click(cx, "sign-in-cancel");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/agents/api/integration/cursor/connect/oauth/att_1").len(), 1);
    assert_eq!(step(&view, cx), Some(Step::Choose));
    // Closed: no more asking.
    click(cx, "sign-in-x");
    let asked = stand_in.seen_at("GET", "kuvo/sign-in/status").len();
    tick(cx);
    assert_eq!(stand_in.seen_at("GET", "kuvo/sign-in/status").len(), asked);
}

#[gpui::test]
fn an_agent_not_installed_is_installed_then_signed_in(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-grok-cli-install");
    assert_eq!(stand_in.seen_at("POST", "kuvo/agents/api/integration/grok-cli/install").len(), 1);
    assert!(matches!(step(&view, cx), Some(Step::Waiting { .. })), "on to signing in");
}

#[gpui::test]
fn an_agent_signs_out_and_an_added_account_is_removed_asked_first(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    // The agent's own sign-in: Sign Out, asked first in the box's words (Codex's is yours, so
    // Terminal's goes too, and it says so). Nothing's sent until it's confirmed.
    click(cx, "account-codex-cli-menu");
    click(cx, "menu-item-Sign Out…");
    let (title, message, button) = view.read_with(cx, |view, _| view.confirm_words()).expect("Sign Out asks first");
    assert_eq!((title.as_str(), button.as_str()), ("Sign out of Codex?", "Sign Out"));
    assert!(message.contains("in Terminal too"), "{message}");
    assert!(stand_in.seen_at("POST", "kuvo/agents/api/integration/codex-cli/disconnect").is_empty());
    click(cx, "confirm-go");
    assert_eq!(stand_in.seen_at("POST", "kuvo/agents/api/integration/codex-cli/disconnect").len(), 1);
    assert!(shown(cx, "account-codex-cli-sign-in"));
    // One you added: no Sign Out of its own, only Remove Account, which says it's forgotten.
    click(cx, "account-claude-code@w0rk1a2b-menu");
    assert!(!shown(cx, "menu-item-Sign Out…"));
    click(cx, "menu-item-Remove Account…");
    assert!(shown(cx, "settings-confirm"));
    let (title, message, button) = view.read_with(cx, |view, _| view.confirm_words()).expect("Remove asks first");
    assert_eq!((title.as_str(), button.as_str()), ("Remove Claude Code · Work?", "Remove Account"));
    assert!(message.contains("signed out and forgotten"), "{message}");
    click(cx, "confirm-go");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/agents/api/kuvo/accounts/claude-code%40w0rk1a2b").len(), 1);
    assert!(!shown(cx, "account-claude-code@w0rk1a2b"));
}

// MARK: The other sections

#[gpui::test]
fn models_are_set_in_this_computers_settings_from_what_was_read(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Models");
    click(cx, "model-2-menu");
    click(cx, "model-rail-openai");
    click(cx, "model-row-openai/gpt-6");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/config"), vec![json!({ "changes": [{ "path": ["subagent_model"], "value": "openai/gpt-6", "was": null }] })]);
    // The box's answer is shown: the menu names it.
    click(cx, "model-2-menu");
    click(cx, "model-row-unset");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/config").last(), Some(&json!({ "changes": [{ "path": ["subagent_model"], "value": null, "was": "openai/gpt-6" }] })));
}

#[gpui::test]
fn permissions_are_the_boxs_page_and_change_through_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Permissions");
    assert!(shown(cx, "row-default-mode") && shown(cx, "row-full-access"));
    click(cx, "row-other-computers-switch");
    let sent = bodies(&stand_in, "PATCH", "kuvo/settings/permissions");
    assert_eq!((sent[0]["row"].clone(), sent[0]["value"].clone()), (json!("other-computers"), json!(true)));
    // A choice, by the box's ids.
    click(cx, "row-when-shell-menu");
    click(cx, "menu-item-Ask First");
    let sent = bodies(&stand_in, "PATCH", "kuvo/settings/permissions");
    assert_eq!((sent[1]["row"].clone(), sent[1]["value"].clone()), (json!("when-shell"), json!("ask")));
    // A list's item does what it offers.
    click(cx, "row-allowed-alw_1-remove");
    assert_eq!(stand_in.seen_at("POST", "kuvo/settings/permissions/allowed-alw_1/remove").len(), 1);
    assert!(!shown(cx, "row-allowed-alw_1") && shown(cx, "row-allowed-alw_2"));
}

/// The sandbox's row is the box's alone: its words, and its button, which asks the box, or opens
/// what the box names (a page anywhere; its own computer's Software Update only beside it).
#[gpui::test]
fn the_sandbox_is_drawn_in_the_boxs_words(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Permissions");
    let line = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| settings(workspace, cx).read_with(cx, |view, _| view.row_detail("row-sandbox"));
    assert_eq!(line(&workspace, cx).as_deref(), Some("Agents’ commands here run in it."));
    assert!(!shown(cx, "row-sandbox-try-again"));

    stand_in.set_sandbox(json!({ "on": false, "needsYou": true, "able": true, "agents": false, "why": "Its check failed: a command could write outside its folder.", "reason": "checkFailed", "action": "retry", "route": "POST kuvo/sandbox/check", "actionLabel": "Try Again" }));
    section(&workspace, cx, "Browser");
    section(&workspace, cx, "Permissions");
    assert_eq!(line(&workspace, cx).as_deref(), Some("Its check failed: a command could write outside its folder."));
    click(cx, "row-sandbox-try-again");
    assert_eq!(stand_in.seen_at("POST", "kuvo/settings/permissions/sandbox/retry").len(), 1, "asked of the box");

    // A page the box names: opened here, nothing sent.
    stand_in.set_sandbox(json!({ "on": false, "needsYou": true, "able": false, "agents": false, "why": "Linux 5.4 can't sandbox commands.", "reason": "kernel", "action": "help", "actionLabel": "How to Check…", "opens": "https://docs.stand-in.invalid/sandbox", "needs": "Linux 5.13" }));
    section(&workspace, cx, "Browser");
    section(&workspace, cx, "Permissions");
    click(cx, "row-sandbox-how-to-check");
    assert_eq!(cx.opened_url().as_deref(), Some("https://docs.stand-in.invalid/sandbox"), "the address the box named, none of this app's own");
    assert!(stand_in.seen_at("POST", "kuvo/settings/permissions/sandbox/help").is_empty());

    // The computer's own Software Update: opened where this app runs beside its box; from
    // another computer the box says why not, and nothing opens.
    let update = json!({ "on": false, "needsYou": true, "able": false, "agents": false, "why": "macOS's sandbox isn't on this Mac.", "reason": "macos", "action": "softwareUpdate", "actionLabel": "Software Update…", "opens": "x-apple.systempreferences:stand-in", "opensHere": true });
    stand_in.set_sandbox(update);
    section(&workspace, cx, "Browser");
    section(&workspace, cx, "Permissions");
    click(cx, "row-sandbox-software-update");
    assert_eq!(cx.opened_url().as_deref(), Some("x-apple.systempreferences:stand-in"));
    stand_in.set_elsewhere(true);
    section(&workspace, cx, "Browser");
    section(&workspace, cx, "Permissions");
    assert!(shown(cx, "row-sandbox-why"), "why it can't be done from here");
    // Nothing but a web page is opened at the word of a box on another computer.
    for address in ["file:///Users/sam/run.command", "x-apple.systempreferences:stand-in"] {
        stand_in.set_sandbox(json!({ "on": false, "needsYou": true, "able": false, "agents": false, "why": "It can't run here.", "reason": "kernel", "action": "help", "actionLabel": "How to Check…", "opens": address }));
        section(&workspace, cx, "Browser");
        section(&workspace, cx, "Permissions");
        let opened = cx.opened_url();
        click(cx, "row-sandbox-how-to-check");
        assert_eq!(cx.opened_url(), opened, "{address}");
    }
}

#[gpui::test]
fn usage_notices_change_as_one_part_from_the_version_read(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Usage");
    assert!(shown(cx, "note-unavailable-0"), "a provider that can't send them, in the box's words");
    click(cx, "notice-close-switch");
    assert_eq!(
        bodies(&stand_in, "PUT", "kuvo/agents/api/kuvo/usage/settings"),
        vec![json!({ "notifications": { "underTenPercent": true, "healthyToClose": true, "closeToRunningOut": true }, "based": { "notifications": "3/mac" } })]
    );
    // The next change is based on the version the box answered.
    click(cx, "notice-almost-out-switch");
    assert_eq!(bodies(&stand_in, "PUT", "kuvo/agents/api/kuvo/usage/settings").last().and_then(|body| body.pointer("/based/notifications").cloned()), Some(json!("4/mac")));
}

#[gpui::test]
fn the_browsers_settings_change_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Browser");
    click(cx, "row-search-engine-menu");
    click(cx, "menu-item-Kagi");
    click(cx, "row-tidies-switch");
    let sent: Vec<(serde_json::Value, serde_json::Value)> = bodies(&stand_in, "PATCH", "kuvo/settings/browser").into_iter().map(|body| (body["row"].clone(), body["value"].clone())).collect();
    assert_eq!(sent, vec![(json!("search-engine"), json!("kagi")), (json!("tidies"), json!(false))]);
    click(cx, "row-never-bank.example-offer-again");
    assert_eq!(stand_in.seen_at("POST", "kuvo/settings/browser/never-bank.example/offer-again").len(), 1);
    assert!(!shown(cx, "row-never-bank.example") && shown(cx, "row-never-intranet.example"));
}

#[gpui::test]
fn the_theme_row_switches_light_and_dark(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    side(&workspace, cx, Side::Yours);
    click(cx, "setting-appearance");
    click(cx, "row-theme-menu");
    click(cx, "menu-item-Dark");
    assert!(cx.update(|_, cx| kuvo_ui::ActiveTheme::theme(cx).appearance.is_dark()));
}

#[gpui::test]
fn usage_wraps_equal_width_cards_and_stacks_their_limits(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Usage");
    for row in ["usage-0", "usage-1", "usage-2", "usage-3", "usage-4", "usage-5"] {
        assert!(shown(cx, row), "{row}");
    }
    // At this width two accounts fit, align at the top, and retain their natural heights.
    let [first, second, third] = [0, 1, 2].map(|ix| bounds(cx, &format!("usage-{ix}")).expect("usage card"));
    assert_eq!(first.origin.y, second.origin.y);
    assert!(first.origin.x < second.origin.x);
    assert!(first.size.height > second.size.height);
    assert_eq!(second.origin.x, third.origin.x);
    assert_eq!(third.origin.y, second.origin.y + second.size.height + px(12.));
    assert!(third.origin.y < first.origin.y + first.size.height);
    assert_eq!(first.size.width, second.size.width);
    assert_eq!(first.size.width, third.size.width);
    let [session, weekly, sonnet] = [0, 1, 2].map(|ix| bounds(cx, &format!("usage-0-window-{ix}")).expect("usage window"));
    assert_eq!(session.origin.x, weekly.origin.x);
    assert_eq!(session.origin.x, sonnet.origin.x);
    assert_eq!(session.size.width, sonnet.size.width);
    assert!(session.origin.y < weekly.origin.y && weekly.origin.y < sonnet.origin.y);

    cx.simulate_resize(size(px(1600.), px(1800.)));
    cx.run_until_parked();
    let [first, second, third, fourth] = [0, 1, 2, 3].map(|ix| bounds(cx, &format!("usage-{ix}")).expect("usage card"));
    assert!(first.origin.x < second.origin.x && second.origin.x < third.origin.x);
    assert_eq!(first.origin.y, third.origin.y);
    assert_eq!(first.size.width, third.size.width);
    assert!(fourth.origin.y > first.origin.y);
    assert!(!shown(cx, "usage-0-window-3"));
    // Cursor's plan ended: listed, saying so, with no meters.
    assert!(shown(cx, "usage-1") && !shown(cx, "usage-1-window-0"));
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("usage-2")).as_deref(), Some("mini-local"));
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("usage-0")).as_deref(), Some(""));
    assert!(!shown(cx, "settings-whose"));
    assert!(shown(cx, "usage-updated"));
    click(cx, "usage-refresh");
    assert_eq!(stand_in.seen_at("POST", "kuvo/agents/api/kuvo/limits/refresh").len(), 1);
}

#[gpui::test]
fn usage_on_a_first_run_says_theres_nothing_yet(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::empty(Replies::Never));
    section(&workspace, cx, "Usage");
    assert!(shown(cx, "note-usage-none"));
}

#[gpui::test]
fn a_change_the_box_turns_down_is_said_over_the_page(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Models");
    // Changed on its computer meanwhile: refused, and read again.
    stand_in.set_config_value(&["model"], json!("google/gemini-3-pro"));
    click(cx, "model-0-menu");
    click(cx, "model-rail-openai");
    click(cx, "model-row-openai/gpt-6");
    assert!(shown(cx, "settings-problem"));
    assert_eq!(stand_in.seen_at("GET", "kuvo/config").len(), 2);
    click(cx, "settings-problem-close");
    assert!(!shown(cx, "settings-problem"));
}

#[gpui::test]
fn the_page_keeps_its_place_when_a_row_changes(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    cx.simulate_resize(size(px(1280.), px(700.)));
    cx.run_until_parked();
    section(&workspace, cx, "Accounts");
    // Scrolled down to the providers.
    let page = bounds(cx, "settings-page").unwrap();
    cx.simulate_event(gpui::ScrollEvent { position: page.center(), delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-700.))), ..Default::default() });
    cx.run_until_parked();
    let before = bounds(cx, "account-openrouter").expect("scrolled to it");
    click(cx, "account-github-copilot-disconnect");
    click(cx, "confirm-go");
    let after = bounds(cx, "account-openrouter").expect("still on screen");
    assert!(after.origin.y < px(400.), "not back at the top: {after:?} (was {before:?})");
}

#[gpui::test]
fn closing_a_sign_in_under_way_gives_it_up_and_reads_accounts_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "Accounts");
    click(cx, "account-gitlab-sign-in-again");
    let reads = stand_in.seen_at("GET", "kuvo/sign-in/accounts").len();
    click(cx, "sign-in-x");
    assert_eq!(stand_in.seen_at("DELETE", "api/integration/gitlab/connect/oauth/att_1").len(), 1);
    assert!(stand_in.seen_at("GET", "kuvo/sign-in/accounts").len() > reads, "read again however it closed");
    let asked = stand_in.seen_at("GET", "kuvo/sign-in/status").len();
    tick(cx);
    assert_eq!(stand_in.seen_at("GET", "kuvo/sign-in/status").len(), asked, "no more asking");
}

#[gpui::test]
fn escape_closes_the_sign_in_and_the_confirm(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    section(&workspace, cx, "Accounts");
    click(cx, "account-anthropic-sign-in");
    assert!(shown(cx, "sign-in"));
    // The sidebar waits under the dimming (drawn as the sheet opens, by the area's own view): a
    // section clicked there isn't opened.
    assert!(shown(cx, "settings-sidebar-dim"));
    click(cx, "setting-clock");
    assert!(shown(cx, "sign-in") && !shown(cx, "row-theme"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "sign-in"));
    click(cx, "account-github-copilot-disconnect");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "settings-confirm"));
}

#[gpui::test]
fn a_sign_in_the_engine_forgot_is_given_up_after_a_while(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Accounts");
    click(cx, "account-gitlab-sign-in-again");
    stand_in.forget_sign_ins();
    for _ in 0..9 {
        tick(cx);
    }
    assert!(matches!(step(&view, cx), Some(Step::Waiting { .. })), "a few not heard is waited through");
    tick(cx);
    assert_eq!(step(&view, cx), Some(Step::Failed("The engine started over while you were signing in. Try again.".into())));
}

/// What an account says and offers is the box's answer, drawn as it comes: its line, its buttons'
/// words, and a confirm in the box's words. An action this app has no control for, or one that
/// only works on the box's computer while this app isn't there, isn't drawn, and no rule of this
/// app's fills in for what the box didn't say.
#[gpui::test]
fn an_account_is_drawn_from_the_boxs_state_line_and_actions(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let again = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| {
        section(workspace, cx, "Usage");
        section(workspace, cx, "Accounts")
    };
    let asks = json!({ "id": "disconnect", "label": "Forget This Plan", "confirm": { "title": "Forget it?", "message": "The box’s own words.", "label": "Forget" } });
    // A plan that isn't signed in, said by the box to have ended, with words of its own.
    stand_in.set_account_said("anthropic", json!({ "state": "ended", "line": "The box’s line", "actions": [{ "id": "signInAgain", "label": "Sign In Once More", "sheetTitle": "Continue with Anthropic" }, asks] }));
    let view = again(&workspace, cx);
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("account-anthropic")).as_deref(), Some("The box’s line"));
    assert!(shown(cx, "account-anthropic-sign-in-once-more") && shown(cx, "account-anthropic-forget-this-plan"));
    click(cx, "account-anthropic-forget-this-plan");
    assert_eq!(view.read_with(cx, |view, _| view.confirm_words()), Some(("Forget it?".into(), "The box’s own words.".into(), "Forget".into())));
    assert!(stand_in.seen_at("POST", "kuvo/accounts/anthropic/forget").is_empty(), "nothing sent until it's confirmed");
    cx.simulate_keystrokes("escape");

    click(cx, "account-anthropic-sign-in-once-more");
    assert_eq!(view.read_with(cx, |view, cx| view.sign_in().map(|sheet| sheet.read(cx).title())), Some("Continue with Anthropic".into()));
    cx.simulate_keystrokes("escape");

    // Signed in with several: a menu, the one that asks first marked by this app, not in its label.
    stand_in.set_account_said("anthropic", json!({ "state": "signedIn", "line": "Signed in", "actions": [{ "id": "signInAgain", "label": "Sign In Again", "sheetTitle": "Sign In to Anthropic" }, asks] }));
    again(&workspace, cx);
    click(cx, "account-anthropic-menu");
    assert!(shown(cx, "menu-item-Sign In Again…") && shown(cx, "menu-item-Forget This Plan…"));
    cx.simulate_keystrokes("escape");

    // A signed-in plan the box offers nothing this app has a control for: nothing drawn, though
    // it's a saved sign-in this app once offered Disconnect for by itself.
    stand_in.set_account_said("github-copilot", json!({ "actions": [{ "id": "somethingNew", "label": "A Newer Thing" }, { "id": "update", "label": "Update to 9" }, { "id": "addAnother", "label": "Add Another Account" }] }));
    again(&workspace, cx);
    assert!(shown(cx, "account-github-copilot"));
    for button in ["account-github-copilot-disconnect", "account-github-copilot-menu", "account-github-copilot-a-newer-thing", "account-github-copilot-update-to-9", "account-github-copilot-add-another-account"] {
        assert!(!shown(cx, button), "{button}");
    }

    // A sign-in whose page comes back to the box's computer: only where this app runs there.
    stand_in.set_account_said("cursor", json!({ "actions": [{ "id": "signIn", "label": "Sign In", "sheetTitle": "Sign In to Cursor", "here": true }] }));
    again(&workspace, cx);
    assert!(shown(cx, "account-cursor-sign-in"));
    stand_in.set_elsewhere(true);
    again(&workspace, cx);
    assert!(shown(cx, "account-cursor") && !shown(cx, "account-cursor-sign-in"));
}

/// The app's way: the box asked off the window's thread, its answers coming in later.
mod live {
    use super::*;
    use gpui::{AppContext as _, Context, IntoElement, ParentElement, Render, Styled, Window, div};
    use kuvo::settings::Pick;

    // These tests exercise asynchronous requests on GPUI's executor. A streaming
    // transport also starts a real follow thread, whose channel wakes are outside
    // the test scheduler. Use the supported request/response path so every answer
    // is driven by that scheduler, including pages discovered during a search.
    struct RequestBox(StandIn);

    impl kuvo_box::transport::Transport for RequestBox {
        fn send(&self, request: &kuvo_box::transport::Request) -> kuvo_box::Result<kuvo_box::transport::Response> {
            kuvo_box::transport::Transport::send(&self.0, request)
        }

        fn open(&self, _: &kuvo_box::transport::Request, _: &dyn Fn(kuvo_box::transport::Closer) -> bool) -> kuvo_box::Result<kuvo_box::transport::Stream> {
            panic!("these tests use scheduler-driven requests")
        }

        fn streams(&self) -> bool { false }
        fn here(&self) -> bool { kuvo_box::transport::Transport::here(&self.0) }
        fn describe(&self) -> String { kuvo_box::transport::Transport::describe(&self.0) }
    }

    struct Host {
        settings: Entity<SettingsView>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let sidebar = self.settings.update(cx, |settings, cx| settings.sidebar(cx));
            div().size_full().flex().child(div().w(px(272.)).h_full().child(sidebar)).child(div().flex_1().h_full().child(self.settings.clone()))
        }
    }

    #[gpui::test]
    fn a_search_typed_before_the_pages_are_read_finds_them_as_they_come(cx: &mut TestAppContext) {
        cx.update(|cx| {
            Theme::init(Appearance::Light, cx);
            kuvo::bind_keys(cx);
        });
        let client = BoxClient::new(RequestBox(StandIn::at(Replies::Never, NOW)));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let settings = cx.new(|cx| SettingsView::new(client, true, cx));
            cx.subscribe(&settings, |host: &mut Host, _, pick: &Pick, cx| host.settings.update(cx, |settings, cx| settings.set_place(&pick.place, cx))).detach();
            Host { settings }
        });
        cx.simulate_resize(size(px(1280.), px(1400.)));
        // The computer's settings, opened and searched before its box has said anything.
        host.update(cx, |host, cx| host.settings.update(cx, |settings, cx| settings.set_place("Clock", cx)));
        click(cx, "settings-search");
        cx.simulate_input("copilot");
        cx.run_until_parked();
        assert!(shown(cx, "setting-accounts"));
        assert!(shown(cx, "account-github-copilot"), "the page with it, once Accounts is read");
        let _ = host;
    }

    /// Install in the app's own way, its answers coming in later: installed, the sign-in opens,
    /// from what the box says of the account after the install (before it, only Install was offered).
    #[gpui::test]
    fn an_agent_installed_goes_on_to_signing_in_once_its_box_offers_it(cx: &mut TestAppContext) {
        cx.update(|cx| {
            Theme::init(Appearance::Light, cx);
            kuvo::bind_keys(cx);
        });
        let stand_in = StandIn::at(Replies::Never, NOW);
        let client = BoxClient::new(RequestBox(stand_in.clone()));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let settings = cx.new(|cx| SettingsView::new(client, true, cx));
            cx.subscribe(&settings, |host: &mut Host, _, pick: &Pick, cx| host.settings.update(cx, |settings, cx| settings.set_place(&pick.place, cx))).detach();
            Host { settings }
        });
        cx.simulate_resize(size(px(1280.), px(1400.)));
        host.update(cx, |host, cx| host.settings.update(cx, |settings, cx| settings.set_place("Accounts", cx)));
        cx.run_until_parked();
        click(cx, "account-grok-cli-install");
        cx.run_until_parked();
        assert_eq!(stand_in.seen_at("POST", "kuvo/agents/api/integration/grok-cli/install").len(), 1);
        assert!(shown(cx, "sign-in"), "on to signing in");
        assert_eq!(stand_in.seen_at("POST", "kuvo/sign-in/start").len(), 1, "its sign-in started");
    }
}

#[gpui::test]
fn lowering_a_budget_asks_before_sending_and_cancel_sends_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(json!({"id":"storage", "title":"Storage", "group":"System", "icon":"drive"}), json!({
        "id":"storage","title":"Storage","groups":[{"id":"copies","rows":[{
            "id":"budget","title":"Budget","kind":"number","value":2000000000,"can":true,"unit":{"name":"GiB","factor":1073741824,"decimals":2,"rounding":"nearest"},
            "confirm":{"title":"Ordinary question","body":"Generic words.","button":"Save"},
            "confirmBelow":{"value":1500000000,"confirm":{"title":"Keep less?","body":"Old copies go.","button":"Keep"}}
        }]}]
    }));
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "storage");
    click(cx, "row-budget-field");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("0.5");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.confirm_words()), Some(("Keep less?".into(), "Old copies go.".into(), "Keep 0.5 GiB".into())));
    assert!(bodies(&stand_in,"PATCH","kuvo/settings/storage").is_empty());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(bodies(&stand_in,"PATCH","kuvo/settings/storage").is_empty());
    click(cx, "row-budget-field");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    click(cx,"confirm-go");
    let sent = bodies(&stand_in,"PATCH","kuvo/settings/storage");
    assert_eq!(sent.len(),1);
    assert_eq!(sent[0]["value"],json!(536870912.0));
    assert_eq!(sent[0]["confirmed"],json!(true));
}

#[gpui::test]
fn a_described_list_search_filters_items_without_sending_a_change(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(json!({"id":"archived","title":"Archived Sessions","group":"System","icon":"archive"}), json!({
        "id":"archived","title":"Archived Sessions","groups":[{"id":"sessions","title":"Kuvo Code","rows":[{
            "id":"sessions","title":"Archived Sessions","kind":"list",
            "search":{"placeholder":"Search sessions…","empty":"No sessions match {query}."},
            "items":[{"id":"alpha","title":"Alpha","line":"First project","kind":"status","value":3}, {"id":"beta","title":"Beta","line":"Second project","kind":"status","value":4}]
        }]}]
    }));
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace,cx,"archived");
    assert!(shown(cx,"row-alpha") && shown(cx,"row-beta"));
    click(cx,"row-sessions-field");
    cx.simulate_input("Second");
    cx.run_until_parked();
    assert!(!shown(cx,"row-alpha") && shown(cx,"row-beta"));
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("archived");
    cx.run_until_parked();
    assert!(shown(cx,"row-alpha") && shown(cx,"row-beta"));
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("missing");
    cx.run_until_parked();
    assert!(!shown(cx,"row-alpha") && !shown(cx,"row-beta") && shown(cx,"row-sessions-empty"));
    assert!(bodies(&stand_in,"PATCH","kuvo/settings/archived").is_empty());
}

#[gpui::test]
fn a_rounded_unit_field_sends_nothing_untouched_and_ordinary_confirmation_has_no_attestation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(json!({"id":"storage","title":"Storage","group":"System","icon":"drive"}), json!({
        "id":"storage","title":"Storage","groups":[{"id":"copies","rows":[{
            "id":"budget","title":"Budget","kind":"number","value":1073741831,"can":true,
            "unit":{"name":"GiB","factor":1073741824,"decimals":2,"rounding":"nearest"},
            "confirm":{"title":"Save?","body":"Keep the budget.","button":"Save"}
        }]}]
    }));
    let (workspace,cx) = open(cx,stand_in.clone());
    let view = section(&workspace,cx,"storage");
    // Return, or leaving it, as it is: nothing to save.
    click(cx,"row-budget-field");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    click(cx,"settings-whose");
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.confirm_words()).is_none());
    assert!(bodies(&stand_in,"PATCH","kuvo/settings/storage").is_empty());
    click(cx,"row-budget-field");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("2");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    click(cx,"confirm-go");
    let sent = bodies(&stand_in,"PATCH","kuvo/settings/storage");
    assert_eq!(sent.len(),1);
    assert_eq!(sent[0]["value"],json!(2147483648.0));
    assert!(sent[0].get("confirmed").is_none());
}

#[gpui::test]
fn a_folder_picker_browses_the_selected_computers_box_and_cancel_sends_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never,NOW);
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    devbox.add_settings_section(json!({"id":"worktrees","title":"Worktrees","group":"Projects","icon":"folder"}), json!({
        "id":"worktrees","title":"Worktrees","groups":[{"id":"place","rows":[{
            "id":"place","title":"Folder","kind":"folder","value":"","can":true,
            "folder":{"choose":"Choose a Folder…","clear":"Default","prompt":"Choose","message":"Choose where worktrees go."}
        }]}]
    }));
    let (workspace,cx)=open(cx,stand_in.clone());
    section(&workspace,cx,"Permissions");
    click(cx,"settings-computer");
    click(cx,"menu-item-devbox");
    section(&workspace,cx,"worktrees");
    click(cx,"row-place-browse");
    assert!(shown(cx,"settings-folder-picker"));
    assert!(!devbox.seen_at("GET","kuvo/folders").is_empty());
    assert!(stand_in.seen_at("GET","kuvo/folders").is_empty());
    click(cx,"settings-folder-cancel");
    assert!(bodies(&devbox,"PATCH","kuvo/settings/worktrees").is_empty());
    click(cx,"row-place-browse");
    click(cx,"settings-folder-2");
    click(cx,"settings-folder-parent");
    click(cx,"settings-folder-2");
    click(cx,"settings-folder-choose");
    let sent=bodies(&devbox,"PATCH","kuvo/settings/worktrees");
    assert_eq!(sent.len(),1);
    assert!(sent[0]["value"].as_str().unwrap().ends_with("/Documents"));
}

#[gpui::test]
fn multiline_instructions_keep_whitespace_and_return_adds_a_line(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never,NOW);
    stand_in.add_settings_section(json!({"id":"git","title":"Git","group":"Projects","icon":"branch"}),json!({
        "id":"git","title":"Git","groups":[{"id":"instructions","rows":[{"id":"instructions","title":"Instructions","kind":"text","can":true,"value":"","lines":{"min":3,"max":8}}]}]
    }));
    let (workspace,cx)=open(cx,stand_in.clone());
    section(&workspace,cx,"git");
    click(cx,"row-instructions-field");
    cx.simulate_input("  First");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Second  ");
    assert!(bodies(&stand_in,"PATCH","kuvo/settings/git").is_empty());
    // Saved as it's left: a press anywhere else on the page.
    click(cx,"settings-whose");
    cx.run_until_parked();
    let sent=bodies(&stand_in,"PATCH","kuvo/settings/git");
    assert_eq!(sent[0]["value"],json!("  First\nSecond  "));
}

#[gpui::test]
fn single_line_text_trims_whitespace_and_refreshes_canonical_value(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(json!({"id":"git","title":"Git","group":"System","icon":"branch"}), json!({
        "id":"git","title":"Git","groups":[{"id":"prefix","rows":[{"id":"prefix","title":"Branch prefix","kind":"text","can":true,"value":"feature/","revision":"rev-1"}]}]
    }));
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "git");
    click(cx, "row-prefix-field");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("  release/  ");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = bodies(&stand_in, "PATCH", "kuvo/settings/git");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["value"], json!("release/"));
    let shown_text = view.read_with(cx, |view, cx| view.field("row-prefix").unwrap().read(cx).text().to_owned());
    assert_eq!(shown_text, "release/");
}

#[gpui::test]
fn matching_list_heading_puts_search_first_without_a_duplicate_heading_row(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(json!({"id":"archived","title":"Archived Sessions","group":"System","icon":"archive"}), json!({
        "id":"archived","title":"Archived Sessions","groups":[{"id":"project-kuvo","title":"Kuvo Code","rows":[{
            "id":"sessions","title":"Kuvo Code","kind":"list",
            "search":{"placeholder":"Search archived sessions","empty":"No archived session matches “{query}”."},
            "items":[{"id":"alpha","title":"Alpha","line":"Kuvo Code","kind":"status","value":3}]
        }]}]
    }));
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "archived");
    assert!(!shown(cx, "row-sessions-heading"));
    assert!(shown(cx, "row-sessions-field"));
    assert!(shown(cx, "row-alpha"));
}

/// Settings › This Mac is the box's page of how Kuvo stands on this computer: drawn from its
/// description, each row's title saying how it stands, with no word of a box anywhere.
#[gpui::test]
fn this_mac_says_how_kuvo_stands_here_and_what_macos_allows_its_helper(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_permissions(|permissions| *permissions = kuvo::stand_in::permissions::Permissions { accessibility: true, later: vec!["screenRecording".into()], ..kuvo::stand_in::permissions::Permissions::nothing_allowed() });
    stand_in.set_computer(|computer| computer.at_login = false);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Computer");
    assert_eq!(title(&workspace, cx).as_deref(), Some("This Mac"));
    assert!(shown(cx, "row-state") && shown(cx, "row-version") && shown(cx, "row-devices-switch"));
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-state")).as_deref(), Some("Kuvo Helper keeps it running."));
    // What needs you is there to look up, each with its way to it, though "Not Now" put the
    // heads-up off: Login Items off, and a permission that isn't allowed.
    assert!(!shown(cx, "permission-heads-up"));
    assert!(shown(cx, "row-login-items-open-login-items") && shown(cx, "row-allows-screenRecording-open-system-settings"));
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-allows-screenRecording")).as_deref(), Some("Not allowed. Agents can’t look at the screen."));
    // One that's allowed, and one macOS asks about itself, offer nothing.
    assert!(shown(cx, "row-allows-accessibility") && !shown(cx, "row-allows-accessibility-open-system-settings") && shown(cx, "row-allows-files"));
    // Login Items opens its own pane in System Settings.
    click(cx, "row-login-items-open-login-items");
    assert_eq!(cx.opened_url().as_deref(), Some("x-apple.systempreferences:com.apple.LoginItems-Settings.extension"));
}

#[gpui::test]
fn let_my_devices_connect_is_asked_first_as_its_turned_off_and_restart_says_what_stops(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_computer(|computer| computer.working = true);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Computer");
    // On for a new Mac. Turning it off is asked first, in the box's words; Cancel sends nothing.
    assert!(stand_in.computer().devices);
    click(cx, "row-devices-switch");
    assert_eq!(view.read_with(cx, |view, _| view.confirm_words()).map(|(title, _, button)| (title, button)), Some(("Stop letting your devices connect?".into(), "Turn Off".into())));
    assert!(bodies(&stand_in, "PATCH", "kuvo/settings/computer").is_empty());
    click(cx, "confirm-go");
    assert!(!stand_in.computer().devices);
    // Back on, with nothing to ask.
    click(cx, "row-devices-switch");
    assert!(stand_in.computer().devices && !shown(cx, "settings-confirm"));

    // Restart says agents are at work before it stops them.
    click(cx, "row-state-restart");
    assert_eq!(view.read_with(cx, |view, _| view.confirm_words()), Some(("Restart Kuvo on this Mac?".into(), "Agents are at work on this Mac now: what they’re doing stops. Kuvo is back in a moment.".into(), "Restart".into())));
    assert_eq!(stand_in.computer().restarted, 0);
    click(cx, "confirm-go");
    assert_eq!(stand_in.computer().restarted, 1);
}

#[gpui::test]
fn a_newer_version_is_said_in_the_rows_title_and_update_now_takes_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_computer(|computer| computer.ready = Some("1.0.1".into()));
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "Computer");
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-version")).as_deref(), Some("It installs when nothing’s at work, or now."));
    // (Nothing at work: nothing to ask first.)
    click(cx, "row-version-update-now");
    assert_eq!(stand_in.computer().asked_to_update, 1);
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-version")).as_deref(), Some("Up to date. It updates itself when nothing’s at work."));
}

/// From another computer's app, This Mac's page reads the same under that Mac's name, and what
/// opens only on that Mac says so instead of offering it.
#[gpui::test]
fn another_computers_page_is_named_for_it_and_opens_nothing_of_its_own_here(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_elsewhere(true);
    stand_in.set_permissions(|permissions| *permissions = kuvo::stand_in::permissions::Permissions::nothing_allowed());
    let (workspace, cx) = open(cx, stand_in);
    let view = section(&workspace, cx, "Computer");
    assert_ne!(title(&workspace, cx).as_deref(), Some("This Mac"));
    assert!(view.read_with(cx, |view, _| view.row_why("row-allows-screenRecording")).is_some_and(|why| why.starts_with("Open it on ")));
}

// MARK: Flow sections (`settings.flows`): MCP Servers, drawn from the box's description alone.

/// The form open over the page, read for what it shows.
fn form_words(view: &Entity<SettingsView>, cx: &mut VisualTestContext) -> Vec<(String, String)> {
    view.read_with(cx, |view, cx| view.form().and_then(|form| form.read(cx).checked().cloned()).map(|checked| checked.fields.into_iter().map(|each| (each.id, each.words)).collect()).unwrap_or_default())
}

#[gpui::test]
fn a_flow_sections_things_show_how_each_stands_in_the_boxs_words(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_mcp();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "mcp");
    assert_eq!(title(&workspace, cx).as_deref(), Some("MCP Servers"));
    for (row, badge) in [("row-server-files", true), ("row-server-linear", true), ("row-server-postgres", true), ("row-server-sentry", true)] {
        assert!(shown(cx, row) && shown(cx, &format!("{row}-badge")) == badge, "{row}");
    }
    // Its error, set apart under it; what it offers now as a button, what asks first in its menu.
    assert!(shown(cx, "row-server-postgres-notice"));
    assert!(shown(cx, "row-server-postgres-try-again") && shown(cx, "row-server-postgres-menu") && shown(cx, "row-server-postgres-switch"));
    assert!(shown(cx, "row-server-linear-sign-in"));
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-server-files")).as_deref(), Some("npx -y @modelcontextprotocol/server-filesystem ~/Documents"));
    // Turned off from its switch, from the row's revision.
    click(cx, "row-server-files-switch");
    let sent = bodies(&stand_in, "PATCH", "kuvo/settings/mcp");
    assert_eq!((sent[0]["row"].clone(), sent[0]["value"].clone()), (json!("server-files"), json!(false)));
    // Removed, asked first in the box's words.
    click(cx, "row-server-sentry-menu");
    click(cx, "menu-item-Remove…");
    assert_eq!(view.read_with(cx, |view, _| view.confirm_words()), Some(("Remove sentry?".into(), "The agent loses its tools.".into(), "Remove".into())));
    click(cx, "confirm-go");
    assert!(!shown(cx, "row-server-sentry"));
}

#[gpui::test]
fn add_servers_form_says_the_boxs_words_as_its_typed_and_adds_one(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_mcp();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "mcp");
    click(cx, "row-servers-add-add-server");
    assert!(shown(cx, "settings-form"));
    // As it opens: nothing to send yet, and only the fields for a command.
    assert!(!view.read_with(cx, |view, cx| view.form().unwrap().read(cx).is_ready()));
    assert!(shown(cx, "settings-form-command") && !shown(cx, "settings-form-url"));
    click(cx, "settings-form-name-field");
    cx.simulate_input("my files");
    assert!(form_words(&view, cx).contains(&("name".into(), "It'll be called my-files.".into())));
    click(cx, "settings-form-command-field");
    cx.simulate_input("npx -y @scope/files ~/Docs");
    assert!(form_words(&view, cx).contains(&("command".into(), "npx is on this Mac.".into())));
    // On the web shows its URL instead; a word alone isn't an address.
    click(cx, "settings-form-runs-remote");
    assert!(shown(cx, "settings-form-url") && !shown(cx, "settings-form-command"));
    click(cx, "settings-form-url-field");
    cx.simulate_input("nonsense");
    assert!(form_words(&view, cx).contains(&("url".into(), "That isn’t a web address. Give one like https://example.com/mcp.".into())));
    click(cx, "settings-form-submit");
    assert!(stand_in.seen_at("POST", "kuvo/settings/mcp/servers/add").is_empty(), "not ready: nothing sent");
    // Back to a command: what was typed there is still there, and it goes.
    click(cx, "settings-form-runs-local");
    click(cx, "settings-form-environment-add");
    cx.simulate_input("TOKEN");
    click(cx, "settings-form-environment-value-0");
    cx.simulate_input("{env:TOKEN}");
    click(cx, "settings-form-submit");
    let sent = bodies(&stand_in, "POST", "kuvo/settings/mcp/servers/add");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["values"]["name"], json!("my files"));
    assert_eq!(sent[0]["values"]["command"], json!("npx -y @scope/files ~/Docs"));
    assert_eq!(sent[0]["values"]["environment"], json!([{ "key": "TOKEN", "value": "{env:TOKEN}" }]));
    assert!(sent[0]["was"].as_str().is_some_and(|was| !was.is_empty()));
    assert!(!shown(cx, "settings-form"), "done: the sheet closes");
    assert!(shown(cx, "row-server-my-files"));
}

#[gpui::test]
fn a_sign_in_the_box_leads_opens_its_page_here_and_shows_how_its_going(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_mcp();
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "mcp");
    click(cx, "row-server-linear-sign-in");
    // The app that started it opens its page, once; the flow shows under the row.
    assert_eq!(cx.opened_url().as_deref(), Some(kuvo::stand_in::settings::mcp::SIGN_IN_PAGE));
    assert!(shown(cx, "row-server-linear-flow"));
    assert!(shown(cx, "row-server-linear-flow-open-sign-in-page") && shown(cx, "row-server-linear-flow-cancel"));
    // The app beside the box has nothing to paste back.
    assert!(!shown(cx, "row-server-linear-flow-code-go"));
    // Signed in on its page: the page read again says so, and OK puts it away.
    stand_in.with_mcp(|settings| settings.mcp_signed_in("linear"));
    section(&workspace, cx, "Permissions");
    section(&workspace, cx, "mcp");
    assert!(shown(cx, "row-server-linear-flow-ok"));
    click(cx, "row-server-linear-flow-ok");
    assert!(!shown(cx, "row-server-linear-flow"));
}

#[gpui::test]
fn another_computers_sign_in_takes_the_address_its_page_ended_on(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    devbox.show_mcp();
    side(&workspace, cx, Side::Computer);
    click(cx, "settings-computer");
    click(cx, "menu-item-devbox");
    section(&workspace, cx, "mcp");
    click(cx, "row-server-linear-sign-in");
    assert!(shown(cx, "row-server-linear-flow-code-go"));
    let view = settings(&workspace, cx);
    let field = view.read_with(cx, |view, _| view.code_field("row-server-linear").cloned()).expect("its field");
    field.update(cx, |field, cx| field.set_text("http://127.0.0.1:19876/mcp/oauth/callback?code=abc", cx));
    click(cx, "row-server-linear-flow-code-go");
    let sent = bodies(&devbox, "POST", "kuvo/settings/mcp/server-linear/finish");
    assert_eq!(sent[0]["values"]["code"], json!("http://127.0.0.1:19876/mcp/oauth/callback?code=abc"));
    assert!(shown(cx, "row-server-linear-flow-ok"), "done, as the box answered");
}

#[gpui::test]
fn a_form_whose_thing_changed_meanwhile_stays_open_with_what_was_typed(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_mcp();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "mcp");
    click(cx, "row-server-files-menu");
    click(cx, "menu-item-Edit…");
    click(cx, "settings-form-command-field");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("npx -y files-server --readonly");
    // Turned off on another device meanwhile.
    stand_in.with_mcp(|settings| settings.mcp.servers[0].entry["disabled"] = json!(true));
    click(cx, "settings-form-submit");
    assert!(shown(cx, "settings-form"), "kept open");
    assert_eq!(view.read_with(cx, |view, cx| view.form().and_then(|form| form.read(cx).refused().map(str::to_owned))).as_deref(), Some("It changed since you opened it. Close this and open it again to see it as it is now."));
    let typed = view.read_with(cx, |view, cx| view.form().unwrap().read(cx).values(cx)["command"].clone());
    assert_eq!(typed, json!("npx -y files-server --readonly"));
}

#[gpui::test]
fn agents_are_listed_as_the_box_says_and_edited_in_its_form(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_agents();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "agents");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Agents"));
    // The built-in ones, then yours; a built-in one you changed says so beside its name.
    for row in ["row-agent-build", "row-agent-plan", "row-agent-general", "row-agent-explore", "row-agent-reviewer", "row-agent-docs-writer"] {
        assert!(shown(cx, row), "{row}");
    }
    assert!(shown(cx, "row-agent-plan-badge") && !shown(cx, "row-agent-build-badge"));
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-agent-reviewer")).as_deref(), Some("A mode and a subagent · Reviews a change and says what's wrong, without fixing it."));
    // Turned on from its switch, from the row's revision.
    click(cx, "row-agent-docs-writer-switch");
    let sent = bodies(&stand_in, "PATCH", "kuvo/settings/agents");
    assert_eq!((sent[0]["row"].clone(), sent[0]["value"].clone()), (json!("agent-docs-writer"), json!(true)));

    // Edit…: its form, with how it's used and its model each from a menu (how it's used: its
    // choices have lines, which side-by-side buttons can't show).
    click(cx, "row-agent-reviewer-menu");
    click(cx, "menu-item-Edit…");
    assert!(shown(cx, "settings-form"));
    assert!(view.read_with(cx, |view, cx| view.form().unwrap().read(cx).is_ready()));
    assert!(shown(cx, "settings-form-mode-menu") && shown(cx, "settings-form-may-edit-deny"));
    assert!(form_words(&view, cx).contains(&("mode".into(), "New sessions can start in it, and it works when you name it or another agent hands it part of the work.".into())));
    click(cx, "settings-form-mode-menu");
    click(cx, "menu-item-Subagent");
    // Its model from the model picker, as everywhere a model is picked.
    click(cx, "settings-form-model-menu");
    click(cx, "model-rail-anthropic");
    click(cx, "model-row-anthropic/claude-sonnet-4-5");
    click(cx, "settings-form-submit");
    let sent = bodies(&stand_in, "POST", "kuvo/settings/agents/agent-reviewer/edit");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["values"]["mode"], json!("subagent"));
    assert_eq!(sent[0]["values"]["model"], json!("anthropic/claude-sonnet-4-5"));
    // What it didn't change goes back as it opened: its rules, one a line.
    assert_eq!(sent[0]["values"]["allowed"], json!("Run commands: git diff *\nRun commands: git log *"));
    assert!(!shown(cx, "settings-form"), "done: the sheet closes");
}

#[gpui::test]
fn a_new_agent_is_made_from_its_form_and_one_of_yours_is_deleted_asked_first(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_agents();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "agents");
    // A built-in one offers Reset to Built-In, not Delete.
    click(cx, "row-agent-plan-menu");
    assert!(shown(cx, "menu-item-Reset to Built-In…") && !shown(cx, "menu-item-Delete Agent…"));
    cx.simulate_keystrokes("escape");
    click(cx, "row-agents-add-new-agent");
    assert!(shown(cx, "settings-form"));
    click(cx, "settings-form-name-field");
    cx.simulate_input("Bug Hunter");
    click(cx, "settings-form-system-field");
    cx.simulate_input("Look for bugs; change nothing.");
    click(cx, "settings-form-submit");
    let sent = bodies(&stand_in, "POST", "kuvo/settings/agents/agents/add");
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0]["values"]["name"].clone(), sent[0]["values"]["mode"].clone(), sent[0]["values"]["system"].clone()), (json!("Bug Hunter"), json!("subagent"), json!("Look for bugs; change nothing.")));

    // Delete, asked first in the box's words.
    click(cx, "row-agent-docs-writer-menu");
    click(cx, "menu-item-Delete Agent…");
    assert_eq!(view.read_with(cx, |view, _| view.confirm_words()), Some(("Delete docs-writer?".into(), "It's removed from this Mac's settings.".into(), "Delete".into())));
    click(cx, "confirm-go");
    assert!(!shown(cx, "row-agent-docs-writer"));
    assert!(shown(cx, "row-agent-plan") && shown(cx, "row-agent-reviewer"), "the rest stay");
}

#[gpui::test]
fn claude_codes_agents_are_offered_and_one_is_imported(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_agents();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "agents");
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-claude-security-auditor")).as_deref(), Some("Checks a change for security problems."));
    assert!(!shown(cx, "row-claude-security-auditor-import"), "imported already");
    click(cx, "row-claude-test-writer-import");
    assert_eq!(stand_in.seen_at("POST", "kuvo/settings/agents/claude-test-writer/import").len(), 1);
    assert!(!shown(cx, "row-claude-test-writer-import"));
}

// MARK: Vault (`settings.vault`): a computer's vault, drawn from the box's description alone. No
// value is ever on the page: one is shown only when Show is pressed, after Touch ID or the Mac's
// password, read through the vault's own route, and gone with the sheet; a check is sent each value
// masked. What lets something use what's yours more asks first too; taking access away doesn't.

/// The stand-in's vault item named `title`, by its row's id.
fn vault_item(title: &str) -> String {
    let page = kuvo::stand_in::settings::vault::Vault::default().page();
    let items = page["groups"].as_array().unwrap().iter().flat_map(|group| group["rows"].as_array().unwrap().iter()).find(|row| row["id"] == json!("items")).unwrap()["items"].clone();
    items.as_array().unwrap().iter().find(|item| item["title"] == json!(title)).unwrap()["id"].as_str().unwrap().to_owned()
}

/// The text in a form's field, as it's drawn there now.
fn form_text(view: &Entity<SettingsView>, cx: &mut VisualTestContext, field: &str) -> Option<String> {
    view.read_with(cx, |view, cx| view.form().map(|form| form.read(cx).values(cx)).and_then(|values| values.get(field).and_then(|value| value.as_str()).map(str::to_owned)))
}

/// Touch ID or the Mac's password, as a test answers it (`it_is_you`): each ask's reason, kept.
fn confirming(cx: &mut TestAppContext, it_is_you: bool) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = asked.clone();
    cx.update(|cx| {
        cx.set_global(kuvo::confirm_you::ConfirmYou(std::sync::Arc::new(move |reason: &str| {
            kept.lock().unwrap().push(reason.to_owned());
            it_is_you
        })))
    });
    asked
}

/// The actions sent to the stand-in's vault, in order.
fn vault_sent(stand_in: &StandIn) -> Vec<String> {
    stand_in.with_vault(|vault| vault.sent.iter().map(|(_, action, _)| action.clone()).collect())
}

#[gpui::test]
fn a_computers_vault_lists_what_waits_its_items_and_activity_in_the_boxs_words(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_vault();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "vault");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Vault"));
    // What an agent waits for, its answers as buttons. Always Allow lets it use the item from now
    // on, which asks Touch ID or the computer's password first: where this app can't (as in this
    // test, with no way to ask), it's there, can't be pressed, and says why.
    let asks = view.read_with(cx, |view, _| view.page_rows().into_iter().filter(|key| key.starts_with("row-ask-")).collect::<Vec<_>>());
    assert_eq!(asks.len(), 2);
    assert!(shown(cx, &format!("{}-allow-once", asks[0])) && shown(cx, &format!("{}-always-allow", asks[0])));
    assert!(view.read_with(cx, |view, _| view.row_why(&asks[0])).is_some_and(|why| why.starts_with("Always Allow needs Touch ID") && !why.contains("iPhone")));
    click(cx, &format!("{}-always-allow", asks[0]));
    assert!(vault_sent(&stand_in).is_empty(), "Always Allow sends nothing from here");
    click(cx, &format!("{}-allow-once", asks[0]));
    assert_eq!(vault_sent(&stand_in), vec!["once".to_owned()]);
    assert!(!shown(cx, &asks[0]), "answered: gone from the page");
    // The items, each with how it's used beside its name.
    let github = vault_item("GitHub");
    assert_eq!(view.read_with(cx, |view, _| view.row_detail(&format!("row-{github}"))).as_deref(), Some("Sign-in · octo@example.com · github.com"));
    assert!(shown(cx, &format!("row-{github}-badge")) && shown(cx, &format!("row-{github}-menu")));
    assert!(shown(cx, "row-pause-switch") && shown(cx, "row-paused-bot_ranger-let-go-on"));
}

/// On a Mac, Always Allow asks Touch ID or the Mac's password after its confirmation, and goes only
/// once it's you; and every settings request says the app asks (`x-kuvo-confirms-you`).
#[gpui::test]
fn always_allow_asks_its_you_first_and_goes_only_then(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_vault();
    let asked = confirming(cx, false);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "vault");
    let ask = view.read_with(cx, |view, _| view.page_rows().into_iter().find(|key| key.starts_with("row-ask-")).unwrap());
    assert!(view.read_with(cx, |view, _| view.row_why(&ask)).is_none());
    click(cx, &format!("{ask}-always-allow"));
    click(cx, "confirm-go");
    // Not you: nothing sent, and said.
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(vault_sent(&stand_in).is_empty());
    assert!(view.read_with(cx, |view, _| view.problem().map(str::to_owned)).is_some_and(|said| said.contains("wasn’t confirmed")));
    // You: sent.
    cx.update(|_, cx| cx.set_global(kuvo::confirm_you::ConfirmYou(std::sync::Arc::new(|_: &str| true))));
    click(cx, &format!("{ask}-always-allow"));
    click(cx, "confirm-go");
    assert_eq!(vault_sent(&stand_in), vec!["always".to_owned()]);
}

#[gpui::test]
fn add_item_checks_what_is_typed_without_the_value_and_sends_it_only_to_add_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_vault();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "vault");
    click(cx, "row-items-add-add-item");
    assert!(shown(cx, "settings-form"));
    assert!(shown(cx, "settings-form-password") && !shown(cx, "settings-form-value") && !shown(cx, "settings-form-file"));
    click(cx, "settings-form-name-field");
    cx.simulate_input("Linear");
    click(cx, "settings-form-site-field");
    cx.simulate_input("linear.app");
    click(cx, "settings-form-password-field");
    cx.simulate_input("abc");
    assert!(form_words(&view, cx).contains(&("password".into(), "A value needs at least 4 characters, so it can be hidden from agents.".into())));
    cx.simulate_input("-much-longer");
    // Every check was sent the password as dots, never the password.
    let checked = stand_in.with_vault(|vault| vault.checked.clone());
    assert!(!checked.is_empty());
    for values in &checked {
        let sent = values["password"].as_str().unwrap_or_default();
        assert!(sent.chars().all(|letter| letter == '•'), "a check carried {sent:?}");
    }
    assert!(checked.iter().any(|values| values["password"] == json!("•".repeat("abc-much-longer".len()))));
    click(cx, "settings-form-submit");
    let sent = stand_in.with_vault(|vault| vault.sent.clone());
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].0.as_str(), sent[0].1.as_str()), ("items", "add"));
    assert_eq!(sent[0].2["password"], json!("abc-much-longer"));
    assert!(!shown(cx, "settings-form"), "done: the sheet closes");
}

/// Where this app can't ask (no Touch ID here), what a secret keeps isn't read: its eye shows only
/// what's typed, and the sheet says why.
#[gpui::test]
fn a_kept_value_isnt_read_where_this_app_cant_make_sure_its_you(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_vault();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "vault");
    let github = vault_item("GitHub");
    click(cx, &format!("row-{github}-menu"));
    click(cx, "menu-item-Edit…");
    click(cx, "settings-form-password-show");
    assert!(stand_in.with_vault(|vault| vault.shown.is_empty()), "nothing read from the vault");
    assert_eq!(form_text(&view, cx, "password").as_deref(), Some(""));
    // What's typed shows.
    click(cx, "settings-form-password-field");
    cx.simulate_input("a-new-password");
    assert!(view.read_with(cx, |view, cx| view.form().unwrap().read(cx).reveals("password")));
    assert_eq!(form_text(&view, cx, "password").as_deref(), Some("a-new-password"));
    assert!(stand_in.with_vault(|vault| vault.shown.is_empty()));
}

/// On a Mac, Show asks Touch ID or the Mac's password, then reads what's kept through the vault's
/// own route; the value lives in its field alone, and goes with the sheet as the page is left.
#[gpui::test]
fn a_kept_value_shows_after_its_you_and_goes_with_the_sheet(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_vault();
    let asked = confirming(cx, true);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "vault");
    let github = vault_item("GitHub");
    let id = github.trim_start_matches("item-").to_owned();
    let secret = kuvo::stand_in::settings::vault::made_up(&id, "password");
    click(cx, &format!("row-{github}-menu"));
    click(cx, "menu-item-Edit…");
    assert_eq!(form_text(&view, cx, "password").as_deref(), Some(""));
    assert!(stand_in.with_vault(|vault| vault.shown.is_empty()), "nothing read as it opens");
    click(cx, "settings-form-password-show");
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert_eq!(stand_in.with_vault(|vault| vault.shown.clone()), vec![id.clone()]);
    assert_eq!(form_text(&view, cx, "password").as_deref(), Some(secret.as_str()));
    // Another section: the sheet and its value are gone; opened again, empty, nothing read.
    section(&workspace, cx, "clock");
    assert!(view.read_with(cx, |view, _| view.form().is_none()));
    section(&workspace, cx, "vault");
    click(cx, &format!("row-{github}-menu"));
    click(cx, "menu-item-Edit…");
    assert_eq!(form_text(&view, cx, "password").as_deref(), Some(""));
    assert_eq!(stand_in.with_vault(|vault| vault.shown.len()), 1);
    assert!(vault_sent(&stand_in).is_empty());
}

/// Where this app can't make sure it's you, taking access away is still done here, asking nothing;
/// and should it be offered more (the stand-in's page is the one an app that asks gets; the box
/// itself offers such an app no more), the app sends nothing and says so.
#[gpui::test]
fn who_can_use_it_is_only_taken_away_where_this_app_cant_make_sure_its_you(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_vault();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "vault");
    let github = vault_item("GitHub");
    click(cx, &format!("row-{github}-menu"));
    click(cx, "menu-item-Who Can Use It…");
    // (Scout uses it on its own: No takes that away.)
    click(cx, "settings-form-bot-bot_scout-no");
    assert_eq!(view.read_with(cx, |view, cx| view.form().unwrap().read(cx).values(cx)["bot-bot_scout"].clone()), json!("no"));
    click(cx, "settings-form-submit");
    let sent = stand_in.with_vault(|vault| vault.sent.clone());
    assert_eq!((sent.len(), sent[0].1.as_str(), sent[0].2["bot-bot_scout"].clone()), (1, "who", json!("no")));
    assert!(!shown(cx, "settings-form"), "done: the sheet closes");
    // Letting another bot use it: nothing's sent, the sheet stays open, and says why.
    click(cx, &format!("row-{github}-menu"));
    click(cx, "menu-item-Who Can Use It…");
    click(cx, "settings-form-add-bot-bot_ranger");
    click(cx, "settings-form-submit");
    assert_eq!(vault_sent(&stand_in).len(), 1);
    assert!(shown(cx, "settings-form"));
    assert_eq!(view.read_with(cx, |view, cx| view.form().and_then(|form| form.read(cx).refused().map(str::to_owned))).as_deref(), Some("It wasn’t confirmed, so nothing was done."));
}

/// On a Mac: taking access away asks nothing; letting a bot use it asks Touch ID or the Mac's
/// password first, and goes only once it's you.
#[gpui::test]
fn who_can_use_it_asks_its_you_only_to_let_it_be_used_more(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_vault();
    let asked = confirming(cx, true);
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "vault");
    let github = vault_item("GitHub");
    click(cx, &format!("row-{github}-menu"));
    click(cx, "menu-item-Who Can Use It…");
    click(cx, "settings-form-bot-bot_scout-ask");
    click(cx, "settings-form-submit");
    assert!(asked.lock().unwrap().is_empty(), "less: nothing asked");
    let sent = stand_in.with_vault(|vault| vault.sent.clone());
    assert_eq!((sent[0].1.as_str(), sent[0].2["bot-bot_scout"].clone()), ("who", json!("ask")));
    click(cx, &format!("row-{github}-menu"));
    click(cx, "menu-item-Who Can Use It…");
    click(cx, "settings-form-add-bot-bot_ranger");
    click(cx, "settings-form-submit");
    assert_eq!(asked.lock().unwrap().len(), 1, "more: asked first");
    let sent = stand_in.with_vault(|vault| vault.sent.clone());
    assert_eq!(sent[1].2["add-bot"], json!("bot_ranger"));
}

#[gpui::test]
fn a_locked_vault_says_so_and_lists_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.show_vault();
    stand_in.with_vault(|vault| vault.locked = true);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "vault");
    assert!(shown(cx, "note-vault-0"));
    assert!(view.read_with(cx, |view, _| view.page_rows()).is_empty());
}

#[gpui::test]
fn skills_fixed_page_opens_the_generic_add_form(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(kuvo::stand_in::settings::skills::section(), kuvo::stand_in::settings::skills::page());
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "skills");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Skills"));
    assert!(shown(cx, "row-skill-release-notes-switch"));
    click(cx, "row-skills-new-skill");
    assert!(shown(cx, "settings-form"));
    assert!(shown(cx, "settings-form-name-field"));
    assert!(shown(cx, "settings-form-description-field"));
    assert!(shown(cx, "settings-form-instructions-field"));
}

#[gpui::test]
fn custom_auto_descriptions_draw_ordered_lists_and_new_list_form(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(kuvo::stand_in::settings::custom_auto::section(), kuvo::stand_in::settings::custom_auto::page());
    let (workspace, cx) = open(cx, stand_in);
    section(&workspace, cx, "custom-auto");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Custom Auto"));
    click(cx, "row-lists-new-list");
    assert!(shown(cx, "settings-form"));
    assert!(shown(cx, "settings-form-name-field"));
}

#[gpui::test]
fn commands_list_and_add_are_described_by_the_service(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(kuvo::stand_in::settings::commands::section(), kuvo::stand_in::settings::commands::page());
    let (workspace, cx) = open(cx, stand_in);
    section(&workspace, cx, "commands");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Commands"));
    click(cx, "row-commands-new-command");
    assert!(shown(cx, "settings-form"));
    assert!(shown(cx, "settings-form-name-field"));
    assert!(shown(cx, "settings-form-description-field"));
    assert!(shown(cx, "settings-form-template-field"));
}

/// Git as the box describes it: no Save buttons; a field is saved as it's left (or on Return),
/// with its row's revision, and leaving one unchanged sends nothing. The instructions are written
/// full width under their group's heading, not titled again.
#[gpui::test]
fn fields_save_as_theyre_left_and_instructions_are_full_width(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    for (section, page) in kuvo::stand_in::settings::described_pages(false) {
        stand_in.add_settings_section(section, page);
    }
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "git");
    assert!(!shown(cx, "row-branch-prefix-save") && !shown(cx, "row-commit-instructions-save"));
    // The instructions' own title isn't drawn: the group's heading says it.
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-commit-instructions")).as_deref(), Some(""));
    let prefix = bounds(cx, "row-branch-prefix-field").unwrap();
    let area = bounds(cx, "row-commit-instructions-field").unwrap();
    assert!(area.size.width > prefix.size.width * 2., "full width");
    // Left unchanged: nothing.
    click(cx, "row-branch-prefix-field");
    click(cx, "settings-whose");
    assert!(bodies(&stand_in, "PATCH", "kuvo/settings/git").is_empty());
    // Typed and left: saved, from the row's revision.
    click(cx, "row-branch-prefix-field");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("sam/");
    click(cx, "row-commit-instructions-field");
    let sent = bodies(&stand_in, "PATCH", "kuvo/settings/git");
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0]["row"].clone(), sent[0]["value"].clone()), (json!("branch-prefix"), json!("sam/")));
    assert!(sent[0]["was"].as_str().is_some_and(|was| !was.is_empty()));
    // Return adds a line in the instructions; leaving saves them.
    cx.simulate_input("Say why");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Keep it short");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/settings/git").len(), 1);
    click(cx, "settings-whose");
    let sent = bodies(&stand_in, "PATCH", "kuvo/settings/git");
    assert_eq!((sent[1]["row"].clone(), sent[1]["value"].clone()), (json!("commit-instructions"), json!("Say why\nKeep it short")));
    // Escape puts back what the box has.
    click(cx, "row-branch-prefix-field");
    cx.simulate_input("oops");
    cx.simulate_keystrokes("escape");
    let text = view.read_with(cx, |view, cx| view.field("row-branch-prefix").unwrap().read(cx).text().to_owned());
    let shown_value = view.read_with(cx, |view, _| view.row_value("row-branch-prefix"));
    assert_eq!(Some(text), shown_value);
}

/// Storage as the box describes it: the whole as one bar with each part's name and size, a
/// breakdown that opens, Refresh, and rows that open Worktrees and Archived Sessions.
#[gpui::test]
fn storage_shows_its_parts_a_breakdown_and_opens_the_pages_that_clear_the_rest(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    for (section, page) in kuvo::stand_in::settings::described_pages(false) {
        stand_in.add_settings_section(section, page);
    }
    let (workspace, cx) = open(cx, stand_in.clone());
    section(&workspace, cx, "storage");
    assert!(shown(cx, "row-used-bar"));
    for ix in 0..5 {
        assert!(shown(cx, &format!("row-used-part-{ix}")), "part {ix}");
    }
    assert!(!shown(cx, "row-used-breakdown-list"));
    click(cx, "row-used-breakdown");
    assert!(shown(cx, "row-used-breakdown-list"));
    click(cx, "row-used-breakdown");
    assert!(!shown(cx, "row-used-breakdown-list"));
    click(cx, "row-used-refresh");
    assert_eq!(stand_in.seen_at("POST", "kuvo/settings/storage/used/refresh").len(), 1);
    // The size beside Kuvo's folder; the budget's common values in its field's menu.
    assert!(shown(cx, "row-home-folder-aside"));
    assert!(shown(cx, "row-older-budget-picks-menu"));
    // A row that opens a page is pressed whole.
    click(cx, "row-manage-worktrees");
    assert_eq!(workspace.read_with(cx, |workspace, cx| workspace.area_view(Area::Settings).and_then(|view| view.read(cx).settings().cloned())).map(|view| view.read_with(cx, |view, _| view.section().to_owned())).as_deref(), Some("worktrees"));
}

#[gpui::test]
fn added_agent_signs_in_to_its_new_provider_and_cancel_removes_it(cx: &mut TestAppContext) {
    added_agent_cancel(cx, false);
}

#[gpui::test]
fn closing_after_oauth_completed_before_ui_poll_keeps_the_added_account(cx: &mut TestAppContext) {
    added_agent_cancel(cx, true);
}

fn added_agent_cancel(cx: &mut TestAppContext, completed: bool) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = section(&workspace, cx, "accounts");
    assert!(view.read_with(cx, |view, _| view.account_act("codex-cli", "addAnother").is_some()));
    click(cx, "account-codex-cli-menu");
    click(cx, "menu-item-Add Another Account…");
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.sign_in().is_some()));
    // Codex offers two ways (its browser sign-in and an API key): the browser's is picked.
    click(cx, "sign-in-way-0");
    let starts = stand_in.seen_at("POST", "kuvo/sign-in/start");
    let start = starts.last().expect("new account sign-in");
    assert!(start.path.contains("integration=codex-cli%40added"), "{}", start.path);
    if completed { stand_in.end_sign_ins(Ending::Complete, None); }
    let sheet = view.read_with(cx, |view, _| view.sign_in().unwrap().clone());
    sheet.update(cx, |sheet, cx| sheet.close(cx));
    cx.run_until_parked();
    assert!(stand_in.seen().iter().any(|seen| seen.method == "DELETE" && seen.path.starts_with("kuvo/agents/api/kuvo/accounts/codex-cli")));
    let accounts = BoxClient::new(stand_in).sign_in_accounts().unwrap();
    assert_eq!(accounts.agents.iter().any(|entry| entry.integration.id.starts_with("codex-cli@added")), completed);
}
