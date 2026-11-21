//! The Computers area as someone uses it, against the stand-in box (nothing real is reached): only
//! this Mac; this Mac with a machine at work, a Mac that can't be reached, one on an older Kuvo Code
//! and a machine; a long list; a computer's page and what it says; Add a Machine. Clicks go
//! through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, Pixels, ScrollDelta, ScrollEvent, TestAppContext, TouchPhase, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::computers::{ComputersView, Shown};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::{Workspace, held_words};
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, Entity<ComputersView>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(900.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    click(cx, "rail-computers");
    let page = workspace.read_with(cx, |workspace, cx| workspace.area_view(Area::Computers).and_then(|view| view.read(cx).computers_page().cloned())).expect("Computers' page");
    (workspace, page, cx)
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

fn said(page: &Entity<ComputersView>, cx: &mut VisualTestContext, key: &str) -> Option<(String, String)> {
    page.read_with(cx, |page, _| page.said(key))
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.title(cx).0.map(|title| title.to_string()))
}

/// Only this Mac: it's listed, its page says what it runs, and adding a machine is offered.
#[gpui::test]
fn only_this_mac_is_listed_with_a_way_to_add_a_machine(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.computers_alone();
    let (workspace, page, cx) = open(cx, stand_in);
    // The area opens on all of them: this Mac alone, and how to add another.
    assert!(shown(cx, "computers-page") && shown(cx, "computers-row-this") && shown(cx, "computer-this"));
    assert!(!shown(cx, "computer-key_devbox") && !shown(cx, "computers-row-key_devbox"));
    assert!(shown(cx, "computers-alone"));
    assert!(shown(cx, "computers-version-this-Kuvo 0.8.0-beta.33"));
    assert!(!shown(cx, "computers-version-this-Kuvo 0.8.0-beta.12"));
    // This Mac's page: connected, its facts as its box says them, and its session at work, which opens.
    click(cx, "computers-row-this");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::This);
    assert!(shown(cx, "computer-head") && !shown(cx, "notice-away") && !shown(cx, "notice-older"));
    assert_eq!(said(&page, cx, "fact-version").map(|(_, value)| value).as_deref(), Some("0.8.0-beta.33"));
    assert_eq!(said(&page, cx, "fact-system").map(|(_, value)| value).as_deref(), Some("macOS 15.1"));
    assert_eq!(said(&page, cx, "fact-reached").map(|(_, value)| value).as_deref(), Some("Shown in this window"));
    assert!(!shown(cx, "action-remove"), "this window's own computer isn't removed from here");
    // Its session at work, counted as its box counts it, is under Sessions.
    assert_eq!(said(&page, cx, "fact-at-work").map(|(title, _)| title).as_deref(), Some("1 session at work"));
    assert!(!shown(cx, "at-work-elsewhere"));
    click(cx, "at-work-show");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Sessions);
}

