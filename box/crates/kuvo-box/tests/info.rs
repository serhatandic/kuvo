//! Starts the real box in a fresh home and reads its HTTP answer as the desktop does.

#[allow(dead_code)]
#[path = "../../../../gpui/kuvo_box/src/info.rs"]
mod desktop;

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpStream},
    process::{Child, Command, Stdio},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn serve_answers_info_with_the_fields_the_desktop_reads() {
    let home = tempfile::tempdir().unwrap();
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_kuvo-box"))
            .arg("serve")
            .env_clear()
            .env("KUVO_HOME", home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = server.0.stdout.take().unwrap();
    let (ready_line, readiness) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line);
        let _ = ready_line.send(read.map(|_| line));
    });
    let ready = readiness
        .recv_timeout(Duration::from_secs(10))
        .expect("serve reports readiness within the startup deadline")
        .unwrap();
    let ready: Value = serde_json::from_str(&ready).expect("serve announces readiness");
    let record: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("run/serve.json")).unwrap())
            .unwrap();
    let port: u16 = record["port"].as_u64().unwrap().try_into().unwrap();
    assert_eq!(ready["url"], format!("http://127.0.0.1:{port}"));
    assert_eq!(record["pid"], server.0.id());
    assert_eq!(record["formatVersion"], 1);

    let send = |authorization: &str| {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(socket,
            "GET /kuvo/info HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nx-kuvo-route-version: 1\r\n{authorization}\r\n"
        ).unwrap();
        let mut reply = String::new();
        socket.read_to_string(&mut reply).unwrap();
        reply
    };

    let refused = send("");
    assert!(refused.starts_with("HTTP/1.1 401 "), "{refused}");
    let password = record["password"].as_str().unwrap();
    let wrong = format!(
        "{}{}",
        if password.starts_with('0') { '1' } else { '0' },
        &password[1..]
    );
    let wrong = STANDARD.encode(format!("opencode:{wrong}"));
    let refused = send(&format!("Authorization: Basic {wrong}\r\n"));
    assert!(refused.starts_with("HTTP/1.1 401 "), "{refused}");
    let token = STANDARD.encode(format!("opencode:{password}"));
    let reply = send(&format!("Authorization: Basic {token}\r\n"));
    let (headers, body) = reply.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("HTTP/1.1 200 "), "{reply}");
    assert!(
        headers.contains("\r\nx-kuvo-route-version: 1\r\n"),
        "{headers}"
    );
    let info: desktop::BoxInfo = serde_json::from_str(body).unwrap();
    assert_eq!(info.kind, "machine");
    assert_eq!(info.name, hostname::get().unwrap().to_string_lossy());
    assert_eq!(info.bridge.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(info.engine, None);
    assert_eq!(info.features, ["info"]);
    assert!(
        matches!(info.compatibility, desktop::Compatibility::Held { missing }
        if missing.iter().any(|name| name == "sessions.list"))
    );
    assert!(!info.agents);
    assert_eq!(info.started, None);
    assert_eq!(info.chats.as_deref(), home.path().join("chats").to_str());
    assert_eq!(info.running, Some(false));
    assert_eq!(info.stopped, Some(false));
}
