//! The Bots area as someone uses it, against the stand-in box (nothing real is reached): a bot's
//! page (who it is, what it's doing, its sessions), its conversation, changing it through the
//! box's routes with the box's answer drawn after (the bot as it is now, an edit held for another
//! computer, or why not in the box's words), New Bot as the box offers it, and Delete asked first.
//! Clicks and keys go through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::bots::{BotsView, Sheet, Shown};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::{BoxClient, bots::BotChange};
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
    assert!(shown(cx, "bot-chat-strip"));
    click(cx, "bot-chat-settings");
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
    assert!(shown(cx, "bot-head") && !shown(cx, "bot-now"), "activity stays in the sidebar");
    assert_eq!(name_and_job(&page, cx).1, "Looks after the website");
    // Its sessions are the box's (`kuvo/work?bot=`), the latest first, each as the box says it stands.
    assert_eq!(stand_in.seen_at("GET", "kuvo/work").last().map(|seen| seen.path.clone()).as_deref(), Some("kuvo/work?bot=bot_mira"));
    let order: Vec<Pixels> = ["work-ses_signup", "work-ses_cms", "work-ses_dark", "work-ses_lighthouse"].iter().map(|row| bounds(cx, row).unwrap_or_else(|| panic!("{row}")).top()).collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "in the box's order");
    assert!(shown(cx, "work-working-ses_signup") && shown(cx, "work-asking-ses_cms"), "one at work spins, one waiting on you is marked");

    // A bot with nothing under way and no sessions says so, and claims nothing of now.
    click(cx, "bot-bot_release");
    click(cx, "bot-chat-settings");
    assert!(shown(cx, "work-none") && !shown(cx, "bot-now"));

    // A session opens where sessions are, and Back comes back to the bot's page.
    click(cx, "bot-bot_mira");
    click(cx, "bot-chat-settings");
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
    click(cx, "bot-chat-settings");
    assert!(shown(cx, "bot-head") && !shown(cx, "bot-chat-strip"));
    cx.simulate_keystrokes("cmd-[");
    cx.run_until_parked();
    assert!(shown(cx, "bot-chat-strip"));
    assert_eq!(composer.read_with(cx, |composer, cx| composer.field().read(cx).text().to_owned()), "How did it go?");
    // Another area and back: Bots shows the conversation it showed; its own row, its page.
    cx.simulate_keystrokes("ctrl-2");
    cx.simulate_keystrokes("ctrl-1");
    cx.run_until_parked();
    assert!(shown(cx, "bot-chat-strip"));
    click(cx, "bot-bot_mira");
    assert!(shown(cx, "bot-chat-strip"));
}

#[gpui::test]
fn renaming_goes_to_the_box_and_its_answer_is_what_shows(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    let name = bounds(cx, "bot-name-field").unwrap();
    let jobs = bounds(cx, "bot-job-line").unwrap();
    let instructions = bounds(cx, "bot-job-details").unwrap();
    let avatar = bounds(cx, "bot-avatar-edit").unwrap();
    assert_eq!(name.left(), jobs.left());
    assert_eq!(name.right(), jobs.right());
    assert_eq!(name.left() - avatar.right(), px(24.));
    assert_eq!(avatar.top(), name.top());
    assert_eq!(avatar.bottom(), jobs.bottom());
    assert_eq!(avatar.size.width, avatar.size.height);
    assert_eq!(instructions.left(), avatar.left());
    assert_eq!(instructions.right(), name.right());
    let name_label = bounds(cx, "bot-name-field-label").unwrap();
    let job_label = bounds(cx, "bot-job-line-label").unwrap();
    let instructions_label = bounds(cx, "bot-job-details-label").unwrap();
    assert_eq!(name.top() - name_label.bottom(), px(6.));
    assert_eq!(job_label.top() - name.bottom(), px(16.));
    assert_eq!(jobs.top() - job_label.bottom(), px(6.));
    assert_eq!(instructions_label.top() - jobs.bottom(), px(24.));
    assert_eq!(instructions.top() - instructions_label.bottom(), px(6.));
    assert_eq!(bounds(cx, "bot-group-Brain").unwrap().top() - instructions.bottom(), px(28.));
    click(cx, "bot-name-field");
    cx.simulate_keystrokes("cmd-a backspace enter");
    cx.run_until_parked();
    assert!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").is_empty());
    assert!(shown(cx, "bot-edit-error"));
    assert_eq!(name_and_job(&page, cx).0, "Mira");

    click(cx, "bot-name-field");
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("Mira Two");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "name": "Mira Two" })]);
    assert_eq!(bounds(cx, "bot-name-field").unwrap(), name);
    assert_eq!(title(&workspace, cx).as_deref(), Some("Mira Two"), "the sidebar's list is read again");
    // A change answers without an activity update, so the sidebar keeps its state.
    assert_eq!(page.read_with(cx, |page, _| page.bot().unwrap().now.asking), 1);
}

