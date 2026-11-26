use super::*;

/// A home whose OpenCode database has two conversations, in OpenCode's own columns.
fn home_with_two() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("data/opencode");
    std::fs::create_dir_all(&folder).unwrap();
    let db = Connection::open(folder.join("opencode.db")).unwrap();
    db.execute_batch(&format!(
        "CREATE TABLE session_v2 ({COLUMNS}, slug TEXT);
         CREATE TABLE session_message (id TEXT, session_id TEXT, type TEXT, seq INTEGER, data TEXT);
         INSERT INTO session_v2 (id, project_id, directory, title, metadata, model, cost, tokens_input, tokens_output,
           tokens_reasoning, tokens_cache_read, tokens_cache_write, idle_outcome, time_created, time_updated, time_archived)
           VALUES ('ses_old', 'prj', '/work', 'Old one', '{{\"kuvo\":{{\"chat\":\"/work\"}}}}',
             '{{\"id\":\"deepseek-v4.1-flash\",\"providerID\":\"opencode-go\"}}', 0.5, 100, 10, 0, 0, 0, 'succeeded', 1, 2, 3);
         INSERT INTO session_v2 (id, parent_id, project_id, directory, title, time_created, time_updated)
           VALUES ('ses_child', 'ses_old', 'prj', '/work', 'Its subagent', 4, 5);
         INSERT INTO session_message (id, session_id) VALUES ('msg_1', 'ses_old'), ('msg_2', 'ses_old');"
    ))
    .unwrap();
    home
}

#[tokio::test]
async fn its_conversations_are_listed_as_opencode_gave_them_without_starting_it() {
    let home = home_with_two();
    let store = SessionStore::new(home.path().join("agents/sessions"));
    let history = OpenCodeHistory::new(home.path(), None, store);
    let listed = history.listed(BTreeSet::new()).await.unwrap();
    assert_eq!(
        Value::Object(listed[0].clone()),
        json!({
            "id": "ses_old", "projectID": "prj",
            "model": { "id": "deepseek-v4.1-flash", "providerID": "opencode-go" },
            "cost": 0.5, "tokens": { "input": 100, "output": 10, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
            "outcome": "succeeded", "time": { "created": 1, "updated": 2, "archived": 3 }, "title": "Old one",
            "metadata": { "kuvo": { "chat": "/work" }, "kuvoAgent": "opencode" },
            "location": { "directory": "/work" },
        })
    );
    assert_eq!(listed[1]["parentID"], "ses_old");
    // One the agents keep already isn't listed again.
    let kept = BTreeSet::from(["ses_old".to_owned()]);
    let rest = history.listed(kept).await.unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0]["id"], "ses_child");
}

#[tokio::test]
async fn a_home_without_its_database_lists_none_and_takes_none() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("agents/sessions"));
    let history = OpenCodeHistory::new(home.path(), None, store);
    assert!(history.listed(BTreeSet::new()).await.unwrap().is_empty());
    assert!(!history.take("ses_old").await.unwrap());
}

#[tokio::test]
async fn one_of_its_conversations_needs_opencode_to_be_taken() {
    let home = home_with_two();
    let store = SessionStore::new(home.path().join("agents/sessions"));
    let history = OpenCodeHistory::new(home.path(), None, store.clone());
    assert!(!history.take("ses_unknown").await.unwrap());
    let error = history.take("ses_old").await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(store.read("ses_old").await.unwrap().is_none());
}
