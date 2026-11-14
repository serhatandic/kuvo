//! The Bots area as someone uses it, against the stand-in box (nothing real is reached): a bot's
//! page (who it is, what it's doing, its sessions), its conversation, changing it through the
//! box's routes with the box's answer drawn after (the bot as it is now, an edit held for another
//! computer, or why not in the box's words), New Bot as the box offers it, and Delete asked first.
//! Clicks and keys go through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::bots::{BotsView, Sheet, Shown};
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
    cx.simulate_resize(size(px(1280.), px(1600.)));
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

fn page(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<BotsView> {
    workspace.read_with(cx, |workspace, cx| workspace.area_view(Area::Bots).and_then(|view| view.read(cx).page().cloned()).expect("Bots"))
}

/// Bots, with `bot`'s row picked.
fn bot(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, bot: &str) -> Entity<BotsView> {
    // (Its button again would fold the sidebar away.)
    if workspace.read_with(cx, |workspace, _| workspace.area()) != Area::Bots {
        click(cx, "rail-bots");
    }
    click(cx, &format!("bot-{bot}"));
    page(workspace, cx)
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.title(cx).0.map(|title| title.to_string()))
}

/// The bodies sent to `route`, in order.
fn bodies(stand_in: &StandIn, method: &str, route: &str) -> Vec<Value> {
    stand_in.seen_at(method, route).into_iter().map(|seen| seen.body.unwrap_or(Value::Null)).collect()
}

/// Whether the sidebar has `bot`, on another computer, as waiting for that computer.
fn waiting(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, bot: &str) -> bool {
    workspace.read_with(cx, |workspace, cx| {
        let view = workspace.area_view(Area::Bots).expect("Bots").read(cx);
        match view.bots() {
            kuvo::areas::Reading::Ready(bots) => bots.there().iter().any(|there| there.id == bot && there.waiting),
            _ => false,
        }
    })
}

/// What `kuvo/bots` says now, as the stand-in box answers it.
fn said(stand_in: &StandIn) -> Value {
    serde_json::from_slice(&kuvo_box::transport::Transport::send(stand_in, &kuvo_box::transport::Request::get("kuvo/bots")).expect("answered").body).expect("JSON")
}

/// The bot shown, as the page has it from the box.
fn name_and_job(page: &Entity<BotsView>, cx: &mut VisualTestContext) -> (String, String, String) {
    page.read_with(cx, |page, _| page.bot().map(|bot| (bot.name.clone(), bot.job.clone(), bot.job_details.clone())).expect("a bot shown"))
}

#[gpui::test]
fn a_bots_page_shows_who_it_is_what_its_doing_and_its_sessions(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    // Nothing picked yet: the area says what it's for; nothing is asked about any bot.
    assert!(shown(cx, "bots-empty"));
    assert!(stand_in.seen_at("GET", "kuvo/work").is_empty());

    let page = bot(&workspace, cx, "bot_mira");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Mira"));
    assert!(shown(cx, "bot-head") && shown(cx, "bot-now"), "its face, name and job, and what it's doing now");
    assert_eq!(name_and_job(&page, cx).1, "Looks after the website");
    // Its sessions are the box's (`kuvo/work?bot=`), the latest first, each as the box says it stands.
    assert_eq!(stand_in.seen_at("GET", "kuvo/work").last().map(|seen| seen.path.clone()).as_deref(), Some("kuvo/work?bot=bot_mira"));
    let order: Vec<Pixels> = ["work-ses_signup", "work-ses_cms", "work-ses_dark", "work-ses_lighthouse"].iter().map(|row| bounds(cx, row).unwrap_or_else(|| panic!("{row}")).top()).collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "in the box's order");
    assert!(shown(cx, "work-working-ses_signup") && shown(cx, "work-asking-ses_cms"), "one at work spins, one waiting on you is marked");

    // A bot with nothing under way and no sessions says so, and claims nothing of now.
    click(cx, "bot-bot_release");
    assert!(shown(cx, "work-none") && !shown(cx, "bot-now"));

    // A session opens where sessions are, and Back comes back to the bot's page.
    click(cx, "bot-bot_mira");
    click(cx, "work-ses_cms");
    assert_eq!(workspace.read_with(cx, |workspace, _| (workspace.area(), workspace.opened().map(str::to_owned))), (Area::Sessions, Some("ses_cms".into())));
    cx.simulate_keystrokes("cmd-[");
    cx.run_until_parked();
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Bots);
    assert!(shown(cx, "bot-head"));
}

