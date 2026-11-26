//! Group chats as someone uses them, against the stand-in box (nothing real is reached): opening
//! one from the sidebar, who wrote each message (a bot of the chat, one that left it, one that's
//! gone), writing to it and naming a bot with "&", Stop and Keep Going, an empty chat's welcome,
//! New Group and a chat's editor with the box's answer drawn after (the chat as it is now, or why
//! not in its words), and Delete asked first. Clicks and keys go through the window as AppKit hands
//! them on.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::groups::chat::{Author, GroupChatView};
use kuvo::groups::{GroupsView, Shown};
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

fn groups(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<GroupsView> {
    workspace.read_with(cx, |workspace, cx| workspace.area_view(Area::Bots).and_then(|view| view.read(cx).groups().cloned()).expect("Bots"))
}

/// Bots, with group chat `id`'s row picked: its conversation.
fn chat(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, id: &str) -> Entity<GroupChatView> {
    if workspace.read_with(cx, |workspace, _| workspace.area()) != Area::Bots {
        click(cx, "rail-bots");
    }
    click(cx, &format!("group-{id}"));
    groups(workspace, cx).read_with(cx, |groups, _| groups.chat(id).cloned()).expect("its conversation")
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.title(cx).0.map(|title| title.to_string()))
}

fn bodies(stand_in: &StandIn, method: &str, route: &str) -> Vec<Value> {
    stand_in.seen_at(method, route).into_iter().map(|seen| seen.body.unwrap_or(Value::Null)).collect()
}

fn typed(chat: &Entity<GroupChatView>, cx: &mut VisualTestContext) -> String {
    let composer = chat.read_with(cx, |chat, _| chat.composer().clone());
    composer.read_with(cx, |composer, cx| composer.field().read(cx).text().to_owned())
}

#[gpui::test]
fn a_chat_shows_who_wrote_each_message_and_whos_talking_now(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    // The sidebar: each chat with when it last spoke (the box's seconds), one with something unread.
    assert!(shown(cx, "group-grp_launch-unread"), "a bot said something since you looked");
    assert!(shown(cx, "group-grp_launch-when-12m"));
    let chat = chat(&workspace, cx, "grp_launch");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Launch crew"));
    assert!(shown(cx, "group-strip"));
    // Your message in your bubble; each bot's under its face and name.
    assert!(shown(cx, "entry-gm_l1"));
    assert!(shown(cx, "author-gm_l2-Release Captain"));
    assert!(shown(cx, "author-gm_l5-Mira"));
    // A bot that's gone since: the chat keeps what it said, and says so.
    assert!(shown(cx, "author-gm_l3-A bot that left"));
    assert_eq!(chat.read_with(cx, |chat, _| chat.author("bot_quill")), Author::Gone);
    // Who's talking now, and what it's doing.
    assert!(shown(cx, "talking"));
    let talking = chat.read_with(cx, |chat, _| chat.chat().speaking.clone());
    assert_eq!(talking.as_deref(), Some("bot_docs"));
    // Opened, it's read: the box is told, and the sidebar's dot goes.
    assert_eq!(stand_in.seen_at("POST", "kuvo/groups/grp_launch/viewed").len(), 1);
    assert!(!shown(cx, "group-grp_launch-unread"));
}

#[gpui::test]
fn a_bot_that_left_the_chat_is_still_named_and_its_wrap_up_shows_the_parts(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let chat = chat(&workspace, cx, "grp_design");
    // Docs Keeper is still a bot here, but not in the chat any more.
    assert!(matches!(chat.read_with(cx, |chat, _| chat.author("bot_docs")), Author::Left { name, .. } if name == "Docs Keeper"));
    assert!(shown(cx, "author-gm_d3-Docs Keeper"));
    // The wrap-up: who does what, and how it's going.
    assert!(shown(cx, "task-tk_1") && shown(cx, "task-tk_2"));
    assert!(!shown(cx, "start-work"), "nothing waits to be handed out");
    // Paused after its turns: Keep Going carries on, through the box.
    assert!(shown(cx, "paused-bar"));
    click(cx, "keep-going");
    assert!(!shown(cx, "paused-bar"));
    assert!(shown(cx, "talking"), "the box says who talks now");
}

