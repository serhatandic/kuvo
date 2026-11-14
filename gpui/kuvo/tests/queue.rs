//! The Work Queue as someone uses it, against the stand-in box (nothing real is reached): the queue
//! under the box's categories with its counts, scope tabs, the search and the filters, Done folded;
//! an item's page; moving an item, marking it done, deleting it and bringing it back, adding one and
//! rewriting it, and making, renaming and removing a category, each through the box's routes with
//! the box's answer drawn after, or why not in its words. Clicks and keys go through the window as
//! AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::queue::{QueueView, Scope, Sheet, Shown};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

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
    cx.simulate_resize(size(px(1280.), px(1800.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

/// The Work Queue, open on the whole queue.
fn queue<'a>(cx: &'a mut TestAppContext, stand_in: &StandIn) -> (Entity<Workspace>, Entity<QueueView>, &'a mut VisualTestContext) {
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-queue");
    let page = workspace.read_with(cx, |workspace, cx| workspace.area_view(Area::Queue).and_then(|view| view.read(cx).queue_page().cloned()).expect("the Work Queue"));
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

fn top(cx: &mut VisualTestContext, selector: &str) -> Pixels {
    bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen")).top()
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.title(cx).0.map(|title| title.to_string()))
}

/// The bodies sent to `route`, in order.
fn bodies(stand_in: &StandIn, method: &str, route: &str) -> Vec<Value> {
    stand_in.seen_at(method, route).into_iter().map(|seen| seen.body.unwrap_or(Value::Null)).collect()
}

/// What the stand-in box answers `route` with now.
fn said(stand_in: &StandIn, route: &str) -> Value {
    serde_json::from_slice(&kuvo_box::transport::Transport::send(stand_in, &kuvo_box::transport::Request::get(route)).expect("answered").body).expect("JSON")
}

/// An item of the box's view, as the box says it now.
fn item_said(stand_in: &StandIn, id: &str) -> Value {
    said(stand_in, &format!("kuvo/work-queue/items/{id}"))["item"].clone()
}

#[gpui::test]
fn the_queue_lists_the_boxs_items_under_its_categories_with_done_folded(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = queue(cx, &stand_in);
    assert_eq!(title(&workspace, cx).as_deref(), Some("Work Queue"));
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::All));

    // The box's categories, in its order, each with the items the box lists in it, in its order.
    let order: Vec<Pixels> = ["group-next", "item-wqi_231", "item-wqi_232", "item-wqi_236", "group-later", "item-wqi_237", "group-cat_design", "item-wqi_243", "group-parked", "group-done"].iter().map(|row| top(cx, row)).collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "in the box's order: {order:?}");
    // Each item wears its category as a chip; one at work spins.
    assert!(shown(cx, "item-wqi_231-chip") && shown(cx, "item-working-wqi_231") && !shown(cx, "item-working-wqi_234"));
    // The counts are the box's.
    assert!(shown(cx, "group-next-count-6") && shown(cx, "group-done-count-12"));

    // Done is folded until it's opened; so is what's deleted.
    assert!(!shown(cx, "item-wqi_251") && !shown(cx, "item-wqi_262"));
    click(cx, "group-done");
    assert!(shown(cx, "item-wqi_251") && shown(cx, "item-done-wqi_251"));
    click(cx, "group-done");
    assert!(!shown(cx, "item-wqi_251"));
    // Any group folds away.
    click(cx, "group-next");
    assert!(!shown(cx, "item-wqi_231") && shown(cx, "item-wqi_237"));
}

#[gpui::test]
fn the_tabs_and_the_sidebar_pick_the_same_scope(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "tab-all-on"));

    // In Progress: only what a session is at work on, as the box says.
    click(cx, "tab-progress");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::Progress));
    assert!(shown(cx, "item-wqi_231") && shown(cx, "item-wqi_232") && !shown(cx, "item-wqi_234") && !shown(cx, "item-wqi_237"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("In Progress"));

    // A category from the sidebar: its items alone, and its tab on. Done's list is open there.
    click(cx, "category-cat_design");
    assert!(shown(cx, "tab-cat_design-on") && shown(cx, "item-wqi_243") && !shown(cx, "item-wqi_231") && !shown(cx, "group-cat_design"));
    click(cx, "tab-done");
    assert!(shown(cx, "item-wqi_251"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Done"));

    // Back goes to the scope before, and All Items to the whole queue.
    cx.simulate_keystrokes("cmd-[");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::Category("cat_design".into())));
    click(cx, "queue-all");
    assert!(shown(cx, "tab-all-on") && shown(cx, "group-next"));
}