#[gpui::test]
fn chat_opens_the_bots_conversation_in_bots_and_back_returns_to_its_page(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    bot(&workspace, cx, "bot_mira");
    click(cx, "bot-chat");
    // Its own conversation, in its page's place, its row still picked; the keyboard is in its composer.
    assert!(shown(cx, "bot-chat-strip") && !shown(cx, "bot-head"));
    assert_eq!(workspace.read_with(cx, |workspace, _| (workspace.area(), workspace.bot_chat().map(|(bot, _)| bot.to_owned()))), (Area::Bots, Some("bot_mira".into())));
    cx.simulate_input("How did it go?");
    let composer = workspace.read_with(cx, |workspace, cx| workspace.bot_chat().unwrap().1.read(cx).composer().clone());
    assert_eq!(composer.read_with(cx, |composer, cx| composer.field().read(cx).text().to_owned()), "How did it go?");
    // Sessions keeps the conversation it had (none): the bot's isn't opened there.
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), None);

    // The strip's arrow goes to its page; Back, to the conversation again, what was written still there.
    click(cx, "bot-chat-back");
    assert!(shown(cx, "bot-head") && !shown(cx, "bot-chat-strip"));
    cx.simulate_keystrokes("cmd-[");
    cx.run_until_parked();
    assert!(shown(cx, "bot-chat-strip"));
    assert_eq!(composer.read_with(cx, |composer, cx| composer.field().read(cx).text().to_owned()), "How did it go?");
    // Another area and back: Bots shows the conversation it showed; its own row, its page.
    cx.simulate_keystrokes("ctrl-1");
    cx.simulate_keystrokes("ctrl-2");
    cx.run_until_parked();
    assert!(shown(cx, "bot-chat-strip"));
    click(cx, "bot-bot_mira");
    assert!(shown(cx, "bot-head"));
}

#[gpui::test]
fn renaming_goes_to_the_box_and_its_answer_is_what_shows(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    click(cx, "bot-name-button");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::Name));
    assert!(shown(cx, "area-sidebar-dim"), "the sidebar waits under the sheet");
    // Its name is there, selected: typing replaces it. An empty one is the box's to turn down,
    // in its words, and the sheet stays.
    cx.simulate_keystrokes("backspace enter");
    cx.run_until_parked();
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "name": "" })]);
    assert!(shown(cx, "sheet-error") && shown(cx, "bot-sheet"));
    assert_eq!(name_and_job(&page, cx).0, "Mira");

    cx.simulate_input("Mira Two");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "name": "Mira Two" })));
    assert!(!shown(cx, "bot-sheet") && !shown(cx, "area-sidebar-dim"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Mira Two"), "the sidebar's list is read again");
    // What it's doing stays as the box last said it (a change answers without it).
    assert!(shown(cx, "bot-now"));
}

#[gpui::test]
fn escape_and_cancel_close_a_sheet_and_send_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    bot(&workspace, cx, "bot_mira");
    for opener in ["bot-name-button", "bot-job-button", "bot-look-button", "bot-delete-button"] {
        click(cx, opener);
        assert!(shown(cx, "bot-sheet"), "{opener}");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!shown(cx, "bot-sheet"), "Escape closes {opener}'s sheet");
        click(cx, opener);
        click(cx, "sheet-cancel");
        assert!(!shown(cx, "bot-sheet"));
    }
    assert!(stand_in.seen_at("PATCH", "kuvo/bots/bot_mira").is_empty() && stand_in.seen_at("DELETE", "kuvo/bots/bot_mira").is_empty());
}

