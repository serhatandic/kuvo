//! A machine's box over SSH, against a stand-in: an `ssh` that runs what it's given here, with a
//! home of the test's own laid out as a machine's (`~/.kuvo-code/bin/kuvo-link.js`, an engine
//! runtime in `engine/<version>/opencode`), whose stand-in runtime answers `box route` from files.
//! So the script that finds the runtime and runs `box route` there is the one that runs.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kuvo_box::follow::{ListUpdate, Timing};
use kuvo_box::{BoxClient, Error, Gate, SshBox};
use serde_json::{Value, json};

struct Machine {
    folder: tempfile::TempDir,
}

impl Machine {
    fn new() -> Machine {
        let folder = support::home();
        let root = folder.path().join("home/.kuvo-code");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("workspace")).unwrap();
        std::fs::create_dir_all(folder.path().join("answers")).unwrap();
        std::fs::write(root.join("bin/kuvo-link.js"), "// stand-in\n").unwrap();
        // Two runtimes: the newest is the one used.
        for version in ["1.9.0", "1.10.2"] {
            let runtime = root.join(format!("engine/{version}/opencode"));
            std::fs::create_dir_all(runtime.parent().unwrap()).unwrap();
            executable(&runtime, &engine_script(folder.path(), version));
        }
        executable(&folder.path().join("ssh"), &ssh_script(folder.path()));
        Machine { folder }
    }

    fn path(&self) -> &Path {
        self.folder.path()
    }

    fn client(&self) -> BoxClient {
        BoxClient::machine(SshBox::new("me@devbox.invalid").program(self.path().join("ssh")).sharing_connections_in(None))
    }

    /// What `box route` prints for a route.
    fn answer(&self, name: &str, status: u16, body: &Value) {
        let printed = json!({ "status": status, "body": body.to_string() });
        // Put in place whole, as a read may come at any moment.
        let file = self.path().join("answers").join(name);
        std::fs::write(file.with_extension("new"), format!("{printed}\n")).unwrap();
        std::fs::rename(file.with_extension("new"), file).unwrap();
    }

    /// What its line says, once (`kuvo/line?once=1`): the hello, then each topic's state, for the
    /// first topic followed.
    fn line(&self, states: &[Value]) {
        let mut body = format!("{}\n", json!({ "hello": { "line": "once", "fresh": true, "beat": 5000 } }));
        for (n, state) in states.iter().enumerate() {
            body.push_str(&format!("{}\n", json!({ "t": "1", "n": n + 1, "d": state })));
        }
        let printed = json!({ "status": 200, "body": body });
        let file = self.path().join("answers").join("line");
        std::fs::write(file.with_extension("new"), format!("{printed}\n")).unwrap();
        std::fs::rename(file.with_extension("new"), file).unwrap();
    }

    /// What `box route` was asked, a request a line.
    fn asked(&self) -> Vec<Value> {
        std::fs::read_to_string(self.path().join("asked")).unwrap_or_default().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path().join(name)).unwrap_or_default()
    }
}

fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// `ssh`: notes its arguments, then runs the command it was given, as a login shell there would,
/// in the machine's home.
fn ssh_script(folder: &Path) -> String {
    format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{dir}/ssh-arguments'\nfor last; do :; done\ncd '{dir}/home' && HOME='{dir}/home' exec /bin/sh -c \"$last\"\n",
        dir = folder.display()
    )
}

/// The engine's runtime: as `box route`, notes what it ran with and was asked, and prints the
/// answer the test left for that route.
fn engine_script(folder: &Path, version: &str) -> String {
    format!(
        r#"#!/bin/sh
dir='{dir}'
printf '%s %s\n' '{version}' "$*" > "$dir/ran"
printf '%s\n' "$KUVO_HOME" > "$dir/kuvo-home"
asked=$(cat)
printf '%s\n' "$asked" >> "$dir/asked"
case "$asked" in
  *'"path":"kuvo/info"'*) cat "$dir/answers/info" ;;
  *'"path":"kuvo/sessions/list'*) cat "$dir/answers/list" ;;
  *'"path":"kuvo/line?'*) cat "$dir/answers/line" ;;
  *'"path":"kuvo/file'*) printf '%s\n' '{{"status":200,"bodyBase64":"AAEC/w==","type":"image/png"}}' ;;
  *'"path":"kuvo/broken'*) echo 'not an answer' ;;
  *'"path":"kuvo/gone'*) echo 'Connection to devbox closed.' >&2; exit 255 ;;
  *) printf '%s\n' '{{"status":404,"body":"{{}}"}}' ;;