#[gpui::test]
fn escape_and_cancel_close_editors_and_send_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    let (name, job) = page.read_with(cx, |page, _| (page.name_field().clone(), page.job_fields().0.clone()));
    for (selector, field) in [("bot-name-field", name), ("bot-job-line", job)] {
        let saved = field.read_with(cx, |field, _| field.text().to_owned());
        click(cx, selector);
        cx.simulate_input("Draft");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(field.read_with(cx, |field, _| field.text().to_owned()), saved);
    }
    for opener in ["bot-avatar-edit", "bot-delete-button"] {
        click(cx, opener);
        assert!(shown(cx, "bot-sheet"));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!shown(cx, "bot-sheet"));
        click(cx, opener);
        click(cx, "sheet-cancel");
        assert!(!shown(cx, "bot-sheet"));
    }
    click(cx, "bot-personality-menu");
    assert!(shown(cx, "menu-item-Mentor") && !shown(cx, "bot-sheet"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "menu-item-Mentor"));
    assert!(stand_in.seen_at("PATCH", "kuvo/bots/bot_mira").is_empty() && stand_in.seen_at("DELETE", "kuvo/bots/bot_mira").is_empty());
}

#[gpui::test]
fn clicking_away_saves_inline_edits_only_when_they_changed(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    click(cx, "bot-name-field");
    click(cx, "bot-lives");
    assert!(shown(cx, "bot-name-field"));
    assert!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").is_empty());

    click(cx, "bot-name-field");
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("Mira Two");
    click(cx, "bot-lives");
    assert_eq!(name_and_job(&page, cx).0, "Mira Two");

    click(cx, "bot-job-line");
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("Keeps the website working");
    click(cx, "bot-lives");
    assert!(shown(cx, "bot-job-line"));
    assert_eq!(name_and_job(&page, cx).1, "Keeps the website working");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").len(), 2);
}

#[gpui::test]
fn its_job_and_look_change_through_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_release");
    // Tab saves Job on blur and leaves Instructions focused. Escape cancels only Instructions.
    click(cx, "bot-job-line");
    assert!(!shown(cx, "bot-sheet"));
    let head = bounds(cx, "bot-head").unwrap();
    let field = bounds(cx, "bot-job-details").unwrap();
    assert!(field.bottom() <= head.bottom());
    cx.simulate_keystrokes("cmd-a backspace");
    cx.simulate_input("Ships a beta every Thursday");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert!(cx.update(|window, cx| page.read(cx).job_fields().1.focus_handle(cx).is_focused(window)));
    let job_patch = json!({ "jobLine": "Ships a beta every Thursday" });
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_release"), vec![job_patch.clone()]);
    cx.simulate_input("Discard this.");
    cx.simulate_keystrokes("escape");
    assert_eq!(page.read_with(cx, |page, cx| page.job_fields().1.read(cx).text().to_owned()), "");
    assert_eq!(name_and_job(&page, cx).1, "Ships a beta every Thursday");
    cx.simulate_input("Tag it and wait for the build.");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Post the notes.");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Check the changelog.");
    assert!(bounds(cx, "bot-job-details").unwrap().size.height > field.size.height);
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_release"), vec![job_patch.clone()]);
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    let instructions = "Tag it and wait for the build.\nPost the notes.\nCheck the changelog.";
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_release"), vec![job_patch, json!({ "jobDetails": instructions })]);
    assert_eq!(name_and_job(&page, cx), ("Release Captain".into(), "Ships a beta every Thursday".into(), instructions.into()));

    // The look: a colour and a shape among those the box offers; its face is kept.
    click(cx, "bot-avatar-pencil");
    assert!(shown(cx, "look-color-graphite") && shown(cx, "look-shape-bloom"), "the box's colours and shapes");
    click(cx, "look-color-amber");
    click(cx, "look-shape-gem");
    assert!(stand_in.seen_at("PATCH", "kuvo/bots/bot_release").len() == 2, "nothing is sent until Save");
    click(cx, "sheet-save");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_release").last(), Some(&json!({ "look": { "color": "amber", "shape": "gem", "face": "calm" } })));
    assert_eq!(page.read_with(cx, |page, _| page.bot().map(|bot| bot.look.color.clone())), Some("amber".into()));
}