#[gpui::test]
fn writing_names_a_bot_with_and_its_list_offers_the_chats_bots(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let chat = chat(&workspace, cx, "grp_planning");
    // An empty chat: who's here, and how it goes.
    assert!(shown(cx, "group-intro"));
    // The keyboard is in its composer. "&" offers what the chat's box offers there: its bots first,
    // then the guests a message can name.
    let offers = |cx: &mut VisualTestContext| chat.read_with(cx, |chat, cx| chat.composer().read(cx).shown_offers(cx).into_iter().map(|offer| (offer.name, offer.summary)).collect::<Vec<_>>());
    cx.simulate_input("&Ke");
    let listed = offers(cx);
    assert_eq!(listed.first(), Some(&("Docs Keeper".to_owned(), "In this chat".to_owned())), "the chat's own bot first");
    assert!(listed.iter().any(|(name, _)| name == "Rate limits per API key"), "a guest after it: {listed:?}");
    assert!(stand_in.seen_at("GET", "kuvo/groups/grp_planning/mentions").iter().any(|seen| seen.path.ends_with("?q=Ke")), "asked of the box");
    cx.simulate_keystrokes("cmd-a backspace");
    // Return puts one in.
    cx.simulate_input("What's on this week? &Do");
    assert_eq!(offers(cx).into_iter().map(|(name, _)| name).collect::<Vec<_>>(), vec!["Docs Keeper".to_owned()]);
    cx.simulate_keystrokes("enter");
    assert_eq!(typed(&chat, cx), "What's on this week? &Docs Keeper ");
    // A face under the field asks that bot by name.
    click(cx, "ask-bot_release");
    assert_eq!(typed(&chat, cx), "What's on this week? &Docs Keeper &Release Captain ");
    cx.simulate_input("go first.");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // Only the words go: the box names who they name.
    assert_eq!(bodies(&stand_in, "POST", "kuvo/groups/grp_planning"), vec![json!({ "text": "What's on this week? &Docs Keeper &Release Captain go first." })]);
    assert_eq!(typed(&chat, cx), "");
    assert!(!shown(cx, "group-intro"));
    assert!(shown(cx, "talking"));
    // While they talk, Escape in the empty field stops the discussion.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/groups/grp_planning/stop").len(), 1);
    assert!(!shown(cx, "talking"));
}

#[gpui::test]
fn a_reply_that_comes_in_shows_under_its_bots_name(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let chat = chat(&workspace, cx, "grp_launch");
    stand_in.group_says("grp_launch", "bot_docs", "The notes are drafted.");
    // (The app follows the chat; here it's read as it would be told.)
    chat.update(cx, |chat, cx| chat.take(kuvo_box::follow::Followed::Now(BoxClient::new(stand_in.clone()).group_chat("grp_launch").unwrap()), cx));
    cx.run_until_parked();
    let last = chat.read_with(cx, |chat, _| chat.rows().last().map(|row| row.key().to_owned())).unwrap();
    assert!(shown(cx, &format!("author-{last}-Docs Keeper")));
    assert!(!shown(cx, "talking"), "its turn ended");
}

#[gpui::test]
fn new_group_is_made_by_the_box_and_opens(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    click(cx, "new-group");
    assert_eq!(title(&workspace, cx).as_deref(), Some("New Group"));
    let view = groups(&workspace, cx);
    // Who can be in it is the box's: its bots, then another computer's.
    assert!(shown(cx, "pick-bot_mira") && shown(cx, "pick-bot_scout"));
    // The keyboard is in its name. One bot isn't enough: the box says so, in its words.
    cx.simulate_input("Support desk");
    click(cx, "pick-bot_inbox");
    click(cx, "group-save");
    assert_eq!(view.read_with(cx, |view, _| view.error().map(str::to_owned)).as_deref(), Some("A group chat needs at least two bots."));
    assert!(shown(cx, "group-error"));
    click(cx, "pick-bot_mira");
    click(cx, "group-save");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/groups").last(), Some(&json!({ "name": "Support desk", "members": ["bot_inbox", "bot_mira"] })));
    // Made: its conversation opens, and the sidebar lists it.
    assert_eq!(title(&workspace, cx).as_deref(), Some("Support desk"));
    assert!(matches!(view.read_with(cx, |view, _| view.shown().clone()), Shown::Chat(id) if id == "grp_new1"));
    assert!(shown(cx, "group-grp_new1") && shown(cx, "group-intro"));
}

#[gpui::test]
fn editing_a_chat_sends_only_what_changed_and_delete_is_asked_first(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    chat(&workspace, cx, "grp_launch");
    click(cx, "group-edit");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Edit Launch crew"));
    let view = groups(&workspace, cx);
    assert_eq!(view.read_with(cx, |view, _| view.picked().to_vec()), vec!["bot_mira", "bot_release", "bot_docs"]);
    // Docs Keeper out: only the bots go.
    click(cx, "pick-bot_docs");
    click(cx, "group-save");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/groups/grp_launch"), vec![json!({ "members": ["bot_mira", "bot_release"] })]);
    // Back in the chat, as the box has it now.
    assert!(shown(cx, "group-chat"));
    let members = workspace.read_with(cx, |workspace, cx| match workspace.area_view(Area::Bots).unwrap().read(cx).bots() {
        kuvo::areas::Reading::Ready(bots) => bots.groups.iter().find(|group| group.id == "grp_launch").map(|group| group.members.len()),
        _ => None,
    });
    assert_eq!(members, Some(2));

    // Delete: asked first, then gone.
    click(cx, "group-edit");
    click(cx, "group-delete");
    assert!(shown(cx, "group-delete-ask"));
    click(cx, "group-delete-confirm");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/groups/grp_launch").len(), 1);
    assert!(!shown(cx, "group-grp_launch"), "gone from the sidebar");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Bots"));
}

