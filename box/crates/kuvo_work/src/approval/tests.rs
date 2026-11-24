use std::fs;

use serde_json::json;

use super::*;

fn rule(action: &str, resource: &str, effect: &str) -> Rule {
    Rule::new(action, resource, effect)
}

fn machine(settings: Option<serde_json::Value>) -> (tempfile::TempDir, Approvals) {
    let home = tempfile::tempdir().expect("home");
    fs::create_dir_all(home.path().join("box")).expect("box");
    if let Some(settings) = settings {
        fs::write(home.path().join("box/opencode.json"), settings.to_string()).expect("settings");
    }
    let approvals = Approvals::new(home.path(), false);
    (home, approvals)
}

#[test]
fn ask_first_asks_before_everything_and_keeps_settings_and_its_folders() {
    let settings = [
        rule("shell", "git status*", "allow"),
        rule("shell", "rm -rf*", "deny"),
        rule("external_directory", "/work/*", "allow"),
    ];
    let current = [
        rule("shell", "*", "ask"),
        rule("external_directory", "/bots/bot_a/*", "allow"),
        rule("external_directory", "/work/*", "allow"),
    ];
    let made = refreshed_rules(&current, &settings);
    let asks: Vec<_> = made.iter().take(8).map(|rule| rule.action.as_str()).collect();
    assert_eq!(
        asks,
        ["edit", "write", "patch", "shell", "webfetch", "kuvo-browser_browser_save_pdf",
         "kuvo-browser_browser_upload", "kuvo-browser_watch_for_change"],
        "outside folders aren't asked about where Settings doesn't allow them everywhere"
    );
    assert_eq!(made[8], rule("shell", "git status*", "allow"));
    assert_eq!(made[9], rule("shell", "rm -rf*", "deny"));
    assert_eq!(made[10], rule("external_directory", "/bots/bot_a/*", "allow"));
    assert_eq!(made.len(), 11, "a folder Settings names is Settings' to say");
}

#[test]
fn approve_all_runs_what_settings_asks_about_and_keeps_its_denies() {
    let settings = [rule("shell", "npm publish*", "ask"), rule("webfetch", "*", "deny")];
    let made = refreshed_rules(&[], &settings);
    assert_eq!(made, [rule("shell", "npm publish*", "allow"), rule("webfetch", "*", "deny")]);
}

#[test]
fn a_new_bot_gets_this_computers_default_and_auto_asks_first_without_the_sandbox() {
    let (home, approvals) = machine(Some(json!({})));
    let rules = [rule("external_directory", "/bots/bot_a/*", "allow")];
    let kept = approvals.of_bot(&rules, None);
    assert!(!kept.stored);
    let expected = if kuvo_os::platform::Platform::current().is_windows_host() {
        Approval::AskFirst
    } else {
        Approval::Auto
    };
    assert_eq!(kept.approval, expected);
    assert_eq!(kept.effective, Approval::AskFirst);
    let made = approvals.rules(kept.effective, &rules, &[]);
    assert_eq!(made[0], rule("edit", "*", "ask"));
    assert_eq!(made.last(), Some(&rules[0]));
    // (When approvals were first kept here is kept, as it's read.)
    let file: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.path().join("box/approval.json")).unwrap())
            .unwrap();
    assert!(file["since"].is_number());
}

#[test]
fn full_access_is_had_only_once_confirmed_and_never_holds_env_reads() {
    let (home, approvals) = machine(None);
    assert_eq!(approvals.effective(Approval::FullAccess).0, Approval::AskFirst);
    fs::write(
        home.path().join("box/approval.json"),
        json!({"fullAccess": true, "last": "fullAccess"}).to_string(),
    )
    .unwrap();
    assert_eq!(approvals.default_approval(), Approval::FullAccess);
    assert_eq!(approvals.effective(Approval::FullAccess).0, Approval::FullAccess);
    // (A machine whose settings aren't its own yet runs with the Don't Allows it has.)
    assert_eq!(approvals.permissions_here(), None);
    let made = approvals.rules(Approval::FullAccess, &[rule("bash", "x", "deny")], &[]);
    assert_eq!(
        &made[..4],
        [
            rule("read", "*.env", "allow"),
            rule("read", "*.env.*", "allow"),
            rule("doom_loop", "*", "allow"),
            rule("external_directory", "*", "allow"),
        ]
    );
    assert_eq!(made[4], rule("bash", "x", "deny"));
}

#[test]
fn a_kept_approval_holds_while_its_rules_are_those_made_for_it() {
    let (_home, approvals) = machine(Some(json!({})));
    let rules = [rule("shell", "*", "ask")];
    let notes = json!({"kuvo": {"approval": "acceptEdits", "applied": "acceptEdits"}});
    let kept = approvals.of_bot(&rules, Some(&notes));
    assert!(kept.stored);
    assert_eq!(kept.effective, Approval::AcceptEdits);
    // (Changed by a client since: as its rules say.)
    let kept = approvals.of_bot(&[], Some(&notes));
    assert!(!kept.stored);
}

#[test]
fn former_asks_go_unless_settings_has_them() {
    let rules = [
        rule("shell", "sudo *", "ask"),
        rule("shell", "git push*", "ask"),
        rule("shell", "make", "allow"),
    ];
    assert_eq!(
        without_former_asks(&rules, &[rule("shell", "git push*", "ask")]),
        [rule("shell", "git push*", "ask"), rule("shell", "make", "allow")]
    );
    assert!(has_former_asks(&rules, &[]));
    assert!(!has_former_asks(&rules[2..], &[]));
}