#[gpui::test]
fn changing_job_keeps_instructions_changed_elsewhere(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    BoxClient::new(stand_in.clone()).change_bot("bot_mira", &BotChange {
        job_details: Some("Updated elsewhere.".into()), ..BotChange::default()
    }).unwrap();
    click(cx, "bot-job-line");
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("Keeps the website working");
    click(cx, "bot-lives");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "jobLine": "Keeps the website working" })));
    assert_eq!(name_and_job(&page, cx).2, "Updated elsewhere.");
}

#[gpui::test]
fn an_older_computer_keeps_the_job_draft_and_asks_for_an_update(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_lacks(&["bots.job-fields"]);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    click(cx, "bot-job-line");
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("Keeps the website working");
    cx.simulate_keystrokes("enter");
    assert!(shown(cx, "bot-edit-error-Couldn’t save the change. Update Kuvo and try again."));
    assert!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").is_empty());
    assert_eq!(page.read_with(cx, |page, cx| page.job_fields().0.read(cx).text().to_owned()), "Keeps the website working");
}

#[gpui::test]
fn overlong_job_and_instructions_keep_their_drafts_and_name_the_limit(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in);
    let page = bot(&workspace, cx, "bot_mira");
    for (index, selector, submit, limit, message) in [
        (0, "bot-job-line", "enter", 200, "Shorten Job to 200 characters or fewer."),
        (1, "bot-job-details", "cmd-enter", 8000, "Shorten Instructions to 8,000 characters or fewer."),
    ] {
        let draft = "x".repeat(limit + 1);
        click(cx, selector);
        cx.simulate_keystrokes("cmd-a");
        page.update(cx, |page, cx| {
            let fields = page.job_fields();
            let field = if index == 0 { fields.0 } else { fields.1 };
            field.update(cx, |field, cx| field.set_text(draft.clone(), cx));
        });
        cx.simulate_keystrokes(submit);
        cx.run_until_parked();
        assert!(shown(cx, &format!("bot-edit-error-{message}")));
        assert_eq!(page.read_with(cx, |page, cx| {
            let fields = page.job_fields();
            if index == 0 { fields.0 } else { fields.1 }.read(cx).text().to_owned()
        }), draft);
    }
    assert!(shown(cx, "bot-edit-error-Shorten Job to 200 characters or fewer."));
    cx.simulate_keystrokes("escape");
    assert!(!shown(cx, "bot-edit-error-Shorten Instructions to 8,000 characters or fewer."));
    assert!(shown(cx, "bot-edit-error-Shorten Job to 200 characters or fewer."));
}

#[gpui::test]
fn personality_and_model_are_picked_from_what_the_box_offers(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    click(cx, "bot-personality-menu");
    click(cx, "menu-item-Mentor");
    assert!(!shown(cx, "menu-item-Mentor"));
    assert!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").is_empty(), "the current personality needs no change");
    click(cx, "bot-personality-menu");
    let details = bounds(cx, "menu-item-Ship’s Cook-description").expect("the custom personality’s description");
    assert!(details.size.height > px(36.), "a long description wraps in the picker");
    click(cx, "menu-item-Straight Shooter");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "personality": "preset.straight" })]);
    assert_eq!(page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.personality.clone())), Some("preset.straight".into()));
    // None: the model's own way, sent as null.
    click(cx, "bot-personality-menu");
    click(cx, "menu-item-The model’s own way");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "personality": null })));
    assert_eq!(page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.personality.clone())), None);

    // From the model picker: OpenAI's models, then GPT-6.
    click(cx, "bot-model-menu");
    click(cx, "model-rail-openai");
    click(cx, "model-row-openai/gpt-6");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "model": { "providerID": "openai", "id": "gpt-6" } })));
    assert_eq!(page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.model.as_ref().map(|model| model.id.clone()))), Some("gpt-6".into()));
}

