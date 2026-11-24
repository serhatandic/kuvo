use std::{collections::BTreeMap, sync::Mutex};

use serde_json::json;

use super::*;

/// What the stand-in was sent: the conversation, the note, its summary, and how it went.
type Sent = (String, String, Option<String>, WakeDelivered);

/// A stand-in engine: its sessions, what's stopped, what waits in its inbox, what it was sent,
/// and whether it takes anything now.
#[derive(Default)]
struct Stand {
    sessions: BTreeMap<String, Value>,
    stopped: Vec<String>,
    inbox: Vec<(String, String)>,
    sent: Mutex<Vec<Sent>>,
    down: bool,
}

impl Reach for Stand {
    async fn session(&self, id: &str) -> Result<Option<Value>, Unreachable> {
        if self.down {
            return Err(Unreachable("down".into()));
        }
        Ok(self.sessions.get(id).cloned())
    }
    async fn wait_for_tools(&self, _: &str) {}
    async fn stopped(&self, session: &str) -> bool {
        self.stopped.iter().any(|each| each == session)
    }
    async fn waits_in_inbox(&self, session: &str, note: &str) -> bool {
        self.inbox
            .iter()
            .any(|(id, text)| id == session && text == note)
    }
    async fn waits_on_box(&self, _: &str, _: &str) -> bool {
        false
    }
    async fn deliver(
        &self,
        session: &str,
        note: &str,
        summary: Option<&str>,
    ) -> Result<WakeDelivered, Unreachable> {
        if self.down {
            return Err(Unreachable("down".into()));
        }
        let how = if self.stopped.iter().any(|each| each == session) {
            WakeDelivered::Held
        } else {
            WakeDelivered::Sent
        };
        self.sent.lock().unwrap().push((
            session.to_owned(),
            note.to_owned(),
            summary.map(str::to_owned),
            how,
        ));
        Ok(how)
    }
}

struct Home {
    _root: tempfile::TempDir,
    roots: Roots,
}

fn home() -> Home {
    let root = tempfile::tempdir().unwrap();
    let roots = Roots::of(root.path());
    fs::create_dir_all(&roots.run).unwrap();
    Home { _root: root, roots }
}

fn session_folder(roots: &Roots, session: &str) -> PathBuf {
    let folder = roots.sessions.join(session);
    fs::create_dir_all(folder.join("wakes")).unwrap();
    fs::write(folder.join("session"), session).unwrap();
    fs::write(folder.join("directory"), "/work").unwrap();
    folder
}

fn bot_folder(roots: &Roots, id: &str, session: &str) -> PathBuf {
    let folder = roots.bots.join(id);
    fs::create_dir_all(folder.join("wakes")).unwrap();
    fs::write(folder.join("session"), session).unwrap();
    folder
}

fn engine_with(sessions: &[(&str, Value)]) -> Stand {
    Stand {
        sessions: sessions
            .iter()
            .map(|(id, value)| ((*id).to_owned(), value.clone()))
            .collect(),
        ..Stand::default()
    }
}

const NOW: i64 = 1_893_456_000;

#[tokio::test]
async fn a_due_wake_up_goes_and_a_repeating_one_comes_back_on_your_clock() {
    let home = home();
    let folder = bot_folder(&home.roots, "bot_ada", "ses_ada");
    fs::write(folder.join("timezone"), "Asia/Tokyo").unwrap();
    fs::write(
        folder.join("wakes").join(format!("{}-Ab1-1d", NOW - 5)),
        "Morning digest\n",
    )
    .unwrap();
    fs::write(
        folder.join("wakes").join(format!("{}-Cd2", NOW - 5)),
        "Once",
    )
    .unwrap();
    fs::write(
        folder.join("wakes").join(format!("{}-Ef3", NOW + 600)),
        "Later",
    )
    .unwrap();
    fs::create_dir_all(folder.join("summaries")).unwrap();
    fs::write(folder.join("summaries/Ab1"), "Digest").unwrap();
    let engine = engine_with(&[]);
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW,
    }
    .wakes()
    .await;
    let sent = engine.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[0],
        (
            "ses_ada".into(),
            "Morning digest".into(),
            Some("Digest".into()),
            WakeDelivered::Sent
        )
    );
    assert_eq!(sent[1].1, "Once");
    // 09:00 in Tokyo the next day, its letters and time of day kept.
    let left = names(&folder.join("wakes"));
    assert_eq!(
        left,
        [
            format!("{}-Ef3", NOW + 600),
            format!("{}-Ab1-1d@0859", NOW - 60 + 86_400)
        ]
    );
    assert!(
        folder.join("summaries/Ab1").exists(),
        "a repeating one keeps its summary"
    );
    assert!(!folder.join(".retry").exists());
}