/// Add a Machine, start to end: reached from the sidebar's row and the page's button; a host the
/// box suggests fills the field; the box asks about the host's key, with its fingerprint; Trust
/// goes on, the copy says how far it's got; done, the machine is one of your computers and its
/// page opens. Only the address and the answer are sent: every word drawn is the box's.
#[gpui::test]
fn adding_a_machine_asks_about_its_key_then_shows_each_step_until_its_one_of_yours(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computers-add");
    assert!(shown(cx, "add-machine-page") && shown(cx, "add-form") && shown(cx, "add-hosts") && !shown(cx, "add-how"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Add a Machine"));
    // Nothing typed: nothing to add, and nothing is sent.
    click(cx, "add-machine");
    assert!(stand_in.seen_at("POST", "kuvo/computers/add").is_empty());
    // A host from the box's SSH config, picked.
    click(cx, "add-host-mac-mini");
    assert_eq!(page.read_with(cx, |page, cx| page.typed_host(cx)), "mac-mini");
    assert!(!shown(cx, "add-host-mac-mini"), "what's typed already isn't suggested");
    click(cx, "add-machine");
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/add").last().and_then(|seen| seen.body.clone()), Some(serde_json::json!({ "destination": "mac-mini" })));
    // The box asks: its question and the fingerprint, as it says them; the form gives way to the try.
    assert!(shown(cx, "add-question") && !shown(cx, "add-form") && !shown(cx, "add-machine"));
    let tried = page.read_with(cx, |page, _| page.tries_drawn()).pop().expect("the try");
    assert_eq!(tried.title, "Adding mac-mini…");
    let question = tried.question.expect("its question");
    assert_eq!(question.title, "Is this mac-mini?");
    assert_eq!(question.facts[1].value, "SHA256:Vq0bYJx3mD2Wc7PpN1tKfUo9sLhGe5RaZi8XwCnEyTM");
    assert!(shown(cx, "add-answer-cancel") && shown(cx, "add-answer-trust"));
    click(cx, "add-answer-trust");
    assert_eq!(stand_in.seen_at("POST", &format!("kuvo/computers/add/{}/answer", tried.id)).last().and_then(|seen| seen.body.clone()), Some(serde_json::json!({ "answer": "trust", "at": question.at })));
    // Copying: each step as the box says it stands, the one under way with how far it's got.
    let tried = page.read_with(cx, |page, _| page.tries_drawn()).pop().expect("the try");
    assert_eq!(tried.steps.iter().map(|step| step.state.as_str()).collect::<Vec<_>>(), ["done", "done", "now", "waiting", "waiting", "waiting"]);
    assert!(!shown(cx, "add-question") && shown(cx, "add-step-engine") && shown(cx, "add-try-cancel"));
    assert!(!shown(cx, "computers-row-key_added_mac-mini"));
    // Done: it's one of your computers, and Show opens its page, with what the box offers for it.
    stand_in.adding_goes_on();
    page.update(cx, |page, cx| page.read_adding(cx));
    cx.run_until_parked();
    let tried = page.read_with(cx, |page, _| page.tries_drawn()).pop().expect("the try");
    assert_eq!((tried.state.as_str(), tried.title.as_str()), ("done", "mac-mini is one of your computers"));
    assert!(shown(cx, "add-form"), "the form is back for another");
    click(cx, "add-try-show");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::Computer("key_added_mac-mini".into()));
    assert!(shown(cx, "action-setup") && shown(cx, "action-uninstall"));
    // Uninstall asks first, in the box's words, and then it's gone from your computers.
    click(cx, "action-uninstall-button");
    assert_eq!(page.read_with(cx, |page, _| page.confirm_words()).map(|(title, _, label)| (title, label)), Some(("Uninstall Kuvo Code from mac-mini?".into(), "Uninstall".into())));
    click(cx, "computers-confirm-go");
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/key_added_mac-mini/uninstall").len(), 1);
    assert!(shown(cx, "computer-missing"));
    // What became of it is said with your computers, where its page no longer is.
    click(cx, "computers-all");
    let gone = page.read_with(cx, |page, _| page.tries_drawn()).pop().expect("the uninstall");
    assert_eq!((gone.kind.as_str(), gone.title.as_str()), ("uninstall", "mac-mini no longer has Kuvo Code running"));
    click(cx, "add-try-dismiss");
    assert!(page.read_with(cx, |page, _| page.tries_drawn()).is_empty());
}

/// What can't be added is said in the box's words, where it's asked: an address that isn't one
/// keeps what was typed to mend; a host that can't be reached fails with why, and Try Again asks
/// again; Dismiss forgets it; Cancel on the question stops the try.
#[gpui::test]
fn a_machine_that_cant_be_added_says_why_and_can_be_tried_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computers-add-row");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::Add);
    click(cx, "add-form");
    cx.simulate_input("host:2222");
    cx.simulate_keystrokes("enter");
    assert_eq!(said(&page, cx, "add-problem").map(|(_, words)| words).as_deref(), Some("Give a host name, or user@host. For a port or an IPv6 address, add a Host for the machine to ~/.ssh/config and give its name."));
    assert_eq!(page.read_with(cx, |page, cx| page.typed_host(cx)), "host:2222");
    // Typing again clears what was said.
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("nowhere.example");
    assert!(!shown(cx, "add-problem"));
    cx.simulate_keystrokes("enter");
    let failed = page.read_with(cx, |page, _| page.tries_drawn()).pop().expect("the try");
    assert_eq!(failed.state, "failed");
    assert!(shown(cx, "add-failure") && shown(cx, "add-form"), "what failed stays, with the form for another");
    assert!(failed.failure.as_ref().is_some_and(|failure| failure.message.starts_with("Couldn’t reach nowhere.example.")));
    click(cx, "add-try-again");
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/add").len(), 3);
    click(cx, "add-try-dismiss");
    assert!(page.read_with(cx, |page, _| page.tries_drawn()).is_empty());
    // Cancel on the box's question stops it: nothing was trusted, and the form is back.
    // (What failed stays typed, to mend; the hosts suggested go with what's typed.)
    assert_eq!(page.read_with(cx, |page, cx| page.typed_host(cx)), "nowhere.example");
    click(cx, "add-form");
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("gpu");
    assert!(shown(cx, "add-host-gpu-box") && !shown(cx, "add-host-mac-mini"));
    click(cx, "add-host-gpu-box");
    click(cx, "add-machine");
    click(cx, "add-answer-cancel");
    assert!(page.read_with(cx, |page, _| page.tries_drawn()).is_empty() && shown(cx, "add-form"));
    click(cx, "add-cancel");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::All);
}

