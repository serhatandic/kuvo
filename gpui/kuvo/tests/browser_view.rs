//! A computer's browser in the side panel as someone uses it, against the stand-in box (nothing real
//! is reached, nothing waits on a clock): its pages' facts drawn as the address and the tab's name,
//! a picture drawn as it comes and the box told only once it's drawn, clicks and the wheel sent at
//! the page's own coordinates through a letterboxed picture, an address typed and sent, and what
//! can't be shown said plainly. Events go through the window, as the platform sends them.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, Focusable as _, Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase, VisualTestContext, point, px, size};
use kuvo::browser_view::{BrowserEvent, BrowserState, BrowserView};
use kuvo::stand_in::{Replies, StandIn};
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

const NOW: f64 = 1_794_650_000_000.0;

fn page(id: &str, url: &str, title: &str) -> Value {
    json!({ "targetId": id, "url": url, "title": title, "opener": null, "session": null, "loading": false, "canGoBack": true, "canGoForward": false, "working": false, "held": false })
}

/// A machine's box, its browser running with one page open.
fn machine() -> StandIn {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_elsewhere(true);
    stand_in.browser_pages(json!({ "active": "P", "pages": [page("P", "https://example.com/agent", "Example Domain")] }));
    stand_in
}

/// The browser on its own in a window 600 wide, its page drawn under its 40-point bar, as the panel
/// shows it.
fn open<'a>(cx: &'a mut TestAppContext, stand_in: &StandIn) -> (Entity<BrowserView>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |_, cx| BrowserView::new(client, false, cx));
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(600.), px(440.)));
    view.update_in(cx, |view, window, cx| window.focus(&view.focus_handle(cx), cx));
    cx.run_until_parked();
    (view, cx)
}

/// A JPEG `width` by `height`, of one colour.
fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let picture = image::RgbImage::from_pixel(width, height, image::Rgb([40, 120, 220]));
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 80).encode_image(&picture).unwrap();
    bytes
}

/// The box sends picture `frame_id` of page P, `width` by `height`.
fn frame(stand_in: &StandIn, frame_id: u64, width: u32, height: u32) {
    stand_in.browser_frame(json!({ "targetId": "P", "frameId": frame_id, "width": width, "height": height, "scrollX": 0, "scrollY": 0 }), &jpeg(width, height));
}

fn state(view: &Entity<BrowserView>, cx: &mut VisualTestContext) -> BrowserState {
    view.read_with(cx, |view, _| view.state())
}

fn address(view: &Entity<BrowserView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |view, cx| view.address().read(cx).text().to_owned())
}

fn events(view: &Entity<BrowserView>, cx: &mut VisualTestContext) -> Rc<RefCell<Vec<BrowserEvent>>> {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let kept = heard.clone();
    cx.update(|_, cx| cx.subscribe(view, move |_, event: &BrowserEvent, _| kept.borrow_mut().push(event.clone())).detach());
    heard
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

/// The mouse events the page was sent, as `(type, x, y)`.
fn mouse(stand_in: &StandIn) -> Vec<(String, f64, f64)> {
    stand_in
        .browser_sent_of("Kuvo.input")
        .into_iter()
        .filter(|input| input["kind"] == "mouse")
        .map(|input| (input["type"].as_str().unwrap().to_owned(), input["x"].as_f64().unwrap(), input["y"].as_f64().unwrap()))
        .collect()
}

#[gpui::test]
fn pages_arrive_and_the_page_shows_its_address_and_names_its_tab(cx: &mut TestAppContext) {
    let stand_in = machine();
    let (view, cx) = open(cx, &stand_in);
    assert_eq!(state(&view, cx), BrowserState::Open);
    assert_eq!(address(&view, cx), "example.com/agent");
    assert_eq!(view.read_with(cx, |view, _| view.title()), "Example Domain");

    // Pictures are asked for at the size the page is drawn, under the bar.
    let area = cx.debug_bounds("browser-page").unwrap();
    let bar = cx.debug_bounds("browser-bar").unwrap();
    assert_eq!((bar.size.height, area.size), (px(40.), size(px(600.), px(400.))));
    let asked = stand_in.browser_sent_of("Kuvo.view");
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!((&asked[0]["targetId"], &asked[0]["width"], &asked[0]["height"]), (&json!("P"), &json!(600), &json!(400)));

    // A page without a title is named for its host, and its tab hears so.
    let heard = events(&view, cx);
    stand_in.browser_pages(json!({ "active": "P", "pages": [page("P", "https://www.kuvo.run/code/docs/", "")] }));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.title()), "kuvo.run");
    assert_eq!(address(&view, cx), "www.kuvo.run/code/docs/");
    assert!(heard.borrow().contains(&BrowserEvent::Changed));
    assert_eq!(stand_in.browser_sent_of("Kuvo.view").len(), 1, "the same page, the same size: not asked again");
}