#[tokio::test]
async fn one_that_cant_go_waits_longer_each_time() {
    let home = home();
    let folder = bot_folder(&home.roots, "bot_ada", "ses_ada");
    fs::write(folder.join("wakes").join(format!("{}-Ab1", NOW - 5)), "Hi").unwrap();
    let engine = Stand {
        down: true,
        ..Stand::default()
    };
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW,
    }
    .wakes()
    .await;
    assert_eq!(text(&folder.join(".retry")), format!("{} 60\n", NOW + 60));
    // Not tried again before then.
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW + 30,
    }
    .wakes()
    .await;
    assert_eq!(text(&folder.join(".retry")), format!("{} 60\n", NOW + 60));
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW + 61,
    }
    .wakes()
    .await;
    assert_eq!(
        text(&folder.join(".retry")),
        format!("{} 120\n", NOW + 61 + 120)
    );
    assert_eq!(next_wake(&home.roots), Some((NOW + 61 + 120) as f64));
}

#[tokio::test]
async fn a_repeating_one_still_waiting_from_last_time_isnt_sent_again() {
    let home = home();
    let folder = bot_folder(&home.roots, "bot_ada", "ses_ada");
    fs::write(
        folder.join("wakes").join(format!("{}-Ab1-30m", NOW - 5)),
        "Check",
    )
    .unwrap();
    let engine = Stand {
        inbox: vec![("ses_ada".into(), "Check".into())],
        ..Stand::default()
    };
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW,
    }
    .wakes()
    .await;
    assert!(engine.sent.lock().unwrap().is_empty());
    assert_eq!(
        names(&folder.join("wakes")),
        [format!("{}-Ab1-30m", NOW - 5 + 1_800)]
    );
}

#[tokio::test]
async fn a_stopped_conversations_wake_up_waits_on_the_box() {
    let home = home();
    let folder = session_folder(&home.roots, "ses_plan");
    fs::write(folder.join("wakes").join(format!("{}-Ab1", NOW - 5)), "Hi").unwrap();
    let mut engine = engine_with(&[("ses_plan", json!({"id": "ses_plan"}))]);
    engine.stopped.push("ses_plan".into());
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW,
    }
    .wakes()
    .await;
    assert_eq!(engine.sent.lock().unwrap()[0].3, WakeDelivered::Held);
    assert!(names(&folder.join("wakes")).is_empty());
}