/// A box that doesn't add computers (a member of a setup) says so in its words: the field takes
/// no typing, and nothing is sent.
#[gpui::test]
fn a_box_that_doesnt_add_machines_says_where_to_add_them(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.cannot_add("Add and remove computers from Studio.");
    let (_workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computers-add");
    assert_eq!(said(&page, cx, "add-cannot"), Some(("Machines aren’t added from This Mac".into(), "Add and remove computers from Studio.".into())));
    assert!(!shown(cx, "add-hosts"));
    click(cx, "add-form");
    cx.simulate_input("studio");
    assert_eq!(page.read_with(cx, |page, cx| page.typed_host(cx)), "");
    click(cx, "add-machine");
    assert!(stand_in.seen_at("POST", "kuvo/computers/add").is_empty());
}

/// This Mac and its other computers: each row says how it stands; a Mac that can't be reached and
/// one on an older Kuvo Code say why on their pages, the older one in the window's held words.
#[gpui::test]
fn a_computer_away_and_one_older_say_why_on_their_pages(cx: &mut TestAppContext) {
    let (workspace, page, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    for row in ["this", "key_devbox", "key_studio", "key_old"] {
        assert!(shown(cx, &format!("computers-row-{row}")), "{row} among all of them");
        assert!(shown(cx, &format!("computer-{row}")), "{row} in the sidebar");
    }
    assert!(shown(cx, "computers-version-key_studio-Kuvo 0.8.0-beta.32"));
    assert!(shown(cx, "computers-version-key_devbox-Kuvo 0.8.0"));
    assert!(!shown(cx, "computers-version-key_old-Kuvo 0.9.180"));
    // A machine at work: connected, its sessions counted as its box says.
    click(cx, "computer-key_devbox");
    assert_eq!(title(&workspace, cx).as_deref(), Some("devbox"));
    assert_eq!(said(&page, cx, "fact-at-work").map(|(title, _)| title).as_deref(), Some("2 sessions at work"));
    assert_eq!(said(&page, cx, "fact-reached").map(|(_, value)| value).as_deref(), Some("Through the relay, end to end encrypted"));
    assert!(shown(cx, "at-work-elsewhere") && !shown(cx, "at-work-show"), "another computer's sessions aren't opened from here yet");
    assert!(shown(cx, "action-remove") && !shown(cx, "action-reconnect"));
    // Away: why, and nothing claimed of what runs there.
    click(cx, "computer-key_studio");
    assert_eq!(said(&page, cx, "notice-away").map(|(title, _)| title).as_deref(), Some("studio-mini can’t be reached"));
    assert!(!shown(cx, "fact-at-work") && !shown(cx, "at-work-none"));
    // Connect is the card's own button, beside why.
    assert!(shown(cx, "notice-away-action") && !shown(cx, "action-reconnect"));
    // Older, but with what this app can't do without: used as it is, and says so.
    click(cx, "computer-key_old");
    let (held_title, _) = held_words(Some("old-mini"), false);
    assert_eq!(said(&page, cx, "notice-behind"), Some((held_title, kuvo::workspace::behind_words().to_owned())));
    assert!(!shown(cx, "notice-older"), "never said to wait for its update");
    assert_eq!(said(&page, cx, "fact-version").map(|(_, value)| value).as_deref(), Some("Older than this app"));
    assert!(!shown(cx, "at-work-none"), "an older computer's count isn't claimed");
    // All of them again, from the sidebar's first row.
    click(cx, "computers-all");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::All);
}

/// A computer gone from the list while its page is open says so, rather than what it said before.
#[gpui::test]
fn a_computer_gone_from_the_list_says_so(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computer-key_devbox");
    stand_in.computers_alone();
    // Read again as Computers opens (where the list isn't followed).
    click(cx, "rail-sessions");
    click(cx, "rail-computers");
    assert!(shown(cx, "computer-missing"));
    assert_eq!(said(&page, cx, "computer-missing").map(|(words, _)| words).as_deref(), Some("This computer isn’t among yours any more."));
    drop(workspace);
}

/// The list while it can't be read: this window's computer stays, and its page still opens.
#[gpui::test]
fn a_list_that_cant_be_read_keeps_this_computer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.computers_readable(false);
    let (_workspace, page, cx) = open(cx, stand_in);
    assert!(shown(cx, "computers-failed") && shown(cx, "computer-this"));
    click(cx, "computer-this");
    assert!(shown(cx, "computer-head"));
    // (What it hasn't said isn't drawn as "Not said yet".)
    assert_eq!(said(&page, cx, "fact-version"), None);
}

