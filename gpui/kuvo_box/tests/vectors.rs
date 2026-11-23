//! The shared cases every client is held to (protocol/vectors), as KuvoClient's tests
//! hold the Swift apps: what a sidebar shows from a box's session facts, the fields a message is
//! sent with, and that a box of this version passes the gate.

use std::collections::HashMap;
use std::path::PathBuf;

use kuvo_box::gate;
use kuvo_box::sessions::{self, Facts, Line, Listing, Project};
use serde_json::Value;

fn vectors(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/vectors").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        if error.kind() == std::io::ErrorKind::NotFound
            && matches!(name, "routes/features.json" | "routes/shapes/answers.json")
        {
            panic!("Missing {}. Run scripts/route-shapes.sh from the repository root.", path.display());
        }
        panic!("{}: {error}", path.display());
    });
    serde_json::from_str(&text).expect("the vectors are JSON")
}

fn facts_and_projects(case: &Value) -> (HashMap<String, Facts>, HashMap<String, Project>) {
    let sessions: HashMap<String, Facts> = serde_json::from_value(case["expect"]["sessions"].clone()).expect("every fact the box answers is read");
    let projects: HashMap<String, Project> = serde_json::from_value(case["expect"]["projects"].clone()).expect("every project the box answers is read");
    (sessions, projects)
}

#[test]
fn every_case_of_the_box_answer_is_read_whole() {
    let vectors = vectors("session-lists.json");
    let cases = vectors["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let (sessions, _) = facts_and_projects(case);
        // Read back as the box wrote it: nothing it says is lost.
        let again = serde_json::to_value(&sessions).unwrap();
        assert_eq!(numbers_as_floats(again), numbers_as_floats(case["expect"]["sessions"].clone()), "{}", case["name"]);
    }
}

/// "Needs you" and each project's sums are the box's: read as it says them, in its order, with
/// nothing counted or sorted here.
#[test]
fn needs_you_and_each_projects_sums_are_read_as_the_box_says_them() {
    let vectors = vectors("session-lists.json");
    let (mut asking, mut needing) = (0, 0);
    for case in vectors["cases"].as_array().unwrap() {
        let expect = &case["expect"];
        let line: Line = serde_json::from_value(serde_json::json!({ "archivedGroups": [], "at": "1", "full": true, "sessions": expect["sessions"], "projects": expect["projects"], "needsYou": expect["needsYou"] })).unwrap();
        let mut listing = Listing::new();
        listing.take(line);
        let said: Vec<(String, String, String)> = expect["needsYou"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|needs| (needs["session"].as_str().unwrap().to_owned(), needs["asks"]["kind"].as_str().unwrap().to_owned(), needs["asks"]["summary"].as_str().unwrap().to_owned()))
            .collect();
        let read: Vec<(String, String, String)> = listing.needs_you().iter().map(|needs| (needs.session.clone(), needs.asks.kind.clone(), needs.asks.summary.clone())).collect();
        assert_eq!(read, said, "{}", case["name"]);
        needing += read.len();
        for (directory, project) in expect["projects"].as_object().into_iter().flatten() {
            let read = &listing.projects()[directory];
            assert_eq!((read.asking as u64, read.working as u64), (project["asking"].as_u64().unwrap(), project["working"].as_u64().unwrap()), "{}: {directory}", case["name"]);
            asking += read.asking;
        }
    }
    assert!(needing > 0 && asking > 0, "a case with a session waiting on you");
}

/// (Times are numbers: 590 and 590.0 are the same.)
fn numbers_as_floats(value: Value) -> Value {
    match value {
        Value::Number(number) => serde_json::json!(number.as_f64().unwrap()),
        Value::Array(items) => Value::Array(items.into_iter().map(numbers_as_floats).collect()),
        Value::Object(fields) => Value::Object(fields.into_iter().map(|(name, value)| (name, numbers_as_floats(value))).collect()),
        other => other,
    }
}