#[gpui::test]
fn its_job_and_look_change_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_release");
    // The job: its line and all of it, sent whole.
    click(cx, "bot-job-button");
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("Ships a beta every Thursday");
    click(cx, "sheet-job-details");
    cx.simulate_input("Tag it, wait for the build, post the notes.");
    click(cx, "sheet-save");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_release"), vec![json!({ "job": { "line": "Ships a beta every Thursday", "details": "Tag it, wait for the build, post the notes." } })]);
    assert_eq!(name_and_job(&page, cx), ("Release Captain".into(), "Ships a beta every Thursday".into(), "Tag it, wait for the build, post the notes.".into()));

    // The look: a colour and a shape among those the box offers; its face is kept.
    click(cx, "bot-look-button");
    assert!(shown(cx, "look-color-graphite") && shown(cx, "look-shape-bloom"), "the box's colours and shapes");
    click(cx, "look-color-amber");
    click(cx, "look-shape-gem");
    assert!(stand_in.seen_at("PATCH", "kuvo/bots/bot_release").len() == 1, "nothing is sent until Save");
    click(cx, "sheet-save");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_release").last(), Some(&json!({ "look": { "color": "amber", "shape": "gem", "face": "calm" } })));
    assert_eq!(page.read_with(cx, |page, _| page.bot().map(|bot| bot.look.color.clone())), Some("amber".into()));
}

#[gpui::test]
fn personality_and_model_are_picked_from_what_the_box_offers(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    click(cx, "bot-personality-menu");
    assert!(shown(cx, "menu-item-Ship’s Cook"), "one of your own, as the box lists it");
    click(cx, "menu-item-Straight Shooter");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "personality": "preset.straight" })]);
    assert_eq!(page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.personality.clone())), Some("preset.straight".into()));
    // None: the model's own way, sent as null.
    click(cx, "bot-personality-menu");
    click(cx, "menu-item-The model’s own way");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "personality": null })));
    assert_eq!(page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.personality.clone())), None);

    click(cx, "bot-model-menu");
    click(cx, "menu-item-GPT-6");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "model": { "providerID": "openai", "id": "gpt-6" } })));
    assert_eq!(page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.model.as_ref().map(|model| model.id.clone()))), Some("gpt-6".into()));
}

#[gpui::test]
fn a_change_the_box_turns_down_shows_its_words_and_changes_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    stand_in.bots_changeable(false);
    click(cx, "bot-personality-menu");
    click(cx, "menu-item-Cheerleader");
    assert!(shown(cx, "bots-problem"), "the box's refusal, over the page");
    assert_eq!(page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.personality.clone())), Some("preset.mentor".into()), "as it was");
    click(cx, "bots-problem-close");
    assert!(!shown(cx, "bots-problem"));
}

#[gpui::test]
fn an_edit_for_a_bot_on_another_computer_is_held_and_says_waiting(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    // What the box already holds for a bot that lives elsewhere lists under its own header.
    click(cx, "rail-bots");
    click(cx, "elsewhere-bot_scout");
    assert_eq!(page(&workspace, cx).read_with(cx, |page, _| page.shown().clone()), Shown::Elsewhere("bot_scout".into()));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Scout"), "the name it's being given");
    assert!(shown(cx, "bot-held") && shown(cx, "elsewhere-note"));

    // A bot that moves to another computer while its page is open: the box takes the change
    // (202) and holds it for that computer, and the app keeps nothing itself.
    click(cx, "bot-bot_docs");
    stand_in.move_bot("bot_docs", "studio-mini");
    click(cx, "bot-name-button");
    cx.simulate_input("Docs Warden");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_docs"), vec![json!({ "name": "Docs Warden" })]);
    // The list read again no longer has it here: it's on the other computer, waiting.
    assert!(!shown(cx, "bot-bot_docs") && shown(cx, "elsewhere-bot_docs"));
    assert!(waiting(&workspace, cx, "bot_docs"));

    // While the box can't say what it holds (it leaves the lists out), what it last said stays.
    stand_in.bots_held_sayable(false);
    click(cx, "rail-sessions");
    click(cx, "rail-bots");
    assert!(waiting(&workspace, cx, "bot_docs") && waiting(&workspace, cx, "bot_scout"), "kept, not taken for gone");
    stand_in.bots_held_sayable(true);

    // What that computer refuses comes back in the box's words.
    stand_in.refuse_bot_edit("bot_docs", "studio-mini", "studio-mini is full.");
    click(cx, "rail-sessions");
    click(cx, "rail-bots");
    click(cx, "elsewhere-bot_docs");
    assert!(shown(cx, "bot-refused-0") && !shown(cx, "bot-held"));
}