/// A long list: every computer is in the sidebar's list, and the page scrolls through them all.
#[gpui::test]
fn a_long_list_reaches_its_last_computer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.more_computers(40);
    let (_workspace, page, cx) = open(cx, stand_in);
    assert!(shown(cx, "computers-row-this"));
    assert!(!shown(cx, "computers-row-key_more_39"), "drawn only as it's scrolled to");
    // Scrolled with the wheel over the page, as a trackpad does, until its end.
    let at = bounds(cx, "computers-row-this").expect("the page").center();
    for _ in 0..20 {
        cx.simulate_event(ScrollEvent { position: at, delta: ScrollDelta::Pixels(gpui::point(px(0.), px(-400.))), touch_phase: TouchPhase::Moved, ..Default::default() });
        cx.run_until_parked();
    }
    assert!(shown(cx, "computers-row-key_more_39"));
    click(cx, "computers-row-key_more_39");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::Computer("key_more_39".into()));
}

/// Manage offers what the box offers for each computer, in its words: Remove… opens its sheet and
/// has the box remove it; Connect has the box reach one that's away; a computer that offers
/// nothing has no Manage at all.
#[gpui::test]
fn manage_does_what_the_box_offers_and_asks_first_where_it_says(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computer-key_devbox");
    assert!(shown(cx, "action-rename") && shown(cx, "action-remove") && !shown(cx, "action-reconnect"));
    // Remove…: its sheet first; Cancel does nothing.
    click(cx, "action-remove-button");
    assert_eq!(page.read_with(cx, |page, _| page.sheet_open()), Some("remove"));
    click(cx, "computers-sheet-cancel");
    assert!(!shown(cx, "computers-sheet") && stand_in.seen_at("DELETE", "kuvo/computers/key_devbox").is_empty());
    // Escape closes it too, doing nothing.
    click(cx, "action-remove-button");
    cx.simulate_keystrokes("escape");
    assert!(!shown(cx, "computers-sheet") && stand_in.seen_at("DELETE", "kuvo/computers/key_devbox").is_empty());
    // Going ahead, the box removes it, and it's gone from your computers, said over them all.
    click(cx, "action-remove-button");
    click(cx, "computers-sheet-go");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/computers/key_devbox").len(), 1);
    assert!(!shown(cx, "computer-key_devbox"));
    assert_eq!(said(&page, cx, "removed").map(|(title, _)| title).as_deref(), Some("devbox was removed from your setup"));

    // Connect, for a Mac that's away: the box reaches it, and it's being connected to.
    click(cx, "computer-key_studio");
    assert!(shown(cx, "notice-away-action") && !shown(cx, "action-remove") && !shown(cx, "action-rename"), "a Mac is neither renamed nor removed from here");
    click(cx, "notice-away-action");
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/key_studio/connect").len(), 1);
    assert!(shown(cx, "notice-connecting") && !shown(cx, "action-reconnect"));

    // Offered nothing (an older Mac): no Manage; and what it never said of what's at work isn't
    // drawn.
    click(cx, "computer-key_old");
    assert!(!shown(cx, "action-rename") && !shown(cx, "action-remove") && !shown(cx, "action-reconnect"));
    assert!(!shown(cx, "fact-at-work") && !shown(cx, "at-work-none"));
}