#[test]
fn the_sidebar_follows_from_the_box_facts_alone() {
    let vectors = vectors("session-lists.json");
    let mut checked = 0;
    for case in vectors["cases"].as_array().unwrap() {
        let Some(expected) = case["expect"].get("sidebar").filter(|sidebar| !sidebar.is_null()) else { continue };
        let (sessions, projects) = facts_and_projects(case);
        let sidebar = sessions::sidebar(&sessions, &projects);
        let ids = |value: &Value| -> Vec<String> { serde_json::from_value(value.clone()).unwrap() };
        assert_eq!(sidebar.pinned, ids(&expected["pinned"]), "pinned: {}", case["name"]);
        assert_eq!(sidebar.chats, ids(&expected["chats"]), "chats: {}", case["name"]);
        let drawn: Vec<(String, Vec<String>)> = sidebar.projects.iter().map(|group| (group.directory.clone(), group.sessions.clone())).collect();
        let wanted: Vec<(String, Vec<String>)> = expected["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| (group["directory"].as_str().unwrap().to_owned(), ids(&group["sessions"])))
            .collect();
        assert_eq!(drawn, wanted, "projects: {}", case["name"]);
        checked += 1;
    }
    assert!(checked > 0, "a case with a sidebar");
}

#[test]
fn a_list_taken_line_by_line_ends_as_the_box_says() {
    let vectors = vectors("session-lists.json");
    let case = &vectors["cases"][0];
    let (sessions, projects) = facts_and_projects(case);
    let whole: Line = serde_json::from_value(serde_json::json!({ "archivedGroups": [], "at": "1", "full": true, "sessions": case["expect"]["sessions"], "projects": case["expect"]["projects"] })).unwrap();
    let mut listing = Listing::new();
    assert!(listing.take(whole));
    assert_eq!(listing.sidebar(), sessions::sidebar(&sessions, &projects));

    // One session changed and one deleted: the rest stay as they were.
    let (changed, gone) = {
        let mut ids: Vec<&String> = sessions.keys().collect();
        ids.sort();
        (ids[0].clone(), ids[1].clone())
    };
    let mut facts = sessions[&changed].clone();
    facts.title = Some("Renamed".into());
    facts.updated += 1.0;
    let line: Line = serde_json::from_value(serde_json::json!({ "archivedGroups": [], "at": "2", "sessions": { changed.clone(): facts }, "gone": [gone.clone()], "projects": case["expect"]["projects"] })).unwrap();
    assert!(listing.take(line));
    assert_eq!(listing.at(), Some("2"));
    assert_eq!(listing.sessions().len(), sessions.len() - 1);
    assert_eq!(listing.sessions()[&changed].title.as_deref(), Some("Renamed"));
    // What it said of the one let go stays at hand for a row still shown as it goes.
    assert_eq!(listing.facts(&gone), Some(&sessions[&gone]));
    assert!(!listing.sessions().contains_key(&gone));

    // The same again changes nothing.
    let same: Line = serde_json::from_value(serde_json::json!({ "archivedGroups": [], "at": "3", "sessions": {}, "projects": case["expect"]["projects"] })).unwrap();
    assert!(!listing.take(same));
    assert_eq!(listing.at(), Some("3"));
}

#[test]
fn a_box_of_this_version_passes_the_gate() {
    let vectors = vectors("routes/features.json");
    let expect = &vectors["scenarios"][0]["steps"][0]["expect"];
    for kind in ["mac", "machine"] {
        // A Mac's names what it includes at least; a machine's, its whole list.
        let said = &expect[kind]["json"]["features"];
        let names: Vec<String> = serde_json::from_value(said.get("<includes>").unwrap_or(said).clone()).unwrap();
        if kind == "machine" {
            assert_eq!(kuvo_box::FEATURES, names, "all advertised machine features");
        }
        let missing: Vec<&str> = gate::REQUIRED.iter().copied().filter(|name| !names.iter().any(|has| has == name)).collect();
        assert_eq!(missing, Vec::<&str>::new(), "{kind}");
    }
}

/// The names in a list kept one a line, in order: each once, sorted, as union merges may leave two
/// branches' names side by side in either order (which changes nothing a client does).
fn names_once(list: Vec<&str>, file: &str) -> Vec<String> {
    let mut names: Vec<String> = list.into_iter().map(str::to_owned).collect();
    names.sort();
    for pair in names.windows(2) {
        assert_ne!(pair[0], pair[1], "{} names {:?} twice: a union merge kept both branches' line", file, pair[0]);
    }
    names
}

/// The protocol names exactly the same required features as the desktop app
/// (protocol/gate-required.txt, which PROTOCOL.md's "The update gate" points to).
#[test]
fn the_gate_matches_the_protocols_update_gate_list() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/gate-required.txt");
    let protocol = std::fs::read_to_string(path).unwrap();
    let names: Vec<&str> = protocol.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).collect();
    assert_eq!(names_once(names, "gate-required.txt"), names_once(gate::REQUIRED.to_vec(), "kuvo_protocol::REQUIRED"));
}