#[gpui::test]
fn the_search_and_the_filters_narrow_what_shows(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    let asked = stand_in.seen_at("GET", "kuvo/work-queue").len();

    // Typing narrows the list as it's typed, folded Done included; the counts are of the whole
    // view, so none shows.
    click(cx, "queue-search");
    cx.simulate_input("DOCS");
    assert!(shown(cx, "item-wqi_239") && shown(cx, "item-wqi_252") && !shown(cx, "item-wqi_231"));
    assert!(!shown(cx, "group-later-count-6") && shown(cx, "group-later"));
    // A number finds its item.
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("#234");
    assert!(shown(cx, "item-wqi_234") && !shown(cx, "item-wqi_239"));
    cx.simulate_input("zzz");
    assert!(shown(cx, "queue-empty"));
    // Escape clears it.
    cx.simulate_keystrokes("escape");
    assert!(shown(cx, "item-wqi_231") && shown(cx, "group-later-count-6"));

    // A filter the box's view offers, on as a chip to take off.
    click(cx, "queue-filter");
    click(cx, "menu-item-devbox");
    assert!(shown(cx, "filter-0") && shown(cx, "item-wqi_236") && !shown(cx, "item-wqi_231"));
    // Another of the same kind widens it; one of another kind narrows it.
    click(cx, "queue-filter");
    click(cx, "menu-item-studio-mini");
    assert!(shown(cx, "item-wqi_236") && shown(cx, "item-wqi_242"));
    click(cx, "queue-filter");
    click(cx, "menu-item-GitHub");
    assert!(shown(cx, "queue-empty"));
    assert_eq!(page.read_with(cx, |page, _| page.filters().len()), 3);
    click(cx, "filters-clear");
    assert!(!shown(cx, "queue-filters") && shown(cx, "item-wqi_231"));
    // None of it asked the box again: it only chooses which of its items show.
    assert_eq!(stand_in.seen_at("GET", "kuvo/work-queue").len(), asked);
}

#[gpui::test]
fn an_items_page_shows_what_the_box_says_of_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = queue(cx, &stand_in);
    assert!(stand_in.seen_at("GET", "kuvo/work-queue/items/wqi_231").is_empty(), "nothing is asked of an item until it's opened");
    click(cx, "tab-next");
    click(cx, "item-wqi_231");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::Item("wqi_231".into()));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Keep what was typed when the sign-up email is taken"));
    assert_eq!(stand_in.seen_at("GET", "kuvo/work-queue/items/wqi_231").len(), 1);

    // Its title and how it stands, how far it is, what it says, and what's kept with it.
    for part in ["item-head", "item-state", "item-progress", "item-progress-value", "item-body", "item-session-ses_signup", "item-link-https://example.com/tone-guide", "item-file-before.png", "item-note-0", "item-note-2", "item-facts"] {
        assert!(shown(cx, part), "{part}");
    }
    let whole = page.read_with(cx, |page, _| page.whole().cloned().expect("read whole"));
    assert_eq!((whole.notes.len(), whole.files.len(), whole.links.len()), (3, 2, 2));
    // The newest note first.
    assert!(top(cx, "item-note-0") < top(cx, "item-note-2"));

    // A session working on it opens where sessions are; Back comes back to the item.
    click(cx, "item-session-ses_signup");
    assert_eq!(workspace.read_with(cx, |workspace, _| (workspace.area(), workspace.opened().map(str::to_owned))), (Area::Sessions, Some("ses_signup".into())));
    cx.simulate_keystrokes("cmd-[");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Queue);
    assert!(shown(cx, "item-page") && shown(cx, "item-head"));
    // Its way back goes to the list it was opened from.
    click(cx, "item-back");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::Category("next".into())));

    // One with nothing written says so, and shows no empty sections.
    click(cx, "item-wqi_234");
    assert!(shown(cx, "item-no-body") && !shown(cx, "item-progress") && !shown(cx, "item-note-0"));
}