/// What the box refuses is shown in its words where it was asked (Remove's sheet), and nothing
/// changes.
#[gpui::test]
fn a_refusal_is_shown_in_the_boxs_words(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.manage_refused("The Mac that set devbox up isn't answering now (it may be asleep, off or offline), so nothing was changed there. Try again once it's back.");
    let (_workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computer-key_devbox");
    click(cx, "action-remove-button");
    click(cx, "computers-sheet-go");
    assert_eq!(
        page.read_with(cx, |page, _| page.sheet_refused().map(str::to_owned)).as_deref(),
        Some("The Mac that set devbox up isn't answering now (it may be asleep, off or offline), so nothing was changed there. Try again once it's back.")
    );
    assert!(shown(cx, "sheet-problem") && shown(cx, "computers-sheet-go"), "still there, to try again");
    click(cx, "computers-sheet-cancel");
    assert!(shown(cx, "action-remove-button"), "still there");
    // Another page doesn't carry it.
    click(cx, "computer-key_studio");
    assert!(!shown(cx, "manage-problem"));
}

/// While the old app is open on the Mac that keeps your machines' records, Remove waits and says
/// why, in the box's words, and sends nothing.
#[gpui::test]
fn remove_waits_for_the_old_app_and_says_so(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.old_app_open();
    let (_workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computer-key_devbox");
    assert!(shown(cx, "action-remove"));
    assert_eq!(
        said(&page, cx, "manage-why").map(|(words, _)| words).as_deref(),
        Some("Quit the old Kuvo Code app on Stand-in Mac first: it keeps its own list of machines, and would add devbox back.")
    );
    click(cx, "action-remove-button");
    assert!(!shown(cx, "computers-confirm") && stand_in.seen_at("DELETE", "kuvo/computers/key_devbox").is_empty(), "nothing asked, nothing sent");
}

#[gpui::test]
fn a_computer_whose_app_was_removed_says_so_and_is_allowed_again_from_its_page(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.block_vouching("key_devbox");
    let (workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computer-key_devbox");
    // The box's own words, and its own action: nothing is asked before the confirm's button.
    let (_, words) = said(&page, cx, "notice-vouching").expect("the notice");
    assert_eq!(words, "Kuvo on devbox was removed from your devices. devbox can’t add Kuvo Code’s app again until you allow it.");
    click(cx, "action-allow-devices-button");
    assert!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/allow-devices").is_empty(), "it asks first");
    click(cx, "computers-confirm-go");
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/allow-devices").len(), 1);
    // Allowed: the list says so, and the notice and the action go.
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(!shown(cx, "action-allow-devices") && said(&page, cx, "notice-vouching").is_none());
    // A computer that isn't blocked never shows either.
    assert!(shown(cx, "action-remove"));
}

/// An older computer whose Kuvo this box brings up to date: its page says how that stands in the
/// box's words, and Update Now asks first while sessions are at work there.
#[gpui::test]
fn an_older_computer_being_brought_up_to_date_says_how_and_updates_now_on_your_word(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.bring_up_to_date("key_old", "waiting", true, 3);
    let (_workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computer-key_old");
    assert_eq!(said(&page, cx, "notice-update"), Some(("Kuvo on old-mini hasn’t been free for a day".to_owned(), "It updates once nothing’s at work there, or now with Update Now.".to_owned())));
    assert!(!shown(cx, "notice-older"), "one card, the box's");
    // Update Now is the card's own button, not a row of Manage's too.
    assert!(!shown(cx, "action-update"));
    click(cx, "notice-update-action");
    assert_eq!(page.read_with(cx, |page, _| page.confirm_words()).map(|(title, _, _)| title).as_deref(), Some("Update Kuvo on old-mini now?"));
    assert!(stand_in.seen_at("POST", "kuvo/computers/key_old/update").is_empty());
    click(cx, "computers-confirm-go");
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/key_old/update").len(), 1);
    assert_eq!(said(&page, cx, "notice-update").map(|(title, _)| title).as_deref(), Some("Updating Kuvo on old-mini…"));
    assert!(!shown(cx, "notice-update-action") && !shown(cx, "action-update"), "nothing to press while it runs: how it goes is in the same card");
}

/// This computer's Kuvo waiting for the release its app carries (`computers.update.here`): its
/// page says so in its box's words, and Update Now takes it now, asked first while sessions are
/// at work.
#[gpui::test]
fn this_computers_kuvo_waiting_for_its_release_updates_now_on_your_word(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.own_update_waits("waiting", 1);
    let (_workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computer-this");
    let title = said(&page, cx, "notice-update").map(|(title, _)| title).unwrap_or_default();
    assert!(title.ends_with("updates once your sessions there finish."), "{title}");
    click(cx, "notice-update-action");
    assert!(page.read_with(cx, |page, _| page.confirm_words()).is_some_and(|(title, _, _)| title.starts_with("Update Kuvo on")));
    assert!(stand_in.seen_at("POST", "kuvo/update").is_empty());
    click(cx, "computers-confirm-go");
    assert_eq!(stand_in.seen_at("POST", "kuvo/update").len(), 1);
    assert!(said(&page, cx, "notice-update").is_some_and(|(title, _)| title.starts_with("Updating Kuvo on")));
    assert!(!shown(cx, "notice-update-action") && !shown(cx, "action-update-here"), "nothing to press while it runs");
}

/// An older machine this box didn't set up over SSH: its page says so in the box's words, and its
/// button opens Add a Machine at its address, adding nothing until you say.
#[gpui::test]
fn an_older_machine_to_set_up_over_ssh_opens_add_a_machine_at_its_address(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.needs_set_up_over_ssh("key_devbox");
    let (_workspace, page, cx) = open(cx, stand_in.clone());
    click(cx, "computer-key_devbox");
    assert_eq!(said(&page, cx, "notice-update").map(|(title, _)| title).as_deref(), Some("Kuvo on devbox needs an update"));
    click(cx, "notice-update-action");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), kuvo::computers::Shown::Add);
    assert_eq!(page.read_with(cx, |page, cx| page.typed_host(cx)), "devbox");
    assert!(stand_in.seen_at("POST", "kuvo/computers/add").is_empty());
}

/// What was sent last to `route`, as its body.
fn sent(stand_in: &StandIn, method: &str, route: &str) -> Option<serde_json::Value> {
    stand_in.seen_at(method, route).last().map(|seen| seen.body.clone().unwrap_or(serde_json::Value::Null))
}

/// Renaming a computer in place: Rename… turns its page's head into the name, all of it picked;
/// what's typed is sent to the box on Save, and the page names it so as the box lists it.
#[gpui::test]
fn renaming_a_computer_edits_its_name_in_place_and_the_box_names_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, page, cx) = open(cx, stand_in.clone());
    click(cx, "computers-row-key_devbox");
    assert!(shown(cx, "action-rename-button") && !shown(cx, "rename-editor"));
    click(cx, "action-rename-button");
    assert!(shown(cx, "rename-editor") && shown(cx, "rename-save"));
    // The name, picked whole: typing replaces it.
    cx.simulate_input("build-box");
    assert_eq!(page.read_with(cx, |page, cx| page.rename_field().map(|field| field.read(cx).text().to_owned())).as_deref(), Some("build-box"));
    click(cx, "rename-save");
    assert_eq!(sent(&stand_in, "PATCH", "kuvo/computers/key_devbox"), Some(serde_json::json!({ "name": "build-box" })));
    cx.run_until_parked();
    assert!(!shown(cx, "rename-editor"));
    assert_eq!(bounds(cx, "computer-head-name").is_some(), true);
    assert!(page.read_with(cx, |page, _| page.rename_field().is_none()));
}

/// Cancel leaves the name as it was, and sends nothing.
#[gpui::test]
fn cancelling_a_rename_sends_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, _, cx) = open(cx, stand_in.clone());
    click(cx, "computers-row-key_devbox");
    click(cx, "action-rename-button");
    cx.simulate_input("other");
    click(cx, "rename-cancel");
    assert!(!shown(cx, "rename-editor"));
    assert!(stand_in.seen_at("PATCH", "kuvo/computers/key_devbox").is_empty());
}

