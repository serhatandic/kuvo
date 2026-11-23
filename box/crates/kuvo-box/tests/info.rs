//! Starts the real box in a fresh home and reads its HTTP answer as the desktop does.

#[allow(dead_code)]
#[path = "../../../../gpui/kuvo_box/src/info.rs"]
mod desktop;

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpStream},
    path::Path,
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

fn box_command(home: &Path) -> Command {
    let binary =
        std::env::var_os("KUVO_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_kuvo-box").into());
    let mut command = Command::new(binary);
    command.env_clear().env("KUVO_HOME", home);
    // Winsock loads the provider DLLs through SystemRoot in a cleared environment.
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot").expect("Windows directory"),
    );
    command
}

fn start(home: &Path) -> (Server, Value) {
    let mut server = Server(
        box_command(home)
            .arg("serve")
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
    (server, ready)
}

fn read_record(home: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(home.join("run/serve.json")).unwrap()).unwrap()
}

#[test]
fn serve_answers_info_with_the_fields_the_desktop_reads() {
    let home = tempfile::tempdir().unwrap();
    let (server, ready) = start(home.path());
    let record = read_record(home.path());
    let port: u16 = record["port"].as_u64().unwrap().try_into().unwrap();
    assert_eq!(ready["url"], format!("http://127.0.0.1:{port}"));
    assert_eq!(record["pid"], server.0.id());
    assert_eq!(record["formatVersion"], 1);
    assert!(record["startedAt"].as_u64().unwrap() > 0);
    assert_eq!(record["kept"], false);
    let generation = record["generation"].as_str().unwrap();
    assert_eq!(generation.len(), 36);

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
    let value: Value = serde_json::from_str(body).unwrap();
    assert_eq!(value["serverGeneration"], generation);
    assert_eq!(value["serverBuild"], env!("CARGO_PKG_VERSION"));
    let info: desktop::BoxInfo = serde_json::from_str(body).unwrap();
    assert_eq!(info.kind, "machine");
    assert_eq!(info.name, hostname::get().unwrap().to_string_lossy());
    assert_eq!(info.bridge.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(info.engine, None);
    assert_eq!(
        info.features,
        ["info", "line", "engine.restart", "engine.start"]
    );
    assert!(
        matches!(info.compatibility, desktop::Compatibility::Held { missing }
        if missing.iter().any(|name| name == "sessions.list"))
    );
    assert!(!info.agents);
    assert_eq!(info.started, None);
    assert_eq!(info.chats.as_deref(), home.path().join("chats").to_str());
    assert_eq!(info.running, Some(false));
    assert_eq!(info.stopped, Some(false));

    let via_ssh = box_command(home.path())
        .args(["route", "--internal", "kuvo/info"])
        .output()
        .unwrap();
    assert!(
        via_ssh.status.success(),
        "{}",
        String::from_utf8_lossy(&via_ssh.stderr)
    );
    let reply: Value = serde_json::from_slice(&via_ssh.stdout).unwrap();
    assert_eq!(reply["status"], 200);
    assert_eq!(reply["routeVersion"], "1");
    assert_eq!(reply["internal"], true);
    assert_eq!(
        serde_json::from_str::<Value>(reply["body"].as_str().unwrap()).unwrap(),
        value
    );

    let mut addressed = Server(
        box_command(home.path())
            .arg("route-to")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stdin = addressed.0.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"computer":"A".repeat(43),"hello":""})
    )
    .unwrap();
    let mut stdout = BufReader::new(addressed.0.stdout.take().unwrap());
    let mut answer = String::new();
    stdout.read_line(&mut answer).unwrap();
    let answer: Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(answer["status"], 409);
    assert_eq!(answer["routeVersion"], "1");
    assert!(
        addressed.0.wait().unwrap().success(),
        "addressed refusal closes the command even with stdin open"
    );
}

#[test]
fn new_resident_reclaims_discovery_and_ipc_after_a_process_is_cut_off() {
    let home = tempfile::tempdir().unwrap();
    let (mut first, _) = start(home.path());
    let previous = read_record(home.path());
    first.0.kill().unwrap();
    first.0.wait().unwrap();
    assert!(home.path().join("run/.client-owner").is_dir());

    let (second, _) = start(home.path());
    let current = read_record(home.path());
    assert_eq!(current["pid"], second.0.id());
    assert_ne!(current["generation"], previous["generation"]);
    assert_ne!(current["password"], previous["password"]);
    let reply = box_command(home.path())
        .args(["route", "--internal", "kuvo/info"])
        .output()
        .unwrap();
    assert!(
        reply.status.success(),
        "{}",
        String::from_utf8_lossy(&reply.stderr)
    );
    let answer: Value = serde_json::from_slice(&reply.stdout).unwrap();
    assert_eq!(answer["status"], 200);
    let info: Value = serde_json::from_str(answer["body"].as_str().unwrap()).unwrap();
    assert_eq!(info["serverGeneration"], current["generation"]);
}