#[gpui::test]
fn new_bot_is_made_as_the_box_offers_it_and_opens_its_conversation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    assert!(stand_in.seen_at("GET", "kuvo/bots/templates").is_empty(), "asked only as New Bot opens");
    click(cx, "new-bot");
    assert_eq!(title(&workspace, cx).as_deref(), Some("New Bot"));
    assert!(shown(cx, "new-bot-page") && shown(cx, "template-blank") && shown(cx, "template-ci"), "the box's templates");

    // Without a name the box says so; nothing is made, and what was picked stays.
    click(cx, "template-inbox");
    let view = page(&workspace, cx);
    assert_eq!(view.read_with(cx, |page, cx| page.new_name_field().read(cx).text().to_owned()), "Courier", "the template's name, to change");
    // (The keyboard is back in its name.)
    cx.simulate_keystrokes("cmd-a backspace");
    click(cx, "new-create");
    assert!(shown(cx, "new-error"));
    assert_eq!(stand_in.seen_at("GET", "kuvo/bots").len(), 1, "no bot was made: the list isn't read again");

    click(cx, "new-name-field");
    cx.simulate_input("Postie");
    click(cx, "new-project-menu");
    click(cx, "menu-item-API");
    click(cx, "new-create");
    let sent = bodies(&stand_in, "POST", "kuvo/bots");
    assert_eq!(sent.len(), 2);
    // Only what was picked is sent: the template's look and personality are the box's to give.
    let mut made = sent[1].clone();
    let request = made.as_object_mut().and_then(|made| made.remove("request")).and_then(|request| request.as_str().map(str::to_owned)).expect("a request id");
    assert_eq!(made, json!({ "name": "Postie", "template": "inbox", "project": "/Users/sam/Projects/api" }));
    assert_eq!(sent[0]["request"], json!(request), "a retry carries the same request, so the bot is made once");

    // On to its conversation, in Bots, its job in the composer to edit and send.
    let (bot, composer) = workspace.read_with(cx, |workspace, cx| workspace.bot_chat().map(|(bot, view)| (bot.to_owned(), view.read(cx).composer().clone())).expect("its conversation"));
    assert!(shown(cx, &format!("bot-{bot}")), "listed in the sidebar");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Postie"));
    assert_eq!(composer.read_with(cx, |composer, cx| composer.field().read(cx).text().to_owned()), "Read my mail twice a day and draft replies to what needs one.");
}

#[gpui::test]
fn a_blank_bot_sends_what_was_chosen_and_cancel_makes_none(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    click(cx, "new-bot");
    click(cx, "new-cancel");
    assert!(shown(cx, "bots-empty") && stand_in.seen_at("POST", "kuvo/bots").is_empty());

    click(cx, "new-bot");
    cx.simulate_input("Scribe");
    click(cx, "new-look-button");
    click(cx, "look-color-pink");
    click(cx, "sheet-save");
    click(cx, "new-personality-menu");
    click(cx, "menu-item-Mentor");
    click(cx, "new-model-menu");
    click(cx, "menu-item-GPT-6");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    click(cx, "new-create");
    let mut made = bodies(&stand_in, "POST", "kuvo/bots").pop().expect("made");
    made.as_object_mut().map(|made| made.remove("request"));
    assert_eq!(made, json!({ "name": "Scribe", "look": { "color": "pink", "shape": "round", "face": "calm" }, "personality": "preset.mentor", "model": { "providerID": "openai", "id": "gpt-6" } }));
    // No template: nothing waits in its composer (the bot writes first).
    let composer = workspace.read_with(cx, |workspace, cx| workspace.bot_chat().expect("its conversation").1.read(cx).composer().clone());
    assert_eq!(composer.read_with(cx, |composer, cx| composer.field().read(cx).text().to_owned()), "");
}