/// Who leads your setup, over all your computers; Change… offers only the computers that can
/// take it (reached, and their Kuvo answers a handover: studio-mini is away, old-mini older), and
/// hands it to the one picked, at the setup's epoch.
#[gpui::test]
fn the_main_computer_is_handed_over_to_one_that_can_take_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.setup_roles();
    let (workspace, page, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "setup-roles") && shown(cx, "roles-main-button"));
    click(cx, "roles-main-button");
    assert_eq!(page.read_with(cx, |page, _| page.sheet_open()), Some("handover"));
    assert!(shown(cx, "sheet-candidate-key_devbox"));
    assert!(!shown(cx, "sheet-candidate-key_studio"), "one that's away isn't offered");
    assert!(!shown(cx, "sheet-candidate-key_old"), "one whose Kuvo doesn't answer a handover isn't offered");
    click(cx, "computers-sheet-go");
    assert_eq!(sent(&stand_in, "POST", "kuvo/setup/handover"), Some(serde_json::json!({ "primary": "key_devbox", "epoch": 3 })));
    assert_eq!(page.read_with(cx, |page, _| page.sheet_open()), None);
    // devbox leads now: its page says so, and offers to change it again.
    click(cx, "computers-row-key_devbox");
    assert!(shown(cx, "role-chip-main") && shown(cx, "role-main-button"));
}

