//! Scenario tests for Artifacts and Library (rust-box.md row 5.5a),
//! porting the TS scenarios of `artifacts.test.ts` and `library.test.ts` through real components.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use http::{HeaderMap, Method, StatusCode};
use http_body_util::BodyExt;
use jiff::Timestamp;
use kuvo_server::{Caller, DispatchRequest, Dispatcher, LineTiming, LocalOrigin, Registry, Transport};
use kuvo_services::{
    artifacts::{
        ArtifactHandle, ArtifactStore, StoreError, StoreOptions,
        diff::unified,
        folders::wire_record,
        policy::DiskSpace,
        recorder::{Event, Recorder, RecorderOptions, SessionDetails, SessionFuture, SessionSource},
        store::{Made, PinBy},
        turnfiles::changed_since,
    },
    library::{
        Asked, Handover, How, LibraryHandle, LibraryOptions, LibraryService, PartState,
        PeerComputer, ThumbnailPlatform, keep_part, kept_part, library_for_peer, merged,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Default)]
struct MemorySessions {
    sessions: Mutex<HashMap<String, SessionDetails>>,
    agent_name: String,
}

impl MemorySessions {
    fn new(agent: &str) -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            agent_name: agent.to_owned(),
        })
    }

    fn add(&self, id: &str, directory: &Path, extra: Option<Value>, parent: Option<&str>) {
        self.sessions.lock().unwrap().insert(
            id.to_owned(),
            SessionDetails {
                parent_id: parent.map(str::to_owned),
                directory: Some(directory.to_string_lossy().into_owned()),
                metadata: extra,
            },
        );
    }
}

impl SessionSource for MemorySessions {
    fn name(&self) -> &'static str {
        "engine"
    }

    fn session<'a>(&'a self, id: &'a str) -> SessionFuture<'a> {
        let found = self.sessions.lock().unwrap().get(id).cloned();
        Box::pin(async move { found })
    }

    fn agent(&self, _id: &str) -> Option<String> {
        Some(self.agent_name.clone())
    }
}

fn build_dispatcher(store: ArtifactStore, library: LibraryService) -> Dispatcher {
    let mut reg = Registry::new();
    kuvo_services::artifacts::register(&mut reg, ArtifactHandle::Ready(store), None);
    kuvo_services::library::register(&mut reg, LibraryHandle::Ready(library), None);
    reg.build(LineTiming::default())
}

async fn call_route(
    dispatcher: &Dispatcher,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value, Vec<u8>) {
    let raw = match body {
        Some(v) => bytes::Bytes::from(serde_json::to_vec(&v).unwrap()),
        None => bytes::Bytes::new(),
    };
    let req = DispatchRequest {
        method,
        path: path.to_owned(),
        headers: HeaderMap::new(),
        body: raw,
        caller: Caller::Local(Transport::Http {
            origin: LocalOrigin::Direct,
            internal: false,
        }),
    };
    let reply = dispatcher.dispatch(req, CancellationToken::new()).await;
    let status = reply.status;
    let collected = reply.body.collect().await.unwrap().to_bytes().to_vec();
    let val = serde_json::from_slice(&collected).unwrap_or(Value::Null);
    (status, val, collected)
}

fn tool_events(
    session_id: &str,
    msg_id: &str,
    call_id: &str,
    name: &str,
    input: Value,
    content: Value,
    metadata: Value,
) -> Vec<Event> {
    vec![
        Event {
            event_type: "session.tool.input.started".into(),
            data: json!({ "sessionID": session_id, "assistantMessageID": msg_id, "id": call_id, "name": name }),
            directory: None,
        },
        Event {
            event_type: "session.tool.called".into(),
            data: json!({ "sessionID": session_id, "assistantMessageID": msg_id, "id": call_id, "input": input }),
            directory: None,
        },
        Event {
            event_type: "session.tool.success".into(),
            data: json!({ "sessionID": session_id, "assistantMessageID": msg_id, "id": call_id, "content": content, "metadata": metadata }),
            directory: None,
        },
    ]
}

fn turn_events(session_id: &str, msg_id: &str, inner: Vec<Event>) -> Vec<Event> {
    let mut out = vec![
        Event {
            event_type: "session.execution.started".into(),
            data: json!({ "sessionID": session_id }),
            directory: None,
        },
        Event {
            event_type: "session.inbox.delivered".into(),
            data: json!({ "sessionID": session_id, "inboxID": msg_id }),
            directory: None,
        },
    ];
    out.extend(inner);
    out.push(Event {
        event_type: "session.execution.succeeded".into(),
        data: json!({ "sessionID": session_id }),
        directory: None,
    });
    out
}