#[gpui::test]
fn an_item_on_another_computer_says_where_it_is_and_what_isnt_here(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    // One on a computer that's away: as it last told it, without what only it keeps.
    click(cx, "item-wqi_242");
    assert!(shown(cx, "item-elsewhere") && !shown(cx, "item-body") && !shown(cx, "item-no-body"));
    assert!(page.read_with(cx, |page, _| page.whole().is_some_and(|whole| whole.told)));
    // What's done to it is done there: that computer isn't answering, said in the box's words,
    // and nothing changes here.
    click(cx, "item-finish");
    assert!(shown(cx, "queue-problem"));
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items/wqi_242/move"), vec![json!({ "category": "done" })]);
    assert!(shown(cx, "item-finish") && !shown(cx, "item-state"));

    // One on a computer that answers is read there, whole.
    click(cx, "item-back");
    click(cx, "item-wqi_236");
    assert!(shown(cx, "item-elsewhere") && shown(cx, "item-no-body"));
    assert!(!shown(cx, "queue-problem"), "a refusal is its own page's");
}

#[gpui::test]
fn moving_an_item_and_marking_it_done_go_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);

    // From the list: an item's menu moves it to another of the box's categories.
    click(cx, "item-wqi_234-more");
    click(cx, "menu-item-Later");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items/wqi_234/move"), vec![json!({ "category": "later" })]);
    assert_eq!(item_said(&stand_in, "wqi_234")["category"], "later");
    // It's drawn where the box puts it: under Later, after what was there.
    assert!(top(cx, "item-wqi_234") > top(cx, "item-wqi_242") && top(cx, "item-wqi_234") < top(cx, "group-cat_design"));
    // The counts drawn are the box's, read again after the change.
    assert!(shown(cx, "group-next-count-5") && shown(cx, "group-later-count-7") && !shown(cx, "group-next-count-6"));

    // From its page: its category is a menu of the box's categories.
    click(cx, "item-wqi_234");
    click(cx, "item-category");
    click(cx, "menu-item-Design");
    assert_eq!(item_said(&stand_in, "wqi_234")["category"], "cat_design");

    // Mark Done is a move to Done; done, it says so and offers it no more.
    click(cx, "item-finish");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items/wqi_234/move").last(), Some(&json!({ "category": "done" })));
    assert!(item_said(&stand_in, "wqi_234")["doneAt"].is_number());
    assert!(shown(cx, "item-state") && !shown(cx, "item-finish"));
    // Back in the queue it's in Done, folded away.
    click(cx, "item-back");
    assert!(!shown(cx, "item-wqi_234"));
    click(cx, "group-done");
    assert!(shown(cx, "item-done-wqi_234"));
}

#[gpui::test]
fn an_issue_that_github_closes_is_never_offered_done_here(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    // The box says Done is GitHub's for it (`closesOn`): no Mark Done, and Done isn't among its moves.
    click(cx, "item-wqi_233");
    assert!(shown(cx, "item-head") && !shown(cx, "item-finish"));
    click(cx, "item-category");
    assert!(shown(cx, "menu-item-Later") && !shown(cx, "menu-item-Done"));
    click(cx, "menu-item-Later");
    assert_eq!(item_said(&stand_in, "wqi_233")["category"], "later");
}