/// A box that doesn't say who leads (no `kuvo/setup` setup) draws no roles and no way to change
/// them: nothing that leads nowhere.
#[gpui::test]
fn no_roles_are_drawn_where_the_box_isnt_in_a_setup(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, _, cx) = open(cx, stand_in);
    assert!(!shown(cx, "setup-roles"));
    click(cx, "computers-row-key_devbox");
    assert!(!shown(cx, "role-main"));
}

/// Remove…: unlink, or unlink and wipe; the box's answer is said over all your computers.
#[gpui::test]
fn removing_a_computer_asks_unlink_or_wipe_and_says_what_the_box_did(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.setup_roles();
    let (_, page, cx) = open(cx, stand_in.clone());
    // (Tall enough for its Manage under its role.)
    cx.simulate_resize(size(px(1200.), px(1400.)));
    click(cx, "computers-row-key_devbox");
    click(cx, "action-remove-button");
    assert_eq!(page.read_with(cx, |page, _| page.sheet_open()), Some("remove"));
    assert!(shown(cx, "remove-unlink") && shown(cx, "remove-wipe") && !shown(cx, "remove-offline"));
    click(cx, "remove-wipe");
    click(cx, "computers-sheet-go");
    assert_eq!(sent(&stand_in, "POST", "kuvo/computers/key_devbox/unlink-and-wipe"), Some(serde_json::json!({})));
    assert!(stand_in.seen_at("DELETE", "kuvo/computers/key_devbox").is_empty());
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::All);
    assert_eq!(said(&page, cx, "removed").map(|(title, _)| title).as_deref(), Some("devbox was removed and wiped"));
    assert!(!shown(cx, "computers-row-key_devbox"));
}

/// Where the box this window shows is from before unlinking and wiping, Remove… offers Unlink
/// alone (no choice it can't take), sent as that box takes it.
#[gpui::test]
fn an_older_box_is_offered_unlink_alone(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, page, cx) = open(cx, stand_in.clone());
    click(cx, "computers-row-key_devbox");
    click(cx, "action-remove-button");
    assert!(shown(cx, "remove-unlink") && !shown(cx, "remove-wipe"));
    click(cx, "computers-sheet-go");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/computers/key_devbox").len(), 1);
    assert_eq!(said(&page, cx, "removed").map(|(title, _)| title).as_deref(), Some("devbox was removed from your setup"));
}