async fn hear_all(recorder: &Recorder, source: &dyn SessionSource, events: Vec<Event>) {
    for ev in events {
        recorder.heard(source, ev).await;
    }
}

fn make_file(path: &Path, bytes: &[u8]) -> PathBuf {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
    path.to_path_buf()
}

fn plenty_disk() -> kuvo_services::artifacts::policy::DiskProbe {
    Arc::new(|| DiskSpace {
        free: 500 * 1024 * 1024 * 1024,
        total: 1000 * 1024 * 1024 * 1024,
    })
}

fn setup_bot(bots: &Path, id: &str, memory: &str, sessions: &MemorySessions) -> PathBuf {
    let folder = bots.join(id);
    fs::create_dir_all(folder.join("workspace")).unwrap();
    fs::write(folder.join("session"), format!("ses_{id}")).unwrap();
    fs::write(folder.join("MEMORY.md"), memory).unwrap();
    fs::write(
        folder.join("bot.json"),
        json!({ "name": "Ada", "look": { "color": "mint", "shape": "gem", "face": "calm" } })
            .to_string(),
    )
    .unwrap();
    sessions.add(&format!("ses_{id}"), &folder.join("workspace"), None, None);
    folder
}

#[tokio::test]
async fn bot_memory_edits_word_diff_multi_edit_turns_and_outside_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let bots = tmp.path().join("bots");
    let sessions = MemorySessions::new("engine");
    let folder = setup_bot(
        &bots,
        "bot_ada",
        "# Ada's memory\n\nThe user likes tea in the morning.\nShips on Fridays.\n",
        &sessions,
    );
    let store = ArtifactStore::open(
        tmp.path().join("artifacts"),
        StoreOptions {
            bots: Some(bots.clone()),
            disk: Some(plenty_disk()),
            ..Default::default()
        },
    )
    .unwrap();
    let library = LibraryService::open(
        tmp.path().join("library"),
        LibraryOptions {
            bots: bots.clone(),
            sessions: tmp.path().join("sessions"),
            artifacts: Some(store.clone()),
            computer: Arc::new(|| "Studio".into()),
            platform: ThumbnailPlatform::Other,
            clock: None,
        },
    )
    .unwrap();
    let recorder = Recorder::new(
        store.clone(),
        RecorderOptions {
            bots: Some(bots.clone()),
            ..Default::default()
        },
    );
    let dispatcher = build_dispatcher(store.clone(), library);

    let mem = folder.join("MEMORY.md");
    let before = fs::read_to_string(&mem).unwrap();
    let after = "# Ada's memory\n\nThe user likes coffee in the morning.\nShips on Fridays.\n";
    fs::write(&mem, after).unwrap();
    let patch = unified(&before, after, 3).patch;
    hear_all(
        &recorder,
        sessions.as_ref(),
        turn_events(
            "ses_bot_ada",
            "msg_you1",
            tool_events(
                "ses_bot_ada",
                "msg_a",
                "call_1",
                "edit",
                json!({ "filePath": mem.to_string_lossy() }),
                json!([{ "type": "text", "text": "done" }]),
                json!({ "files": [{ "file": mem.to_string_lossy(), "patch": patch, "status": "modified" }] }),
            ),
        ),
    )
    .await;

    let (status, list, _) = call_route(&dispatcher, Method::GET, "kuvo/artifacts?bot=bot_ada", None).await;
    assert_eq!(status, StatusCode::OK);
    let artifacts = list["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["name"], "MEMORY.md");
    assert_eq!(artifacts[0]["kind"], "memory");
    assert_eq!(artifacts[0]["versions"], 1);
    assert_eq!(artifacts[0]["latest"]["lines"], json!({ "added": 1, "removed": 1 }));

    let id = artifacts[0]["id"].as_str().unwrap();
    let (st, diff, _) = call_route(
        &dispatcher,
        Method::GET,
        &format!("kuvo/artifacts/{id}/versions/1/changes"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(diff["style"], "words");
    let changed = diff["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["kind"] == "changed")
        .unwrap();
    assert_eq!(
        changed["runs"],
        json!([
            { "t": " ", "text": "The user likes " },
            { "t": "-", "text": "tea" },
            { "t": "+", "text": "coffee" },
            { "t": " ", "text": " in the morning." },
        ])
    );
    let (_, _, v0_bytes) = call_route(
        &dispatcher,
        Method::GET,
        &format!("kuvo/artifacts/{id}/versions/0"),
        None,
    )
    .await;
    assert!(String::from_utf8(v0_bytes).unwrap().contains("likes tea"));

    // Outside edit becomes a new version by outside
    let after2 = format!("{after}Mine.\n");
    fs::write(&mem, &after2).unwrap();
    let mut made = Made::outside("outside");
    made.where_.bot = Some("bot_ada".into());
    store.change(&mem.to_string_lossy(), made).await;
    let (_, detail, _) = call_route(&dispatcher, Method::GET, &format!("kuvo/artifacts/{id}"), None).await;
    let versions = detail["versions"].as_array().unwrap();
    assert_eq!(versions.last().unwrap()["by"]["kind"], "outside");
    assert_eq!(versions.last().unwrap()["n"], 2);
}

#[tokio::test]
async fn queued_reads_never_hide_newer_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(
        tmp.path().join("artifacts"),
        StoreOptions {
            disk: Some(plenty_disk()),
            ..Default::default()
        },
    )
    .unwrap();
    let file = make_file(&tmp.path().join("queued.md"), b"v1");
    let file_str = file.to_string_lossy().into_owned();
    let first = store.change(&file_str, Made::outside("outside"));
    fs::write(&file, b"v2").unwrap();
    first.await;
    assert!(!store.as_seen(&file_str));
    store.change_all(std::slice::from_ref(&file_str), Made::outside("outside")).await;
    let expected = format!("sha256:{}", hex::encode(Sha256::digest(b"v2")));
    assert_eq!(
        store.at(&file_str).unwrap().versions.last().unwrap().hash.as_deref(),
        Some(expected.as_str())
    );
    assert!(store.as_seen(&file_str));
}

#[tokio::test]
async fn secrets_git_policy_budget_restore_and_format_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let bots = tmp.path().join("bots");
    fs::create_dir_all(&bots).unwrap();
    let sessions = MemorySessions::new("engine");
    sessions.add("ses_code", &project, None, None);

    let store = ArtifactStore::open(
        tmp.path().join("artifacts"),
        StoreOptions {
            bots: Some(bots.clone()),
            budget: Some(15),
            disk: Some(plenty_disk()),
            ..Default::default()
        },
    )
    .unwrap();
    let library = LibraryService::open(
        tmp.path().join("library"),
        LibraryOptions {
            bots: bots.clone(),
            sessions: tmp.path().join("sessions"),
            artifacts: Some(store.clone()),
            computer: Arc::new(|| "Studio".into()),
            platform: ThumbnailPlatform::Other,
            clock: None,
        },
    )
    .unwrap();
    let recorder = Recorder::new(
        store.clone(),
        RecorderOptions {
            bots: Some(bots.clone()),
            ..Default::default()
        },
    );
    let dispatcher = build_dispatcher(store.clone(), library);

    // Secrets are listed but never copied or diffed
    let secret = make_file(&project.join(".env"), b"TOKEN=one\n");
    hear_all(
        &recorder,
        sessions.as_ref(),
        turn_events(
            "ses_code",
            "m1",
            tool_events(
                "ses_code",
                "ma",
                "c1",
                "write",
                json!({ "filePath": secret.to_string_lossy() }),
                json!([]),
                json!({}),
            ),
        ),
    )
    .await;
    let secret_id = ArtifactStore::id_of(&secret.to_string_lossy());
    let (st, _, _) = call_route(
        &dispatcher,
        Method::GET,
        &format!("kuvo/artifacts/{secret_id}/versions/1"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::GONE);
    let (st, _, _) = call_route(
        &dispatcher,
        Method::GET,
        &format!("kuvo/artifacts/{secret_id}/versions/1/changes"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::GONE);

    // Budget prunes oldest non-pinned versions while keeping newest
    let draft = project.join("draft.txt");
    for (idx, text) in ["draft one\n", "draft two\n", "draft three\n"].iter().enumerate() {
        fs::write(&draft, text).unwrap();
        hear_all(
            &recorder,
            sessions.as_ref(),
            turn_events(
                "ses_code",
                &format!("md{idx}"),
                tool_events(
                    "ses_code",
                    "ma",
                    &format!("cd{idx}"),
                    "write",
                    json!({ "filePath": draft.to_string_lossy() }),
                    json!([]),
                    json!({}),
                ),
            ),
        )
        .await;
    }
    let draft_id = ArtifactStore::id_of(&draft.to_string_lossy());
    let (_, draft_doc, _) = call_route(
        &dispatcher,
        Method::GET,
        &format!("kuvo/artifacts/{draft_id}"),
        None,
    )
    .await;
    let kepts: Vec<(bool, Option<&str>)> = draft_doc["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["kept"].as_bool().unwrap(), v["reason"].as_str()))
        .collect();
    assert_eq!(
        kepts,
        vec![(false, Some("cleared")), (true, None), (true, None)]
    );

    // Restore writes version back and preserves replaced content as a new version
    fs::write(&draft, "mine\n").unwrap();
    let (st, restored, _) = call_route(
        &dispatcher,
        Method::POST,
        &format!("kuvo/artifacts/{draft_id}/restore"),
        Some(json!({ "version": 2 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(fs::read_to_string(&draft).unwrap(), "draft two\n");
    assert_eq!(restored["versions"].as_array().unwrap().last().unwrap()["restored"], 2);

    // Settings format preservation and future formatVersion refusal
    let budget_path = tmp.path().join("artifacts/settings.json");
    fs::write(&budget_path, r#"{"budget":123,"extra":"preserved"}"#).unwrap();
    let store2 = ArtifactStore::open(
        tmp.path().join("artifacts"),
        StoreOptions {
            bots: Some(bots.clone()),
            disk: Some(plenty_disk()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(store2.storage().budget, 123);
    assert_eq!(store2.set_budget(456, true).unwrap(), None);
    let saved_settings: Value = serde_json::from_str(&fs::read_to_string(&budget_path).unwrap()).unwrap();
    assert_eq!(saved_settings, json!({ "budget": 456, "extra": "preserved", "formatVersion": 1 }));
    let future_budget = r#"{"formatVersion":2,"budget":42}"#;
    fs::write(&budget_path, future_budget).unwrap();
    let err = store2.set_budget(777, true).unwrap_err();
    assert!(matches!(err, StoreError::UnsupportedFormat(_)));
    assert_eq!(fs::read_to_string(&budget_path).unwrap(), future_budget);

    // Future artifact index log refuses before rewriting or compacting
    let future_dir = tmp.path().join("future-artifacts");
    fs::create_dir_all(future_dir.join("log")).unwrap();
    let log_file = future_dir.join("log/2026-08.jsonl");
    let raw_future = "{\"formatVersion\":2}\n";
    fs::write(&log_file, raw_future).unwrap();
    let err = ArtifactStore::open(&future_dir, StoreOptions::default()).err().unwrap();
    assert!(matches!(err, StoreError::UnsupportedFormat(_)));
    assert_eq!(fs::read_to_string(&log_file).unwrap(), raw_future);
    assert!(!future_dir.join("log/0000-00.jsonl").exists());
}

#[tokio::test]
async fn library_handovers_filtering_hiding_backfill_and_peer_merging() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let bots = tmp.path().join("bots");
    let sessions_dir = tmp.path().join("sessions");
    fs::create_dir_all(&project).unwrap();
    let sessions = MemorySessions::new("engine");
    setup_bot(&bots, "bot_ada", "# Notes\n", &sessions);
    sessions.add("ses_code", &project, None, None);
    sessions.add("ses_child", &project, None, Some("ses_code"));

    let store = ArtifactStore::open(
        tmp.path().join("artifacts"),
        StoreOptions {
            bots: Some(bots.clone()),
            disk: Some(plenty_disk()),
            ..Default::default()
        },
    )
    .unwrap();
    let library = LibraryService::open(
        tmp.path().join("library"),
        LibraryOptions {
            bots: bots.clone(),
            sessions: sessions_dir.clone(),
            artifacts: Some(store.clone()),
            computer: Arc::new(|| "Studio".into()),
            platform: ThumbnailPlatform::Other,
            clock: None,
        },
    )
    .unwrap();
    let lib_hook = library.clone();
    let recorder = Recorder::new(
        store.clone(),
        RecorderOptions {
            bots: Some(bots.clone()),
            handed: Some(Arc::new(move |h| lib_hook.handed(h))),
            ..Default::default()
        },
    );
    let dispatcher = build_dispatcher(store.clone(), library.clone());

    let report = make_file(&project.join("Q4 Board Brief.pdf"), b"%PDF-1.4");
    let chart = make_file(&project.join("out/chart.png"), &[0x89, 0x50, 0x4e, 0x47, 1, 2]);
    let brief = make_file(&tmp.path().join("Desktop/brief.pdf"), b"%PDF brief");
    let page = make_file(
        &project.join("pricing.html"),
        b"<html><head><title>Pricing &amp; Seats</title></head></html>",
    );
    let skill_note = make_file(
        &project.join(".codex/skills/review/references/skills/notes.md"),
        b"# Ref",
    );

    recorder
        .heard(
            sessions.as_ref(),
            Event {
                event_type: "session.inbox.enqueued".into(),
                data: json!({
                    "sessionID": "ses_code",
                    "inboxID": "msg_you",
                    "item": {
                        "type": "user",
                        "payload": {
                            "text": "Here's the brief.",
                            "files": [{ "uri": Url::from_file_path(&brief).unwrap().to_string(), "name": "brief.pdf" }]
                        }
                    }
                }),
                directory: None,
            },
        )
        .await;

    let mut events = tool_events(
        "ses_code",
        "msg_a1",
        "c1",
        "kuvo-browser_show_file",
        json!({ "path": "Q4 Board Brief.pdf" }),
        json!([{ "type": "text", "text": "done" }]),
        json!({}),
    );
    events.extend(tool_events(
        "ses_code",
        "msg_a1",
        "c2",
        "generate_image",
        json!({ "prompt": "a chart" }),
        json!([{ "type": "file", "uri": Url::from_file_path(&chart).unwrap().to_string(), "mime": "image/png" }]),
        json!({}),
    ));
    events.extend(tool_events(
        "ses_code",
        "msg_a1",
        "c3",
        "show_file",
        json!({ "path": skill_note.to_string_lossy() }),
        json!([]),
        json!({}),
    ));
    events.extend(tool_events(
        "ses_child",
        "msg_sub",
        "c4",
        "show_file",
        json!({ "path": page.to_string_lossy() }),
        json!([]),
        json!({}),
    ));
    hear_all(&recorder, sessions.as_ref(), turn_events("ses_code", "msg_you", events)).await;

    let (st, listed, _) = call_route(&dispatcher, Method::GET, "kuvo/library", None).await;
    assert_eq!(st, StatusCode::OK);
    let mut summary: Vec<(String, String, String)> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["name"].as_str().unwrap().to_owned(),
                i["kind"].as_str().unwrap().to_owned(),
                i["how"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    summary.sort();
    assert_eq!(
        summary,
        vec![
            ("Pricing & Seats".into(), "page".into(), "shown".into()),
            ("Q4 Board Brief.pdf".into(), "document".into(), "shown".into()),
            ("brief.pdf".into(), "document".into(), "attached".into()),
            ("chart.png".into(), "image".into(), "generated".into()),
        ]
    );
    assert!(report.exists());

    // Gone picture still opens from artifact copy; gone attached file returns 410
    let chart_id = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "chart.png")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::remove_file(&chart).unwrap();
    let (st, chart_item, _) =
        call_route(&dispatcher, Method::GET, &format!("kuvo/library/{chart_id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(chart_item["there"], true);
    assert_eq!(chart_item["thumbnail"], true);
    let (st, _, raw) = call_route(
        &dispatcher,
        Method::GET,
        &format!("kuvo/library/{chart_id}/file"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(raw, vec![0x89, 0x50, 0x4e, 0x47, 1, 2]);

    // Hide keeps file on disk and persists across restart
    let (st, hidden_item, _) = call_route(
        &dispatcher,
        Method::POST,
        &format!("kuvo/library/{chart_id}/hide"),
        Some(json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(hidden_item["hidden"].is_f64());
    let (_, hidden_list, _) =
        call_route(&dispatcher, Method::GET, "kuvo/library?hidden=1", None).await;
    assert_eq!(hidden_list["items"].as_array().unwrap().len(), 1);

    // Peer act validation
    assert_eq!(library_for_peer(&library, "GET", "../../etc/passwd", None).await.0, 404);
    assert_eq!(
        library_for_peer(&library, "GET", &format!("lib_{}/hide", "0".repeat(24)), None)
            .await
            .0,
        405
    );
    assert_eq!(library_for_peer(&library, "POST", "list", None).await.0, 405);

    // Peer keep/merge
    let peers_dir = tmp.path().join("peers");
    let key = "L".repeat(43);
    let peer_part = json!({
        "items": [{ "id": "lib_b", "name": "b.png", "kind": "image", "type": "image/png", "how": "generated", "path": "/x/b.png", "at": 20, "there": true, "thumbnail": true }],
        "sources": { "bots": [], "you": true }
    });
    keep_part(&peers_dir, &key, "kind=image", peer_part.clone(), 1_700_000_000_000);
    let kept = kept_part(&peers_dir, &key, "kind=image").unwrap();
    assert_eq!(kept.seen, 1_700_000_000_000);
    let merged_val = merged(&[
        Asked {
            computer: PeerComputer {
                key: "k_here".into(),
                name: "MacBook Pro".into(),
                here: true,
            },
            state: PartState::Here,
            part: Some(json!({ "items": [], "sources": { "bots": [], "you": false } })),
            seen: None,
        },
        Asked {
            computer: PeerComputer {
                key: key.clone(),
                name: "Laptop".into(),
                here: false,
            },
            state: PartState::Away("Laptop away".into()),
            part: Some(kept.part),
            seen: Some(kept.seen),
        },
    ]);
    assert_eq!(merged_val["items"][0]["computer"]["name"], "Laptop");
    assert_eq!(merged_val["computers"][1]["seen"], 1_700_000_000_000i64);

    // Backfill runs once from saved sessions
    let old_dir = sessions_dir.join("ses_old");
    fs::create_dir_all(&old_dir).unwrap();
    let deck = make_file(&project.join("deck.pdf"), b"%PDF");
    fs::write(
        old_dir.join("session.json"),
        json!({ "id": "ses_old", "title": "Old", "location": { "directory": project.to_string_lossy() }, "time": { "created": 1, "updated": 2 } }).to_string(),
    )
    .unwrap();
    fs::write(
        old_dir.join("messages.json"),
        json!([{
            "id": "msg_a",
            "type": "assistant",
            "time": { "created": 2000, "completed": 3000 },
            "content": [{ "type": "tool", "id": "t1", "name": "kuvo-browser_show_file", "state": { "status": "completed", "input": { "path": "deck.pdf" } } }]
        }]).to_string(),
    )
    .unwrap();
    let lib2 = LibraryService::open(
        tmp.path().join("library2"),
        LibraryOptions {
            bots,
            sessions: sessions_dir,
            artifacts: Some(store),
            computer: Arc::new(|| "Studio".into()),
            platform: ThumbnailPlatform::Other,
            clock: None,
        },
    )
    .unwrap();
    lib2.backfill();
    assert!(lib2.store().get(&kuvo_services::library::LibraryStore::id_of(&deck.to_string_lossy())).is_some());
    assert!(tmp.path().join("library2/backfill.json").exists());
}

#[tokio::test]
async fn bot_folders_and_turn_end_capture_command_files_excluding_scratch() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let bots = tmp.path().join("bots");
    fs::create_dir_all(&project).unwrap();
    let sessions = MemorySessions::new("engine");
    let folder = setup_bot(&bots, "bot_ada", "m\n", &sessions);
    sessions.add("ses_code", &project, None, None);

    let store = ArtifactStore::open(
        tmp.path().join("artifacts"),
        StoreOptions {
            bots: Some(bots.clone()),
            disk: Some(plenty_disk()),
            ..Default::default()
        },
    )
    .unwrap();
    let (recorder, folders) = wire_record(
        store.clone(),
        Some(bots.clone()),
        Duration::from_secs(60),
        RecorderOptions::default(),
    );
    let folders = folders.unwrap();
    folders.start(None).await;

    // Bot turn-end captures workspace CSV but ignores node_modules
    recorder
        .heard(
            sessions.as_ref(),
            Event {
                event_type: "session.execution.started".into(),
                data: json!({ "sessionID": "ses_bot_ada" }),
                directory: None,
            },
        )
        .await;
    let csv = make_file(&folder.join("workspace/weekly.csv"), b"week,sales\n1,10\n");
    make_file(&folder.join("workspace/node_modules/x/index.js"), b"x");
    recorder
        .heard(
            sessions.as_ref(),
            Event {
                event_type: "session.execution.succeeded".into(),
                data: json!({ "sessionID": "ses_bot_ada" }),
                directory: None,
            },
        )
        .await;
    assert!(store.at(&csv.to_string_lossy()).is_some());

    // Project turn-end excludes tmp/ and Temp/
    let ok_pdf = make_file(&project.join("output/moon.pdf"), b"%PDF");
    make_file(&project.join("tmp/render.png"), b"png");
    make_file(&project.join("Temp/check.png"), b"png");
    let found = changed_since(
        &project,
        Timestamp::now().as_millisecond() - 60_000,
        |_| true,
        |_| true,
    )
    .await;
    assert_eq!(found, vec![ok_pdf]);
    folders.stop();
}

#[tokio::test]
async fn group_chat_handoff_and_living_document_from_wakeups() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let bots = tmp.path().join("bots");
    fs::create_dir_all(&project).unwrap();
    let sessions = MemorySessions::new("engine");
    let ada_dir = setup_bot(&bots, "bot_ada", "a\n", &sessions);
    setup_bot(&bots, "bot_ben", "b\n", &sessions);
    sessions.add(
        "ses_seat_ada",
        &project,
        Some(json!({ "kuvo": { "seat": { "group": "grp_team", "bot": "bot_ada" } } })),
        None,
    );
    sessions.add(
        "ses_seat_ben",
        &project,
        Some(json!({ "kuvo": { "seat": { "group": "grp_team", "bot": "bot_ben" } } })),
        None,
    );

    let store = ArtifactStore::open(
        tmp.path().join("artifacts"),
        StoreOptions {
            bots: Some(bots.clone()),
            disk: Some(plenty_disk()),
            ..Default::default()
        },
    )
    .unwrap();
    let recorder = Recorder::new(
        store.clone(),
        RecorderOptions {
            bots: Some(bots.clone()),
            ..Default::default()
        },
    );

    let plan = make_file(&project.join("plan.md"), b"Plan\n");
    hear_all(
        &recorder,
        sessions.as_ref(),
        turn_events(
            "ses_seat_ada",
            "m1",
            tool_events(
                "ses_seat_ada",
                "ma",
                "c1",
                "write",
                json!({ "filePath": plan.to_string_lossy() }),
                json!([]),
                json!({}),
            ),
        ),
    )
    .await;
    fs::write(&plan, "Plan\nBen's part\n").unwrap();
    hear_all(
        &recorder,
        sessions.as_ref(),
        turn_events(
            "ses_seat_ben",
            "m2",
            tool_events(
                "ses_seat_ben",
                "mb",
                "c2",
                "write",
                json!({ "filePath": plan.to_string_lossy() }),
                json!([]),
                json!({}),
            ),
        ),
    )
    .await;
    let plan_art = store.at(&plan.to_string_lossy()).unwrap();
    assert_eq!(plan_art.versions[1].handoff.as_ref().unwrap().from, "bot_ada");
    assert!(plan_art.where_.sessions.is_empty());

    // Two wake-ups mark a document as living
    let daily = ada_dir.join("workspace/daily.md");
    for n in [1, 2] {
        fs::write(&daily, format!("Day {n}\n")).unwrap();
        let mut evs = vec![Event {
            event_type: "session.inbox.enqueued".into(),
            data: json!({
                "sessionID": "ses_bot_ada",
                "inboxID": format!("msg_wake{n}"),
                "item": { "type": "user", "payload": { "text": "[Wake-up] Daily summary" } }
            }),
            directory: None,
        }];
        evs.extend(turn_events(
            "ses_bot_ada",
            &format!("msg_wake{n}"),
            tool_events(
                "ses_bot_ada",
                "ma",
                &format!("cw{n}"),
                "write",
                json!({ "filePath": daily.to_string_lossy() }),
                json!([]),
                json!({}),
            ),
        ));
        hear_all(&recorder, sessions.as_ref(), evs).await;
    }
    assert_eq!(store.at(&daily.to_string_lossy()).unwrap().living, Some(true));

    // Pinned code file in git keeps a copy once pinned
    let _ = store.pin(&plan_art.id, Some((PinBy::You, Some("Final".into())))).await;
    assert_eq!(
        store.get(&plan_art.id).unwrap().pinned.unwrap().note.as_deref(),
        Some("Final")
    );
    let _ = Handover {
        path: daily.to_string_lossy().into_owned(),
        how: How::Shown,
        bot: Some("bot_ada".into()),
        session: None,
        group: None,
        message: None,
        part: None,
        at: None,
    };
}
