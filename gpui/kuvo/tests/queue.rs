//! The Work Queue as someone uses it, against the stand-in box (nothing real is reached): the queue
//! under the box's statuses with its counts, scope tabs, the search the box answers and the filters,
//! what's closed folded; an item's page; New Item with each chip's menu and its number keys; changing
//! an item's status, priority, assignee, due day and labels; closing, deleting and restoring; only
//! what the box says may be done offered; and making, changing and removing labels, each through
//! the box's routes with the box's answer drawn after, or why not in its words. Clicks and keys go
//! through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::queue::{Field, QueueView, Scope, Sheet, Shown};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

/// 2026-11-14, 09:53 UTC.
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

fn right_click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen")).center();
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
}

fn keys(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
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

/// The menu open under a chip, if one is.
fn menu(page: &Entity<QueueView>, cx: &mut VisualTestContext) -> Option<Field> {
    page.read_with(cx, |page, _| page.picker().map(|picker| picker.field))
}

#[gpui::test]
fn the_queue_lists_the_boxs_items_under_its_statuses_with_whats_closed_folded(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = queue(cx, &stand_in);
    assert_eq!(title(&workspace, cx).as_deref(), Some("Work Queue"));
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::All));

    // The box's statuses, in its order, each with the items the box lists in it, in its order.
    let rows = ["group-needs-you", "item-wqi_232", "group-in-review", "item-wqi_235", "group-in-progress", "item-wqi_231", "group-waiting", "item-wqi_242", "group-todo", "item-wqi_233", "item-wqi_234", "group-backlog", "item-wqi_237", "group-done", "group-canceled", "group-deleted"];
    let order: Vec<Pixels> = rows.iter().map(|row| top(cx, row)).collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "in the box's order: {order:?}");
    // A row says where its item stands, how pressing it is, what kind of work it is, who it's for
    // and when it's due; one a session is at work on says what's being done.
    for part in ["item-wqi_231-status-in-progress", "item-wqi_231-priority-high", "item-wqi_231-label-lbl_bug", "item-wqi_231-assignee", "item-wqi_231-due", "item-working-wqi_231", "item-wqi_232-priority-urgent"] {
        assert!(shown(cx, part), "{part}");
    }
    // A coding agent wears its own mark; an item on another computer says which.
    assert!(shown(cx, "agent-mark-claude") && shown(cx, "item-wqi_242-computer") && !shown(cx, "item-wqi_234-computer"));
    // Normal is no mark; an item with nothing set shows none of them.
    assert!(!shown(cx, "item-wqi_234-priority-normal") && !shown(cx, "item-wqi_234-assignee") && !shown(cx, "item-wqi_234-due") && !shown(cx, "item-working-wqi_234"));
    // More labels than a row shows fold to how many more.
    assert!(shown(cx, "item-wqi_235-label-lbl_docs") && shown(cx, "item-wqi_235-label-lbl_launch") && shown(cx, "item-wqi_235-more-labels") && !shown(cx, "item-wqi_235-label-lbl_design"));
    // The counts are the box's: on the groups, the tabs and the sidebar's rows.
    // (All says how many are open, in the sidebar and on its tab alike.)
    for count in ["group-todo-count-6", "group-done-count-12", "tab-todo-count-6", "status-todo-count-6", "status-canceled-count-2", "label-lbl_design-count-5", "queue-open-19", "tab-all-open-19"] {
        assert!(shown(cx, count), "{count}");
    }

    // What's closed is folded until it's opened; so is what's deleted.
    assert!(!shown(cx, "item-wqi_251") && !shown(cx, "item-wqi_248") && !shown(cx, "item-wqi_262"));
    let folded_height = bounds(cx, "group-done").unwrap().size.height;
    click(cx, "group-done");
    assert_eq!(bounds(cx, "group-done").unwrap().size.height, folded_height);
    assert!(shown(cx, "item-wqi_251") && shown(cx, "item-wqi_251-status-done"));
    click(cx, "group-done");
    assert!(!shown(cx, "item-wqi_251"));
    click(cx, "group-canceled");
    assert!(shown(cx, "item-wqi_248-status-canceled"));
    // Any group folds away.
    click(cx, "group-todo");
    assert!(!shown(cx, "item-wqi_234") && shown(cx, "item-wqi_237"));
}

#[gpui::test]
fn the_tabs_and_the_sidebar_pick_the_same_scope(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "tab-all-on"));

    // A status's tab: its items alone, with no header over them.
    click(cx, "tab-todo");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::Status("todo".into())));
    assert!(shown(cx, "item-wqi_234") && shown(cx, "item-wqi_243") && !shown(cx, "item-wqi_231") && !shown(cx, "group-todo"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Todo"));

    // A closed one from the sidebar: its list is open there, and it has a tab while it shows.
    assert!(!shown(cx, "tab-done"));
    click(cx, "status-done");
    assert!(shown(cx, "tab-done-on") && shown(cx, "item-wqi_251") && !shown(cx, "item-wqi_234"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Done"));

    // A label from the sidebar: the items wearing it, under their statuses.
    click(cx, "label-lbl_design");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::Label("lbl_design".into())));
    assert!(shown(cx, "tab-label-lbl_design-on") && shown(cx, "item-wqi_235") && shown(cx, "item-wqi_233") && shown(cx, "item-wqi_243") && !shown(cx, "item-wqi_234"));
    assert!(top(cx, "group-in-review") < top(cx, "group-todo"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Design"));
    // Its list is the box's (`labels=`), with the box's counts of it.
    assert_eq!(stand_in.seen_at("GET", "kuvo/work-queue").last().map(|seen| seen.path.clone()).as_deref(), Some("kuvo/work-queue?owner=all&counts=1&labels=lbl_design"));
    assert!(shown(cx, "group-todo-count-4") && shown(cx, "group-in-review-count-1") && shown(cx, "tab-todo-count-4") && shown(cx, "tab-all-open-5"));

    // Back goes to the scope before, and All Items to the whole queue.
    keys(cx, "cmd-[");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::Status("done".into())));
    click(cx, "queue-all");
    assert!(shown(cx, "tab-all-on") && shown(cx, "group-todo"));
}

#[gpui::test]
fn the_search_is_the_boxs_and_shows_its_counts_of_what_it_found(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);

    // What's typed is sent to the box, which searches what items say, their labels and notes: one
    // found by its description alone shows, and one in Done, unfolded.
    click(cx, "queue-search");
    cx.simulate_input("DOCS");
    assert_eq!(stand_in.seen_at("GET", "kuvo/work-queue").last().map(|seen| seen.path.clone()).as_deref(), Some("kuvo/work-queue?owner=all&counts=1&q=DOCS"));
    for found in ["item-wqi_235", "item-wqi_239", "item-wqi_240", "item-wqi_250", "item-wqi_252"] {
        assert!(shown(cx, found), "{found}");
    }
    assert!(!shown(cx, "item-wqi_231") && !shown(cx, "item-wqi_237") && !shown(cx, "group-todo"));
    // The counts over the list are the box's, of what it found; the sidebar's stay the whole queue's.
    for count in ["group-backlog-count-2", "group-done-count-2", "tab-backlog-count-2", "tab-todo-count-0", "status-backlog-count-7"] {
        assert!(shown(cx, count), "{count}");
    }
    // A number finds its item.
    keys(cx, "cmd-a backspace");
    cx.simulate_input("#234");
    assert!(shown(cx, "item-wqi_234") && !shown(cx, "item-wqi_239"));
    cx.simulate_input("zzz");
    assert!(shown(cx, "queue-empty"));
    // Escape clears it: the whole queue again, with its counts.
    keys(cx, "escape");
    assert!(shown(cx, "item-wqi_231") && shown(cx, "group-backlog-count-7") && !shown(cx, "item-wqi_252"));
}

#[gpui::test]
fn a_narrowed_list_follows_the_queue_as_it_changes(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "queue-filter");
    click(cx, "menu-item-devbox");
    assert!(shown(cx, "item-wqi_236") && shown(cx, "tab-all-open-1"));
    let asks = |stand_in: &StandIn| stand_in.seen_at("GET", "kuvo/work-queue").iter().filter(|seen| seen.path.contains("computers=key_devbox")).count();
    let before = asks(&stand_in);
    // An agent closes it meanwhile, and the queue is read again (as the area reads it): the list
    // under the filter is the box's answer to another ask, so it's asked again, not left as it was.
    let client = BoxClient::new(stand_in.clone());
    client
        .change_work_item("wqi_236", &kuvo_box::work_queue::ItemChange { status: Some("done".into()), ..Default::default() })
        .expect("the stand-in takes the change");
    let said = client.work_queue_view().expect("the stand-in's queue");
    page.update(cx, |page, cx| page.set_said(kuvo::areas::Reading::Ready(said), cx));
    cx.run_until_parked();
    assert_eq!(asks(&stand_in), before + 1);
    assert!(shown(cx, "tab-all-open-0") && shown(cx, "tab-todo-count-0"), "its counts are the box's, of the list as it is now");
}