#[gpui::test]
fn the_bots_of_your_other_computers_list_as_the_box_says_and_change_through_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    // One nothing waits for: listed all the same, as its computer last told the box.
    assert!(shown(cx, "elsewhere-bot_wren") && !waiting(&workspace, cx, "bot_wren"));
    click(cx, "elsewhere-bot_wren");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Wren"));
    assert!(shown(cx, "elsewhere-head-bot_wren") && !shown(cx, "bot-held"));
    // What this box can't say of it is said, and isn't offered: its job, personality and model.
    assert!(shown(cx, "elsewhere-note") && !shown(cx, "bot-personality-menu") && !shown(cx, "bot-model-menu") && !shown(cx, "bot-job-button"));

    // Renamed from here: asked of this box, which holds it for the bot's computer (202).
    click(cx, "bot-name-button");
    cx.simulate_input("Wren II");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_wren"), vec![json!({ "name": "Wren II" })]);
    assert!(!shown(cx, "bot-sheet") && shown(cx, "bot-held") && waiting(&workspace, cx, "bot_wren"));
    assert_eq!(title(&workspace, cx).as_deref(), Some("Wren II"), "as you left it");

    // Deleting it shows what that computer's delete takes, in the box's words for it: a machine's.
    let page = page(&workspace, cx);
    click(cx, "bot-delete-button");
    let lines = page.read_with(cx, |page, _| page.deletes());
    let there = said(&stand_in)["elsewhere"].as_array().and_then(|all| all.iter().find(|bot| bot["id"] == json!("bot_wren")).cloned()).expect("listed");
    assert_eq!(lines.iter().map(|line| json!(line.words)).collect::<Vec<_>>(), there["deletes"].as_array().unwrap().iter().map(|line| line["words"].clone()).collect::<Vec<_>>());
    assert!(lines.iter().any(|line| line.words.contains("Kuvo Code’s trash folder on lab")));
    click(cx, "sheet-save");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/bots/bot_wren").len(), 1);
    assert!(shown(cx, "bot-held") && shown(cx, "elsewhere-bot_wren"), "held for lab: still listed until lab says it's gone");
}

#[gpui::test]
fn a_bot_held_for_a_computer_shows_as_itself_by_the_name_and_look_the_box_gives(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    // A delete held for a bot (no name in what's held), which its computer then refuses: both name
    // it as the box does.
    click(cx, "elsewhere-bot_wren");
    click(cx, "bot-delete-button");
    click(cx, "sheet-save");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Wren"));
    stand_in.refuse_bot_edit("bot_wren", "lab", "lab didn’t delete the bot.");
    stand_in.forget_bot_elsewhere("bot_wren");
    click(cx, "rail-sessions");
    click(cx, "rail-bots");
    // No computer lists it now: known by the refusal alone, still as itself, and not changed from here.
    assert!(shown(cx, "elsewhere-bot_wren"));
    click(cx, "elsewhere-bot_wren");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Wren"));
    assert!(shown(cx, "bot-refused-0") && !shown(cx, "bot-name-button") && !shown(cx, "bot-delete-button"));
}

#[gpui::test]
fn its_effort_is_picked_from_its_models_own_and_sent_alone(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    let variant = |cx: &mut VisualTestContext| page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.model.as_ref().and_then(|model| model.variant.clone())));
    assert_eq!(variant(cx), Some("high".into()));
    click(cx, "bot-effort-menu");
    assert!(shown(cx, "menu-item-Max") && !shown(cx, "menu-item-Extra"), "its model's own efforts, as the box lists them");
    click(cx, "menu-item-Low");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "effort": "low" })]);
    assert_eq!(variant(cx), Some("low".into()), "the box's answer");
    // None: the model's own, sent as null.
    click(cx, "bot-effort-menu");
    click(cx, "menu-item-The model’s own");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "effort": null })));
    assert_eq!(variant(cx), None);
    // A model with no efforts of its own has no Effort row.
    click(cx, "bot-model-menu");
    click(cx, "menu-item-Gemini 3 Pro");
    assert!(!shown(cx, "bot-effort-menu") && shown(cx, "bot-model-menu"));
}

