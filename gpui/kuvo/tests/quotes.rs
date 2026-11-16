//! Replying to several parts of a conversation, as 0.7 let you (its `QuoteList`): each quote over
//! the composer with your answer to it in a field of its own, then your own words; the message
//! goes as every app writes it (KuvoClient's `Quoting.message`), quote, answer, quote, answer, a
//! rule, your words. Against the stand-in box; keys and clicks through the window.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::conversation::ConversationView;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<ConversationView>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(800.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_recipes", window, cx));
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    (view, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn quote(view: &Entity<ConversationView>, text: &str, cx: &mut VisualTestContext) {
    view.update_in(cx, |view, window, cx| view.quote(text, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn each_quote_has_its_own_answer_and_the_message_goes_as_every_app_writes_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, stand_in.clone());
    // Quoted: its answer opens under it, with the keyboard.
    quote(&view, "Lentils take 25 minutes", cx);
    assert!(bounds(cx, "quote-answer-0").is_some());
    cx.simulate_input("Red ones are faster");
    quote(&view, "Rice needs 18", cx);
    assert!(bounds(cx, "quote-answer-1").is_some(), "the next one's opens");
    assert!(bounds(cx, "quote-answer-0").is_some(), "one with words stays open");
    cx.simulate_input("Basmati?");
    // Your own words, in the composer's field.
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Plan a week of dinners");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let body = stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/send").last().and_then(|seen| seen.body.clone()).expect("sent");
    assert_eq!(body["words"], "> Lentils take 25 minutes\n\nRed ones are faster\n\n> Rice needs 18\n\nBasmati?\n\n---\n\nPlan a week of dinners");
    assert!(bounds(cx, "quotes").is_none(), "gone with the message");
}

#[gpui::test]
fn an_empty_answer_closes_a_quote_opens_its_answer_when_clicked_and_quotes_go_one_or_all_at_once(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, stand_in);
    quote(&view, "Lentils take 25 minutes", cx);
    quote(&view, "Rice needs 18", cx);
    // The first's answer, left empty as the keyboard went to the second's, closed.
    assert!(bounds(cx, "quote-answer-0").is_none());
    click(cx, "quote-0");
    assert!(bounds(cx, "quote-answer-0").is_some(), "clicked: open, to answer");
    // With two: how many, and Remove All; × takes one.
    assert!(bounds(cx, "quotes-remove-all").is_some());
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    composer.update(cx, |composer, cx| composer.unquote(1, cx));
    cx.run_until_parked();
    assert_eq!(composer.read_with(cx, |composer, cx| composer.quotes(cx).len()), 1);
    assert!(bounds(cx, "quotes-remove-all").is_none(), "one: no count");
    quote(&view, "Beans overnight", cx);
    click(cx, "quotes-remove-all");
    assert!(bounds(cx, "quotes").is_none());
}
