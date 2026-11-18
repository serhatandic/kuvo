//! A box's form with what's kept secret, filled in through the window: a secret hidden as it's
//! typed, a note's lines, names whose values are hidden, a file of this computer's and a pick of
//! many. Each check the box gets carries dots and a file's name and size, never what was typed or
//! the file's bytes; what's sent with the action is what was typed. Show reads a secret's kept value
//! from the vault's own route only when asked, after Touch ID or the computer's password, and hides
//! it again; a field that lets something use what's yours more asks the same before it's sent. A
//! form that changed since it opened isn't sent again. (Nothing here knows what the form is for:
//! the fields are made up, as a box could describe them.)

use std::sync::{Arc, Mutex};

use gpui::{Entity, Modifiers, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point, px, size};
use kuvo::settings::form::{FormSheet, Target};
use kuvo::stand_in::{Replies, StandIn};
use kuvo_box::BoxClient;
use kuvo_box::settings_pages::Form;
use kuvo_box::transport::{Closer, Request, Response, Stream, Transport};
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

const NOW: f64 = 1_794_650_000_000.0;
const FORM: &str = "kuvo/settings/things/one/edit";
const SHOWN: &str = "kuvo/vault/items/v_abcdefghij/values";
const GONE: &str = "kuvo/vault/items/v_0000000000/values";

/// The stand-in box (its `kuvo/info`, so the client's gate opens), answering this form's routes
/// itself and keeping every request it's sent.
#[derive(Clone)]
struct ThingsBox {
    stand_in: StandIn,
    seen: Arc<Mutex<Vec<Request>>>,
    /// The form's row changed meanwhile: its action answers `409` with the page.
    stale: Arc<std::sync::atomic::AtomicBool>,
}

fn answer(status: u16, body: Value) -> kuvo_box::Result<Response> {
    Ok(Response { status, content_type: Some("application/json".into()), body: serde_json::to_vec(&body).unwrap() })
}

impl Transport for ThingsBox {
    fn send(&self, request: &Request) -> kuvo_box::Result<Response> {
        self.seen.lock().unwrap().push(request.clone());
        match (request.method.as_str(), request.route()) {
            ("POST", route) if route == format!("{FORM}/check") => answer(200, json!({ "fields": [], "ready": true })),
            ("POST", FORM) if self.stale.load(std::sync::atomic::Ordering::Relaxed) => answer(409, json!({ "message": "It changed since you read it.", "page": { "id": "things", "title": "Things", "groups": [] } })),
            ("POST", FORM) => answer(200, json!({ "id": "things", "title": "Things", "groups": [] })),
            ("GET", "kuvo/settings/things") if request.path.ends_with("?refresh-models=1") => answer(200, json!({
                "id": "things", "title": "Things", "groups": [{ "id": "things", "rows": [{
                    "id": "one", "title": "Thing", "kind": "actions", "revision": "r1", "actions": [{
                        "id": "edit", "title": "Edit", "form": { "title": "Edit", "submit": "Save", "fields": [{
                            "id": "model", "title": "Model", "kind": "choice", "choices": [
                                { "id": "old-choice", "title": "Old Model", "model": { "providerID": "openai", "id": "old" } },
                                { "id": "new-choice-id", "title": "New Model", "model": { "providerID": "openai", "id": "new" } }
                            ]
                        }] }
                    }]
                }] }]
            })),
            ("GET", SHOWN) => answer(200, json!({ "values": { "password": "kept pass", "other": "never shown" } })),
            _ => self.stand_in.send(request),
        }
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> kuvo_box::Result<Stream> {
        self.stand_in.open(request, hold)
    }

    fn streams(&self) -> bool {
        false
    }

    fn describe(&self) -> String {
        "this Mac".into()
    }
}

impl ThingsBox {
    fn bodies(&self, method: &str, route: &str) -> Vec<Value> {
        self.seen.lock().unwrap().iter().filter(|seen| seen.method == method && seen.route() == route).filter_map(|seen| seen.body.clone()).collect()
    }
}

fn form() -> Form {
    serde_json::from_value(json!({ "title": "Edit Thing", "submit": "Save", "fields": [
        { "id": "password", "title": "Password", "kind": "secret", "placeholder": "Unchanged", "shows": { "route": SHOWN, "field": "password" } },
        { "id": "pin", "title": "PIN", "kind": "secret", "placeholder": "Unchanged", "shows": { "route": GONE, "field": "pin" } },
        { "id": "note", "title": "Note", "kind": "secret", "multi": true, "placeholder": "Unchanged" },
        { "id": "env", "title": "Values", "kind": "pairs", "secret": true, "pairs": { "key": "NAME", "value": "value" } },
        { "id": "key", "title": "Key file", "kind": "file", "value": { "name": "old.pem" }, "file": { "choose": "Choose…", "none": "No file" } },
        { "id": "who", "title": "Who can use it", "kind": "text", "value": "Only me", "verified": true },
        { "id": "model", "title": "Model", "kind": "choice", "value": "a", "choices": [
            { "id": "a", "title": "Alpha" }, { "id": "b", "title": "Beta" }, { "id": "c", "title": "Gamma" }, { "id": "d", "title": "Delta" }, { "id": "e", "title": "Epsilon" }
        ] },
        { "id": "share", "title": "Shared with", "kind": "choice", "value": "me", "choices": [{ "id": "me", "title": "Me" }, { "id": "all", "title": "Everyone", "verified": true }] }
    ] }))
    .unwrap()
}

fn open(cx: &mut TestAppContext) -> (ThingsBox, Entity<FormSheet>, &mut VisualTestContext) {
    open_where(cx, None)
}

/// Opened where the app asks Touch ID or the computer's password (`it_is_you`: how a test answers
/// it), its asks kept; none: where it can't ask.
fn open_where(cx: &mut TestAppContext, it_is_you: Option<(bool, Arc<Mutex<Vec<String>>>)>) -> (ThingsBox, Entity<FormSheet>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
        if let Some((answer, asked)) = it_is_you {
            cx.set_global(kuvo::confirm_you::ConfirmYou(Arc::new(move |reason: &str| {
                asked.lock().unwrap().push(reason.to_owned());
                answer
            })));
        }
    });
    let the_box = ThingsBox { stand_in: StandIn::at(Replies::Never, NOW), seen: Arc::default(), stale: Arc::default() };
    let client = BoxClient::new(the_box.clone());
    let target = Target { section: "things".into(), row: "one".into(), was: "r1".into(), action: "edit".into() };
    let window = cx.add_window(move |_, cx| FormSheet::new(client, false, target, form(), cx));
    let sheet = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(800.), px(1000.)));
    sheet.update(cx, |sheet, cx| sheet.start(cx));
    cx.run_until_parked();
    (the_box, sheet, cx)
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str())).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str())).is_some()
}