#[gpui::test]
fn a_picture_is_drawn_as_it_comes_and_the_box_hears_only_once_its_drawn(cx: &mut TestAppContext) {
    let stand_in = machine();
    let (view, cx) = open(cx, &stand_in);
    assert!(stand_in.browser_sent_of("Kuvo.frameAck").is_empty());
    frame(&stand_in, 12, 600, 400);
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.drawn_frame()), Some(12));
    assert_eq!(stand_in.browser_sent_of("Kuvo.frameAck"), [json!({ "frameId": 12 })]);

    // Two at once: only the newer is drawn; the older, never drawn, is said dropped, not drawn.
    frame(&stand_in, 13, 600, 400);
    frame(&stand_in, 14, 600, 400);
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.drawn_frame()), Some(14));
    assert_eq!(stand_in.browser_sent_of("Kuvo.frameAck"), [json!({ "frameId": 12 }), json!({ "frameId": 14 })]);
    assert_eq!(stand_in.browser_sent_of("Kuvo.frameDropped"), [json!({ "frameId": 13, "reason": "backpressure" })]);
}

#[gpui::test]
fn hidden_it_asks_for_no_pictures_and_shown_again_asks_once_more(cx: &mut TestAppContext) {
    let stand_in = machine();
    let (view, cx) = open(cx, &stand_in);
    view.update(cx, |view, cx| view.set_shown(false, cx));
    cx.run_until_parked();
    assert_eq!(stand_in.browser_sent_of("Kuvo.view").last(), Some(&json!({ "targetId": null })));
    assert!(cx.debug_bounds("browser-page").is_none(), "not drawn");

    // One already on its way isn't drawn, and is said so.
    frame(&stand_in, 7, 600, 400);
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.drawn_frame()), None);
    assert_eq!(stand_in.browser_sent_of("Kuvo.frameDropped"), [json!({ "frameId": 7, "reason": "unsent" })]);

    view.update(cx, |view, cx| view.set_shown(true, cx));
    cx.run_until_parked();
    let asked = stand_in.browser_sent_of("Kuvo.view");
    assert_eq!((asked.len(), &asked[2]["targetId"]), (3, &json!("P")));
    assert_eq!(stand_in.browser_opened(), (1, false), "its socket stayed open");
}

#[gpui::test]
fn a_click_lands_in_the_page_where_it_lands_on_a_letterboxed_picture(cx: &mut TestAppContext) {
    let stand_in = machine();
    // An agent works in the page at its own size, wider than the panel's.
    stand_in.browser_viewport(json!({ "targetId": "P", "width": 1280, "height": 800, "scale": 2, "by": "agent" }));
    let (_view, cx) = open(cx, &stand_in);
    frame(&stand_in, 1, 640, 400);
    cx.run_until_parked();

    // Drawn as wide as the panel, 600 by 375, in a band 12.5 high above and below.
    let area = cx.debug_bounds("browser-page").unwrap();
    assert_eq!(area.size, size(px(600.), px(400.)));
    let (left, top) = (area.left(), area.top() + px(12.5));
    cx.simulate_click(point(left + px(300.), top + px(187.5)), Modifiers::default());
    assert_eq!(mouse(&stand_in), [("mousePressed".into(), 640., 400.), ("mouseReleased".into(), 640., 400.)], "the page's middle");
    let pressed = &stand_in.browser_sent_of("Kuvo.input")[0];
    assert_eq!((&pressed["targetId"], &pressed["button"], &pressed["clickCount"]), (&json!("P"), &json!("left"), &json!(1)));

    cx.simulate_click(point(left + px(150.), top + px(93.75)), Modifiers::default());
    assert_eq!(mouse(&stand_in)[2], ("mousePressed".into(), 320., 200.), "a quarter of the way in");

    // A click in the band above the picture is no click in the page.
    cx.simulate_click(point(left + px(300.), area.top() + px(5.)), Modifiers::default());
    assert_eq!(mouse(&stand_in).len(), 4);

    // The wheel scrolls the page as far as it moved on screen, in the page's pixels.
    cx.simulate_event(ScrollEvent { position: point(left + px(300.), top + px(187.5)), delta: ScrollDelta::Pixels(point(px(0.), px(-75.))), modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved, source: ScrollSource::Pointer });
    let wheel = stand_in.browser_sent_of("Kuvo.input").into_iter().find(|input| input["kind"] == "wheel").expect("a turn of the wheel");
    assert_eq!((&wheel["x"], &wheel["y"], &wheel["deltaY"]), (&json!(640.0), &json!(400.0), &json!(160.0)));
}