esac
"#,
        dir = folder.display()
    )
}

fn machine_info(features: Value) -> Value {
    json!({ "kind": "machine", "name": "devbox", "bridge": "abc123", "engine": "2.2.6", "features": features, "agents": false, "started": null, "chats": "/home/me/.kuvo-code/chats", "running": true, "stopped": false })
}

fn list() -> Value {
    json!({ "archivedGroups": [], "at": "4", "full": true, "sessions": { "ses_m": { "kind": "project", "project": "/home/me/site", "directory": "/home/me/site", "created": 1, "updated": 2, "title": "On the machine" } }, "projects": { "/home/me/site": { "updated": 2 } } })
}

#[test]
fn a_machines_box_answers_through_box_route_over_ssh() {
    let machine = Machine::new();
    machine.answer("info", 200, &machine_info(support::current_features()));
    machine.answer("list", 200, &list());
    let client = machine.client();
    assert!(!client.streams());

    let info = client.connect().expect("kuvo/info over SSH");
    assert_eq!((info.kind.as_str(), info.name.as_str()), ("machine", "devbox"));
    assert_eq!(client.gate(), Gate::Open);
    let line = client.session_list(Some("3")).unwrap();
    assert_eq!(line.sessions["ses_m"].title.as_deref(), Some("On the machine"));

    // Asked as the Mac asks a machine whose keeper is from before `route`: the newest runtime there,
    // `box route`, the box's home, the request on its input.
    assert_eq!(machine.read("ran").trim(), format!("1.10.2 --config=/dev/null --no-env-file {}/home/.kuvo-code/bin/kuvo-link.js box route", machine.path().display()));
    assert_eq!(PathBuf::from(machine.read("kuvo-home").trim()), machine.path().join("home/.kuvo-code"));
    assert_eq!(machine.asked(), vec![
        json!({ "method": "GET", "path": "kuvo/info", "headers": { "x-kuvo-route-version": "1" } }),
        json!({ "method": "GET", "path": "kuvo/sessions/list?since=3", "headers": { "x-kuvo-route-version": "1" } }),
    ]);
    // Never a password prompt, nor a terminal; the machine as it was named.
    let arguments = machine.read("ssh-arguments");
    for option in ["BatchMode=yes", "RequestTTY=no", "RemoteCommand=none"] {
        assert!(arguments.lines().any(|line| line == option), "{option}");
    }
    // Its name after `--`, never read as an option.
    let lines: Vec<&str> = arguments.lines().collect();
    let at = lines.iter().position(|line| *line == "me@devbox.invalid").expect("the machine");
    assert_eq!(lines[at - 1], "--");
}

/// A machine whose keeper answers `route` runs each request through it, on the engine the keeper
/// picks (here the older of two, as after a failed start), never the newest folder in `engine/`.
#[test]
fn a_machine_with_the_keepers_route_runs_on_the_engine_the_keeper_picks() {
    let machine = Machine::new();
    machine.answer("info", 200, &machine_info(support::current_features()));
    let root = machine.path().join("home/.kuvo-code");
    // (A stand-in keeper with the verb, as `box/src/keeper/kuvo-start` has it: it notes what it was
    // asked, and runs its own pick.)
    executable(&root.join("bin/kuvo-start"), &format!(
        "#!/bin/sh\nK=\"$HOME/.kuvo-code\"\ncase \"$1\" in\nroute|route-to)\n  printf '%s\\n' \"$1\" > '{dir}/keeper'\n  export KUVO_HOME=\"$K\"\n  exec \"$K/engine/1.9.0/opencode\" --config=/dev/null --no-env-file \"$K/bin/kuvo-link.js\" box \"$1\"\n  ;;\nesac\nexit 9\n",
        dir = machine.path().display()
    ));
    let client = machine.client();
    client.connect().expect("kuvo/info over SSH, by its keeper");
    assert_eq!(machine.read("keeper").trim(), "route");
    assert_eq!(machine.read("ran").trim(), format!("1.9.0 --config=/dev/null --no-env-file {}/home/.kuvo-code/bin/kuvo-link.js box route", machine.path().display()));
}