/// A message's parts as every client reads them, kept with a message or read by its box from one
/// from before; and the fields a client sends, as the box takes them (protocol/vectors/message-parts.json).
#[test]
fn a_messages_parts_are_read_as_the_box_wrote_them() {
    let vectors = vectors("message-parts.json");
    let cases: Vec<&Value> = vectors["compose"].as_array().unwrap().iter().chain(vectors["read"].as_array().unwrap()).collect();
    assert!(cases.len() >= 30);
    for case in cases {
        let metadata = serde_json::json!({ "kuvo": { "parts": case["parts"], "other": 1 } });
        let read = kuvo_box::Parts::kept(Some(&metadata)).unwrap_or_else(|| panic!("{}", case["about"]));
        // Read and written again, the same parts.
        assert_eq!(numbers_as_floats(serde_json::to_value(&read).unwrap()), numbers_as_floats(case["parts"].clone()), "{}", case["about"]);
        // And as the box answers `kuvo/messages/parts`.
        let told: kuvo_box::Parts = serde_json::from_value(case["parts"].clone()).unwrap();
        assert_eq!(told, read, "{}", case["about"]);
    }
    // One with none kept is from before.
    assert_eq!(kuvo_box::Parts::kept(Some(&serde_json::json!({ "kuvo": { "wake": true } }))), None);
}

#[test]
fn a_client_sends_the_fields_the_box_takes() {
    use kuvo_box::parts::{Comment, Notes, Sending, Side};
    let vectors = vectors("message-parts.json");
    for case in vectors["compose"].as_array().unwrap() {
        let fields = &case["fields"];
        let strings = |value: &Value| -> Vec<String> { serde_json::from_value(value.clone()).unwrap_or_default() };
        let sending = Sending {
            words: fields["words"].as_str().unwrap().to_owned(),
            widget: fields["widget"].as_str().map(str::to_owned),
            comments: fields["comments"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|each| Comment {
                    file: each["file"].as_str().unwrap().to_owned(),
                    line: each["line"].as_i64().unwrap(),
                    side: if each["side"] == "old" { Side::Old } else { Side::New },
                    snippet: each["snippet"].as_str().unwrap().to_owned(),
                    text: each["text"].as_str().unwrap().to_owned(),
                })
                .collect(),
            named: strings(&fields["named"]),
            notes: Notes { included: strings(&fields["notes"]["included"]), paths: strings(&fields["notes"]["paths"]), unsent: strings(&fields["notes"]["unsent"]) },
            ..Sending::default()
        };
        assert_eq!(numbers_as_floats(serde_json::to_value(&sending).unwrap()), numbers_as_floats(fields.clone()), "{}", case["about"]);
    }
}

/// What a session's model being ready means for a composer, as its box words it
/// (`protocol/vectors/model-readiness.json`, which the box's words and KuvoClient's `SessionModelState` are
/// held to too): read as `kuvo/sessions/models` answers it, and drawn as it says, nothing worded here.
#[test]
fn a_models_readiness_is_drawn_as_its_box_words_it() {
    let vectors = vectors("model-readiness.json");
    let cases = vectors["cases"].as_array().expect("cases");
    assert!(cases.len() >= 10);
    for case in cases {
        let about = case["about"].as_str().unwrap_or_default();
        let mut answer = case["state"].clone();
        let readiness = case["readiness"].as_object().expect("readiness");
        for (key, value) in readiness {
            answer[key] = value.clone();
        }
        let state: kuvo_box::models::ModelState = serde_json::from_value(answer).expect("the box's answer is read");
        assert_eq!(state.refusal.as_deref(), readiness.get("refusal").and_then(Value::as_str), "{about}");
        assert_eq!(state.choose, readiness.get("choose").and_then(Value::as_bool).unwrap_or(false), "{about}");
        let action = readiness.get("action");
        assert_eq!(state.action.as_ref().map(|action| action.kind.as_str()), action.and_then(|action| action["do"].as_str()), "{about}");
        assert_eq!(state.action.as_ref().map(|action| action.label.as_str()), action.and_then(|action| action["label"].as_str()), "{about}");
        assert_eq!(state.action.as_ref().map(|action| action.help.as_str()), action.and_then(|action| action["help"].as_str()), "{about}");
        // A plain message on it is kept, saying why in the box's words, as the box would.
        let kept = kuvo_box::typed::plain_answer("fix it", state.refusal.as_deref());
        assert_eq!(kept.and_then(|answer| answer.error), state.refusal.clone(), "{about}");
    }
}

