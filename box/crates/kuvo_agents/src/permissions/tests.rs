use super::{Allowance, ConfigLocation, Decision, Effect, Policy, Rule, check, read_blocks, shell};
use crate::session::PermissionAsk;
use std::{fs, path::PathBuf};

fn ask(action: &str, name: impl Into<String>) -> PermissionAsk {
    PermissionAsk {
        id: "fixture".into(),
        tool_call_id: None,
        action: action.into(),
        resources: vec![name.into()],
        save: Vec::new(),
        message: None,
        metadata: Default::default(),
        sandboxed: None,
        already_weighed: false,
        unclassified: false,
    }
}

fn has(line: &str, command: &str) -> bool {
    shell::split(line)
        .parts()
        .iter()
        .any(|part| part == command)
}

#[test]
fn each_command_counts_in_groups_compounds_and_substitutions() {
    assert_eq!(
        shell::split("a && b || c").parts(),
        ["a && b || c", "a", "b", "c"]
    );
    for (line, command) in [
        ("true & git push", "git push"),
        ("if true; then git push; fi", "git push"),
        ("{ git push; }", "git push"),
        ("! git push", "git push"),
        ("time git push", "git push"),
        ("for r in o; do git push $r; done", "git push $r"),
        ("(cd x && git push)", "git push"),
        ("echo \"$(git push)\"", "git push"),
        ("echo `git push`", "git push"),
        ("diff <(git show HEAD:a) a", "git show HEAD:a"),
    ] {
        assert!(has(line, command), "{line}");
    }
}

#[test]
fn blocks_apply_to_prefixed_commands_and_shell_commands_inside_commands() {
    let policy = Policy::new(
        PathBuf::from("project"),
        Some(vec![Rule::new("shell", "git push *", Effect::Deny)]),
    );
    for line in [
        "# now let's push\ngit push origin main",
        "git commit -m \"$(cat <<'EOF'\nIt's a fix (really)\nEOF\n)\" && git push origin main",
        "cat <<EOF && git push origin main\nbody\nEOF",
        "git \\\npush",
        "VAR=1 git push",
        "2>/dev/null git push",
        "sudo -u me git push",
        "env FOO=1 nohup git push &",
        "git -C ../other push origin",
        "bash -c 'cd x && git push'",
        "sh -lc \"git push\"",
        "echo 'oops; git push",
        "git add -A && git commit -m x && git push origin main",
        "cd /tmp/project; git push",
        "ls | git push",
    ] {
        assert_eq!(
            check(&policy, &ask("shell", line), false, |_| false),
            Some(Decision::Deny),
            "{line}"
        );
    }
    for line in [
        "ls",
        "git status",
        "git commit -m \"wip; git push later\"",
        "cat <<'EOF' > notes.md\ngit push\nEOF",
        "echo done # git push later",
    ] {
        assert_eq!(
            check(&policy, &ask("shell", line), false, |_| false),
            None,
            "{line}"
        );
    }
}

#[test]
fn commands_under_wrappers_and_programs_are_checked_without_assuming_dynamic_arguments() {
    for line in [
        "echo a\\ #; rm -rf /",
        "cat <<< \"x\"\nrm -rf /\nx",
        "(( n << 2 ))\nrm -rf /\n2",
        "sudo -n rm -rf /",
        "command -p rm -rf /",
        "caffeinate -u rm -rf /",
        "time -p rm -rf /",
        "timeout 5 rm -rf /",
        "/bin/rm -rf /",
        "\\rm -rf /",
        "\"rm\" -rf /",
        "eval \"rm -rf /\"",
        "env -S \"rm -rf /\" ls",
        "bash <<EOF\nrm -rf /\nEOF",
    ] {
        assert!(has(line, "rm -rf /"), "{line}");
    }
    for (line, command) in [
        ("sudo -u bob ls", "ls"),
        ("git \"push\" origin", "git push origin"),
        ("echo / | xargs -n 1 rm -rf", "rm -rf"),
        ("find . -name x -exec rm -rf {} \\;", "rm -rf '{}'"),
    ] {
        assert!(has(line, command), "{line}");
    }
    for (line, known) in [
        ("bash <<EOF\nrm -rf /\nEOF", false),
        ("curl -s x | sh", false),
        ("bash scripts/build.sh 2>&1", true),
        ("cd app && npm run build 2>&1 | tail -5", true),
        ("echo $((1 << 2))", true),
        ("X=rm; $X -rf /", false),
    ] {
        assert_eq!(shell::commands(line).known, known, "{line}");
    }
    let policy = Policy::new(
        PathBuf::from("project"),
        Some(vec![Rule::new("shell", "git push *", Effect::Deny)]),
    );
    assert_eq!(
        check(&policy, &ask("shell", "echo words | sh"), false, |_| false),
        Some(Decision::Ask)
    );
}

#[test]
fn quoting_redirection_and_heredoc_data_stay_in_their_command() {
    for line in [
        "git commit -m \"wip; git push later\"",
        "git commit -m 'docs (git push) flow'",
    ] {
        assert_eq!(shell::split(line).parts(), [line]);
    }
    assert_eq!(
        &shell::split("ls 2>&1 | grep x").parts()[1..],
        ["ls 2>&1", "grep x"]
    );
    assert_eq!(
        &shell::split("make &> log; echo done").parts()[1..],
        ["make &> log", "echo done"]
    );
    let line = "cat > notes <<'EOF'\ngit push\nEOF\necho hi";
    assert!(!has(line, "git push"));
    assert!(has(line, "echo hi"));
}