#[gpui::test]
fn the_filters_are_the_boxs_and_its_counts_are_of_what_it_lists(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    let asked = |stand_in: &StandIn| stand_in.seen_at("GET", "kuvo/work-queue").last().map(|seen| seen.path.clone()).unwrap_or_default();
    // A filter the box's view offers is sent to the box, on as a chip to take off; what's listed
    // and every count over it are the box's answer. The sidebar keeps the whole queue's.
    click(cx, "queue-filter");
    click(cx, "menu-item-devbox");
    assert_eq!(asked(&stand_in), "kuvo/work-queue?owner=all&counts=1&computers=key_devbox");
    assert!(shown(cx, "filter-0") && shown(cx, "item-wqi_236") && !shown(cx, "item-wqi_231") && !shown(cx, "group-backlog"));
    for count in ["group-todo-count-1", "tab-todo-count-1", "tab-backlog-count-0", "tab-all-open-1", "status-todo-count-6", "queue-open-19"] {
        assert!(shown(cx, count), "{count}");
    }
    // Another of the same kind widens it; one of another kind narrows it.
    click(cx, "queue-filter");
    click(cx, "menu-item-studio-mini");
    assert_eq!(asked(&stand_in), "kuvo/work-queue?owner=all&counts=1&computers=key_devbox,key_studio");
    assert!(shown(cx, "item-wqi_236") && shown(cx, "item-wqi_242") && shown(cx, "tab-all-open-2"));
    click(cx, "queue-filter");
    click(cx, "menu-item-GitHub issues");
    assert_eq!(asked(&stand_in), "kuvo/work-queue?owner=all&counts=1&computers=key_devbox,key_studio&sources=github");
    assert!(shown(cx, "queue-empty") && shown(cx, "tab-all-open-0"));
    assert_eq!(page.read_with(cx, |page, _| page.filters().len()), 3);
    click(cx, "filters-clear");
    assert!(!shown(cx, "queue-filters") && shown(cx, "item-wqi_231") && shown(cx, "group-todo-count-6") && shown(cx, "tab-all-open-19"));
    // A source is its kind, as the box counts them: issues and pull requests are two filters.
    click(cx, "queue-filter");
    click(cx, "menu-item-GitHub pull requests");
    assert_eq!(asked(&stand_in), "kuvo/work-queue?owner=all&counts=1&sources=github-pr");
    assert!(shown(cx, "item-wqi_247") && !shown(cx, "item-wqi_233") && !shown(cx, "item-wqi_250"));
    click(cx, "queue-filter");
    click(cx, "menu-item-GitHub issues");
    assert!(shown(cx, "item-wqi_247") && shown(cx, "item-wqi_233") && shown(cx, "item-wqi_250") && !shown(cx, "item-wqi_234"));
    click(cx, "filters-clear");
    // The box asked is the computer with no key; an owner is one the box names.
    click(cx, "queue-filter");
    click(cx, "menu-item-This Mac");
    assert_eq!(asked(&stand_in), "kuvo/work-queue?owner=all&counts=1&computers=");
    assert!(shown(cx, "item-wqi_231") && !shown(cx, "item-wqi_236") && !shown(cx, "item-wqi_242") && shown(cx, "tab-all-open-15"));
    click(cx, "queue-filter");
    click(cx, "menu-item-Launch crew");
    assert_eq!(asked(&stand_in), "kuvo/work-queue?owner=all&counts=1&computers=&owners=grp_launch");
    assert!(shown(cx, "item-wqi_235") && !shown(cx, "item-wqi_231") && shown(cx, "group-in-review-count-1"));
    // With a search, both go to the box together.
    click(cx, "queue-search");
    cx.simulate_input("post");
    assert_eq!(asked(&stand_in), "kuvo/work-queue?owner=all&counts=1&q=post&computers=&owners=grp_launch");
    assert!(shown(cx, "item-wqi_235"));
}

#[gpui::test]
fn an_items_page_shows_what_the_box_says_of_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = queue(cx, &stand_in);
    assert!(stand_in.seen_at("GET", "kuvo/work-queue/items/wqi_231").is_empty(), "nothing is asked of an item until it's opened");
    click(cx, "tab-in-progress");
    click(cx, "item-wqi_231");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::Item("wqi_231".into()));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Keep what was typed when the sign-up email is taken"));
    assert_eq!(stand_in.seen_at("GET", "kuvo/work-queue/items/wqi_231").len(), 1);

    // Its title, that it's at work, its chips, how far it is, what it says, and what's kept with it.
    for part in ["item-head", "item-state", "chip-status", "chip-priority", "chip-assignee", "chip-due", "chip-labels", "item-progress", "item-progress-value", "item-body", "item-session-ses_signup", "item-link-https://example.com/tone-guide", "item-file-before.png", "item-note-0", "item-note-2", "item-facts"] {
        assert!(shown(cx, part), "{part}");
    }
    let whole = page.read_with(cx, |page, _| page.whole().cloned().expect("read whole"));
    assert_eq!((whole.notes.len(), whole.files.len(), whole.links.len()), (3, 2, 2));
    // The newest note first.
    assert!(top(cx, "item-note-0") < top(cx, "item-note-2"));

    // A session working on it opens where sessions are; Back comes back to the item.
    click(cx, "item-session-ses_signup");
    assert_eq!(workspace.read_with(cx, |workspace, _| (workspace.area(), workspace.opened().map(str::to_owned))), (Area::Sessions, Some("ses_signup".into())));
    keys(cx, "cmd-[");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Queue);
    assert!(shown(cx, "item-page") && shown(cx, "item-head"));
    // Its way back goes to the list it was opened from.
    click(cx, "item-back");
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::Status("in-progress".into())));

    // One with nothing written says so, and shows no empty sections.
    click(cx, "queue-all");
    click(cx, "item-wqi_234");
    assert!(shown(cx, "item-no-body") && !shown(cx, "item-progress") && !shown(cx, "item-note-0") && !shown(cx, "item-state"));
}

#[gpui::test]
fn an_item_on_a_computer_thats_away_says_so_and_offers_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    // Nothing may be done to it, the box says: its row has no menu, and a mark that says why (the
    // box's words, when rested on) where the menu would be.
    assert!(shown(cx, "item-wqi_234-more") && !shown(cx, "item-wqi_234-why") && !shown(cx, "item-wqi_242-more") && shown(cx, "item-wqi_242-why"));
    assert!(item_said(&stand_in, "wqi_242")["can"]["why"].as_str().is_some_and(|why| why.starts_with("studio-mini isn't answering now")));
    // Its page: as that computer last told it, without what only it keeps, and nothing offered.
    click(cx, "item-wqi_242");
    assert!(shown(cx, "item-elsewhere") && !shown(cx, "item-body") && !shown(cx, "item-no-body"));
    assert!(page.read_with(cx, |page, _| page.whole().is_some_and(|whole| whole.told)));
    assert!(!shown(cx, "item-finish") && !shown(cx, "item-edit") && !shown(cx, "item-more"));
    // Its status is said, and opens no menu.
    click(cx, "chip-status");
    assert_eq!(menu(&page, cx), None);
    assert!(!shown(cx, "chip-assignee") && !shown(cx, "chip-due"), "what's unset and can't be set isn't drawn");
    // A chip that can't be changed is drawn as a fact, not a control.
    assert!(shown(cx, "chip-status-fact") && shown(cx, "chip-priority-fact"));
    // Why, on the page, in the box's words.
    assert_eq!(page.read_with(cx, |page, _| page.notice("item-elsewhere")).as_deref(), Some("As studio-mini last reported it. Its description, notes, files and links are on studio-mini. studio-mini isn't answering now (it may be asleep, off or offline): it can be changed once it's back."));
    assert!(stand_in.seen_at("PATCH", "kuvo/work-queue/items/wqi_242").is_empty());

    // One on a computer that answers is read there, whole, and changed there.
    click(cx, "item-back");
    click(cx, "item-wqi_236");
    assert!(shown(cx, "item-elsewhere") && shown(cx, "item-no-body") && shown(cx, "item-finish") && !shown(cx, "chip-status-fact"));
    // Who it's for is said, and isn't changed from here (`can.assign`): this box's view lists its own
    // bots, and that computer's item is assigned there. Its other chips open.
    assert!(shown(cx, "chip-assignee-fact"));
    click(cx, "chip-assignee");
    assert_eq!(menu(&page, cx), None);
    click(cx, "chip-priority");
    assert_eq!(menu(&page, cx), Some(Field::Priority));
    keys(cx, "escape");
    assert_eq!(page.read_with(cx, |page, _| page.notice("item-elsewhere")).as_deref(), Some("Read from devbox. What you change here is changed there."));

    // One on a computer that runs an older Kuvo Code, and one on a machine shared outside your
    // setup: read there, and neither takes a change from here. Each says why, as the box does.
    for (item, why) in [("wqi_228", "Read from old-mini. old-mini runs an older Kuvo Code: update it to change this from here."), ("wqi_229", "Read from shared-box. It's on shared-box: change it there.")] {
        click(cx, "item-back");
        assert!(shown(cx, &format!("item-{item}-why")) && !shown(cx, &format!("item-{item}-more")));
        click(cx, &format!("item-{item}"));
        assert_eq!(page.read_with(cx, |page, _| page.notice("item-elsewhere")).as_deref(), Some(why));
        assert!(shown(cx, "chip-status-fact") && !shown(cx, "item-finish") && !shown(cx, "item-edit") && !shown(cx, "item-more"));
        click(cx, "chip-status");
        assert_eq!(menu(&page, cx), None);
    }
}