#[tokio::test]
async fn gone_archived_and_subagents_folders_settle() {
    let home = home();
    let gone = session_folder(&home.roots, "ses_gone");
    fs::write(gone.join("wakes").join(format!("{}-Ab1", NOW - 5)), "Hi").unwrap();
    let archived = session_folder(&home.roots, "ses_away");
    fs::write(
        archived.join("wakes").join(format!("{}-Ab2", NOW - 5)),
        "Hi",
    )
    .unwrap();
    let child = session_folder(&home.roots, "ses_child");
    fs::write(
        child.join("wakes").join(format!("{}-Ab3", NOW + 900)),
        "Later",
    )
    .unwrap();
    bot_folder(&home.roots, "bot_ada", "ses_ada");
    let engine = engine_with(&[
        (
            "ses_away",
            json!({"id": "ses_away", "metadata": {"kuvo.archived": true}}),
        ),
        (
            "ses_child",
            json!({"id": "ses_child", "parentID": "ses_ada", "location": {"directory": "/moved"}}),
        ),
    ]);
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW,
    }
    .wakes()
    .await;
    assert!(engine.sent.lock().unwrap().is_empty());
    assert!(!gone.exists());
    assert!(
        home.roots
            .trash
            .join(format!("wakes-ses_gone-{NOW}"))
            .is_dir()
    );
    assert_eq!(text(&archived.join("paused")), "archived");
    assert!(!child.exists());
    assert_eq!(
        names(&home.roots.bots.join("bot_ada/wakes")),
        [format!("{}-Ab3", NOW + 900)],
        "a subagent's go to its bot"
    );
    // Brought back: let go of, and sent.
    let engine = engine_with(&[("ses_away", json!({"id": "ses_away"}))]);
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW + 1,
    }
    .wakes()
    .await;
    assert!(!archived.join("paused").exists());
    assert_eq!(engine.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn held_moving_and_restoring_folders_wait() {
    let home = home();
    let folder = bot_folder(&home.roots, "bot_ada", "ses_ada");
    fs::write(folder.join("wakes").join(format!("{}-Ab1", NOW - 5)), "Hi").unwrap();
    fs::write(folder.join("paused"), "moving:mov_abc").unwrap();
    let engine = engine_with(&[]);
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW,
    }
    .wakes()
    .await;
    assert!(engine.sent.lock().unwrap().is_empty());
    assert!(!wanted(&home.roots, NOW));
    fs::remove_file(folder.join("paused")).unwrap();
    assert!(wanted(&home.roots, NOW));
    fs::create_dir_all(home.roots.run.parent().unwrap().join("backup")).unwrap();
    fs::write(
        home.roots.run.parent().unwrap().join("backup/restoring"),
        "",
    )
    .unwrap();
    Round {
        roots: &home.roots,
        reach: &engine,
        now: NOW,
    }
    .wakes()
    .await;
    assert!(engine.sent.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn a_watch_wakes_its_conversation_once_a_change_is_seen_twice() {
    let home = home();
    let folder = bot_folder(&home.roots, "bot_ada", "ses_ada");
    let watch = folder.join("watches/Wt1");
    fs::create_dir_all(&watch).unwrap();
    fs::write(watch.join("every"), "5m").unwrap();
    fs::write(watch.join("note"), "Tell me").unwrap();
    fs::write(watch.join("check"), "cat value").unwrap();
    fs::write(folder.join("directory"), "/nowhere").unwrap();
    fs::write(folder.join("value"), "one").unwrap();
    let engine = engine_with(&[]);
    let env = check::environment();
    let round = |now| Round {
        roots: &home.roots,
        reach: &engine,
        now,
    };
    // The first reading is what it heard.
    round(NOW).watches(&Fence::Off, &env).await;
    assert_eq!(text(&watch.join("last")), check::sum("one"));
    fs::write(folder.join("value"), "two").unwrap();
    // A change seen once is looked at again two minutes later.
    round(NOW + 300).watches(&Fence::Off, &env).await;
    assert_eq!(text(&watch.join("next")), (NOW + 420).to_string());
    assert!(engine.sent.lock().unwrap().is_empty());
    round(NOW + 420).watches(&Fence::Off, &env).await;
    let sent = engine.sent.lock().unwrap().clone();
    assert_eq!(
        sent[0].1,
        "Tell me\n\nYour watch (Wt1, every 5m) saw a change. Its check, cat value, now prints:\ntwo"
    );
    assert_eq!(text(&watch.join("last")), check::sum("two"));
    assert!(!watch.join("message").exists());
    assert_eq!(text(&watch.join("next")), (NOW + 600).to_string());
}