/// A value of `shape` as the box's route shapes write them (`protocol/vectors/routes/shapes/answers.json`):
/// every field there, optional ones too, and the first of what may be one of several.
fn sample(shape: &Value) -> Value {
    use serde_json::json;
    match shape {
        Value::String(kind) => match kind.as_str() {
            "string" => json!("words"),
            "number" => json!(1),
            "bool" => json!(true),
            _ => Value::Null,
        },
        Value::Object(shape) => {
            if let Some(literal) = shape.get("literal") {
                literal.clone()
            } else if let Some(list) = shape.get("list") {
                json!([sample(list)])
            } else if let Some(one_of) = shape.get("oneOf").and_then(Value::as_array) {
                one_of.iter().find(|each| each.as_str() != Some("null")).map(sample).unwrap_or(Value::Null)
            } else if let Some(fields) = shape.get("object").and_then(Value::as_object) {
                Value::Object(fields.iter().map(|(name, field)| (name.trim_end_matches('?').to_owned(), sample(field))).collect())
            } else {
                Value::Null
            }
        }
        _ => Value::Null,
    }
}

/// Every kind of ask the box words reads as that kind; one missing any field doesn't keep the rest
/// from showing.
#[test]
fn every_ask_the_box_words_is_read_and_one_amiss_spoils_none_of_the_rest() {
    use kuvo_box::asks::{Ask, Asks};
    let answers = vectors("routes/shapes/answers.json");
    let route = answers["routes"].as_array().unwrap().iter().find(|route| route["route"] == "GET kuvo/sessions/:id/asks").expect("the asks route's shape");
    let kinds = route["answers"]["200"]["object"]["asks"]["list"]["oneOf"].as_array().expect("each kind of ask");
    let samples: Vec<Value> = kinds.iter().map(sample).collect();
    assert_eq!(samples.len(), 5, "permission, question, sandbox, vault, other");
    let asks: Asks = serde_json::from_value(serde_json::json!({ "asks": samples })).unwrap();
    assert!(matches!(asks.asks.as_slice(), [Ask::Permission(_), Ask::Question(_), Ask::Sandbox(_), Ask::Vault(_), Ask::Other { .. }]), "{asks:?}");
    for (n, kind) in samples.iter().enumerate() {
        for field in kind.as_object().unwrap().keys().filter(|field| *field != "kind") {
            let mut amiss = samples.clone();
            amiss[n].as_object_mut().unwrap().remove(field);
            let asks: Asks = serde_json::from_value(serde_json::json!({ "asks": amiss })).unwrap();
            assert_eq!(asks.asks.len(), 5);
            let read = asks.asks.iter().filter(|ask| **ask != Ask::Unknown).count();
            assert!(read >= 4, "without {field}, one ask spoiled the others: {asks:?}");
        }
    }
}

/// One item of "Needs you", or a project's sum, that a newer box says in a way this can't read
/// leaves the rest of the list as it is: the sidebar never loses every session to one bad item.
#[test]
fn a_needs_you_item_this_cant_read_leaves_the_list() {
    let line: Line = serde_json::from_value(serde_json::json!({
        "archivedGroups": [], "at": "1", "full": true,
        "sessions": { "ses_a": { "kind": "project", "project": "/p", "directory": "/p", "created": 1, "updated": 2, "asked": 3 } },
        "projects": { "/p": { "asking": "one", "working": 1 } },
        "needsYou": [{ "asks": "words" }, { "session": "ses_a", "asks": { "kind": "newer", "summary": "asks for a thing" } }],
    }))
    .expect("the line is read");
    assert_eq!(line.needs_you.len(), 1);
    assert_eq!((line.needs_you[0].session.as_str(), line.needs_you[0].asks.summary.as_str()), ("ses_a", "asks for a thing"));
    assert_eq!((line.projects["/p"].asking, line.projects["/p"].working), (0, 1));
    assert_eq!(line.sessions.len(), 1);
}

#[test]
fn archived_groups_are_required_and_group_only_changes_are_observed() {
    use kuvo_box::sessions::{Line, Listing};
    let mut value = serde_json::json!({"at":"1", "full":true, "sessions":{}, "projects":{}});
    assert!(serde_json::from_value::<Line>(value.clone()).is_err());
    value["archivedGroups"] = serde_json::json!([]);
    let mut listing = Listing::default();
    assert!(listing.take(serde_json::from_value(value.clone()).unwrap()));
    assert!(!listing.take(serde_json::from_value(value.clone()).unwrap()));
    value["archivedGroups"] = serde_json::json!([{"directory":"/project", "title":"Project", "kind":"project", "sessions":["ses_a"]}]);
    assert!(listing.take(serde_json::from_value(value.clone()).unwrap()));
    assert_eq!(listing.archived_groups()[0].sessions, vec!["ses_a"]);
    assert!(!listing.take(serde_json::from_value(value).unwrap()));
}