#[gpui::test]
fn deleting_a_bot_is_asked_first(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_docs");
    click(cx, "bot-delete-button");
    assert_eq!(page.read_with(cx, |page, _| page.sheet().cloned()), Some(Sheet::Delete));
    // The confirm shows what the box says deleting it takes with it, each line as it is.
    let lines = page.read_with(cx, |page, _| page.deletes());
    let listed = said(&stand_in)["bots"].as_array().and_then(|all| all.iter().find(|bot| bot["id"] == json!("bot_docs")).cloned()).expect("listed");
    assert_eq!(lines.iter().map(|line| json!(line.words)).collect::<Vec<_>>(), listed["deletes"].as_array().unwrap().iter().map(|line| line["words"].clone()).collect::<Vec<_>>());
    assert!(lines.len() >= 5 && lines.iter().any(|line| line.words.contains("goes to the Trash")) && lines.last().is_some_and(|line| line.kept));
    assert!(shown(cx, "sheet-delete-0") && shown(cx, &format!("sheet-delete-{}", lines.len() - 1)) && !shown(cx, &format!("sheet-delete-{}", lines.len())));
    // Return doesn't delete: only its button does.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(stand_in.seen_at("DELETE", "kuvo/bots/bot_docs").is_empty() && shown(cx, "bot-sheet"));
    click(cx, "sheet-save");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/bots/bot_docs").len(), 1);
    assert!(!shown(cx, "bot-bot_docs") && shown(cx, "bots-empty"), "gone from the list, nothing picked");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Bots"));
    // Back doesn't come back to a page for a bot that's gone: it passes over it.
    cx.simulate_keystrokes("cmd-[");
    cx.run_until_parked();
    assert!(!shown(cx, "bot-head") && !shown(cx, "bot-gone"));
    // (Before its page the window was where it is now, Bots with nothing picked: so on back, to
    // where it was before that.)
    assert_eq!(workspace.read_with(cx, |workspace, _| (workspace.area(), workspace.is_landing())), (Area::Sessions, true));
}

#[gpui::test]
fn deleting_a_bot_that_moved_away_is_held_for_its_computer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    bot(&workspace, cx, "bot_docs");
    stand_in.move_bot("bot_docs", "studio-mini");
    click(cx, "bot-delete-button");
    click(cx, "sheet-save");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/bots/bot_docs").len(), 1);
    // Taken and held (202): it waits under the other computers, and its page says what waits.
    assert!(!shown(cx, "bot-sheet") && shown(cx, "elsewhere-bot_docs"));
    click(cx, "elsewhere-bot_docs");
    assert!(shown(cx, "bot-held"));
}

#[gpui::test]
fn bots_that_cant_be_read_and_a_box_with_none_say_so(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.bots_readable(false);
    let (_workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    assert!(shown(cx, "bots-failed") && !shown(cx, "bots-empty"));
    // Read again as the area opens: what it says now.
    stand_in.bots_readable(true);
    click(cx, "rail-sessions");
    click(cx, "rail-bots");
    assert!(shown(cx, "bot-bot_mira") && shown(cx, "bots-empty"));
}

#[gpui::test]
fn a_first_run_offers_a_new_bot_and_a_narrow_window_keeps_the_page(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::empty(Replies::Never));
    click(cx, "rail-bots");
    assert!(shown(cx, "bots-empty"));
    click(cx, "bots-empty-new");
    assert_eq!(title(&workspace, cx).as_deref(), Some("New Bot"));

    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    bot(&workspace, cx, "bot_mira");
    cx.simulate_resize(size(px(760.), px(900.)));
    cx.run_until_parked();
    assert!(!shown(cx, "area-sidebar") && shown(cx, "bot-head") && shown(cx, "bot-chat"), "the sidebar folds away; the page and its Chat stay");
    let (head, chat) = (bounds(cx, "bot-head").unwrap(), bounds(cx, "bot-chat").unwrap());
    assert!(chat.right() <= head.right() + px(1.) && head.right() <= px(760.), "nothing is cut off");
}