#[gpui::test]
fn each_chip_on_an_items_page_changes_it_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    let sent = |stand_in: &StandIn| bodies(stand_in, "PATCH", "kuvo/work-queue/items/wqi_234").last().cloned();
    click(cx, "item-wqi_234");

    // Its status: a menu of the box's statuses, a number beside each; 3 picks the third.
    click(cx, "chip-status");
    assert_eq!(menu(&page, cx), Some(Field::Status));
    assert!(shown(cx, "picker-status") && shown(cx, "pick-todo-on") && shown(cx, "pick-done"));
    keys(cx, "3");
    assert_eq!(sent(&stand_in), Some(json!({ "status": "in-progress" })));
    assert_eq!(menu(&page, cx), None);
    assert_eq!(item_said(&stand_in, "wqi_234")["status"], "in-progress");

    // Its priority, by a click.
    click(cx, "chip-priority");
    click(cx, "pick-urgent");
    assert_eq!(sent(&stand_in), Some(json!({ "priority": "urgent" })));

    // Who it's for: typing narrows the menu, Return picks.
    click(cx, "chip-assignee");
    assert!(shown(cx, "pick-none-on") && shown(cx, "pick-bot_mira") && shown(cx, "pick-codex-cli"));
    cx.simulate_input("sco");
    assert!(shown(cx, "pick-bot_scout") && !shown(cx, "pick-bot_mira") && !shown(cx, "pick-none"));
    keys(cx, "enter");
    assert_eq!(sent(&stand_in), Some(json!({ "assignee": { "kind": "bot", "id": "bot_scout" } })));
    // Down and Return, to no one again.
    click(cx, "chip-assignee");
    keys(cx, "up up enter");
    assert_eq!(sent(&stand_in), Some(json!({ "assignee": null })));

    // When it's due: tomorrow by its number, a day typed, then none.
    click(cx, "chip-due");
    keys(cx, "2");
    assert_eq!(sent(&stand_in), Some(json!({ "due": "2026-11-15" })));
    click(cx, "chip-due");
    assert!(shown(cx, "pick-tomorrow-on"));
    click(cx, "picker-line");
    cx.simulate_input("2026-12-01");
    keys(cx, "enter");
    assert_eq!(sent(&stand_in), Some(json!({ "due": "2026-12-01" })));
    click(cx, "chip-due");
    assert!(shown(cx, "pick-set-on"));
    click(cx, "pick-none");
    assert_eq!(sent(&stand_in), Some(json!({ "due": null })));
    // A day the box can't read: why not, in its words, and nothing changed.
    click(cx, "chip-due");
    click(cx, "picker-line");
    cx.simulate_input("soon");
    keys(cx, "enter");
    assert!(shown(cx, "queue-problem") && item_said(&stand_in, "wqi_234").get("due").is_none());

    // Its labels: several, the menu staying open; picking one again takes it off.
    click(cx, "chip-labels");
    keys(cx, "1");
    assert_eq!(sent(&stand_in), Some(json!({ "labels": ["lbl_bug"] })));
    assert_eq!(menu(&page, cx), Some(Field::Labels));
    assert!(shown(cx, "pick-lbl_bug-on"));
    keys(cx, "2");
    assert_eq!(sent(&stand_in), Some(json!({ "labels": ["lbl_bug", "lbl_design"] })));
    keys(cx, "1");
    assert_eq!(sent(&stand_in), Some(json!({ "labels": ["lbl_design"] })));
    // Escape closes the menu, and only it.
    keys(cx, "escape");
    assert_eq!(menu(&page, cx), None);
    assert!(shown(cx, "item-page") && shown(cx, "item-head"));
    // A press anywhere else closes a menu too, and does nothing more.
    click(cx, "chip-status");
    click(cx, "item-finish");
    assert_eq!(menu(&page, cx), None);
    assert_eq!(item_said(&stand_in, "wqi_234")["status"], "in-progress");

    // Mark Done closes it; done, it's offered no more, and Mark Canceled is.
    click(cx, "item-finish");
    assert_eq!(sent(&stand_in), Some(json!({ "status": "done" })));
    assert!(item_said(&stand_in, "wqi_234")["doneAt"].is_number() && !shown(cx, "item-finish"));
    click(cx, "item-more");
    click(cx, "menu-item-Mark Canceled");
    assert_eq!(item_said(&stand_in, "wqi_234")["status"], "canceled");
    // Back in the queue it's under Canceled, folded away.
    click(cx, "item-back");
    assert!(!shown(cx, "item-wqi_234"));
    click(cx, "group-canceled");
    assert!(shown(cx, "item-wqi_234-status-canceled") && shown(cx, "item-wqi_234-priority-urgent") && shown(cx, "item-wqi_234-label-lbl_design"));
}

#[gpui::test]
fn rewriting_an_items_title_and_description(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "item-wqi_234");
    click(cx, "item-edit");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::Edit("wqi_234".into())));
    // Its title as it is, to change; Return goes on to the description, and ⌘↩ saves.
    assert_eq!(page.read_with(cx, |page, cx| page.item_fields().0.read(cx).text().to_owned()), "Dark mode for the settings page");
    keys(cx, "cmd-a backspace");
    cx.simulate_input("Dark mode for Settings");
    keys(cx, "enter");
    cx.simulate_input("Every section, and the sheets.");
    assert!(shown(cx, "queue-sheet"));
    keys(cx, "cmd-enter");
    assert!(!shown(cx, "queue-sheet"));
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234"), vec![json!({ "title": "Dark mode for Settings", "body": "Every section, and the sheets." })]);
    // The page shows it as the box has it now.
    assert_eq!(title(&workspace, cx).as_deref(), Some("Dark mode for Settings"));
    assert!(shown(cx, "item-body") && !shown(cx, "item-no-body"));
    // Only what was changed is sent: the title alone; and with nothing changed, nothing.
    click(cx, "item-edit");
    keys(cx, "cmd-a backspace");
    cx.simulate_input("Dark mode, everywhere");
    keys(cx, "cmd-enter");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234").last(), Some(&json!({ "title": "Dark mode, everywhere" })));
    click(cx, "item-edit");
    keys(cx, "cmd-enter");
    assert!(!shown(cx, "queue-sheet"));
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234").len(), 2);
    // A title the box won't take: why not, in its words, and the sheet stays.
    click(cx, "item-edit");
    keys(cx, "cmd-a backspace");
    click(cx, "sheet-save");
    assert!(shown(cx, "sheet-error") && shown(cx, "queue-sheet"));
}

#[gpui::test]
fn only_what_the_box_says_may_be_done_is_offered(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    // An item of your own: everything.
    click(cx, "item-wqi_234-more");
    assert!(shown(cx, "menu-item-Mark Done") && shown(cx, "menu-item-Mark Canceled") && shown(cx, "menu-item-Backlog") && shown(cx, "menu-item-Delete"));
    keys(cx, "escape");

    // An issue: its words and its closing are GitHub's. From its row, no closing.
    click(cx, "item-wqi_233-more");
    assert!(!shown(cx, "menu-item-Mark Done") && !shown(cx, "menu-item-Mark Canceled") && shown(cx, "menu-item-Backlog") && shown(cx, "menu-item-Delete"));
    keys(cx, "escape");
    // On its page: no Mark Done, no Edit…, and its status's menu has the open ones alone.
    click(cx, "item-wqi_233");
    assert!(shown(cx, "item-head") && !shown(cx, "item-finish") && !shown(cx, "item-edit"));
    // Why, in the box's words, in a notice of its own.
    assert!(shown(cx, "item-why"));
    assert_eq!(page.read_with(cx, |page, _| page.notice("item-why")).as_deref(), Some("Its title, its description and closing it are GitHub's."));
    click(cx, "chip-status");
    assert!(shown(cx, "pick-backlog") && shown(cx, "pick-needs-you") && !shown(cx, "pick-done") && !shown(cx, "pick-canceled"));
    click(cx, "pick-backlog");
    assert_eq!(item_said(&stand_in, "wqi_233")["status"], "backlog");
    click(cx, "item-more");
    assert!(shown(cx, "menu-item-Delete") && !shown(cx, "menu-item-Mark Canceled"));
    keys(cx, "escape");

    // One GitHub closed: where it stands is GitHub's too. Only a delete is offered.
    click(cx, "item-back");
    click(cx, "group-done");
    click(cx, "item-wqi_250");
    assert_eq!(page.read_with(cx, |page, _| page.notice("item-why")).as_deref(), Some("It's closed on GitHub: it's back where it stood once it's reopened there."));
    click(cx, "chip-status");
    assert_eq!(menu(&page, cx), None);
    click(cx, "chip-priority");
    assert_eq!(menu(&page, cx), None);
    assert!(!shown(cx, "item-finish") && !shown(cx, "item-edit") && !shown(cx, "chip-assignee"));
    click(cx, "item-more");
    assert!(shown(cx, "menu-item-Delete") && !shown(cx, "menu-item-Mark Canceled"));
    keys(cx, "escape");
    assert!(stand_in.seen_at("PATCH", "kuvo/work-queue/items/wqi_250").is_empty());

    // One on its way to another computer: only its delete, and why is said where that's said.
    click(cx, "item-back");
    click(cx, "item-wqi_241-more");
    assert!(shown(cx, "menu-item-Delete") && !shown(cx, "menu-item-Mark Done") && !shown(cx, "menu-item-Todo"));
    keys(cx, "escape");
    click(cx, "item-wqi_241");
    assert!(shown(cx, "item-handing") && !shown(cx, "item-why") && !shown(cx, "item-finish") && !shown(cx, "item-edit"));
    assert_eq!(page.read_with(cx, |page, _| page.notice("item-handing")).as_deref(), Some("It's on its way to Wren on another computer: it can be changed there once it's arrived."));
    click(cx, "chip-status");
    assert_eq!(menu(&page, cx), None);
    // An item of your own has nothing to explain.
    click(cx, "item-back");
    click(cx, "item-wqi_234");
    assert!(!shown(cx, "item-why"));
}