#[gpui::test]
fn typing_an_address_and_pressing_return_sends_the_page_there(cx: &mut TestAppContext) {
    let stand_in = machine();
    let (view, cx) = open(cx, &stand_in);
    // The first click takes the whole address, so what's typed replaces it.
    click(cx, "browser-address");
    cx.simulate_input("kuvo.run");
    assert_eq!(address(&view, cx), "kuvo.run");
    // What you're typing stays as the page changes meanwhile.
    stand_in.browser_pages(json!({ "active": "P", "pages": [page("P", "https://example.com/elsewhere", "Example Domain")] }));
    cx.run_until_parked();
    assert_eq!(address(&view, cx), "kuvo.run");
    cx.simulate_keystrokes("enter");
    assert_eq!(stand_in.browser_sent_of("Kuvo.go"), [json!({ "targetId": "P", "input": "kuvo.run" })]);

    // Keys go back to the page: a character is typed in it as its text.
    cx.simulate_input("a");
    let keys: Vec<Value> = stand_in.browser_sent_of("Kuvo.input").into_iter().filter(|input| input["kind"] == "key").collect();
    assert_eq!(keys.len(), 2, "down and up: {keys:?}");
    assert_eq!((&keys[0]["type"], &keys[0]["key"], &keys[0]["text"], &keys[1]["type"]), (&json!("keyDown"), &json!("a"), &json!("a"), &json!("keyUp")));

    // Back and Reload, from the bar.
    click(cx, "browser-back");
    click(cx, "browser-reload");
    assert_eq!(stand_in.browser_sent_of("Kuvo.go")[1..], [json!({ "targetId": "P", "back": true }), json!({ "targetId": "P", "reload": true })]);
}

#[gpui::test]
fn a_computer_whose_way_cant_carry_the_live_view_says_so(cx: &mut TestAppContext) {
    let stand_in = machine();
    stand_in.sockets_not_here(Some("Terminals on devbox aren’t available over SSH yet."));
    let (view, cx) = open(cx, &stand_in);
    assert_eq!(state(&view, cx), BrowserState::Unavailable);
    assert!(cx.debug_bounds("browser-unavailable").is_some(), "this computer's browser can't be shown here yet");
    assert!(cx.debug_bounds("browser-bar").is_none());
    assert_eq!(view.read_with(cx, |view, _| view.title()), "Browser");
}

#[gpui::test]
fn a_macs_box_says_its_browser_cant_be_shown_here_without_being_asked_for_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    assert_eq!(state(&view, cx), BrowserState::Unavailable);
    assert!(cx.debug_bounds("browser-unavailable").is_some());
    assert!(stand_in.seen().iter().all(|seen| seen.path != "kuvo/browser/view"), "a box that doesn't list it isn't asked");
}

#[gpui::test]
fn a_browser_that_isnt_running_says_so_and_is_tried_again(cx: &mut TestAppContext) {
    let stand_in = machine();
    stand_in.browser_running(false);
    let (view, cx) = open(cx, &stand_in);
    assert_eq!(state(&view, cx), BrowserState::NotRunning);
    assert!(cx.debug_bounds("browser-not-running").is_some());

    stand_in.browser_running(true);
    click(cx, "browser-try-again");
    assert_eq!(state(&view, cx), BrowserState::Open);
    assert_eq!(address(&view, cx), "example.com/agent");
}

#[gpui::test]
fn a_connection_that_ends_says_so_and_reconnects(cx: &mut TestAppContext) {
    let stand_in = machine();
    let (view, cx) = open(cx, &stand_in);
    let heard = events(&view, cx);
    stand_in.browser_end(None);
    cx.run_until_parked();
    assert_eq!(state(&view, cx), BrowserState::Disconnected(None));
    assert!(cx.debug_bounds("browser-disconnected").is_some());
    assert!(heard.borrow().contains(&BrowserEvent::Changed));

    click(cx, "browser-try-again");
    assert_eq!(state(&view, cx), BrowserState::Open);
    assert_eq!(stand_in.browser_opened().0, 2);
}

#[gpui::test]
fn closing_it_closes_its_socket(cx: &mut TestAppContext) {
    let stand_in = machine();
    let (view, cx) = open(cx, &stand_in);
    drop(view);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
    assert_eq!(stand_in.browser_opened(), (1, true));
}
