//! The shape checker and typed answers use the same fixtures as the JavaScript contract runner.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{FEATURES, REQUIRED, check_answer, check_shape, gate, route, routes};

const SHAPES: &str = include_str!("../../../test/fixtures/routes/shapes/cases.json");
const ANSWERS: &str = include_str!("../../../test/fixtures/routes/shapes/routes.json");
const EXPORT: &str = include_str!("../../../../protocol/vectors/routes/shapes/answers.json");

#[derive(Deserialize)]
struct Cases<T> {
    cases: Vec<T>,
}

#[test]
fn the_shape_checker_agrees_with_the_javascript_cases() {
    #[derive(Deserialize)]
    struct Case {
        shape: Value,
        matches: bool,
    }

    let cases: Cases<Value> = serde_json::from_str(SHAPES).unwrap();
    assert!(!cases.cases.is_empty());
    for raw in cases.cases {
        let case: Case = serde_json::from_value(raw.clone()).unwrap();
        let result = serde_json::from_value(case.shape)
            .ok()
            .is_some_and(|shape| check_shape(&shape, raw.get("value")).is_ok());
        assert_eq!(result, case.matches, "{raw}");
    }
}

#[derive(Deserialize)]
struct AnswerCase {
    send: String,
    status: u16,
    #[serde(rename = "type")]
    content_type: String,
    body: String,
    matches: bool,
}

fn answer_cases() -> Vec<AnswerCase> {
    serde_json::from_str::<Cases<AnswerCase>>(ANSWERS)
        .unwrap()
        .cases
}

#[test]
fn the_answer_checker_agrees_with_the_javascript_cases() {
    let cases = answer_cases();
    assert!(!cases.is_empty());
    for case in cases {
        let (method, path) = case.send.split_once(' ').unwrap();
        let result = check_answer(
            method,
            path,
            case.status,
            &case.content_type,
            case.body.as_bytes(),
        );
        assert_eq!(
            result.is_ok(),
            case.matches,
            "{} {} {}: {result:?}",
            case.send,
            case.status,
            case.body
        );
    }
}

#[test]
fn the_vectors_json_answers_round_trip_through_the_route_types() {
    let mut checked = 0;
    for case in answer_cases() {
        if !case.matches
            || !case.content_type.starts_with("application/json")
            || case.body.is_empty()
        {
            continue;
        }
        let (method, path) = case.send.split_once(' ').unwrap();
        let selected = route(method, path);
        if selected.is_none() && case.status < 400 {
            continue; // Passed-on answers have no Kuvo type.
        }
        let (method, path) = selected.map_or((method, path), |route| (route.method, route.path));
        let value: Value = serde_json::from_str(&case.body).unwrap();
        let encoded = round_trip_sample(method, path, case.status, value.clone()).unwrap();
        assert_eq!(encoded, value, "{} {}", case.send, case.status);
        checked += 1;
    }
    assert!(checked >= 8);
}

#[test]
fn every_exported_route_and_status_has_a_rust_type() {
    let exported: Value = serde_json::from_str(EXPORT).unwrap();
    let exported = exported["routes"].as_array().unwrap();
    assert_eq!(routes::ROUTES.len(), exported.len());
    for (entry, route) in exported.iter().zip(routes::ROUTES) {
        assert_eq!(entry["route"], format!("{} {}", route.method, route.path));
        assert_eq!(entry["feature"], route.feature);
        let answers = entry["answers"].as_object().unwrap();
        assert_eq!(route.answers.len(), answers.len(), "{}", route.path);
        for (code, _) in route.answers {
            assert!(
                answers.contains_key(&code.to_string()),
                "{} {code}",
                route.path
            );
        }
        assert_eq!(route.body.is_some(), entry.get("body").is_some());
    }
}

// Select the generated types independently for the complete answers in the shared fixtures.
fn round_trip_sample(
    method: &str,
    path: &str,
    status: u16,
    value: Value,
) -> serde_json::Result<Value> {
    use routes::{
        delete_kuvo_projects, get_kuvo_wakes, get_kuvo_wakes_session, post_kuvo_engine_restart,
    };

    fn round_trip<T: DeserializeOwned + Serialize>(value: Value) -> serde_json::Result<Value> {
        serde_json::to_value(serde_json::from_value::<T>(value)?)
    }

    match (method, path, status) {
        ("GET", "kuvo/wakes", 200) => round_trip::<get_kuvo_wakes::Status200>(value),
        ("GET", "kuvo/wakes/:session", 200) => {
            round_trip::<get_kuvo_wakes_session::Status200>(value)
        }
        ("DELETE", "kuvo/projects", 409) => round_trip::<delete_kuvo_projects::Status409>(value),
        ("POST", "kuvo/engine/restart", 202) => {
            round_trip::<post_kuvo_engine_restart::Status202>(value)
        }
        _ if status >= 400
            && route(method, path)
                .and_then(|route| route.answer(status))
                .is_none() =>
        {
            round_trip::<crate::Refusal>(value)
        }
        _ => panic!("select a generated answer type for {method} {path} {status}"),
    }
}

#[test]
fn the_update_gate_uses_the_reported_features() {
    let features = |omit| {
        FEATURES
            .iter()
            .copied()
            .filter(|name| *name != omit)
            .collect::<Vec<_>>()
    };
    for required in REQUIRED {
        assert!(
            FEATURES.contains(required),
            "missing advertised feature {required}"
        );
    }
    assert_eq!(gate::compatibility(FEATURES), gate::Compatibility::Open);
    assert_eq!(
        gate::compatibility(&features("bots.state")),
        gate::Compatibility::Behind {
            missing: vec!["bots.state".into()]
        },
    );
    assert_eq!(
        gate::compatibility(&features("messages.send")),
        gate::Compatibility::Held {
            missing: vec!["messages.send".into()]
        },
    );
}

#[test]
fn a_typed_route_preserves_json_objects_and_distinguishes_them_from_arrays() {
    type Wakes = routes::get_kuvo_wakes::Status200;
    let object = json!({"sessions": []});
    let value: Wakes = serde_json::from_value(object.clone()).unwrap();
    assert_eq!(serde_json::to_value(value).unwrap(), object);
    assert!(serde_json::from_value::<Wakes>(json!([[]])).is_err());
}