#[gpui::test]
fn a_new_item_starts_as_the_box_says(cx: &mut TestAppContext) {
    // What an item added with nothing said starts as is the box's (`new`): New Item shows that.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.new_items_start("backlog", "low");
    let (_workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "queue-new");
    assert_eq!(page.read_with(cx, |page, _| (page.draft().status.clone(), page.draft().priority.clone())), (Some("backlog".into()), Some("low".into())));
    click(cx, "chip-priority");
    assert!(shown(cx, "pick-low-on"));
    keys(cx, "escape");
    cx.simulate_input("Later, maybe");
    keys(cx, "cmd-enter");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items"), vec![json!({ "title": "Later, maybe", "status": "backlog", "priority": "low" })]);
    assert!(shown(cx, "item-wqi_263-status-backlog") && shown(cx, "item-wqi_263-priority-low"));
}

#[gpui::test]
fn a_labels_menu_opens_from_its_row_without_picking_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    // The ••• under the pointer on a label's row: the same menu as its right-click.
    let row = bounds(cx, "label-lbl_docs").expect("the label's row");
    cx.simulate_mouse_move(row.center(), None, Modifiers::default());
    click(cx, "label-lbl_docs-more");
    assert!(shown(cx, "menu-item-Edit Label…") && shown(cx, "menu-item-Remove Label…"));
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::All), "its list isn't opened by it");
    click(cx, "menu-item-Edit Label…");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::EditLabel("lbl_docs".into())));
}

#[gpui::test]
fn deleting_an_item_and_bringing_it_back(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "item-wqi_234-more");
    click(cx, "menu-item-Delete");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/work-queue/items/wqi_234").len(), 1);
    assert!(!shown(cx, "item-wqi_234"), "gone from its status");
    assert!(shown(cx, "group-deleted-count-2") && shown(cx, "group-todo-count-5") && shown(cx, "status-todo-count-5"), "counted again by the box");

    // It's kept under Deleted, folded, with the way back and nothing else.
    click(cx, "group-deleted");
    assert!(shown(cx, "item-wqi_234") && shown(cx, "item-wqi_234-restore") && !shown(cx, "item-wqi_234-more"));
    click(cx, "item-wqi_234-restore");
    assert_eq!(stand_in.seen_at("POST", "kuvo/work-queue/items/wqi_234/restore").len(), 1);
    assert!(top(cx, "item-wqi_234") < top(cx, "group-backlog"), "back in its status");
    assert!(!shown(cx, "item-wqi_234-restore"));

    // A deleted item's page says so, offers only to bring it back, and does.
    click(cx, "item-wqi_262");
    assert!(shown(cx, "item-deleted") && !shown(cx, "item-finish") && !shown(cx, "item-edit") && !shown(cx, "item-more"));
    assert_eq!(page.read_with(cx, |page, _| page.notice("item-deleted")).as_deref(), Some("It's deleted: bring it back to change it."));
    click(cx, "chip-status");
    assert_eq!(menu(&page, cx), None);
    click(cx, "notice-act");
    assert!(!shown(cx, "item-deleted") && shown(cx, "item-finish"));
    assert!(item_said(&stand_in, "wqi_262").get("removed").is_none());
}

#[gpui::test]
fn a_new_item_is_written_and_its_chips_picked_by_their_menus(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    let posted = |stand_in: &StandIn| bodies(stand_in, "POST", "kuvo/work-queue/items");
    click(cx, "queue-new");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::New));
    for chip in ["chip-status", "chip-priority", "chip-assignee", "chip-due", "chip-labels", "create-more"] {
        assert!(shown(cx, chip), "{chip}");
    }
    // Sent as written: with no title the box says why not, in its words, and the sheet stays.
    keys(cx, "cmd-enter");
    assert!(shown(cx, "sheet-error") && shown(cx, "queue-sheet"));
    assert_eq!(posted(&stand_in), vec![json!({ "title": "", "status": "todo", "priority": "normal" })]);

    // The keyboard is in the title; Return goes on to the description.
    cx.simulate_input("A status page");
    keys(cx, "enter");
    cx.simulate_input("Uptime, and what's wrong now.");
    assert!(shown(cx, "queue-sheet") && posted(&stand_in).len() == 1);

    // Each chip opens its menu, and a number picks: the second status, the first priority.
    click(cx, "chip-status");
    assert!(shown(cx, "picker-status") && shown(cx, "pick-todo-on") && shown(cx, "pick-canceled"));
    keys(cx, "2");
    assert_eq!(menu(&page, cx), None);
    click(cx, "chip-priority");
    assert!(shown(cx, "pick-normal-on"));
    keys(cx, "1");
    // After a menu the keyboard is back where it was: in the description.
    cx.simulate_input(" More soon.");
    assert_eq!(page.read_with(cx, |page, cx| page.item_fields().1.read(cx).text().to_owned()), "Uptime, and what's wrong now. More soon.");
    // Who it's for (the first is no one; each coding agent by its own mark), and when it's due.
    click(cx, "chip-assignee");
    assert!(shown(cx, "agent-mark-claude") && shown(cx, "agent-mark-codex") && shown(cx, "agent-mark-cursor"));
    keys(cx, "2");
    click(cx, "chip-due");
    assert!(shown(cx, "pick-today") && shown(cx, "pick-tomorrow") && shown(cx, "pick-week") && shown(cx, "pick-none-on"));
    keys(cx, "1");
    // Labels: several, and one made from what's typed; Escape closes the menu, not the sheet.
    click(cx, "chip-labels");
    keys(cx, "1");
    assert_eq!(menu(&page, cx), Some(Field::Labels));
    cx.simulate_input("Perf");
    assert!(shown(cx, "pick-make") && !shown(cx, "pick-lbl_bug"));
    keys(cx, "enter");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/labels"), vec![json!({ "add": "Perf" })]);
    assert!(shown(cx, "pick-lbl_n1-on") && shown(cx, "pick-lbl_bug-on") && shown(cx, "label-lbl_n1"));
    keys(cx, "escape");
    assert_eq!(menu(&page, cx), None);
    assert!(shown(cx, "queue-sheet"));
    assert_eq!(page.read_with(cx, |page, _| page.draft().clone()), kuvo::queue::Draft { status: Some("in-review".into()), priority: Some("urgent".into()), labels: vec!["lbl_bug".into(), "lbl_n1".into()], assignee: Some(("bot".into(), "bot_mira".into())), due: Some("2026-11-14".into()) });

    // ⌘↩ makes it: everything picked is sent, and whose queue it goes in is the box's to decide.
    keys(cx, "cmd-enter");
    assert!(!shown(cx, "queue-sheet"));
    assert_eq!(
        posted(&stand_in).last(),
        Some(&json!({ "title": "A status page", "body": "Uptime, and what's wrong now. More soon.", "status": "in-review", "priority": "urgent", "labels": ["lbl_bug", "lbl_n1"], "assignee": { "kind": "bot", "id": "bot_mira" }, "due": "2026-11-14" }))
    );
    // It's in the queue as the box made it: numbered by the box, under In review.
    assert!(top(cx, "item-wqi_263") > top(cx, "item-wqi_235") && top(cx, "item-wqi_263") < top(cx, "group-in-progress"));
    for part in ["item-wqi_263-status-in-review", "item-wqi_263-priority-urgent", "item-wqi_263-label-lbl_bug", "item-wqi_263-assignee", "item-wqi_263-due"] {
        assert!(shown(cx, part), "{part}");
    }

    // Escape closes the sheet with nothing sent.
    click(cx, "queue-new");
    cx.simulate_input("Never mind");
    keys(cx, "escape");
    assert!(!shown(cx, "queue-sheet"));
    assert_eq!(posted(&stand_in).len(), 2);
}