#[gpui::test]
fn deleting_an_item_and_bringing_it_back(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    click(cx, "item-wqi_235-more");
    click(cx, "menu-item-Delete");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/work-queue/items/wqi_235").len(), 1);
    assert!(!shown(cx, "item-wqi_235"), "gone from its category");
    assert!(shown(cx, "group-deleted-count-2") && shown(cx, "group-next-count-5"), "counted again by the box");

    // It's kept under Deleted, folded, with the way back.
    click(cx, "group-deleted");
    assert!(shown(cx, "item-wqi_235") && shown(cx, "item-wqi_235-restore"));
    click(cx, "item-wqi_235-restore");
    assert_eq!(stand_in.seen_at("POST", "kuvo/work-queue/items/wqi_235/restore").len(), 1);
    assert!(top(cx, "item-wqi_235") < top(cx, "group-later"), "back in its category");
    assert!(!shown(cx, "item-wqi_235-restore"));

    // A deleted item's page says so, and brings it back too.
    click(cx, "item-wqi_262");
    assert!(shown(cx, "item-deleted") && !shown(cx, "item-finish"));
    click(cx, "notice-act");
    assert!(!shown(cx, "item-deleted"));
    assert!(item_said(&stand_in, "wqi_262").get("removed").is_none());
}

#[gpui::test]
fn adding_an_item_where_the_box_takes_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "queue-new");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::New));
    // Sent as written: with no title the box says why not, in its words, and the sheet stays.
    click(cx, "sheet-save");
    assert!(shown(cx, "sheet-error") && shown(cx, "queue-sheet"));
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items"), vec![json!({ "title": "", "body": "" })]);

    // The keyboard is in the title; the category is one of the box's.
    cx.simulate_input("A status page");
    click(cx, "sheet-body-field");
    cx.simulate_input("Uptime, and what's wrong now.");
    click(cx, "sheet-category");
    click(cx, "menu-item-Design");
    click(cx, "sheet-save");
    assert!(!shown(cx, "queue-sheet"));
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items").last(), Some(&json!({ "title": "A status page", "body": "Uptime, and what's wrong now.", "category": "cat_design" })));
    // It's in the queue as the box made it: numbered by the box, under Design.
    assert!(top(cx, "item-wqi_260") > top(cx, "item-wqi_245") && top(cx, "item-wqi_260") < top(cx, "group-parked"));

    // Escape closes the sheet with nothing sent.
    click(cx, "queue-new");
    cx.simulate_input("Never mind");
    cx.simulate_keystrokes("escape");
    assert!(!shown(cx, "queue-sheet"));
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items").len(), 2);
}

#[gpui::test]
fn rewriting_an_item_where_the_box_takes_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "item-wqi_234");
    click(cx, "item-edit");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::Edit("wqi_234".into())));
    // Its title as it is, to change; Return in it saves.
    assert_eq!(page.read_with(cx, |page, cx| page.item_fields().0.read(cx).text().to_owned()), "Dark mode for the settings page");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("Dark mode for Settings");
    click(cx, "sheet-body-field");
    cx.simulate_input("Every section, and the sheets.");
    click(cx, "sheet-save");
    assert!(!shown(cx, "queue-sheet"));
    let item = item_said(&stand_in, "wqi_234");
    assert_eq!((item["title"].as_str(), item["body"].as_str()), (Some("Dark mode for Settings"), Some("Every section, and the sheets.")));
    // The page shows it as the box has it now.
    assert_eq!(title(&workspace, cx).as_deref(), Some("Dark mode for Settings"));
    assert!(shown(cx, "item-body") && !shown(cx, "item-no-body"));

    // What the box won't have rewritten, it says why, and the sheet stays.
    click(cx, "item-back");
    click(cx, "item-wqi_233");
    click(cx, "item-edit");
    click(cx, "sheet-save");
    assert!(shown(cx, "sheet-error") && shown(cx, "queue-sheet"));
}

#[gpui::test]
fn a_box_that_takes_no_writing_offers_none(cx: &mut TestAppContext) {
    // Every box today: agents add items and write them; a client moves, finishes and deletes.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.queue_writable(false);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    assert!(!page.read_with(cx, |page, _| page.writes()));
    // The + is there, off (it says why where it's looked for), and opens nothing.
    assert!(!shown(cx, "queue-new") && shown(cx, "queue-new-off"));
    click(cx, "queue-new-off");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), None);
    // Said plainly, at the queue's foot.
    click(cx, "group-next");
    click(cx, "group-later");
    click(cx, "group-cat_design");
    assert!(shown(cx, "queue-read-only"));
    click(cx, "group-next");
    click(cx, "item-wqi_234");
    assert!(shown(cx, "item-finish") && !shown(cx, "item-edit"));
    click(cx, "item-finish");
    assert_eq!(item_said(&stand_in, "wqi_234")["category"], "done");
    assert!(bodies(&stand_in, "POST", "kuvo/work-queue/items").is_empty());
}