/// The app's way: the box asked off the window's thread, its answers coming in later, perhaps
/// after you've gone on to something else.
mod live {
    use super::*;
    use gpui::{AppContext as _, Context, IntoElement, ParentElement, Render, Styled, Window, div};
    use kuvo::areas::Reading;
    use kuvo::bots::Act;

    struct Host {
        page: Entity<BotsView>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.page.clone())
        }
    }

    fn host<'a>(cx: &'a mut TestAppContext, stand_in: &StandIn) -> (Entity<BotsView>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            Theme::init(Appearance::Light, cx);
            kuvo::bind_keys(cx);
        });
        let client = BoxClient::new(stand_in.clone());
        let said = client.bots_said().expect("the stand-in's bots");
        let (host, cx) = cx.add_window_view(|_, cx| Host { page: cx.new(|cx| BotsView::new(client, true, cx)) });
        let page = host.read_with(cx, |host, _| host.page.clone());
        cx.simulate_resize(size(px(1280.), px(1600.)));
        page.update(cx, |page, cx| page.set_said(Reading::Ready(said), cx));
        cx.run_until_parked();
        (page, cx)
    }

    #[gpui::test]
    fn an_answer_that_comes_after_youve_gone_to_another_bot_is_its_own_bots(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        click(cx, "bot-name-button");
        cx.simulate_input("Mira Two");
        // Save, and before the box answers: on to another bot, and its Delete asked.
        page.update(cx, |page, cx| {
            page.save_sheet(cx);
            page.show(Some("bot:bot_docs"), cx);
            page.act(Act::Delete, cx);
        });
        cx.run_until_parked();
        // Mira's answer renamed Mira; Docs Keeper's confirm is still asked, and nothing was deleted.
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "name": "Mira Two" })]);
        assert_eq!(page.read_with(cx, |page, _| (page.sheet().cloned(), page.bot().map(|bot| bot.name.clone()))), (Some(Sheet::Delete), Some("Docs Keeper".into())));
        assert!(stand_in.seen_at("DELETE", "kuvo/bots/bot_docs").is_empty());
        // Its Delete still works (nothing was left saving).
        click(cx, "sheet-save");
        assert_eq!(stand_in.seen_at("DELETE", "kuvo/bots/bot_docs").len(), 1);
        assert!(!shown(cx, "bot-sheet"));
    }

    #[gpui::test]
    fn a_refusal_that_comes_after_youve_gone_shows_over_no_other_bots_page(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        stand_in.bots_changeable(false);
        page.update(cx, |page, cx| {
            page.act(Act::SetPersonality(None), cx);
            page.show(Some("bot:bot_docs"), cx);
        });
        // Meanwhile Docs Keeper's rows aren't busy with Mira's change.
        cx.run_until_parked();
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "personality": null })]);
        assert!(!shown(cx, "bots-problem") && !shown(cx, "bot-personality-busy") && shown(cx, "bot-personality-menu"));
    }

    #[gpui::test]
    fn a_bot_made_after_youve_left_new_bot_moves_nothing(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        let chats = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = chats.clone();
        let _events = cx.update(|_, cx| {
            cx.subscribe(&page, move |_, event: &kuvo::bots::BotsEvent, _| {
                if matches!(event, kuvo::bots::BotsEvent::Chat { .. }) {
                    counted.set(counted.get() + 1);
                }
            })
        });
        page.update(cx, |page, cx| page.show(Some("new"), cx));
        cx.run_until_parked();
        cx.simulate_input("Scribe");
        page.update(cx, |page, cx| {
            page.act(Act::Create, cx);
            page.show(Some("bot:bot_mira"), cx);
            // Back to New Bot while it's being made: the same form, so Create can't make another.
            page.show(Some("new"), cx);
            page.act(Act::Create, cx);
            page.show(Some("bot:bot_mira"), cx);
        });
        cx.run_until_parked();
        assert_eq!(stand_in.seen_at("POST", "kuvo/bots").len(), 1, "made once");
        assert_eq!(chats.get(), 0, "you've gone to Mira's page: the window isn't pulled to the new bot");
        assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::Bot("bot_mira".into()));
    }
}