#[test]
fn what_a_machine_says_wrong_is_said_plainly() {
    let machine = Machine::new();
    machine.answer("info", 200, &machine_info(support::current_features()));
    let client = machine.client();

    // Bytes that aren't text come as base64.
    let response = client.send(&kuvo_box::transport::Request::get("kuvo/file?uri=x")).unwrap();
    assert_eq!((response.status, response.body.as_slice(), response.content_type.as_deref()), (200, &[0u8, 1, 2, 255][..], Some("image/png")));
    assert!(matches!(client.send(&kuvo_box::transport::Request::get("kuvo/broken")), Err(Error::BadAnswer(_))));
    match client.send(&kuvo_box::transport::Request::get("kuvo/gone")) {
        Err(Error::Unreachable(why)) => assert!(why.contains("devbox"), "{why}"),
        other => panic!("unreachable, not {other:?}"),
    }
    assert_eq!(client.send(&kuvo_box::transport::Request::get("kuvo/nothing")).unwrap().status, 404);

    // A machine Kuvo Code was never set up on.
    std::fs::remove_file(machine.path().join("home/.kuvo-code/bin/kuvo-link.js")).unwrap();
    match machine.client().connect() {
        Err(Error::Unreachable(why)) => assert_eq!(why, "Kuvo Code isn't set up on this machine."),
        other => panic!("not set up, not {other:?}"),
    }
}

/// An older machine that says it's held for this client is held as it says, and still asked.
#[test]
fn an_older_machine_is_held_as_it_says() {
    let machine = Machine::new();
    let mut info = machine_info(json!(["sessions.list"]));
    info["compatibility"] = json!({ "state": "held", "missing": ["messages.send"] });
    machine.answer("info", 200, &info);
    machine.answer("list", 200, &list());
    let client = machine.client();
    assert!(client.session_list(None).is_ok(), "asked, and it answers");
    assert!(client.gate().is_held());
    assert_eq!(machine.asked().len(), 2);
}

#[test]
fn a_machines_list_is_followed_on_its_line_asked_again() {
    let machine = Machine::new();
    machine.answer("info", 200, &machine_info(support::current_features()));
    machine.line(&[list()]);
    let client = machine.client();
    // (Asked again at once each time: what the machine says next is what comes.)
    client.line().set_timing(kuvo_box::line::LineTiming { once_again: Duration::ZERO, ..Default::default() });
    let follow = client.follow_sessions(Timing { retry_after: Duration::ZERO, ..Timing::default() });
    let ListUpdate::Listing(listing) = follow.next_blocking().unwrap() else { panic!("the list") };
    assert_eq!(listing.sidebar().projects[0].sessions, vec!["ses_m"]);
    assert_eq!(listing.at(), Some("4"));

    // A session made there: the next time its line is asked, the list has it.
    let made = json!({ "archivedGroups": [], "at": "5", "sessions": { "ses_n": { "kind": "chat", "directory": "/home/me/.kuvo-code/chats", "created": 3, "updated": 4 } }, "projects": { "/home/me/site": { "updated": 2 } } });
    machine.line(&[made]);
    let listing = loop {
        if let ListUpdate::Listing(listing) = follow.next_blocking().unwrap()
            && listing.at() == Some("5")
        {
            break listing;
        }
    };
    assert_eq!(listing.sidebar().chats, vec!["ses_n"]);
    assert_eq!(listing.sidebar().projects[0].sessions, vec!["ses_m"]);
    drop(follow);
    // One request for everything followed, each time: the line, answered once (a machine over SSH
    // answers once), never the list's own route.
    let paths: Vec<String> = machine.asked().iter().filter_map(|asked| asked["path"].as_str().map(str::to_owned)).collect();
    assert!(paths.iter().any(|path| path.starts_with("kuvo/line?watch=") && path.ends_with("&once=1")), "{paths:?}");
    assert!(!paths.iter().any(|path| path.starts_with("kuvo/sessions/list")), "{paths:?}");
}