#[gpui::test]
fn profile_rows_share_cards_and_model_uses_the_provider_mark(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    bot(&workspace, cx, "bot_mira");

    for (top, bottom) in [
        ("bot-model-menu", "bot-personality-menu"),
        ("bot-lives", "bot-works"),
        ("work-ses_signup", "work-ses_cms"),
    ] {
        let top = bounds(cx, top).unwrap();
        let bottom = bounds(cx, bottom).unwrap();
        assert_eq!(top.left(), bottom.left());
        assert_eq!(top.right(), bottom.right());
        assert_eq!(bottom.top() - top.bottom(), px(1.), "only a hairline between grouped rows");
    }

    let claude = bounds(cx, "bot-model-provider-anthropic").unwrap();
    let model = bounds(cx, "bot-model-menu").unwrap();
    assert_eq!(claude.left(), model.left(), "the mark leads the row");
    click(cx, "bot-model-menu");
    click(cx, "model-rail-openai");
    click(cx, "model-row-openai/gpt-6");
    assert!(shown(cx, "bot-model-provider-openai") && !shown(cx, "bot-model-provider-anthropic"));
}

#[gpui::test]
fn a_long_personality_description_wraps_on_the_profile(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    bot(&workspace, cx, "bot_mira");
    let short = bounds(cx, "bot-personality-detail").unwrap();
    let row = bounds(cx, "bot-personality-menu").unwrap();
    assert!(row.size.width > px(400.), "the whole row opens the picker");
    click(cx, "bot-personality-menu");
    click(cx, "menu-item-Ship’s Cook");
    let long = bounds(cx, "bot-personality-detail").unwrap();
    assert!(long.size.height > short.size.height * 2., "the selected description wraps on the card");
    assert!(bounds(cx, "bot-personality-menu").unwrap().right() > long.right());
}

#[gpui::test]
fn a_custom_personality_can_be_edited_from_the_picker(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    bot(&workspace, cx, "bot_mira");
    click(cx, "bot-personality-menu");
    assert!(!shown(cx, "menu-item-Edit personality…"), "the selected preset is read-only");
    click(cx, "menu-item-Ship’s Cook");
    click(cx, "bot-personality-menu");
    click(cx, "menu-item-Edit personality…");
    assert!(shown(cx, "personality-edit-name") && shown(cx, "personality-edit-description"));
    cx.simulate_keystrokes("shift-tab cmd-a");
    cx.simulate_input("Galley Guide");
    cx.simulate_keystrokes("tab cmd-a");
    cx.simulate_input("Keeps brief notes.");
    cx.simulate_keystrokes("tab tab enter");
    cx.simulate_event(gpui::KeyUpEvent { keystroke: gpui::Keystroke::parse("enter").unwrap() });
    cx.run_until_parked();
    assert!(!shown(cx, "bot-sheet"));
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/personalities/custom.pirate"), vec![
        json!({ "name": "Galley Guide", "description": "Keeps brief notes." })
    ]);
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").len(), 1, "the bot keeps its selected personality");
    click(cx, "bot-personality-menu");
    assert!(shown(cx, "menu-item-Galley Guide"));
}