#[gpui::test]
fn categories_are_made_renamed_and_removed_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    // A new one, from the sidebar's +: it's a tab and a row as the box lists it.
    click(cx, "new-category");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::NewCategory));
    cx.simulate_input("Bugs");
    cx.simulate_keystrokes("enter");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/categories"), vec![json!({ "add": "Bugs" })]);
    assert!(!shown(cx, "queue-sheet") && shown(cx, "tab-cat_bugs") && shown(cx, "category-cat_bugs"));
    // A name the box won't take: its words, in the sheet.
    click(cx, "queue-more");
    click(cx, "menu-item-New Category…");
    cx.simulate_input("bugs");
    cx.simulate_keystrokes("enter");
    assert!(shown(cx, "sheet-error"));
    cx.simulate_keystrokes("escape");

    // One you made is renamed from its own list, where it says nothing's in it yet.
    click(cx, "tab-cat_bugs");
    assert!(shown(cx, "queue-empty"));
    click(cx, "queue-more");
    click(cx, "menu-item-Rename Bugs…");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("Defects");
    cx.simulate_keystrokes("enter");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/categories").last(), Some(&json!({ "rename": "cat_bugs", "to": "Defects" })));
    click(cx, "queue-more");
    assert!(shown(cx, "menu-item-Remove Defects"));
    // Removed, its list goes: back to the whole queue.
    click(cx, "menu-item-Remove Defects");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::All));
    assert!(!shown(cx, "tab-cat_bugs"));

    // One with items in it the box won't remove: it says why, and it stays.
    click(cx, "tab-cat_design");
    click(cx, "queue-more");
    click(cx, "menu-item-Remove Design");
    assert!(shown(cx, "queue-problem") && shown(cx, "tab-cat_design-on"));
    // One of the box's own offers neither.
    click(cx, "tab-next");
    click(cx, "queue-more");
    assert!(shown(cx, "menu-item-New Category…") && !shown(cx, "menu-item-Remove Next"));
}

#[gpui::test]
fn a_queue_that_cant_be_read_says_so_and_is_read_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.queue_readable(false);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "queue-failed") && !shown(cx, "group-next"));
    // Read again as the area is shown again.
    stand_in.queue_readable(true);
    click(cx, "rail-bots");
    click(cx, "rail-queue");
    assert!(shown(cx, "group-next") && !shown(cx, "queue-failed"));
}

#[gpui::test]
fn a_box_with_no_items_says_so(cx: &mut TestAppContext) {
    let stand_in = StandIn::empty(Replies::Never);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "queue-empty") && shown(cx, "tab-next") && !shown(cx, "group-next") && !shown(cx, "group-deleted"));
    // Nothing to filter by: no filter is offered.
    assert!(!shown(cx, "queue-filter"));
}

#[gpui::test]
fn every_count_drawn_is_the_boxs_never_the_apps(cx: &mut TestAppContext) {
    // A box counts what a view may not list whole: where its count and its list differ, the count
    // drawn is the box's.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.count_category("next", 41);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "group-next-count-41") && !shown(cx, "group-next-count-6"));
    assert!(shown(cx, "group-later-count-6"));
}

#[gpui::test]
fn a_change_whose_reread_fails_keeps_what_the_box_answered(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    // The box takes the move, then can't be read: the item stays where the box put it.
    stand_in.queue_readable(false);
    click(cx, "item-wqi_234-more");
    click(cx, "menu-item-Later");
    assert_eq!(item_said(&stand_in, "wqi_234")["category"], "later");
    assert!(top(cx, "item-wqi_234") > top(cx, "group-later"), "under Later, as the box answered");
}