#[gpui::test]
fn create_more_keeps_the_sheet_open_for_the_next_one(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    // From a status's list, a new item starts there.
    click(cx, "status-backlog");
    click(cx, "queue-new");
    assert_eq!(page.read_with(cx, |page, _| page.draft().status.clone()).as_deref(), Some("backlog"));
    click(cx, "create-more");
    click(cx, "sheet-title-field");
    cx.simulate_input("One");
    keys(cx, "cmd-enter");
    // Made, and said so; the sheet stays, its text cleared, the keyboard back in the title.
    assert!(shown(cx, "queue-sheet") && shown(cx, "sheet-created") && shown(cx, "item-wqi_263"));
    assert_eq!(page.read_with(cx, |page, cx| page.item_fields().0.read(cx).text().to_owned()), "");
    cx.simulate_input("Two");
    click(cx, "sheet-save");
    assert!(shown(cx, "queue-sheet") && shown(cx, "item-wqi_264"));
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items"), vec![json!({ "title": "One", "status": "backlog", "priority": "normal" }), json!({ "title": "Two", "status": "backlog", "priority": "normal" })]);
    keys(cx, "escape");
    assert!(!shown(cx, "queue-sheet"));
}

#[gpui::test]
fn labels_are_made_changed_and_removed_through_the_box(cx: &mut TestAppContext) {
    // A box with none: there are no labels until you make one, and it says so.
    let stand_in = StandIn::empty(Replies::Never);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    let sent = |stand_in: &StandIn| bodies(stand_in, "POST", "kuvo/work-queue/labels");
    assert!(shown(cx, "labels-none"));
    // New Item's Labels menu says so too, and makes the first from what's typed.
    click(cx, "queue-new");
    click(cx, "chip-labels");
    assert!(shown(cx, "picker-empty") && !shown(cx, "pick-make"));
    cx.simulate_input("Bug");
    click(cx, "pick-make");
    assert_eq!(sent(&stand_in), vec![json!({ "add": "Bug" })]);
    assert!(shown(cx, "pick-lbl_n1-on") && shown(cx, "label-lbl_n1") && !shown(cx, "labels-none"));
    assert_eq!(page.read_with(cx, |page, _| page.draft().labels.clone()), vec!["lbl_n1".to_owned()]);
    keys(cx, "escape escape");
    assert!(!shown(cx, "queue-sheet"));

    // The sidebar's + makes one, in a colour picked.
    click(cx, "new-label");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::NewLabel));
    cx.simulate_input("Design");
    click(cx, "color-violet");
    assert!(shown(cx, "color-violet-on"));
    keys(cx, "enter");
    assert_eq!(sent(&stand_in).last(), Some(&json!({ "add": "Design", "color": "violet" })));
    assert!(!shown(cx, "queue-sheet") && shown(cx, "label-lbl_n2"));
    // A name the box won't take: its words, in the sheet.
    click(cx, "queue-more");
    click(cx, "menu-item-New Label…");
    cx.simulate_input("design");
    keys(cx, "enter");
    assert!(shown(cx, "sheet-error") && shown(cx, "queue-sheet"));
    keys(cx, "escape");

    // A label's own menu renames and recolours it.
    right_click(cx, "label-lbl_n2");
    click(cx, "menu-item-Edit Label…");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::EditLabel("lbl_n2".into())));
    assert!(shown(cx, "color-violet-on"));
    keys(cx, "cmd-a backspace");
    cx.simulate_input("Look");
    click(cx, "color-sky");
    click(cx, "sheet-save");
    assert_eq!(sent(&stand_in).last(), Some(&json!({ "change": "lbl_n2", "name": "Look", "color": "sky" })));
    assert_eq!(said(&stand_in, "kuvo/work-queue/labels")["labels"][1], json!({ "id": "lbl_n2", "name": "Look", "color": "sky" }));

    // From its own list: removing it is asked first, and takes its list with it.
    click(cx, "label-lbl_n2");
    assert!(shown(cx, "queue-empty") && shown(cx, "tab-label-lbl_n2-on"));
    click(cx, "queue-more");
    click(cx, "menu-item-Remove Look…");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::RemoveLabel("lbl_n2".into())));
    assert!(shown(cx, "sheet-removes"));
    assert_eq!(sent(&stand_in).len(), 4, "nothing is removed until it's said");
    click(cx, "sheet-save");
    assert_eq!(sent(&stand_in).last(), Some(&json!({ "remove": "lbl_n2" })));
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::List(Scope::All));
    assert!(!shown(cx, "label-lbl_n2") && shown(cx, "label-lbl_n1"));
}

#[gpui::test]
fn removing_a_label_takes_it_off_every_item(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "item-wqi_231-label-lbl_bug") && shown(cx, "item-wqi_233-label-lbl_bug"));
    right_click(cx, "label-lbl_bug");
    click(cx, "menu-item-Remove Label…");
    // Cancel leaves it.
    click(cx, "sheet-cancel");
    assert!(shown(cx, "label-lbl_bug") && bodies(&stand_in, "POST", "kuvo/work-queue/labels").is_empty());
    right_click(cx, "label-lbl_bug");
    click(cx, "menu-item-Remove Label…");
    keys(cx, "enter");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/labels"), vec![json!({ "remove": "lbl_bug" })]);
    assert!(!shown(cx, "label-lbl_bug") && !shown(cx, "item-wqi_231-label-lbl_bug") && !shown(cx, "item-wqi_233-label-lbl_bug"));
    // The items stay, with their other labels.
    assert!(shown(cx, "item-wqi_231") && shown(cx, "item-wqi_233-label-lbl_design") && shown(cx, "label-lbl_design-count-5"));
}

#[gpui::test]
fn a_long_list_of_labels_scrolls_and_narrows_as_its_typed(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.many_labels();
    let (_workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "queue-new");
    click(cx, "chip-labels");
    // The menu stays its own height, inside the window; what's past its end is reached by typing.
    let (menu, window) = (bounds(cx, "picker-labels").expect("the menu"), cx.update(|window, _| window.viewport_size()));
    assert!(menu.size.height < px(400.) && menu.bottom() <= window.height);
    assert!(bounds(cx, "pick-lbl_m13").is_none_or(|row| row.top() >= menu.bottom() || !menu.contains(&row.center())));
    cx.simulate_input("edit");
    assert!(shown(cx, "pick-lbl_m13") && !shown(cx, "pick-lbl_bug"));
    keys(cx, "enter");
    assert_eq!(page.read_with(cx, |page, _| page.draft().labels.clone()), vec!["lbl_m13".to_owned()]);
    // Typed over, a number is part of the name: it picks nothing.
    cx.simulate_input("a11y 2");
    assert_eq!(page.read_with(cx, |page, cx| page.picker_field().read(cx).text().to_owned()), "a11y 2");
    assert_eq!(page.read_with(cx, |page, _| page.draft().labels.len()), 1);
}

#[gpui::test]
fn a_due_day_is_red_only_where_the_box_says_its_late(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    // Late is the box's word (`overdue`): yesterday's, still open, is red; one to come isn't.
    assert!(shown(cx, "item-wqi_235-due-late") && shown(cx, "item-wqi_231-due") && !shown(cx, "item-wqi_231-due-late"));
    assert_eq!((item_said(&stand_in, "wqi_235")["overdue"].clone(), item_said(&stand_in, "wqi_251").get("overdue").cloned()), (json!(true), None));
    // A closed item whose day is gone by isn't late, nor is a deleted one: the app works none of it out.
    click(cx, "group-deleted");
    assert!(shown(cx, "item-wqi_262-due") && !shown(cx, "item-wqi_262-due-late"));
    click(cx, "group-deleted");
    click(cx, "group-done");
    assert!(shown(cx, "item-wqi_251-due") && !shown(cx, "item-wqi_251-due-late"));
    // On its page too.
    click(cx, "item-wqi_251");
    assert!(shown(cx, "chip-due"));
}

#[gpui::test]
fn a_changes_answer_keeps_the_sessions_already_known(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    // A change's answer names its sessions "New Session", none at work; the view says them again
    // when it's read, which fails here: what the app knew of them stays.
    stand_in.queue_readable(false);
    click(cx, "item-wqi_231-more");
    click(cx, "menu-item-In review");
    assert_eq!(item_said(&stand_in, "wqi_231")["status"], "in-review");
    assert!(shown(cx, "item-wqi_231-status-in-review") && shown(cx, "item-working-wqi_231"), "still at work, as it was");
    let sessions = page.read_with(cx, |page, _| match page.said() {
        kuvo::areas::Reading::Ready(said) => said.items.iter().find(|item| item.id == "wqi_231").map(|item| item.sessions.clone()).unwrap_or_default(),
        _ => Vec::new(),
    });
    assert_eq!(sessions.iter().map(|session| (session.title.as_str(), session.working)).collect::<Vec<_>>(), vec![("Fix the sign-up form wiping on a taken email", true), ("Move the blog to the new CMS", false)]);
    // Closed, the box unlinks everyone: none is kept then.
    click(cx, "item-wqi_231-more");
    click(cx, "menu-item-Mark Done");
    click(cx, "group-done");
    assert!(shown(cx, "item-wqi_231-status-done") && !shown(cx, "item-working-wqi_231"));
}