/// The last computer: it's wiped, and the box's reinstall offer goes on to the installer.
#[gpui::test]
fn removing_the_last_computer_wipes_it_and_passes_on_the_reinstall_offer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.setup_roles();
    stand_in.computers_alone();
    let (workspace, page, cx) = open(cx, stand_in.clone());
    let offered = std::rc::Rc::new(std::cell::Cell::new(false));
    let told = offered.clone();
    let _watch = cx.update(|_, cx| cx.subscribe(&workspace, move |_, event: &kuvo::workspace::Switch, _| {
        if matches!(event, kuvo::workspace::Switch::Reinstall) {
            told.set(true);
        }
    }));
    click(cx, "computers-row-this");
    click(cx, "action-remove-button");
    assert_eq!(page.read_with(cx, |page, _| page.sheet_open()), Some("remove-last"));
    click(cx, "computers-sheet-go");
    assert_eq!(sent(&stand_in, "POST", "kuvo/computers/key_stand_in/unlink-and-wipe"), Some(serde_json::json!({})));
    assert!(offered.get(), "the reinstall offer reaches the window");
}

/// Remove… on the main computer of more than one: unlinked like any other (not wiped as the last),
/// the box saying who takes its role first, and how it goes after.
#[gpui::test]
fn removing_the_main_computer_hands_its_role_over_first(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.setup_roles();
    let (_, page, cx) = open(cx, stand_in.clone());
    click(cx, "computers-row-this");
    click(cx, "action-remove-button");
    assert_eq!(page.read_with(cx, |page, _| page.sheet_open()), Some("remove"));
    assert!(shown(cx, "remove-hand-over") && shown(cx, "remove-unlink"));
    click(cx, "computers-sheet-go");
    assert_eq!(sent(&stand_in, "POST", "kuvo/computers/key_stand_in/unlink"), Some(serde_json::json!({})));
    let (title, words) = said(&page, cx, "removed").expect("the box's answer is said");
    assert!(title.starts_with("Removing ") && title.ends_with('…'), "{title}");
    assert!(words.contains("devbox is becoming the main computer"), "{words}");
}

/// Escape closes a sheet, sending nothing.
#[gpui::test]
fn escape_closes_a_sheet(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.setup_roles();
    let (_, page, cx) = open(cx, stand_in.clone());
    // (Tall enough for its Manage under its role.)
    cx.simulate_resize(size(px(1200.), px(1400.)));
    click(cx, "computers-row-key_devbox");
    click(cx, "action-remove-button");
    cx.simulate_keystrokes("escape");
    assert_eq!(page.read_with(cx, |page, _| page.sheet_open()), None);
    assert!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/unlink").is_empty());
}

/// The relay lease picks the main computer, so no computer is a standby: Setup roles shows the
/// main computer alone, with Change…, and no computer's page shows or offers a standby, even where
/// an older box still names one.
#[gpui::test]
fn no_standby_is_shown_or_chosen(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.setup_roles();
    stand_in.names_a_standby("key_devbox");
    stand_in.more_computers(3);
    let (workspace, page, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "setup-roles") && shown(cx, "roles-main") && shown(cx, "roles-main-button"));
    assert!(!shown(cx, "roles-standby") && !shown(cx, "roles-standby-button"));
    // (Tall enough for its role under its head.)
    cx.simulate_resize(size(px(1200.), px(1400.)));
    click(cx, "computers-row-key_devbox");
    assert!(!shown(cx, "role-standby") && !shown(cx, "role-standby-button") && !shown(cx, "role-chip-standby"));
    assert!(shown(cx, "role-main-button"), "devbox can be made the main computer");
    click(cx, "role-main-button");
    assert_eq!(page.read_with(cx, |page, _| page.sheet_open()), Some("handover"));
    assert!(stand_in.seen_at("POST", "kuvo/setup/standby").is_empty());
}