/// The page as the app runs it: what it asks of the box is answered off the window's thread, so an
/// answer can come after you've gone elsewhere.
mod live {
    use super::*;
    use gpui::{AppContext as _, Context, IntoElement, ParentElement, Render, Styled, Window, div};
    use kuvo::areas::Reading;
    use kuvo::queue::Act;

    struct Host {
        page: Entity<QueueView>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.page.clone())
        }
    }

    fn host<'a>(cx: &'a mut TestAppContext, stand_in: &StandIn) -> (Entity<QueueView>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            Theme::init(Appearance::Light, cx);
            kuvo::bind_keys(cx);
        });
        let client = BoxClient::new(stand_in.clone());
        let said = client.work_queue_view().expect("the stand-in's queue");
        let (host, cx) = cx.add_window_view(|_, cx| Host { page: cx.new(|cx| QueueView::new(client, true, cx)) });
        let page = host.read_with(cx, |host, _| host.page.clone());
        cx.simulate_resize(size(px(1280.), px(1600.)));
        page.update(cx, |page, cx| {
            page.set_now(NOW);
            page.set_writes(true, cx);
            page.set_said(Reading::Ready(said), cx);
        });
        cx.run_until_parked();
        (page, cx)
    }

    #[gpui::test]
    fn an_item_read_after_youve_gone_to_another_is_never_drawn_as_that_one(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        // One opened, and before the box answers, another.
        page.update(cx, |page, cx| {
            page.show(Some("item:wqi_231"), cx);
            page.show(Some("item:wqi_234"), cx);
        });
        cx.run_until_parked();
        assert_eq!(page.read_with(cx, |page, _| page.whole().map(|whole| whole.item.id.clone())), Some("wqi_234".into()));
        assert!(shown(cx, "item-no-body") && !shown(cx, "item-progress"), "the second one's page, with none of the first's");
    }

    #[gpui::test]
    fn a_refusal_that_comes_after_youve_gone_shows_over_no_other_items_page(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("item:wqi_242"), cx));
        cx.run_until_parked();
        // Mark Done on one whose computer isn't answering, pressed twice, then on to another item
        // before the box says so.
        page.update(cx, |page, cx| {
            page.act(Act::Move { item: "wqi_242".into(), category: "done".into() }, cx);
            page.act(Act::Move { item: "wqi_242".into(), category: "done".into() }, cx);
            page.show(Some("item:wqi_234"), cx);
        });
        cx.run_until_parked();
        assert_eq!(stand_in.seen_at("POST", "kuvo/work-queue/items/wqi_242/move").len(), 1, "one at a time for an item");
        assert!(!shown(cx, "queue-problem") && shown(cx, "item-finish"));
        // Nothing was left busy: its own change still goes.
        click(cx, "item-finish");
        assert_eq!(item_said(&stand_in, "wqi_234")["category"], "done");
    }

    #[gpui::test]
    fn a_sheets_answer_is_its_own_openings(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        // New Item saved, and before the box answers: on to a category, and New Category opened and
        // saved too.
        page.update(cx, |page, cx| page.act(Act::New, cx));
        cx.run_until_parked();
        cx.simulate_input("A status page");
        page.update(cx, |page, cx| {
            page.save_sheet(cx);
            page.show(Some("category:later"), cx);
            page.act(Act::NewCategory, cx);
        });
        cx.run_until_parked();
        // The item was added and is in the list, and the sheet open now is still open, untouched.
        assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items").len(), 1);
        assert!(page.read_with(cx, |page, _| matches!(page.said(), Reading::Ready(said) if said.items.iter().any(|item| item.title == "A status page"))));
        assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::NewCategory));
        assert!(!shown(cx, "sheet-error"));
        // It saves as itself (nothing was left saving).
        cx.simulate_input("Bugs");
        cx.simulate_keystrokes("enter");
        assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/categories"), vec![json!({ "add": "Bugs" })]);
        assert!(!shown(cx, "queue-sheet"));
    }
}