#[gpui::test]
fn a_change_the_box_turns_down_shows_a_problem_and_changes_nothing(cx: &mut TestAppContext) {
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
    click(cx, "bot-chat-settings");
    stand_in.move_bot("bot_docs", "studio-mini");
    click(cx, "bot-name-field");
    cx.simulate_keystrokes("cmd-a");
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
    click(cx, "model-rail-openai");
    click(cx, "model-row-openai/gpt-6");
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
    assert!(shown(cx, "elsewhere-note") && !shown(cx, "bot-personality-menu") && !shown(cx, "bot-model-menu") && !shown(cx, "bot-job-line"));

    // Renamed from here: asked of this box, which holds it for the bot's computer (202).
    click(cx, "bot-name-field");
    cx.simulate_keystrokes("cmd-a");
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
    assert!(shown(cx, "bot-refused-0") && !shown(cx, "bot-name-field") && !shown(cx, "bot-delete-button"));
}

#[gpui::test]
fn its_effort_is_picked_from_its_models_own_and_sent_alone(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let page = bot(&workspace, cx, "bot_mira");
    let variant = |cx: &mut VisualTestContext| page.read_with(cx, |page, _| page.bot().and_then(|bot| bot.model.as_ref().and_then(|model| model.variant.clone())));
    assert_eq!(variant(cx), Some("high".into()));
    click(cx, "bot-model-menu");
    assert!(shown(cx, "bot-effort-max") && !shown(cx, "bot-effort-extra"), "its model's own efforts, as the box lists them");
    click(cx, "bot-effort-low");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "effort": "low" })]);
    assert_eq!(variant(cx), Some("low".into()), "the box's answer");
    // None: the model's own, sent as null.
    click(cx, "bot-model-menu");
    click(cx, "bot-effort-default");
    assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "effort": null })));
    assert_eq!(variant(cx), None);
    assert!(shown(cx, "bot-model-value-Claude Opus 5.5 · Default"));
    // A model with no efforts of its own has no Effort row.
    click(cx, "bot-model-menu");
    click(cx, "model-rail-google");
    click(cx, "model-row-google/gemini-3-pro");
    click(cx, "bot-model-menu");
    assert!(shown(cx, "model-picker") && !shown(cx, "bot-effort-default"));
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
    assert!(lines.len() >= 4 && lines.iter().any(|line| line.words.contains("goes to the Trash")) && lines.last().is_some_and(|line| line.kept));
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
    cx.simulate_resize(size(px(550.), px(820.)));
    cx.run_until_parked();
    assert!(!shown(cx, "area-sidebar") && shown(cx, "bot-head") && shown(cx, "bot-chat"), "the sidebar folds away; the page and its Chat stay");
    let name = bounds(cx, "bot-name-field").unwrap();
    let avatar = bounds(cx, "bot-avatar-edit").unwrap();
    let instructions = bounds(cx, "bot-job-details").unwrap();
    assert_eq!(name.left() - avatar.right(), px(24.));
    assert_eq!(avatar.top(), name.top());
    assert_eq!(avatar.bottom(), bounds(cx, "bot-job-line").unwrap().bottom());
    assert_eq!(instructions.left(), avatar.left());
    assert_eq!(instructions.right(), name.right());
    assert!(instructions.left() >= px(0.) && instructions.right() <= px(550.), "the column fits the window");
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
        click(cx, "bot-name-field");
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("Mira Two");
        // Save, and before the box answers: on to another bot, and its Delete asked.
        page.update(cx, |page, cx| {
            page.save_editor(cx);
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
    fn another_setting_waits_for_an_inline_save(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        click(cx, "bot-name-field");
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("Mira Two");
        page.update(cx, |page, cx| {
            page.save_editor(cx);
            page.act(Act::SetEffort(Some("low".into())), cx);
        });
        cx.run_until_parked();
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "name": "Mira Two" })]);
        click(cx, "bot-model-menu");
        click(cx, "bot-effort-low");
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").last(), Some(&json!({ "effort": "low" })));
    }

    #[gpui::test]
    fn profile_saves_keep_a_new_draft_while_a_response_is_pending(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        let (_, _, instructions) = name_and_job(&page, cx);
        page.update(cx, |page, cx| {
            page.name_field().update(cx, |field, cx| field.set_text("Mira Two", cx));
            page.save_editor(cx);
            page.job_fields().0.update(cx, |field, cx| field.set_text("Keeps the website working", cx));
            page.save_editor(cx);
        });
        cx.run_until_parked();
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![
            json!({ "name": "Mira Two" }),
            json!({ "jobLine": "Keeps the website working" }),
        ]);
        let (name, line, details) = name_and_job(&page, cx);
        assert_eq!((name.as_str(), line.as_str(), details), ("Mira Two", "Keeps the website working", instructions));
    }

    #[gpui::test]
    fn navigating_drains_a_draft_behind_a_pending_save(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        // Both edits and the switch happen before the first response can be dispatched.
        page.update(cx, |page, cx| {
            page.name_field().update(cx, |field, cx| field.set_text("Mira Two", cx));
            page.save_editor(cx);
            page.job_fields().1.update(cx, |field, cx| field.set_text("Keep this on the way out.", cx));
            page.show(Some("bot:bot_docs"), cx);
        });
        cx.run_until_parked();
        assert_eq!(name_and_job(&page, cx).0, "Docs Keeper");
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![
            json!({ "name": "Mira Two" }), json!({ "jobDetails": "Keep this on the way out." }),
        ]);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        assert_eq!(name_and_job(&page, cx).2, "Keep this on the way out.");
    }

    #[gpui::test]
    fn a_failed_profile_save_survives_navigation_and_can_be_retried(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        stand_in.bots_changeable(false);
        page.update(cx, |page, cx| {
            page.name_field().update(cx, |field, cx| field.set_text("Mira Two", cx));
            page.save_editor(cx);
            page.show(Some("bot:bot_docs"), cx);
        });
        cx.run_until_parked();
        assert!(!shown(cx, "bot-edit-error"));
        stand_in.bots_changeable(true);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        assert!(shown(cx, "bot-edit-error"));
        assert_eq!(page.read_with(cx, |page, cx| page.name_field().read(cx).text().to_owned()), "Mira Two");
        click(cx, "bot-name-field");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(name_and_job(&page, cx).0, "Mira Two");
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira").len(), 2);
        assert!(!shown(cx, "bot-edit-error"));
    }

    #[gpui::test]
    fn a_response_preserves_a_new_name_equal_to_the_old_one(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        page.update(cx, |page, cx| {
            page.name_field().update(cx, |field, cx| field.set_text("Mira Two", cx));
            page.save_editor(cx);
            page.name_field().update(cx, |field, cx| field.set_text("Mira", cx));
        });
        cx.run_until_parked();
        assert_eq!(name_and_job(&page, cx).0, "Mira Two");
        assert_eq!(page.read_with(cx, |page, cx| page.name_field().read(cx).text().to_owned()), "Mira");
        page.update(cx, |page, cx| page.save_editor(cx));
        cx.run_until_parked();
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![
            json!({ "name": "Mira Two" }), json!({ "name": "Mira" }),
        ]);
    }

    #[gpui::test]
    fn escape_cancels_only_instructions_while_job_is_saving(cx: &mut TestAppContext) {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (page, cx) = host(cx, &stand_in);
        page.update(cx, |page, cx| page.show(Some("bot:bot_mira"), cx));
        cx.run_until_parked();
        let instructions = name_and_job(&page, cx).2;
        click(cx, "bot-job-details");
        page.update(cx, |page, cx| {
            page.job_fields().0.update(cx, |field, cx| field.set_text("Saving this job", cx));
            page.save_editor(cx);
            page.job_fields().1.update(cx, |field, cx| {
                field.set_text("Discard these instructions.", cx);
                cx.emit(kuvo_ui::text_field::TextFieldEvent::Cancel);
            });
        });
        cx.run_until_parked();
        assert_eq!(bodies(&stand_in, "PATCH", "kuvo/bots/bot_mira"), vec![json!({ "jobLine": "Saving this job" })]);
        assert_eq!(name_and_job(&page, cx).1, "Saving this job");
        assert_eq!(page.read_with(cx, |page, cx| page.job_fields().1.read(cx).text().to_owned()), instructions);
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

#[gpui::test]
fn bot_composer_uses_the_box_catalog_and_refreshes_after_credentials_change(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    bot(&workspace, cx, "bot_mira");
    click(cx, "bot-chat");
    let composer = workspace.read_with(cx, |workspace, cx| workspace.bot_chat().unwrap().1.read(cx).composer().clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let offered = client.offered_models(None, None).unwrap();
    let engine_models = |offered: kuvo_box::models::Offered| offered.models.into_iter().filter(|model| model.side == kuvo_box::models::Side::Engine).collect::<Vec<_>>();
    let before = engine_models(offered);
    assert_eq!(composer.read_with(cx, |composer, _| composer.menu_models()), before);
    stand_in.more_models(1);
    let view = workspace.read_with(cx, |workspace, _| workspace.bot_chat().unwrap().1.clone());
    let mut conversation = kuvo::conversation::read_now(&client, "ses_mira").unwrap();
    conversation.apply(&json!({ "type": "credential.updated", "data": {} }));
    view.update(cx, |view, cx| view.take(kuvo_box::ConversationUpdate::Conversation(conversation), cx));
    cx.run_until_parked();
    assert_eq!(composer.read_with(cx, |composer, _| composer.menu_models()), engine_models(client.offered_models(None, None).unwrap()));
    assert_eq!(composer.read_with(cx, |composer, _| composer.menu_models().len()), before.len() + 1);
}

#[gpui::test]
fn wake_chip_opens_schedule_and_cancel_requires_confirmation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_bot_wakes("bot_mira", vec![json!({
        "id": "at-build", "due": NOW / 1000. + 180., "summary": "Read the latest build report", "cancelKey": "build",
    })], vec![]);
    let (workspace, cx) = open(cx, stand_in.clone());
    bot(&workspace, cx, "bot_mira");
    click(cx, "bot-chat");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "wake-schedule"));
    click(cx, "cancel-entry");
    assert!(shown(cx, "confirm-cancel-entry"));
    assert!(stand_in.seen_at("POST", "kuvo/wakes/ses_mira/cancel").is_empty());
    click(cx, "keep-entry");
    assert!(!shown(cx, "confirm-cancel-entry"));
    click(cx, "cancel-entry");
    click(cx, "confirm-cancel-entry");
    assert_eq!(bodies(&stand_in, "POST", "kuvo/wakes/ses_mira/cancel"), vec![json!({"wake":"build"})]);
    assert!(shown(cx, "wake-schedule") && !shown(cx, "cancel-entry"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    click(cx, "bot-bot_inbox");
    assert!(!shown(cx, "open-wake-schedule"));
}

#[gpui::test]
fn moved_bot_wakes_are_followed_before_opening_bots(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    let peer = stand_in.peer_box("key_devbox").expect("destination computer");
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, true, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    workspace.update_in(cx, |workspace, _, cx| workspace.set_now(NOW, cx));
    cx.run_until_parked();
    stand_in.move_bot("bot_mira", "devbox");
    peer.add_bot("bot_mira", "Mira", json!({"color":"mint","shape":"round","face":"calm"}));
    peer.set_bot_wakes("bot_mira", vec![json!({
        "id": "at-build", "due": NOW / 1000. + 180., "summary": "Read the latest build report", "cancelKey": "build",
    })], vec![]);
    // The follow's bridge runs on a native thread. Wait on its handoff while the app is parked.
    for _ in 0..400 {
        cx.run_until_parked();
        if !peer.seen_at("GET", "kuvo/bots/events").is_empty() { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(workspace.read_with(cx, |workspace, _| workspace.area() == Area::Sessions));
    assert_eq!(peer.seen_at("GET", "kuvo/bots/events").len(), 1);
    assert!(peer.seen_at("GET", "kuvo/wakes/ses_mira/events").is_empty());
    click(cx, "rail-bots");
    for _ in 0..400 {
        cx.run_until_parked();
        if shown(cx, "bot-avatar-wake") { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(shown(cx, "bot-avatar-wake"), "the destination's schedule marks its avatar with the clock icon");
    assert!(!shown(cx, "open-wake-schedule"), "the Bots sidebar row must not show a wake-up pill");

    // Even when a remote bot's wake-up is already due, no "Wake-up due" pill is shown in the sidebar.
    peer.set_bot_wakes("bot_mira", vec![json!({
        "id": "at-due", "due": NOW / 1000. - 60., "summary": "Overdue check", "cancelKey": "due",
    })], vec![]);
    for _ in 0..400 {
        cx.run_until_parked();
        if !shown(cx, "bot-avatar-wake") { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!shown(cx, "open-wake-schedule"), "a due wake-up must not render a Wake-up due pill in the Bots sidebar");
}

#[gpui::test]
fn wake_list_wraps_a_long_summary_and_bounds_many_entries(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let long = "Review the deployment across every region, compare it with previous releases, and flag anything unusual for another look before the Friday handoff.";
    let wakes = (0..12).map(|index| json!({
        "id": index.to_string(), "due": NOW / 1000. + 180. + 60. * index as f64,
        "summary": if index == 0 { long } else { "Check the build" }, "cancelKey": index.to_string(),
    })).collect();
    stand_in.set_bot_wakes("bot_mira", wakes, vec![]);
    let (workspace, cx) = open(cx, stand_in);
    bot(&workspace, cx, "bot_mira");
    click(cx, "bot-chat");
    click(cx, "conversation-activity-button");
    let first = bounds(cx, "wake-0").unwrap();
    let second = bounds(cx, "wake-1").unwrap();
    assert!(first.size.height > second.size.height);
    assert!(second.top() >= first.bottom() - px(1.));
    assert!(bounds(cx, "wake-entries").unwrap().size.height <= px(288.));
}

#[gpui::test]
fn wake_badge_sits_on_the_avatar_and_yields_to_activity(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    for id in ["bot_mira", "bot_inbox", "bot_docs"] {
        stand_in.set_bot_wakes(id, vec![json!({
            "id": "at-build", "due": NOW / 1000. + 180., "summary": "Check the build", "cancelKey": "build",
        })], vec![]);
    }
    stand_in.set_bot_now("bot_mira", json!({"working":0,"asking":0,"state":"idle"}));
    stand_in.set_bot_now("bot_docs", json!({"working":1,"asking":0,"state":"working","waking":true}));
    let (_, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    let wake = bounds(cx, "bot-avatar-wake").unwrap();
    let avatar = bounds(cx, "bot-bot_mira-avatar").unwrap();
    assert_eq!(wake.size, size(px(14.), px(14.)));
    assert_eq!(wake.right(), avatar.right() - px(3.));
    assert_eq!(wake.bottom(), avatar.bottom() - px(3.));
    assert_eq!(wake.size, bounds(cx, "bot-avatar-needs-you").unwrap().size);
    assert_eq!(wake.size, bounds(cx, "bot-avatar-at-work").unwrap().size);
}

#[gpui::test]
fn wake_chip_drops_cached_activity_on_disconnect_until_a_fresh_bot_snapshot(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_bot_now("bot_mira", json!({"working":1,"asking":0,"state":"working","waking":true}));
    let client = BoxClient::new(stand_in.clone());
    let (workspace, cx) = open(cx, stand_in.clone());
    bot(&workspace, cx, "bot_mira");
    click(cx, "bot-chat");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-wake-status"));

    stand_in.set_down(true);
    workspace.update_in(cx, |workspace, _, cx| workspace.connected(client.connect(), cx));
    cx.run_until_parked();
    assert!(shown(cx, "bot-chat-strip"));
    assert!(!shown(cx, "activity-wake-status"));

    stand_in.set_down(false);
    workspace.update_in(cx, |workspace, _, cx| workspace.connected(client.connect(), cx));
    cx.run_until_parked();
    assert!(!shown(cx, "activity-wake-status"), "the old snapshot stays stale after reconnect");
    let bots = workspace.read_with(cx, |workspace, _| workspace.area_view(Area::Bots).unwrap().clone());
    bots.update(cx, |bots, cx| bots.shown(cx));
    cx.run_until_parked();
    assert!(shown(cx, "activity-wake-status"), "a fresh active turn can claim the chip again");

    stand_in.set_bot_now("bot_mira", json!({"working":0,"asking":0,"state":"idle"}));
    bots.update(cx, |bots, cx| bots.shown(cx));
    cx.run_until_parked();
    assert!(!shown(cx, "activity-wake-status"));
}