#[gpui::test]
fn assigning_your_own_item_to_a_bot_moves_it_to_that_bots_queue(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "queue-new");
    cx.simulate_input("Mine for now");
    keys(cx, "cmd-enter");
    assert_eq!(item_said(&stand_in, "wqi_263")["owner"], "usr_you");
    click(cx, "item-wqi_263");
    assert!(page.read_with(cx, |page, _| page.facts_line()).is_some_and(|line| line.ends_with("In your own list")));
    // For a bot of this box, the box hands it on: it's in that bot's queue from then.
    click(cx, "chip-assignee");
    click(cx, "pick-bot_mira");
    let item = item_said(&stand_in, "wqi_263");
    assert_eq!((item["owner"].as_str(), item["handedBy"].as_str()), (Some("ses_mira"), Some("usr_you")));
    assert!(page.read_with(cx, |page, _| page.facts_line()).is_some_and(|line| line.ends_with("In Mira’s queue")));

    // Your own list is an owner the box names ("Your own"): one to filter by, as any other.
    click(cx, "item-back");
    click(cx, "queue-new");
    cx.simulate_input("Mine still");
    keys(cx, "cmd-enter");
    click(cx, "queue-filter");
    click(cx, "menu-item-Your own");
    assert!(shown(cx, "item-wqi_264") && !shown(cx, "item-wqi_263") && !shown(cx, "item-wqi_231"));
}

#[gpui::test]
fn a_label_is_made_from_the_menu_on_an_items_page(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    click(cx, "item-wqi_234");
    click(cx, "chip-labels");
    cx.simulate_input("Infra");
    assert!(shown(cx, "pick-make") && !shown(cx, "pick-lbl_bug"));
    keys(cx, "enter");
    // Made by the box, then put on the item; the menu stays for more.
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/labels"), vec![json!({ "add": "Infra" })]);
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234"), vec![json!({ "labels": ["lbl_n1"] })]);
    assert_eq!(menu(&page, cx), Some(Field::Labels));
    assert!(shown(cx, "pick-lbl_n1-on") && shown(cx, "label-lbl_n1"));
    // A name the box won't take is said in the menu, in its words.
    cx.simulate_input("  ");
    cx.simulate_input("infra");
    assert!(!shown(cx, "pick-make"), "one of that name is there already");
    keys(cx, "escape");
    click(cx, "item-back");
    assert!(shown(cx, "item-wqi_234-label-lbl_n1"));
}

#[gpui::test]
fn a_status_this_app_has_never_heard_of_is_listed_as_the_box_says(cx: &mut TestAppContext) {
    // A newer box lists a status of its own: its group, tab and sidebar row are there, in its place,
    // and it's one of the statuses an item can be given.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.newer_status();
    let (_workspace, page, cx) = queue(cx, &stand_in);
    for part in ["group-blocked", "item-wqi_227", "item-wqi_227-status-blocked", "tab-blocked-count-1", "status-blocked-count-1"] {
        assert!(shown(cx, part), "{part}");
    }
    assert!(top(cx, "group-waiting") < top(cx, "group-blocked") && top(cx, "group-blocked") < top(cx, "group-todo"));
    click(cx, "item-wqi_234");
    click(cx, "chip-status");
    click(cx, "pick-blocked");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234"), vec![json!({ "status": "blocked" })]);
    assert_eq!(menu(&page, cx), None);
    click(cx, "item-back");
    assert!(shown(cx, "item-wqi_234-status-blocked"));
    // Its group holds both now, under the box's name for it, with the box's count; its own list too.
    assert!(shown(cx, "group-blocked-count-2") && top(cx, "item-wqi_227") > top(cx, "group-blocked") && top(cx, "item-wqi_234") < top(cx, "group-todo"));
    click(cx, "status-blocked");
    assert!(shown(cx, "tab-blocked-on") && shown(cx, "item-wqi_227") && shown(cx, "item-wqi_234") && !shown(cx, "item-wqi_233"));
}

#[gpui::test]
fn which_statuses_close_an_item_and_a_labels_colours_are_the_boxs(cx: &mut TestAppContext) {
    // A newer box lists a closed status (Shelved) and a label colour (teal) this app has never
    // heard of: Shelved folds with Done and Canceled, is offered wherever closing is, and teal is
    // one of the colours a label may be. The app keeps no list of its own of either.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.newer_closed_status();
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "group-shelved") && top(cx, "group-canceled") < top(cx, "group-shelved") && !shown(cx, "item-wqi_225"));
    assert!(shown(cx, "queue-open-19"), "the box's count of what's open");
    click(cx, "group-shelved");
    assert!(shown(cx, "item-wqi_225-status-shelved"));

    // From a row: each closed status, by the box's name.
    click(cx, "item-wqi_234-more");
    assert!(shown(cx, "menu-item-Mark Done") && shown(cx, "menu-item-Mark Canceled") && shown(cx, "menu-item-Mark Shelved"));
    click(cx, "menu-item-Mark Shelved");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234").last(), Some(&json!({ "status": "shelved" })));

    // On its page, shelved: Mark Done is its button, and Mark Canceled is in its menu; Mark
    // Shelved is offered no more.
    click(cx, "item-wqi_234");
    assert!(shown(cx, "item-finish"));
    click(cx, "item-more");
    assert!(shown(cx, "menu-item-Mark Canceled") && !shown(cx, "menu-item-Mark Shelved"));
    keys(cx, "escape");
    click(cx, "item-back");

    // A label in the box's colours, teal among them.
    click(cx, "new-label");
    cx.simulate_input("Ops");
    click(cx, "color-teal");
    keys(cx, "enter");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/labels").last(), Some(&json!({ "add": "Ops", "color": "teal" })));
}

#[gpui::test]
fn an_item_in_a_status_the_view_doesnt_list_isnt_hidden(cx: &mut TestAppContext) {
    // One kept as it came from a newer box, in a status this box's view doesn't list: it has a
    // group of its own, after the rest, under the name it came by.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.unlisted_status();
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    click(cx, "group-todo");
    click(cx, "group-backlog");
    assert!(shown(cx, "group-triage") && shown(cx, "item-wqi_226") && shown(cx, "item-wqi_226-status-triage"));
    assert!(top(cx, "group-triage") > top(cx, "group-canceled") && top(cx, "group-triage") < top(cx, "group-deleted"));
    click(cx, "item-wqi_226");
    assert!(shown(cx, "item-head") && shown(cx, "chip-status"));
}

#[gpui::test]
fn every_day_is_counted_from_the_boxs_today_never_this_computers(cx: &mut TestAppContext) {
    // The box says it's the 16th (its own zone, or its own clock); this computer's is the 14th.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.box_today("2026-11-16");
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    // A row's due words follow the box: the 16th is today, and the 13th is no "Yesterday".
    assert!(shown(cx, "item-wqi_231-due:Today") && shown(cx, "item-wqi_235-due:Nov 13") && !shown(cx, "item-wqi_235-due:Yesterday"));
    // So do the chip on its page and the menu's days: Today, Tomorrow and In a week from the 16th.
    click(cx, "item-wqi_234");
    let sent = |stand_in: &StandIn| bodies(stand_in, "PATCH", "kuvo/work-queue/items/wqi_234").last().cloned();
    click(cx, "chip-due");
    keys(cx, "1");
    assert_eq!(sent(&stand_in), Some(json!({ "due": "2026-11-16" })));
    assert!(shown(cx, "chip-due:Today"));
    click(cx, "chip-due");
    assert!(shown(cx, "pick-today-on"));
    keys(cx, "2");
    assert_eq!(sent(&stand_in), Some(json!({ "due": "2026-11-17" })));
    assert!(shown(cx, "chip-due:Tomorrow"));
    click(cx, "chip-due");
    keys(cx, "3");
    assert_eq!(sent(&stand_in), Some(json!({ "due": "2026-11-23" })));
    assert!(shown(cx, "chip-due:Nov 23"));
    // A new item's too.
    click(cx, "item-back");
    click(cx, "queue-new");
    click(cx, "chip-due");
    keys(cx, "2");
    assert!(shown(cx, "chip-due:Tomorrow"));
    cx.simulate_input("Soon");
    keys(cx, "cmd-enter");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items").last().map(|body| body["due"].clone()), Some(json!("2026-11-17")));
}