/// Scrolls the sheet's fields to their end, as the wheel over them would.
fn scroll_fields(cx: &mut VisualTestContext) {
    let at = cx.debug_bounds("settings-form-password-field").expect("the sheet's fields").center();
    cx.simulate_event(ScrollEvent { position: at, delta: ScrollDelta::Pixels(point(px(0.), px(-1000.))), ..Default::default() });
    cx.run_until_parked();
}

fn value(sheet: &Entity<FormSheet>, cx: &mut VisualTestContext, field: &str) -> Value {
    sheet.read_with(cx, |sheet, cx| sheet.values(cx).get(field).cloned().unwrap_or_default())
}

#[gpui::test]
fn a_forms_picker_refreshes_choices_and_sends_the_new_choices_id(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let the_box = ThingsBox { stand_in: StandIn::at(Replies::Never, NOW), seen: Arc::default(), stale: Arc::default() };
    let client = BoxClient::new(the_box.clone());
    let target = Target { section: "things".into(), row: "one".into(), was: "r1".into(), action: "edit".into() };
    let form: Form = serde_json::from_value(json!({ "title": "Edit", "submit": "Save", "fields": [{
        "id": "model", "title": "Model", "kind": "choice", "value": "old-choice", "choices": [
            { "id": "old-choice", "title": "Old Model", "model": { "providerID": "openai", "id": "old" } }
        ]
    }] })).unwrap();
    let window = cx.add_window(move |_, cx| FormSheet::new(client, false, target, form, cx));
    let sheet = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(800.), px(700.)));
    sheet.update(cx, |sheet, cx| sheet.start(cx));
    cx.run_until_parked();
    click(cx, "settings-form-model-menu");
    click(cx, "model-row-openai/new");
    assert_eq!(value(&sheet, cx, "model"), json!("new-choice-id"));
    sheet.update(cx, |sheet, cx| sheet.submit(cx));
    assert_eq!(the_box.bodies("POST", FORM)[0]["values"]["model"], json!("new-choice-id"));
}