#[test]
fn missing_blocks_ask_about_changes_and_auto_tracks_the_sandbox() {
    let temp = tempfile::tempdir().unwrap();
    let mut policy = Policy::new(temp.path().to_owned(), None);
    let shell = ask("shell", "git push");
    let read = ask("read", "notes.txt");
    assert_eq!(
        check(&policy, &shell, false, |_| false),
        Some(Decision::Ask)
    );
    assert_eq!(check(&policy, &read, false, |_| false), None);

    policy.blocks = Some(Vec::new());
    policy.auto_without_sandbox = true;
    assert_eq!(
        check(&policy, &shell, false, |_| false),
        Some(Decision::Ask)
    );
    assert_eq!(
        check(&policy, &ask("edit", "notes.txt"), false, |_| false),
        None
    );
    policy.auto_without_sandbox = false;
    assert_eq!(check(&policy, &shell, false, |_| false), None);
    policy.auto_without_sandbox = true;
    assert_eq!(
        check(&policy, &shell, false, |_| false),
        Some(Decision::Ask)
    );
}

#[test]
fn project_and_owned_paths_follow_native_plan_and_dotenv_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("project");
    let owned = temp.path().join("adapter-plans");
    let mut policy = Policy::new(folder.clone(), Some(Vec::new()));
    let owns = |file: &std::path::Path| file.starts_with(&owned);
    assert_eq!(
        check(&policy, &ask("edit", "notes.txt"), true, owns),
        Some(Decision::Deny)
    );
    assert_eq!(
        check(
            &policy,
            &ask("edit", owned.join("plan.txt").to_string_lossy()),
            false,
            owns
        ),
        None
    );
    assert_eq!(
        check(&policy, &ask("read", ".env"), false, owns),
        Some(Decision::Ask)
    );
    assert_eq!(
        check(&policy, &ask("read", ".env.example"), false, owns),
        None
    );
    assert_eq!(
        check(&policy, &ask("read", "../another/private.txt"), false, owns),
        Some(Decision::Ask)
    );
    let other = temp.path().join("another/private.txt");
    let name = other.to_string_lossy().replace('\\', "/");
    policy.blocks = Some(vec![Rule::new("external_directory", name, Effect::Deny)]);
    assert_eq!(
        check(&policy, &ask("read", "../another/private.txt"), false, owns),
        Some(Decision::Deny)
    );
    policy.attachments.insert(other);
    assert_eq!(
        check(&policy, &ask("read", "../another/private.txt"), false, owns),
        None
    );
}

#[test]
fn always_covers_only_its_commands_and_live_blocks_still_win() {
    let mut policy = Policy::new(PathBuf::from("project"), Some(Vec::new()));
    policy
        .session_rules
        .push(Rule::new("shell", "*", Effect::Ask));
    policy.allowed.push(Allowance {
        action: "shell".into(),
        patterns: vec!["git status *".into()],
    });
    for line in ["git status", "cd elsewhere; git status -sb"] {
        assert_eq!(check(&policy, &ask("shell", line), false, |_| false), None);
    }
    assert_eq!(
        check(
            &policy,
            &ask("shell", "git status && git push"),
            false,
            |_| false
        ),
        Some(Decision::Ask)
    );
    policy.blocks = Some(vec![Rule::new("shell", "git status *", Effect::Deny)]);
    assert_eq!(
        check(&policy, &ask("shell", "git status"), false, |_| false),
        Some(Decision::Deny)
    );
}

#[tokio::test]
async fn reads_existing_block_settings_in_order_and_leaves_the_files_in_place() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let old = root.join("keys/settings.json");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::write(
        &old,
        br#"{"policies":[{"action":"tool.use","resource":"shell:git push *","effect":"deny"}]}"#,
    )
    .unwrap();
    let repo = root.join("project.jsonc");
    let project = br#"{
      // Local overrides.
      "website": "https://example.invalid/a",
      "policies": [
        {"action":"tool.use","resource":"shell:git push safe","effect":"allow"},
        {"action":"tool.use","resource":"edit:","effect":"next-engine-block"},
        {"action":"mcp.use","resource":"*","effect":"deny"},
      ],
    }"#;
    fs::write(&repo, project).unwrap();
    let blocks = read_blocks(root, ConfigLocation::Machine, vec![repo.clone()])
        .await
        .unwrap();
    let policy = Policy::new(root.to_owned(), Some(blocks));
    assert_eq!(
        check(&policy, &ask("shell", "git push safe"), false, |_| false),
        None
    );
    assert_eq!(
        check(&policy, &ask("shell", "git push other"), false, |_| false),
        Some(Decision::Deny)
    );
    assert_eq!(
        check(&policy, &ask("edit", "a.txt"), false, |_| false),
        Some(Decision::Deny)
    );
    assert_eq!(fs::read(&repo).unwrap(), project);
}