#[gpui::test]
fn a_narrow_windows_row_shows_less_and_keeps_its_title(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "item-wqi_235-label-lbl_launch") && shown(cx, "item-wqi_242-computer"));
    let wide = bounds(cx, "item-wqi_235-title").expect("its title").size.width;
    cx.simulate_resize(size(px(760.), px(1800.)));
    cx.run_until_parked();
    // One label, then how many more; no computer chip. What says most stays: its status, priority,
    // who it's for and its due day.
    assert!(shown(cx, "item-wqi_235-label-lbl_docs") && !shown(cx, "item-wqi_235-label-lbl_launch") && shown(cx, "item-wqi_235-more-labels") && !shown(cx, "item-wqi_242-computer"));
    for part in ["item-wqi_235-status-in-review", "item-wqi_235-priority-high", "item-wqi_235-assignee", "item-wqi_235-due-late", "item-wqi_235-more"] {
        assert!(shown(cx, part), "{part}");
    }
    // Its title still has room, and nothing runs past the window.
    let title = bounds(cx, "item-wqi_235-title").expect("its title");
    assert!(title.size.width > px(120.) && title.size.width < wide);
    assert!(bounds(cx, "item-wqi_235-more").expect("its menu").right() <= px(760.));
    // The tabs don't all fit: their end fades, to say they scroll.
    assert!(shown(cx, "queue-tabs-more"));
}

#[gpui::test]
fn a_queue_that_cant_be_read_says_so_and_is_read_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.queue_readable(false);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "queue-failed") && !shown(cx, "group-todo") && !shown(cx, "queue-new"));
    // Read again as the area is shown again.
    stand_in.queue_readable(true);
    click(cx, "rail-bots");
    click(cx, "rail-queue");
    assert!(shown(cx, "group-todo") && !shown(cx, "queue-failed") && shown(cx, "queue-new"));
}

#[gpui::test]
fn a_box_with_no_items_says_so(cx: &mut TestAppContext) {
    let stand_in = StandIn::empty(Replies::Never);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    // A status with nothing in it leaves no header; its tab and its row are there, at none.
    assert!(shown(cx, "queue-empty") && shown(cx, "tab-todo-count-0") && shown(cx, "status-todo-count-0") && !shown(cx, "group-todo") && !shown(cx, "group-deleted"));
    // Nothing to filter by: no filter is offered. An item can be added.
    assert!(!shown(cx, "queue-filter") && shown(cx, "queue-new"));
}

#[gpui::test]
fn every_count_drawn_is_the_boxs_never_the_apps(cx: &mut TestAppContext) {
    // A box counts what a view may not list whole: where its count and its list differ, the count
    // drawn is the box's.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.count_status("todo", 41);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    assert!(shown(cx, "group-todo-count-41") && shown(cx, "tab-todo-count-41") && shown(cx, "status-todo-count-41") && !shown(cx, "group-todo-count-6"));
    assert!(shown(cx, "group-backlog-count-7"));
}

#[gpui::test]
fn a_change_whose_reread_fails_keeps_what_the_box_answered(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, _page, cx) = queue(cx, &stand_in);
    // The box takes the change, then can't be read: the item stays where the box put it.
    stand_in.queue_readable(false);
    click(cx, "item-wqi_234-more");
    click(cx, "menu-item-Backlog");
    assert_eq!(item_said(&stand_in, "wqi_234")["status"], "backlog");
    assert!(top(cx, "item-wqi_234") > top(cx, "group-backlog"), "under Backlog, as the box answered");
}