#[gpui::test]
fn checks_get_dots_and_a_files_size_and_the_action_gets_what_was_typed(cx: &mut TestAppContext) {
    let (the_box, sheet, cx) = open(cx);
    let check = format!("{FORM}/check");
    // Opens empty, with its placeholder; a file kept before shows its name, with its button.
    assert_eq!(value(&sheet, cx, "password"), json!(""));
    assert!(shown(cx, "settings-form-password-show") && shown(cx, "settings-form-key-file") && shown(cx, "settings-form-key-choose"));
    // (A note's lines show as typed: its eye only reads what's kept, and it keeps none here.)
    assert!(!shown(cx, "settings-form-note-show"));
    click(cx, "settings-form-password-field");
    cx.simulate_input("hunter2é");
    click(cx, "settings-form-note-field");
    cx.simulate_input("my note");
    click(cx, "settings-form-env-add");
    cx.simulate_input("TOKEN");
    click(cx, "settings-form-env-value-0");
    cx.simulate_input("abc123");
    sheet.update(cx, |sheet, cx| sheet.chose_file("key", "id.pem", b"PRIVATE".to_vec(), cx));
    // More than four choices: a menu (further down the sheet, scrolled to as you would).
    scroll_fields(cx);
    assert!(!shown(cx, "settings-form-model-b"));
    click(cx, "settings-form-model-menu");
    click(cx, "menu-item-Epsilon");
    assert_eq!(value(&sheet, cx, "model"), json!("e"));

    let checks = the_box.bodies("POST", &check);
    let last = &checks.last().expect("checked")["values"];
    assert_eq!(last["password"], json!("••••••••"));
    assert_eq!(last["note"], json!("•••••••"));
    assert_eq!(last["env"], json!([{ "key": "TOKEN", "value": "••••••" }]));
    assert_eq!(last["key"], json!({ "name": "id.pem", "size": 7 }));
    assert_eq!(last["model"], json!("e"));
    // No check, as it was typed, ever carried a secret or a file's bytes.
    let every = serde_json::to_string(&checks).unwrap();
    for secret in ["hunter2", "my note", "abc123", "UFJJVkFURQ", "PRIVATE"] {
        assert!(!every.contains(secret), "a check carried {secret}");
    }

    click(cx, "settings-form-submit");
    let sent = the_box.bodies("POST", FORM);
    assert_eq!(sent.len(), 1);
    let values = &sent[0]["values"];
    assert_eq!((values["password"].clone(), values["note"].clone()), (json!("hunter2é"), json!("my note")));
    assert_eq!(values["env"], json!([{ "key": "TOKEN", "value": "abc123" }]));
    assert_eq!(values["key"], json!({ "name": "id.pem", "data": "UFJJVkFURQ==" }));
    // A secret left empty sends nothing new; the box keeps what it has.
    assert_eq!(values["pin"], json!(""));
}

#[gpui::test]
fn a_file_not_chosen_goes_as_it_opened_and_one_too_big_by_its_name_and_size(cx: &mut TestAppContext) {
    let (the_box, sheet, cx) = open(cx);
    click(cx, "settings-form-submit");
    assert_eq!(the_box.bodies("POST", FORM)[0]["values"]["key"], json!({ "name": "old.pem" }));
    sheet.update(cx, |sheet, cx| sheet.chose_file("key", "huge.bin", vec![0u8; 1_000_001], cx));
    cx.run_until_parked();
    assert_eq!(value(&sheet, cx, "key"), json!({ "name": "huge.bin", "size": 1_000_001 }));
}

/// Where the app can't make sure it's you, what a secret keeps isn't read: its eye shows only
/// what's typed, and the sheet says why.
#[gpui::test]
fn show_reads_nothing_kept_where_the_app_cant_make_sure_its_you(cx: &mut TestAppContext) {
    let (the_box, sheet, cx) = open(cx);
    click(cx, "settings-form-password-show");
    assert!(the_box.seen.lock().unwrap().iter().all(|seen| seen.route() != SHOWN), "nothing read");
    assert_eq!(value(&sheet, cx, "password"), json!(""));
    click(cx, "settings-form-pin-field");
    cx.simulate_input("1234");
    click(cx, "settings-form-pin-show");
    assert!(sheet.read_with(cx, |sheet, _| sheet.reveals("pin")));
    assert!(the_box.seen.lock().unwrap().iter().all(|seen| seen.route() != SHOWN && seen.route() != GONE));
}

