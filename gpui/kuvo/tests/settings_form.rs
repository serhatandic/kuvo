//! A box's form with what's kept secret, filled in through the window: a secret hidden as it's
//! typed, a note's lines, names whose values are hidden, a file of this computer's and a pick of
//! many. Each check the box gets carries dots and a file's name and size, never what was typed or
//! the file's bytes; what's sent with the action is what was typed. Show reads a secret's kept value
//! from the box only when asked, and hides it again. (Nothing here knows what the form is for: the
//! fields are made up, as a box could describe them.)

use std::sync::{Arc, Mutex};

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, px, size};
use kuvo::settings::form::{FormSheet, Target};
use kuvo::stand_in::{Replies, StandIn};
use kuvo_box::BoxClient;
use kuvo_box::settings_pages::Form;
use kuvo_box::transport::{Closer, Request, Response, Stream, Transport};
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

const NOW: f64 = 1_794_650_000_000.0;
const FORM: &str = "kuvo/settings/things/one/edit";
const SHOWN: &str = "kuvo/things/one/shown";

/// The stand-in box (its `kuvo/info`, so the client's gate opens), answering this form's routes
/// itself and keeping every request it's sent.
#[derive(Clone)]
struct ThingsBox {
    stand_in: StandIn,
    seen: Arc<Mutex<Vec<Request>>>,
}

fn answer(status: u16, body: Value) -> kuvo_box::Result<Response> {
    Ok(Response { status, content_type: Some("application/json".into()), body: serde_json::to_vec(&body).unwrap() })
}

impl Transport for ThingsBox {
    fn send(&self, request: &Request) -> kuvo_box::Result<Response> {
        self.seen.lock().unwrap().push(request.clone());
        match (request.method.as_str(), request.route()) {
            ("POST", route) if route == format!("{FORM}/check") => answer(200, json!({ "fields": [], "ready": true })),
            ("POST", FORM) => answer(200, json!({ "id": "things", "title": "Things", "groups": [] })),
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
        { "id": "pin", "title": "PIN", "kind": "secret", "placeholder": "Unchanged", "shows": { "route": "kuvo/things/one/gone", "field": "pin" } },
        { "id": "note", "title": "Note", "kind": "secret", "multi": true, "placeholder": "Unchanged" },
        { "id": "env", "title": "Values", "kind": "pairs", "secret": true, "pairs": { "key": "NAME", "value": "value" } },
        { "id": "key", "title": "Key file", "kind": "file", "value": { "name": "old.pem" }, "file": { "choose": "Choose…", "none": "No file" } },
        { "id": "who", "title": "Who can use it", "kind": "text", "value": "Only me", "verified": true },
        { "id": "model", "title": "Model", "kind": "choice", "value": "a", "choices": [
            { "id": "a", "title": "Alpha" }, { "id": "b", "title": "Beta" }, { "id": "c", "title": "Gamma" }, { "id": "d", "title": "Delta" }, { "id": "e", "title": "Epsilon" }
        ] }
    ] }))
    .unwrap()
}

fn open(cx: &mut TestAppContext) -> (ThingsBox, Entity<FormSheet>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let the_box = ThingsBox { stand_in: StandIn::at(Replies::Never, NOW), seen: Arc::default() };
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

fn value(sheet: &Entity<FormSheet>, cx: &mut VisualTestContext, field: &str) -> Value {
    sheet.read_with(cx, |sheet, cx| sheet.values(cx).get(field).cloned().unwrap_or_default())
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
    // More than four choices: a menu.
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

/// What a secret keeps isn't read here (that asks a client to make sure it's you first, which this
/// app can't yet): its eye shows only what's typed, and the sheet says where to see what's kept.
#[gpui::test]
fn show_reads_nothing_kept_and_shows_only_whats_typed(cx: &mut TestAppContext) {
    let (the_box, sheet, cx) = open(cx);
    click(cx, "settings-form-password-show");
    assert!(the_box.seen.lock().unwrap().iter().all(|seen| seen.route() != SHOWN), "nothing read");
    assert_eq!(value(&sheet, cx, "password"), json!(""));
    click(cx, "settings-form-pin-field");
    cx.simulate_input("1234");
    click(cx, "settings-form-pin-show");
    assert!(sheet.read_with(cx, |sheet, _| sheet.reveals("pin")));
    assert!(the_box.seen.lock().unwrap().iter().all(|seen| seen.route() != SHOWN && seen.route() != "kuvo/things/one/gone"));
}

#[gpui::test]
fn a_field_that_makes_sure_its_you_first_says_why_and_goes_as_it_opened(cx: &mut TestAppContext) {
    let (the_box, sheet, cx) = open(cx);
    assert_eq!(sheet.read_with(cx, |sheet, _| sheet.field_why("who")).as_deref(), Some("Do this in Kuvo on your iPhone, which makes sure it’s you first: this app can’t yet."));
    assert_eq!(sheet.read_with(cx, |sheet, _| sheet.field_why("password")), None);
    click(cx, "settings-form-who-field");
    cx.simulate_input("Everyone");
    click(cx, "settings-form-submit");
    assert_eq!(the_box.bodies("POST", FORM)[0]["values"]["who"], json!("Only me"));
}
