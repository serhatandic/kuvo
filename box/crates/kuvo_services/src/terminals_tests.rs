use super::*;

#[test]
fn output_is_counted_and_sent_again_in_utf16_units_as_opencode_counts_it() {
    let mut kept = Kept::default();
    kept.push("é✓😀");
    assert_eq!(kept.end(), 4);
    assert_eq!(kept.since(0), ["é✓😀"]);
    assert_eq!(kept.since(2), ["😀"]);
    assert!(kept.since(4).is_empty());
    assert!(kept.since(99).is_empty());
}

#[test]
fn only_the_last_two_mebibytes_are_kept_and_they_come_in_pieces() {
    let mut kept = Kept::default();
    for _ in 0..46 {
        kept.push(&"a".repeat(65_536));
    }
    kept.push(&"b".repeat(50_048));
    assert_eq!(kept.end(), 3_064_704);
    assert_eq!(kept.units, KEPT);
    let pieces = kept.since(0);
    assert_eq!(pieces.len(), 32);
    assert!(pieces.iter().all(|piece| units(piece) == PIECE));
    assert!(pieces[31].ends_with('b'));
    // A cursor from before what's kept starts where it starts.
    assert_eq!(kept.since(10).len(), 32);
}

#[test]
fn a_character_split_across_reads_waits_for_its_end() {
    let mut decoder = Decoder::default();
    let smile = "😀".as_bytes();
    assert_eq!(decoder.decode(&[b'a', smile[0], smile[1]]), "a");
    assert_eq!(decoder.decode(&smile[2..]), "😀");
    assert_eq!(decoder.decode(&[0xff, b'z']), "\u{fffd}z");
}

#[test]
fn a_shell_starts_without_kuvos_settings_and_with_a_terminals_term() {
    let env = shell_environment(
        [
            ("PATH", "/usr/bin"),
            ("KUVO_AGENTS_PASSWORD", "secret"),
            ("OPENCODE_SERVER_PASSWORD", "secret"),
            ("BUN_BE_BUN", "1"),
            ("TERM", "dumb"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into())),
    );
    let names: Vec<_> = env
        .keys()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["PATH", "TERM"]);
    assert_eq!(
        env.get(std::ffi::OsStr::new("TERM")).unwrap(),
        "xterm-256color"
    );
}

#[test]
fn ids_are_in_opencodes_ascending_shape() {
    let (first, second) = (new_id(), new_id());
    assert!(first.starts_with("pty_") && first.len() == 30, "{first}");
    assert!(first < second);
}

/// A paired client (here over the relay) makes a terminal, types into it over its socket and sees
/// what it prints, as the app does through another computer.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_paired_client_types_into_a_terminal_over_its_link() {
    use kuvo_server::{Caller, LineTiming, Pairing, Transport};
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("workspace")).unwrap();
    let mut routes = Registry::new();
    register(&mut routes, home.path());
    let dispatcher = routes.build(LineTiming::default());
    let request = |method: Method, path: &str, body: &str| DispatchRequest {
        method,
        path: path.into(),
        headers: http::HeaderMap::new(),
        body: bytes::Bytes::from(body.to_owned()),
        caller: Caller::Paired {
            transport: Transport::Relay,
            pairing: Pairing::verified([1; 32], "paired".into(), "test".into(), true, || true),
        },
    };
    let cancel = CancellationToken::new();
    let made = dispatcher
        .dispatch(
            request(
                Method::POST,
                "api/pty",
                r#"{"command":"/bin/sh","args":["-i"]}"#,
            ),
            cancel.clone(),
        )
        .await;
    assert_eq!(made.status, StatusCode::OK);
    let body = http_body_util::BodyExt::collect(made.body)
        .await
        .unwrap()
        .to_bytes();
    let id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut socket = dispatcher
        .open_socket(
            request(Method::GET, &format!("api/pty/{id}/connect?cursor=-1"), ""),
            cancel.clone(),
        )
        .await
        .map_err(|refusal| refusal.reply().status)
        .unwrap();
    assert!(matches!(socket.receive.recv().await, Some(Message::Binary(note)) if note[0] == 0));
    socket
        .send
        .send(Message::Text("echo paired-$((6*7))\n".to_owned().into()))
        .await
        .unwrap();
    let mut printed = String::new();
    while !printed.contains("paired-42") {
        match tokio::time::timeout(Duration::from_secs(10), socket.receive.recv()).await {
            Ok(Some(Message::Text(text))) => printed.push_str(text.as_str()),
            other => panic!("{other:?} after {printed:?}"),
        }
    }
    let removed = dispatcher
        .dispatch(
            request(Method::DELETE, &format!("api/pty/{id}"), ""),
            cancel,
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    // (After what it printed last, its close.)
    loop {
        match socket.receive.recv().await {
            Some(Message::Text(_)) => {}
            Some(Message::Close(Some(frame))) => break assert_eq!(frame.code, CloseCode::Normal),
            other => panic!("{other:?}"),
        }
    }
}