/// Where it can: Show asks first, then reads the kept value through the vault's own route and
/// keeps only its field's; not you, nothing's read.
#[gpui::test]
fn show_asks_its_you_then_reads_only_its_own_kept_value(cx: &mut TestAppContext) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (the_box, sheet, cx) = open_where(cx, Some((true, asked.clone())));
    click(cx, "settings-form-password-show");
    assert_eq!(asked.lock().unwrap().as_slice(), ["show what “Password” keeps".to_owned()]);
    assert_eq!(the_box.seen.lock().unwrap().iter().filter(|seen| seen.route() == SHOWN).count(), 1);
    assert_eq!(value(&sheet, cx, "password"), json!("kept pass"), "the kept value, and only its own");
    // What's shown is still checked as dots.
    let checks = the_box.bodies("POST", &format!("{FORM}/check"));
    assert!(!serde_json::to_string(&checks).unwrap().contains("kept pass") && !serde_json::to_string(&checks).unwrap().contains("never shown"));
    // Hide: dots again, nothing asked again.
    click(cx, "settings-form-password-show");
    assert!(!sheet.read_with(cx, |sheet, _| sheet.reveals("password")));
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[gpui::test]
fn show_reads_nothing_when_it_isnt_you(cx: &mut TestAppContext) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (the_box, sheet, cx) = open_where(cx, Some((false, asked.clone())));
    click(cx, "settings-form-password-show");
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(the_box.seen.lock().unwrap().iter().all(|seen| seen.route() != SHOWN), "nothing read");
    assert_eq!(value(&sheet, cx, "password"), json!(""));
    assert!(sheet.read_with(cx, |sheet, _| sheet.refused().is_some_and(|said| said.contains("wasn’t confirmed"))));
}

#[gpui::test]
fn a_field_that_makes_sure_its_you_first_says_why_and_goes_as_it_opened(cx: &mut TestAppContext) {
    let (the_box, sheet, cx) = open(cx);
    assert_eq!(sheet.read_with(cx, |sheet, _| sheet.field_why("who")).as_deref(), Some("This needs Touch ID or your computer’s password, which this app can’t ask for here yet."));
    assert_eq!(sheet.read_with(cx, |sheet, _| sheet.field_why("password")), None);
    click(cx, "settings-form-who-field");
    cx.simulate_input("Everyone");
    click(cx, "settings-form-submit");
    assert_eq!(the_box.bodies("POST", FORM)[0]["values"]["who"], json!("Only me"));
}

/// Where the app can ask: a field, or a choice, that lets something use what's yours more asks
/// before it's sent; what doesn't asks nothing; not you, nothing's sent.
#[gpui::test]
fn what_lets_it_be_used_more_asks_its_you_before_its_sent(cx: &mut TestAppContext) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (the_box, sheet, cx) = open_where(cx, Some((false, asked.clone())));
    // Nothing more: sent, nothing asked.
    click(cx, "settings-form-submit");
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(the_box.bodies("POST", FORM).len(), 1);
    // Everyone (a verified choice): asked; not you, not sent.
    sheet.update(cx, |sheet, cx| sheet.pick("share", json!("all"), cx));
    cx.run_until_parked();
    click(cx, "settings-form-submit");
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert_eq!(the_box.bodies("POST", FORM).len(), 1, "not you: nothing sent");
    assert!(sheet.read_with(cx, |sheet, _| sheet.refused().is_some_and(|said| said.contains("wasn’t confirmed"))));
}

/// A form that changed since it opened (another device changed the item meanwhile) isn't sent
/// again from what it opened with, which would put back what changed: it says to open it again.
#[gpui::test]
fn a_form_that_changed_since_it_opened_isnt_sent_again(cx: &mut TestAppContext) {
    let (the_box, sheet, cx) = open(cx);
    the_box.stale.store(true, std::sync::atomic::Ordering::Relaxed);
    click(cx, "settings-form-submit");
    assert!(sheet.read_with(cx, |sheet, _| sheet.is_stale()));
    assert!(sheet.read_with(cx, |sheet, _| sheet.refused().is_some_and(|said| said.contains("open it again"))));
    assert!(!sheet.read_with(cx, |sheet, _| sheet.is_ready()));
    click(cx, "settings-form-submit");
    the_box.stale.store(false, std::sync::atomic::Ordering::Relaxed);
    sheet.update(cx, |sheet, cx| sheet.submit(cx));
    cx.run_until_parked();
    assert_eq!(the_box.bodies("POST", FORM).len(), 1, "sent once: never again from what it opened with");
    // Typing keeps saying so.
    click(cx, "settings-form-pin-field");
    cx.simulate_input("12345");
    assert!(sheet.read_with(cx, |sheet, _| sheet.refused().is_some_and(|said| said.contains("open it again"))));
}