#[gpui::test]
fn a_change_the_box_cant_take_says_why_and_nothing_changed_asks_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    chat(&workspace, cx, "grp_launch");
    stand_in.groups_readable(false);
    click(cx, "group-edit");
    click(cx, "group-save");
    // Nothing changed: back to the chat without asking the box.
    assert!(stand_in.seen_at("PATCH", "kuvo/groups/grp_launch").is_empty());
    click(cx, "group-edit");
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("Crew");
    click(cx, "group-save");
    assert_eq!(groups(&workspace, cx).read_with(cx, |view, _| view.error().map(str::to_owned)).as_deref(), Some("This Mac’s coding agents aren’t running."));
}

#[gpui::test]
fn a_chat_off_screen_isnt_marked_read(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let chat = chat(&workspace, cx, "grp_launch");
    assert_eq!(stand_in.seen_at("POST", "kuvo/groups/grp_launch/viewed").len(), 1);
    // Away in Sessions, a reply comes: it waits unread for you.
    click(cx, "rail-sessions");
    stand_in.group_says("grp_launch", "bot_docs", "The notes are drafted.");
    chat.update(cx, |chat, cx| chat.take(kuvo_box::follow::Followed::Now(BoxClient::new(stand_in.clone()).group_chat("grp_launch").unwrap()), cx));
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/groups/grp_launch/viewed").len(), 1, "not read while you're elsewhere");
    // Back in Bots, it's read.
    click(cx, "rail-bots");
    assert_eq!(stand_in.seen_at("POST", "kuvo/groups/grp_launch/viewed").len(), 2);
}

#[gpui::test]
fn chats_that_cant_be_read_just_now_stay_as_they_were(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let chat = chat(&workspace, cx, "grp_launch");
    stand_in.groups_readable(false);
    click(cx, "rail-sessions");
    click(cx, "rail-bots");
    // The list as it was, and the chat's bots still its own (not "Left the chat").
    assert!(shown(cx, "group-grp_launch") && !shown(cx, "groups-none"));
    assert!(matches!(chat.read_with(cx, |chat, _| chat.author("bot_mira")), Author::Member { .. }));
    assert!(shown(cx, "group-strip") && !shown(cx, "group-gone"));
}

#[gpui::test]
fn a_part_waiting_on_a_stopped_conversation_resumes_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    chat(&workspace, cx, "grp_design");
    click(cx, "resume-tk_2");
    // The bot's own conversation, resumed where it lives; the part goes on.
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_inbox/queue/resume").len(), 1);
    assert!(!shown(cx, "resume-tk_2"));
}

#[gpui::test]
fn a_chat_deleted_elsewhere_says_its_gone(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    chat(&workspace, cx, "grp_design");
    // The phone deletes it; the list is read again.
    BoxClient::new(stand_in.clone()).delete_group("grp_design").unwrap();
    click(cx, "rail-sessions");
    click(cx, "rail-bots");
    assert!(shown(cx, "group-gone"));
    assert!(!shown(cx, "group-grp_design"));
}

#[gpui::test]
fn a_bot_taken_out_and_put_back_is_no_change(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    chat(&workspace, cx, "grp_launch");
    click(cx, "group-edit");
    click(cx, "pick-bot_mira");
    click(cx, "pick-bot_mira");
    click(cx, "group-save");
    assert!(stand_in.seen_at("PATCH", "kuvo/groups/grp_launch").is_empty());
    assert!(shown(cx, "group-chat"));
}

#[gpui::test]
fn a_chat_listed_again_opens_its_conversation_and_a_failed_first_read_isnt_gone(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    // The chats can't be read as Bots first opens: a chat picked isn't said to be gone.
    stand_in.groups_readable(false);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    assert!(shown(cx, "groups-failed"));
    let view = groups(&workspace, cx);
    let area = workspace.read_with(cx, |workspace, _| workspace.area_view(Area::Bots).cloned().unwrap());
    area.update(cx, |area, cx| area.select(Some("group:grp_launch".into()), cx));
    cx.run_until_parked();
    assert!(!shown(cx, "group-gone"));
    // Readable again: the list comes, and the chat shown opens its conversation.
    stand_in.groups_readable(true);
    click(cx, "rail-sessions");
    click(cx, "rail-bots");
    assert!(view.read_with(cx, |view, _| view.chat("grp_launch").is_some()));
    assert!(shown(cx, "group-chat") && shown(cx, "author-gm_l5-Mira"));
}