/// The page as the app runs it: what it asks of the box is answered off the window's thread, so an
/// answer can come after you've gone elsewhere.
mod live {
    use super::*;
    use gpui::{AppContext as _, Context, IntoElement, ParentElement, Render, Styled, Window, div};
    use kuvo::areas::Reading;
    use kuvo::queue::Act;
    use kuvo_box::work_queue::ItemChange;

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
            page.set_said(Reading::Ready(said), cx);
        });
        cx.run_until_parked();
        (page, cx)
    }

    fn done() -> Box<ItemChange> {
        Box::new(ItemChange { status: Some("done".into()), ..ItemChange::default() })
    }

    #[gpui::test]
    fn an_older_search_never_lands_on_a_newer_query(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        // Two searches asked before the box answers either: the second is what's typed now.
        let field = page.read_with(cx, |page, _| page.search_field().clone());
        field.update(cx, |field, cx| field.set_text("docs", cx));
        field.update(cx, |field, cx| field.set_text("lighthouse", cx));
        cx.run_until_parked();
        assert_eq!(page.read_with(cx, |page, _| page.query().to_owned()), "lighthouse");
        assert!(shown(cx, "item-wqi_237") && !shown(cx, "item-wqi_239") && !shown(cx, "item-wqi_252"), "only what the newer one found");
        assert!(shown(cx, "tab-backlog-count-1"));
        // Cleared before its answer comes: the whole queue, never what the search found.
        field.update(cx, |field, cx| field.set_text("docs", cx));
        field.update(cx, |field, cx| field.set_text("", cx));
        cx.run_until_parked();
        assert!(shown(cx, "item-wqi_231") && shown(cx, "tab-backlog-count-7"));
    }

    #[gpui::test]
    fn a_new_search_never_shows_the_old_result_under_the_new_words(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        let field = page.read_with(cx, |page, _| page.search_field().clone());
        field.update(cx, |field, cx| field.set_text("docs", cx));
        cx.run_until_parked();
        assert!(shown(cx, "item-wqi_239") && !page.read_with(cx, |page, _| page.searching()));
        // Other words typed, and drawn before the box answers: it says it's searching, with none
        // of what the search before found.
        field.update(cx, |field, cx| field.set_text("lighthouse", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(page.read_with(cx, |page, _| page.searching()));
        assert!(shown(cx, "search-loading") && !shown(cx, "item-wqi_239") && !shown(cx, "group-backlog"));
        cx.run_until_parked();
        assert!(shown(cx, "item-wqi_237") && !shown(cx, "search-loading") && !shown(cx, "item-wqi_239"));
    }

    #[gpui::test]
    fn a_second_pick_while_the_first_is_on_its_way_is_kept_and_sent(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("item:wqi_234"), cx));
        cx.run_until_parked();
        // Two labels picked one after the other, the second before the box answers the first.
        click(cx, "chip-labels");
        cx.simulate_keystrokes("1 2");
        cx.run_until_parked();
        // Both are on it: the second was sent when the first answered, with the first.
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234"), vec![json!({ "labels": ["lbl_bug"] }), json!({ "labels": ["lbl_bug", "lbl_design"] })]);
        assert_eq!(item_said(&stand_in, "wqi_234")["labels"], json!(["lbl_bug", "lbl_design"]));
        assert!(shown(cx, "pick-lbl_bug-on") && shown(cx, "pick-lbl_design-on"));
        // Three at once: what's wanted last is what's sent after the one on its way.
        cx.simulate_keystrokes("1 2 3");
        cx.run_until_parked();
        assert_eq!(item_said(&stand_in, "wqi_234")["labels"], json!(["lbl_docs"]));
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234").len(), 4);
        // Different chips, one after the other: each is kept.
        cx.simulate_keystrokes("escape");
        page.update(cx, |page, cx| {
            page.act(Act::Set { item: "wqi_234".into(), change: Box::new(ItemChange { priority: Some("high".into()), ..ItemChange::default() }) }, cx);
            page.act(Act::Set { item: "wqi_234".into(), change: Box::new(ItemChange { due: Some(Some("2026-12-01".into())), ..ItemChange::default() }) }, cx);
            page.act(Act::Set { item: "wqi_234".into(), change: Box::new(ItemChange { status: Some("backlog".into()), ..ItemChange::default() }) }, cx);
        });
        cx.run_until_parked();
        let item = item_said(&stand_in, "wqi_234");
        assert_eq!((item["priority"].as_str(), item["due"].as_str(), item["status"].as_str()), (Some("high"), Some("2026-12-01"), Some("backlog")));
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234").last(), Some(&json!({ "status": "backlog", "due": "2026-12-01" })));
    }

    #[gpui::test]
    fn escape_while_a_sheet_is_saving_leaves_it_until_the_box_answers(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| {
            page.act(Act::New, cx);
            page.act(Act::CreateMore(true), cx);
        });
        cx.run_until_parked();
        cx.simulate_input("One");
        // ⌘↩, then Escape before the box answers: the sheet isn't closed under its own save, so
        // with Create more it's still there for the next one, saying what it made.
        cx.simulate_keystrokes("cmd-enter escape");
        cx.run_until_parked();
        assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items").len(), 1);
        assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::New));
        assert!(shown(cx, "sheet-created"));
        // Answered, Escape closes it.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), None);
    }

    #[gpui::test]
    fn words_typed_quickly_and_filters_ask_the_box_one_at_a_time(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        let asked = |stand_in: &StandIn| stand_in.seen_at("GET", "kuvo/work-queue").into_iter().map(|seen| seen.path.trim_start_matches("kuvo/work-queue?owner=all&counts=1").to_owned()).filter(|path| !path.is_empty()).collect::<Vec<_>>();
        // Four letters before the box answers the first: a request for the first, and one for what's
        // typed when it answers, never one a letter; what's drawn is the last's.
        let field = page.read_with(cx, |page, _| page.search_field().clone());
        for typed in ["d", "do", "doc", "docs"] {
            field.update(cx, |field, cx| field.set_text(typed, cx));
        }
        cx.run_until_parked();
        assert_eq!(asked(&stand_in), vec!["&q=d".to_owned(), "&q=docs".to_owned()]);
        assert!(shown(cx, "item-wqi_239") && shown(cx, "tab-backlog-count-2") && !page.read_with(cx, |page, _| page.searching()));
        // Two filters put on one after the other, the same way: an older answer never lands on the
        // newer filters.
        field.update(cx, |field, cx| field.set_text("", cx));
        page.update(cx, |page, cx| {
            page.act(Act::AddFilter(kuvo::queue::Filter::Computer { key: "key_devbox".into(), name: "devbox".into() }), cx);
            page.act(Act::AddFilter(kuvo::queue::Filter::Computer { key: "key_studio".into(), name: "studio-mini".into() }), cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(shown(cx, "search-loading") && !shown(cx, "item-wqi_239"));
        cx.run_until_parked();
        assert_eq!(asked(&stand_in).last().map(String::as_str), Some("&computers=key_devbox,key_studio"));
        assert!(shown(cx, "item-wqi_236") && shown(cx, "item-wqi_242") && !shown(cx, "item-wqi_231") && shown(cx, "tab-all-open-2"));
    }

    #[gpui::test]
    fn nothing_more_is_offered_for_an_item_while_a_change_to_it_is_on_its_way(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        assert!(shown(cx, "item-wqi_234-more"));
        // A change sent, and drawn before the box answers: its row's ••• waits, and a delete asked
        // for anyway isn't sent over the change.
        page.update(cx, |page, cx| page.act(Act::Set { item: "wqi_234".into(), change: Box::new(ItemChange { priority: Some("high".into()), ..ItemChange::default() }) }, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(shown(cx, "item-wqi_234-more-off") && !shown(cx, "item-wqi_234-more") && shown(cx, "item-wqi_235-more"));
        page.update(cx, |page, cx| page.act(Act::Delete("wqi_234".into()), cx));
        cx.run_until_parked();
        assert!(stand_in.seen_at("DELETE", "kuvo/work-queue/items/wqi_234").is_empty());
        assert_eq!(item_said(&stand_in, "wqi_234")["priority"], "high");
        assert!(shown(cx, "item-wqi_234-more") && !shown(cx, "item-wqi_234-more-off"));
        // On its page the same: Mark Done and its ••• wait.
        page.update(cx, |page, cx| page.show(Some("item:wqi_234"), cx));
        cx.run_until_parked();
        assert!(shown(cx, "item-more"));
        page.update(cx, |page, cx| page.act(Act::Set { item: "wqi_234".into(), change: Box::new(ItemChange { priority: Some("low".into()), ..ItemChange::default() }) }, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(shown(cx, "item-more-off") && !shown(cx, "item-more"));
        cx.run_until_parked();
        assert!(shown(cx, "item-more"));
        // A deleted one's Restore waits the same way while it's being brought back.
        click(cx, "item-more");
        click(cx, "menu-item-Delete");
        page.update(cx, |page, cx| {
            page.act(Act::Restore("wqi_234".into()), cx);
            page.act(Act::Restore("wqi_234".into()), cx);
        });
        cx.run_until_parked();
        assert_eq!(stand_in.seen_at("POST", "kuvo/work-queue/items/wqi_234/restore").len(), 1);
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
        // A change to one whose computer isn't answering (asked as an older view offered it),
        // twice, then on to another item before the box says so.
        page.update(cx, |page, cx| {
            page.act(Act::Set { item: "wqi_242".into(), change: done() }, cx);
            page.act(Act::Set { item: "wqi_242".into(), change: done() }, cx);
            page.show(Some("item:wqi_234"), cx);
        });
        cx.run_until_parked();
        assert_eq!(stand_in.seen_at("PATCH", "kuvo/work-queue/items/wqi_242").len(), 1, "one at a time for an item");
        assert!(!shown(cx, "queue-problem") && shown(cx, "item-finish"));
        // Nothing was left busy: its own change still goes.
        click(cx, "item-finish");
        assert_eq!(item_said(&stand_in, "wqi_234")["status"], "done");
        // The same refusal on its own page is said, in the box's words.
        page.update(cx, |page, cx| page.show(Some("item:wqi_242"), cx));
        cx.run_until_parked();
        page.update(cx, |page, cx| page.act(Act::Set { item: "wqi_242".into(), change: done() }, cx));
        cx.run_until_parked();
        assert!(shown(cx, "queue-problem"));
    }

    #[gpui::test]
    fn a_sheets_answer_is_its_own_openings(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        // New Item saved, and before the box answers: on to a status's list, and New Label opened.
        page.update(cx, |page, cx| page.act(Act::New, cx));
        cx.run_until_parked();
        cx.simulate_input("A status page");
        page.update(cx, |page, cx| {
            page.save_sheet(cx);
            page.show(Some("status:backlog"), cx);
            page.act(Act::NewLabel, cx);
        });
        cx.run_until_parked();
        // The item was added and is in the list, and the sheet open now is still open, untouched.
        assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/items").len(), 1);
        assert!(page.read_with(cx, |page, _| matches!(page.said(), Reading::Ready(said) if said.items.iter().any(|item| item.title == "A status page"))));
        assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::NewLabel));
        assert!(!shown(cx, "sheet-error"));
        // It saves as itself (nothing was left saving).
        cx.simulate_input("Infra");
        keys(cx, "enter");
        assert_eq!(bodies(&stand_in, "POST", "kuvo/work-queue/labels"), vec![json!({ "add": "Infra" })]);
        assert!(!shown(cx, "queue-sheet"));
    }
}

#[gpui::test]
fn images_paste_into_new_and_edit_item_descriptions(cx: &mut TestAppContext) {
    use gpui::{ClipboardItem, Image, ImageFormat};
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, page, cx) = queue(cx, &stand_in);
    let pasted = tempfile::tempdir().unwrap();
    cx.update(|_, cx| cx.set_global(kuvo::pasted::PastedFolder(pasted.path().to_path_buf())));
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(3, 2).write_to(&mut out, image::ImageFormat::Png).unwrap();
    let bytes = out.into_inner();
    let drop_path = pasted.path().join("dropped.png");
    std::fs::write(&drop_path, &bytes).unwrap();
    let drop_image = |cx: &mut VisualTestContext| {
        use gpui::{ExternalPaths, FileDropEvent, InputEvent as _};
        let position = bounds(cx, "item-composer").unwrap().center();
        cx.update(|window, cx| { window.dispatch_event(FileDropEvent::Entered { position, paths: ExternalPaths([drop_path.clone()].into_iter().collect()) }.to_platform_input(), cx); });
        cx.run_until_parked();
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(cx); });
        cx.simulate_mouse_move(position, Some(MouseButton::Left), Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| { window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx); });
        cx.run_until_parked();
    };
    click(cx, "queue-new");
    cx.simulate_input("With a picture");
    keys(cx, "enter");
    cx.write_to_clipboard(ClipboardItem::from(Image::from_bytes(ImageFormat::Png, bytes.clone())));
    keys(cx, "cmd-v");
    let composer = page.read_with(cx, |page, _| page.item_composer().clone());
    let paths = composer.read_with(cx, |composer, _| composer.attached().to_vec());
    assert_eq!(paths.len(), 1);
    assert_eq!(std::fs::read(&paths[0]).unwrap(), bytes);
    assert!(shown(cx, "attachments"));
    assert_eq!(page.read_with(cx, |page, cx| page.item_fields().1.read(cx).text().to_owned()), "[Image 1] ");
    drop_image(cx);
    assert_eq!(composer.read_with(cx, |composer, _| composer.attached().len()), 2);
    keys(cx, "cmd-enter");
    let sent = bodies(&stand_in, "POST", "kuvo/work-queue/items");
    assert_eq!(sent.last().unwrap()["files"].as_array().unwrap().len(), 2);
    assert!(sent.last().unwrap()["files"][0]["uri"].as_str().unwrap().starts_with("data:image/png;base64,"));
    click(cx, "item-wqi_234");
    click(cx, "item-edit");
    assert!(composer.read_with(cx, |composer, _| composer.attached().is_empty()));
    click(cx, "sheet-body-field");
    cx.write_to_clipboard(ClipboardItem::from(Image::from_bytes(ImageFormat::Png, bytes)));
    keys(cx, "cmd-v");
    assert_eq!(composer.read_with(cx, |composer, _| composer.attached().len()), 1);
    drop_image(cx);
    assert_eq!(composer.read_with(cx, |composer, _| composer.attached().len()), 2);
    keys(cx, "cmd-enter");
    let sent = bodies(&stand_in, "PATCH", "kuvo/work-queue/items/wqi_234");
    assert_eq!(sent.last().unwrap()["files"].as_array().unwrap().len(), 2);
}
