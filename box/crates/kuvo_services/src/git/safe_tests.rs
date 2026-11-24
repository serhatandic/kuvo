//! The safe git layer: its arguments against the TS box's for the same inputs, the TS scenarios
//! (`box/test/git-programs.test.ts`, `box/test/git-common.test.ts`) and a hostile repository whose
//! settings name a program for everything git can start. Real repositories in folders of their own;
//! git never sees your own settings (an empty file of the test's own).

use super::*;
use std::fs;
use std::path::Path;

/// Words of an argument list.
fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

/// A scratch folder by its real path (`/var` is a link on a Mac).
fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = real(dir.path()).unwrap();
    (dir, root)
}

/// git as the test runs it: no settings of yours, nothing planted started.
fn git_in(root: &Path, folder: &Path, args: &[&str]) -> String {
    let none = root.join("no-settings");
    if !none.exists() {
        fs::write(&none, "").unwrap();
    }
    let ran = Command::new("git")
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args([
            "-c",
            "hook.planted.enabled=false",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .current_dir(folder)
        .env("GIT_CONFIG_GLOBAL", &none)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(AUTHOR)
        .output()
        .unwrap();
    assert!(
        ran.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    String::from_utf8_lossy(&ran.stdout).trim().to_owned()
}

const AUTHOR: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Test"),
    ("GIT_AUTHOR_EMAIL", "test@example.com"),
    ("GIT_COMMITTER_NAME", "Test"),
    ("GIT_COMMITTER_EMAIL", "test@example.com"),
];

/// A repository with a commit, then `config` added to its settings.
fn repository(root: &Path, name: &str, config: &[(&str, &str)]) -> PathBuf {
    let top = root.join(name);
    fs::create_dir(&top).unwrap();
    git_in(root, &top, &["init", "-q"]);
    fs::write(top.join("a.txt"), "one\n").unwrap();
    git_in(root, &top, &["add", "-A"]);
    git_in(root, &top, &["commit", "-qm", "first"]);
    for (key, value) in config {
        git_in(root, &top, &["config", "--add", key, value]);
    }
    top
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|each| (*each).to_owned()).collect()
}

#[test]
fn own_is_every_and_more() {
    assert_eq!(OWN[..EVERY.len()], EVERY);
}

/// What the TS box answered for each input in the same repositories (`ownArgs`/`askedArgs`, a
/// Mac; on Linux `W`, git's own watcher, is nothing, as it is there). `OWN`, `EVERY`, `NAMED` and
/// `W` stand for the runs they name.
#[rustfmt::skip]
const TABLE: &[(&str, bool, &str, Option<&str>)] = &[
    ("clean", true, "rev-parse HEAD", Some("OWN rev-parse HEAD")),
    ("clean", true, "status --porcelain", Some("OWN status --porcelain")),
    ("clean", true, "diff HEAD", Some("OWN diff --no-ext-diff HEAD")),
    ("clean", true, "log -p -1", Some("OWN log --no-ext-diff -p -1")),
    ("clean", true, "stash show -p", Some("OWN stash show --no-ext-diff -p")),
    ("clean", true, "stash list", Some("OWN stash list")),
    ("clean", true, "blame a.txt", Some("OWN blame a.txt")),
    ("clean", true, "grep one", Some("OWN grep one")),
    ("clean", true, "cat-file blob HEAD:a.txt", Some("OWN cat-file blob HEAD:a.txt")),
    ("clean", true, "cat-file --filters HEAD:a.txt", Some("OWN cat-file --filters HEAD:a.txt")),
    ("clean", true, "format-patch -1", Some("OWN format-patch -1")),
    ("clean", true, "ls-files -tm", Some("OWN ls-files -tm")),
    ("clean", true, "ls-files --stage", Some("OWN ls-files --stage")),
    ("clean", true, "symbolic-ref HEAD", Some("OWN symbolic-ref HEAD")),
    ("clean", true, "symbolic-ref HEAD refs/heads/x", Some("OWN symbolic-ref HEAD refs/heads/x")),
    ("clean", true, "worktree list", Some("OWN worktree list")),
    ("clean", true, "worktree add x", Some("OWN worktree add x")),
    ("clean", true, "-c a.b=c --git-dir .git show", Some("-c a.b=c --git-dir .git OWN show --no-ext-diff")),
    ("clean", true, "--no-pager -C . whatchanged", Some("--no-pager -C . OWN whatchanged --no-ext-diff")),
    ("clean", true, "", Some("OWN")),
    ("clean", false, "commit -m x", Some("EVERY commit -m x")),
    ("clean", false, "diff", Some("EVERY diff --no-ext-diff")),
    ("clean", false, "stash show", Some("EVERY stash show --no-ext-diff")),
    ("clean", false, "clone --quiet -- a b", Some("EVERY clone --quiet -- a b")),
    ("clean", false, "-C . log", Some("-C . EVERY log --no-ext-diff")),
    ("named", true, "rev-parse HEAD", Some("OWN rev-parse HEAD")),
    ("named", true, "status --porcelain", Some("OWN NAMED W status --porcelain")),
    ("named", true, "diff HEAD", Some("OWN NAMED W diff --no-ext-diff --no-textconv HEAD")),
    ("named", true, "log -p -1", Some("OWN NAMED W log --no-ext-diff --no-textconv -p -1")),
    ("named", true, "stash show -p", Some("OWN NAMED W stash show --no-ext-diff --no-textconv -p")),
    ("named", true, "stash list", Some("OWN stash list")),
    ("named", true, "blame a.txt", Some("OWN NAMED W blame --no-textconv a.txt")),
    ("named", true, "grep one", Some("OWN NAMED W grep --no-textconv one")),
    ("named", true, "cat-file blob HEAD:a.txt", Some("OWN cat-file blob HEAD:a.txt")),
    ("named", true, "cat-file --filters HEAD:a.txt", None),
    ("named", true, "format-patch -1", None),
    ("named", true, "ls-files -tm", Some("OWN NAMED W ls-files -tm")),
    ("named", true, "ls-files --stage", Some("OWN ls-files --stage")),
    ("named", true, "symbolic-ref HEAD", Some("OWN symbolic-ref HEAD")),
    ("named", true, "symbolic-ref HEAD refs/heads/x", Some("OWN NAMED W symbolic-ref HEAD refs/heads/x")),
    ("named", true, "worktree list", Some("OWN worktree list")),
    ("named", true, "worktree add x", Some("OWN NAMED W worktree add x")),
    ("named", true, "-c a.b=c --git-dir .git show", Some("-c a.b=c --git-dir .git OWN NAMED W show --no-ext-diff --no-textconv")),
    ("named", true, "--no-pager -C . whatchanged", Some("--no-pager -C . OWN NAMED W whatchanged --no-ext-diff --no-textconv")),
    ("named", true, "", Some("OWN NAMED W")),
    ("named", false, "commit -m x", Some("EVERY W commit -m x")),
    ("named", false, "diff", Some("EVERY W diff --no-ext-diff")),
    ("named", false, "stash show", Some("EVERY W stash show --no-ext-diff")),
    ("named", false, "clone --quiet -- a b", Some("EVERY clone --quiet -- a b")),
    ("named", false, "-C . log", Some("-C . EVERY W log --no-ext-diff")),
    ("unswitchable", true, "rev-parse HEAD", Some("OWN rev-parse HEAD")),
    ("unswitchable", true, "status --porcelain", None),
    ("unswitchable", true, "diff HEAD", None),
    ("unswitchable", true, "log -p -1", None),
    ("unswitchable", true, "stash show -p", None),
    ("unswitchable", true, "stash list", Some("OWN stash list")),
    ("unswitchable", true, "blame a.txt", None),
    ("unswitchable", true, "grep one", None),
    ("unswitchable", true, "cat-file blob HEAD:a.txt", Some("OWN cat-file blob HEAD:a.txt")),
    ("unswitchable", true, "cat-file --filters HEAD:a.txt", None),
    ("unswitchable", true, "format-patch -1", None),
    ("unswitchable", true, "ls-files -tm", None),
    ("unswitchable", true, "ls-files --stage", Some("OWN ls-files --stage")),
    ("unswitchable", true, "symbolic-ref HEAD", Some("OWN symbolic-ref HEAD")),
    ("unswitchable", true, "symbolic-ref HEAD refs/heads/x", None),
    ("unswitchable", true, "worktree list", Some("OWN worktree list")),
    ("unswitchable", true, "worktree add x", None),
    ("unswitchable", true, "-c a.b=c --git-dir .git show", None),
    ("unswitchable", true, "--no-pager -C . whatchanged", None),
    ("unswitchable", true, "", None),
    ("unswitchable", false, "commit -m x", Some("EVERY commit -m x")),
    ("unswitchable", false, "diff", Some("EVERY diff --no-ext-diff")),
    ("unswitchable", false, "stash show", Some("EVERY stash show --no-ext-diff")),
    ("unswitchable", false, "clone --quiet -- a b", Some("EVERY clone --quiet -- a b")),
    ("unswitchable", false, "-C . log", Some("-C . EVERY log --no-ext-diff")),
    ("hookeq", true, "rev-parse HEAD", Some("OWN rev-parse HEAD")),
    ("hookeq", true, "status --porcelain", None),
    ("hookeq", true, "diff HEAD", None),
    ("hookeq", true, "log -p -1", None),
    ("hookeq", true, "stash show -p", None),
    ("hookeq", true, "stash list", Some("OWN stash list")),
    ("hookeq", true, "blame a.txt", None),
    ("hookeq", true, "grep one", None),
    ("hookeq", true, "cat-file blob HEAD:a.txt", Some("OWN cat-file blob HEAD:a.txt")),
    ("hookeq", true, "cat-file --filters HEAD:a.txt", None),
    ("hookeq", true, "format-patch -1", None),
    ("hookeq", true, "ls-files -tm", None),
    ("hookeq", true, "ls-files --stage", Some("OWN ls-files --stage")),
    ("hookeq", true, "symbolic-ref HEAD", Some("OWN symbolic-ref HEAD")),
    ("hookeq", true, "symbolic-ref HEAD refs/heads/x", None),
    ("hookeq", true, "worktree list", Some("OWN worktree list")),
    ("hookeq", true, "worktree add x", None),
    ("hookeq", true, "-c a.b=c --git-dir .git show", None),
    ("hookeq", true, "--no-pager -C . whatchanged", None),
    ("hookeq", true, "", None),
    ("hookeq", false, "commit -m x", Some("EVERY commit -m x")),
    ("hookeq", false, "diff", Some("EVERY diff --no-ext-diff")),
    ("hookeq", false, "stash show", Some("EVERY stash show --no-ext-diff")),
    ("hookeq", false, "clone --quiet -- a b", Some("EVERY clone --quiet -- a b")),
    ("hookeq", false, "-C . log", Some("-C . EVERY log --no-ext-diff")),
];

#[test]
fn the_arguments_are_the_ts_boxs_for_the_same_inputs() {
    let (_dir, root) = scratch();
    let repositories = [
        ("clean", repository(&root, "clean", &[])),
        (
            "named",
            repository(
                &root,
                "named",
                &[
                    ("filter.evil.clean", "cat"),
                    ("filter.evil.smudge", "cat"),
                    ("diff.evil.textconv", "cat"),
                    ("hook.planted.command", "true"),
                    ("hook.planted.event", "pre-commit"),
                    ("merge.evil.driver", "true"),
                    ("core.fsmonitor", "yes"),
                ],
            ),
        ),
        (
            "unswitchable",
            repository(&root, "unswitchable", &[("filter.x=y.clean", "cat")]),
        ),
        (
            "hookeq",
            repository(&root, "hookeq", &[("hook.a=b.command", "true")]),
        ),
    ];
    let expand = |text: &str| -> Vec<String> {
        words(text)
            .into_iter()
            .flat_map(|word| match word.as_str() {
                "OWN" => words("-c core.fsmonitor=false -c core.pager=cat -c log.showSignature=false -c safe.bareRepository=explicit -c core.hooksPath=/dev/null -c commit.gpgSign=false -c tag.gpgSign=false -c credential.helper="),
                "EVERY" => words("-c core.fsmonitor=false -c core.pager=cat -c log.showSignature=false"),
                "NAMED" => words("-c filter.evil.clean= -c filter.evil.required=false -c filter.evil.smudge= -c filter.evil.required=false -c hook.planted.enabled=false"),
                "W" => strings(WATCHER),
                _ => vec![word],
            })
            .collect()
    };
    let here = Here::from_env();
    for (name, own, input, expected) in TABLE {
        let top = &repositories
            .iter()
            .find(|(each, _)| each == name)
            .unwrap()
            .1;
        let args = words(input);
        let answer = if *own {
            own_args_in(&here, &args, Some(top), None)
        } else {
            asked_args_in(&here, &args, Some(top))
        };
        assert_eq!(
            answer.ok(),
            expected.map(expand),
            "{name} {} {input:?}",
            if *own { "own" } else { "asked" }
        );
    }
}

#[test]
fn what_switches_off_what_settings_name() {
    let here = Here::default();
    // As `config --show-scope -z --get-regexp` says it, a scope then each name and value.
    let read = "global\0filter.lfs.clean\nanything\0local\0filter.evil.clean\ncat\0local\0diff.evil.textconv\ncat\0command\0hook.x.command\ntrue\0local\0core.fsmonitor\0";
    let found = switched_off(&here, read, true).unwrap();
    let mut settings = strings(&[
        "-c",
        "filter.evil.clean=",
        "-c",
        "filter.evil.required=false",
        "-c",
        "hook.x.enabled=false",
    ]);
    settings.extend(strings(WATCHER));
    assert_eq!(
        found,
        Found {
            settings,
            converters: true
        }
    );
    // Without scopes, every one is the repository's.
    let found = switched_off(
        &here,
        "filter.lfs.clean\nanything\0core.fsmonitor\n/bin/x\0",
        false,
    )
    .unwrap();
    assert_eq!(
        found.settings,
        strings(&["-c", "filter.lfs.clean=", "-c", "filter.lfs.required=false"])
    );
    assert_eq!(
        switched_off(&here, "local\0filter.a=b.smudge\ncat\0", true),
        Err(Refused::Unswitchable)
    );
    assert_eq!(
        switched_off(&here, "system\0hook.a=b.command\ntrue\0", true),
        Err(Refused::Unswitchable)
    );
    // A system filter with `=` is left as it is, not refused.
    assert!(switched_off(&here, "system\0filter.a=b.smudge\ncat\0", true).is_ok());
}

#[test]
fn what_reads_as_gits_own_watcher() {
    for yes in [
        None,
        Some("true"),
        Some(" YES "),
        Some("on"),
        Some("1"),
        Some("-12"),
    ] {
        assert!(watches(yes), "{yes:?}");
    }
    for no in [
        Some("false"),
        Some("0"),
        Some("01"),
        Some("/bin/watch"),
        Some(""),
        Some("1x"),
    ] {
        assert!(!watches(no), "{no:?}");
    }
    assert_eq!(
        watcher_in("core.fsmonitor\n/x\0core.fsmonitor\ntrue\0"),
        WATCHER
    );
    assert!(watcher_in("core.fsmonitor\ntrue\0core.fsmonitor\n/x\0").is_empty());
    assert!(watcher_in("").is_empty());
}

#[test]
fn the_programs_git_lfs_and_git_crypt_set_up() {
    for (value, program) in [
        ("git-lfs clean -- %f", Some("git-lfs")),
        ("git-lfs filter-process", Some("git-lfs")),
        ("git-lfs smudge --skip -- %f", Some("git-lfs")),
        (
            "\"/usr/local/bin/git-crypt\" smudge --key-name=work-1",
            Some("/usr/local/bin/git-crypt"),
        ),
        ("git-crypt diff", Some("git-crypt")),
        ("git-lfs clean -- %f; touch x", None),
        ("git-lfs clean --key-name=", None),
        ("git lfs clean", None),
        ("git-lfs", None),
    ] {
        assert_eq!(known_program(value), program, "{value}");
    }
}

#[test]
fn options_before_the_command_and_what_only_reads() {
    let args = strings(&[
        "-C",
        "x",
        "-c",
        "a=b",
        "--no-pager",
        "--git-dir",
        "g",
        "log",
        "-p",
    ]);
    let (lead, rest) = split(&args);
    assert_eq!((lead.len(), rest), (7, &args[7..]));
    // A valued option last: nothing after it.
    assert_eq!(split(&strings(&["-c"])).1.len(), 0);
    for read in [
        &["ls-files", "--stage"][..],
        &["symbolic-ref", "-q", "HEAD"],
        &["cat-file", "-p", "HEAD"],
        &["worktree", "prune"],
    ] {
        assert!(plain(&strings(read)), "{read:?}");
    }
    for written in [
        &["ls-files", "-m"][..],
        &["ls-files", "-om"],
        &["ls-files", "--modified"],
        &["symbolic-ref", "-d", "HEAD"],
        &["cat-file", "--textconv", "x"],
        &["worktree", "add"],
        &["write-tree"],
        &[],
    ] {
        assert!(!plain(&strings(written)), "{written:?}");
    }
}

#[test]
fn a_budget_spent_refuses_what_must_be_read_first() {
    let (_dir, root) = scratch();
    let top = repository(&root, "repo", &[]);
    let here = Here::from_env();
    assert_eq!(
        own_args_in(
            &here,
            &strings(&["status"]),
            Some(&top),
            Some(Duration::ZERO)
        ),
        Err(Refused::OutOfTime)
    );
    assert_eq!(
        own_args_in(
            &here,
            &strings(&["status"]),
            Some(&top),
            Some(Duration::from_micros(900))
        ),
        Err(Refused::OutOfTime)
    );
    assert!(
        own_args_in(
            &here,
            &strings(&["rev-parse", "HEAD"]),
            Some(&top),
            Some(Duration::ZERO)
        )
        .is_ok()
    );
    assert!(
        own_args_in(
            &here,
            &strings(&["status"]),
            Some(&top),
            Some(Duration::from_secs(30))
        )
        .is_ok()
    );
}

#[cfg(unix)]
mod hostile {
    //! A repository whose own settings (and a file they include) name a program for everything git
    //! can start, each leaving a mark when it runs.

    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    struct Planted {
        root: PathBuf,
        top: PathBuf,
        marks: PathBuf,
        programs: PathBuf,
        _dir: tempfile::TempDir,
    }

    impl Planted {
        fn ran(&self) -> Vec<String> {
            let mut ran: Vec<String> = fs::read_dir(&self.marks)
                .unwrap()
                .map(|each| each.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            ran.sort();
            ran
        }

        fn forget(&self) {
            fs::remove_dir_all(&self.marks).unwrap();
            fs::create_dir(&self.marks).unwrap();
        }

        fn git(&self, args: &[&str]) -> String {
            git_in(&self.root, &self.top, args)
        }

        /// git with exactly `args`, as the box runs it; stdin given.
        fn run(&self, args: &[String], input: &str) {
            let mut child = Command::new("git")
                .args(args)
                .current_dir(&self.top)
                .env("GIT_CONFIG_GLOBAL", self.root.join("no-settings"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(GIT_QUIET.0, GIT_QUIET.1)
                .envs(AUTHOR)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
            child.wait().unwrap();
        }
    }

    fn planted(driver: &str) -> Planted {
        let (dir, root) = scratch();
        let (top, programs, marks) = (
            root.join("project"),
            root.join("programs"),
            root.join("marks"),
        );
        for folder in [&top, &programs, &marks, &programs.join("hooks")] {
            fs::create_dir(folder).unwrap();
        }
        git_in(&root, &top, &["init", "-q"]);
        fs::write(top.join("a.txt"), "one\n").unwrap();
        fs::write(top.join("b.txt"), "one\n").unwrap();
        fs::write(
            top.join(".gitattributes"),
            format!("a.txt filter={driver} diff={driver} merge={driver}\n"),
        )
        .unwrap();
        git_in(&root, &top, &["add", "-A"]);
        git_in(&root, &top, &["commit", "-qm", "first"]);
        let program = |name: &str, then: &str| {
            let file = programs.join(name);
            fs::write(
                &file,
                format!(
                    "#!/bin/sh\ntouch \"{}/{}\"\n{then}\n",
                    marks.display(),
                    name.replace('/', "-")
                ),
            )
            .unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
            file.display().to_string()
        };
        for hook in [
            "pre-commit",
            "prepare-commit-msg",
            "commit-msg",
            "post-commit",
            "post-checkout",
            "pre-auto-gc",
            "reference-transaction",
            "post-index-change",
            "pre-push",
        ] {
            program(&format!("hooks/{hook}"), "exit 0");
        }
        let config = |key: &str, value: &str| git_in(&root, &top, &["config", "--add", key, value]);
        config("core.fsmonitor", &program("fsmonitor", "exit 0"));
        config(
            "core.hooksPath",
            &programs.join("hooks").display().to_string(),
        );
        config("hook.planted.command", &program("hook-setting", "exit 0"));
        for event in [
            "reference-transaction",
            "post-index-change",
            "pre-commit",
            "post-checkout",
        ] {
            config("hook.planted.event", event);
        }
        config("core.pager", &program("pager", "cat"));
        config("core.editor", &program("editor", "exit 0"));
        config("diff.external", &program("external", "exit 0"));
        config("log.showSignature", "true");
        config("gpg.program", &program("gpg", "exit 1"));
        config("commit.gpgSign", "true");
        config("tag.gpgSign", "true");
        config(&format!("filter.{driver}.clean"), &program("clean", "cat"));
        config(
            &format!("filter.{driver}.smudge"),
            &program("smudge", "cat"),
        );
        config(&format!("filter.{driver}.required"), "true");
        config(
            &format!("merge.{driver}.driver"),
            &program("merge", "exit 1"),
        );
        config("credential.helper", &program("credential", "exit 0"));
        config("protocol.ext.allow", "always");
        config(
            "remote.origin.url",
            &format!("ext::{}", program("ext", "exit 1")),
        );
        config("core.sshCommand", &program("ssh", "exit 1"));
        config("remote.far.url", "ssh://example.invalid/x");
        config("alias.st", &format!("!{}", program("alias", "exit 0")));
        // Its diff programs in a file its settings include.
        let more = programs.join("more.config");
        fs::write(
            &more,
            format!(
                "[diff \"{driver}\"]\n\ttextconv = {}\n\tcommand = {}\n",
                program("textconv", "cat \"$1\""),
                program("command", "exit 0")
            ),
        )
        .unwrap();
        config("include.path", &more.display().to_string());
        fs::write(top.join("a.txt"), "one\ntwo, longer\n").unwrap();
        fs::write(top.join("b.txt"), "one\ntwo\n").unwrap();
        Planted {
            root,
            top,
            marks,
            programs,
            _dir: dir,
        }
    }

    /// What the box runs by itself, as Changes, snapshots, backups and reviews run it.
    const OWN_COMMANDS: &[&[&str]] = &[
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        &["diff"],
        &["diff", "HEAD", "--stat"],
        &["log", "-p", "-2"],
        &["show", "HEAD"],
        &["blame", "a.txt"],
        &["grep", "two"],
        &["ls-files", "-m"],
        &["rev-parse", "HEAD"],
        &["for-each-ref"],
        &["remote", "get-url", "origin"],
        &["add", "-A"],
        &["diff", "--cached"],
        &["write-tree"],
        &["stash", "list"],
        &["branch", "feature"],
        &["update-ref", "refs/kuvo/kept", "HEAD"],
        &["commit", "-qm", "second"],
        &["tag", "-a", "-m", "kept", "kept"],
        &["checkout", "-q", "feature"],
        &["gc", "--auto"],
        &["credential", "fill"],
    ];

    #[test]
    fn the_repositorys_programs_are_really_there_git_itself_runs_them() {
        let planted = planted("evil");
        for args in [
            &["status", "--porcelain"][..],
            &["diff"],
            &["branch", "control"],
            &["commit", "--allow-empty", "-qm", "signed"],
            &["st"],
            &["ls-remote", "origin"],
            &["ls-remote", "far"],
            &["credential", "fill"],
        ] {
            Command::new("git")
                .args(args)
                .current_dir(&planted.top)
                .env("GIT_CONFIG_GLOBAL", planted.root.join("no-settings"))
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
        }
        let ran = planted.ran();
        for mark in [
            "alias",
            "clean",
            "command",
            "fsmonitor",
            "gpg",
            "ext",
            "ssh",
            "external",
        ] {
            assert!(ran.contains(&mark.to_owned()), "{mark} in {ran:?}");
        }
    }

    #[test]
    fn what_the_box_runs_by_itself_starts_none_of_them() {
        let planted = planted("evil");
        let here = Here::from_env();
        for command in OWN_COMMANDS {
            let args = own_args_in(&here, &strings(command), Some(&planted.top), None)
                .unwrap_or_else(|refused| panic!("{command:?}: {refused}"));
            planted.run(&args, "protocol=https\nhost=example.com\n\n");
            assert_eq!(planted.ran(), Vec::<String>::new(), "{command:?}");
        }
        // And it did what it was asked: the commit and the tag are there.
        assert_eq!(planted.git(&["log", "-1", "--format=%s", "main"]), "second");
        assert_eq!(planted.git(&["tag"]), "kept");
    }

    #[test]
    fn what_you_ask_for_runs_its_hooks_but_no_fsmonitor_pager_diff_program_or_signature_check() {
        let planted = planted("evil");
        let here = Here::from_env();
        for command in [
            &["status", "--porcelain"][..],
            &["diff"],
            &["log", "-p", "-1"],
            &["show"],
        ] {
            let args = asked_args_in(&here, &strings(command), Some(&planted.top)).unwrap();
            planted.run(&args, "");
        }
        let ran = planted.ran();
        for never in ["fsmonitor", "pager", "external", "command", "gpg"] {
            assert!(!ran.contains(&never.to_owned()), "{never} in {ran:?}");
        }
        planted.forget();
        let args = asked_args_in(
            &here,
            &strings(&["commit", "-qam", "yours", "--no-gpg-sign"]),
            Some(&planted.top),
        )
        .unwrap();
        planted.run(&args, "");
        let ran = planted.ran();
        assert!(ran.contains(&"hooks-pre-commit".to_owned()), "{ran:?}");
        assert!(
            !ran.contains(&"fsmonitor".to_owned()) && !ran.contains(&"pager".to_owned()),
            "{ran:?}"
        );
    }

    #[test]
    fn a_file_git_lfs_keeps_stays_on_and_one_of_the_same_name_elsewhere_doesnt() {
        let planted = planted("evil");
        let (tools, brought) = (planted.root.join("tools"), planted.root.join("brought"));
        for folder in [&tools, &brought] {
            fs::create_dir(folder).unwrap();
        }
        fs::write(
            tools.join("git-lfs"),
            "#!/bin/sh\ncase \"$1\" in clean) tr a-z A-Z ;; *) cat ;; esac\n",
        )
        .unwrap();
        fs::write(
            brought.join("git-lfs"),
            format!(
                "#!/bin/sh\ntouch \"{}/brought\"\ncat\n",
                planted.marks.display()
            ),
        )
        .unwrap();
        for folder in [&tools, &brought] {
            fs::set_permissions(folder.join("git-lfs"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths(
            std::iter::once(tools.clone())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let here = Here {
            path: Some(path.clone()),
            git_dir_named: false,
        };
        planted.git(&["config", "filter.kept.clean", "git-lfs clean -- %f"]);
        planted.git(&["config", "filter.kept.smudge", "git-lfs smudge -- %f"]);
        planted.git(&[
            "config",
            "filter.other.clean",
            &format!("\"{}\" clean -- %f", brought.join("git-lfs").display()),
        ]);
        let found = switched_off(&here, &own_read(&planted), true).unwrap();
        // The installed git-lfs's filters stay on; the one brought along is switched off.
        assert!(
            !found
                .settings
                .iter()
                .any(|each| each.starts_with("filter.kept.")),
            "{:?}",
            found.settings
        );
        assert!(
            found.settings.contains(&"filter.other.clean=".to_owned()),
            "{:?}",
            found.settings
        );
        // Without it installed, it's switched off too.
        let found = switched_off(&Here::default(), &own_read(&planted), true).unwrap();
        assert!(
            found.settings.contains(&"filter.kept.clean=".to_owned()),
            "{:?}",
            found.settings
        );
        assert_eq!(planted.ran(), Vec::<String>::new());
        let _ = &planted.programs;
    }

    /// The settings read the layer makes, as git says it.
    fn own_read(planted: &Planted) -> String {
        git(&reading(&[], true), Some(&planted.top), LOOK_FOR).out
    }

    #[test]
    fn one_whose_program_has_a_name_settings_cant_switch_off_isnt_read_at_all() {
        let planted = planted("x=y");
        let here = Here::from_env();
        for command in [&["diff"][..], &["status", "--porcelain"]] {
            assert_eq!(
                own_args_in(&here, &strings(command), Some(&planted.top), None),
                Err(Refused::Unswitchable)
            );
        }
        assert_eq!(planted.ran(), Vec::<String>::new());
    }

    #[test]
    fn gits_own_watcher_stays_on_where_settings_ask_on_a_mac_one_naming_a_program_is_off() {
        let planted = planted("evil");
        let here = Here::from_env();
        let on = if cfg!(target_os = "macos") {
            "core.fsmonitor=true"
        } else {
            "core.fsmonitor=false"
        };
        // What git goes by is the last `core.fsmonitor` it's given.
        let watcher = |args: Result<Vec<String>, Refused>| {
            args.unwrap()
                .into_iter()
                .rfind(|each| each.starts_with("core.fsmonitor="))
                .unwrap()
        };
        for command in [&["diff"][..], &["status", "--porcelain"]] {
            assert_eq!(
                watcher(own_args_in(
                    &here,
                    &strings(command),
                    Some(&planted.top),
                    None
                )),
                "core.fsmonitor=false"
            );
        }
        let commit = strings(&["commit", "-m", "x"]);
        assert_eq!(
            watcher(asked_args_in(&here, &commit, Some(&planted.top))),
            "core.fsmonitor=false"
        );
        for yes in ["true", "yes", "1"] {
            planted.git(&["config", "--replace-all", "core.fsmonitor", yes]);
            assert_eq!(
                watcher(own_args_in(
                    &here,
                    &strings(&["diff"]),
                    Some(&planted.top),
                    None
                )),
                on
            );
            assert_eq!(
                watcher(asked_args_in(&here, &commit, Some(&planted.top))),
                on
            );
        }
        planted.git(&[
            "config",
            "--add",
            "core.fsmonitor",
            &planted.programs.join("fsmonitor").display().to_string(),
        ]);
        assert_eq!(
            watcher(own_args_in(
                &here,
                &strings(&["diff"]),
                Some(&planted.top),
                None
            )),
            "core.fsmonitor=false"
        );
        assert_eq!(
            watcher(asked_args_in(&here, &commit, Some(&planted.top))),
            "core.fsmonitor=false"
        );
        assert_eq!(planted.ran(), Vec::<String>::new());
    }
}

#[cfg(unix)]
mod common {
    //! Where git takes a repository's settings and hooks from: its `commondir`.

    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A repository with a commit and a file changed since; `commondir`, what its own says.
    fn changed(root: &Path, name: &str, commondir: Option<&str>) -> PathBuf {
        let top = repository(root, name, &[]);
        fs::write(top.join("a.txt"), "two\n").unwrap();
        if let Some(named) = commondir {
            fs::write(top.join(".git/commondir"), named).unwrap();
        }
        top
    }

    /// A copy of `top`'s repository whose settings and hooks leave a mark, and that mark.
    fn elsewhere(root: &Path, top: &Path, name: &str) -> (PathBuf, PathBuf) {
        let elsewhere = root.join(format!("elsewhere-{name}"));
        let mark = root.join(format!("mark-{name}"));
        git_in(
            root,
            top,
            &[
                "clone",
                "-q",
                "--bare",
                ".",
                &elsewhere.display().to_string(),
            ],
        );
        git_in(root, &elsewhere, &["config", "core.bare", "false"]);
        git_in(
            root,
            &elsewhere,
            &[
                "config",
                "core.fsmonitor",
                &format!("touch '{}'; false", mark.display()),
            ],
        );
        for hook in ["pre-commit", "post-commit", "commit-msg"] {
            let file = elsewhere.join("hooks").join(hook);
            fs::write(&file, format!("#!/bin/sh\ntouch '{}'\n", mark.display())).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        (elsewhere, mark)
    }

    #[test]
    fn a_commondir_leading_to_anothers_runs_no_git_that_could_start_a_program() {
        let (_dir, root) = scratch();
        let here = Here::from_env();
        let top = changed(&root, "repo", None);
        let (elsewhere, mark) = elsewhere(&root, &top, "repo");
        fs::write(
            top.join(".git/commondir"),
            format!("{}\n", elsewhere.display()),
        )
        .unwrap();
        assert_eq!(
            own_args_in(
                &here,
                &strings(&["status", "--porcelain"]),
                Some(&top),
                None
            ),
            Err(Refused::ElsewhereCommon)
        );
        assert_eq!(
            own_args_in(
                &here,
                &strings(&["-C", &top.display().to_string(), "diff"]),
                None,
                None
            ),
            Err(Refused::ElsewhereCommon)
        );
        assert_eq!(
            asked_args_in(&here, &strings(&["commit", "-m", "x"]), Some(&top)),
            Err(Refused::ElsewhereCommon)
        );
        assert!(!mark.exists());
        // (git itself, as your terminal runs it, would have.)
        Command::new("git")
            .arg("status")
            .current_dir(&top)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(mark.exists());
    }

    #[test]
    fn a_broken_git_with_a_bare_repository_made_around_it_runs_no_git_that_could_start_a_program() {
        let (_dir, root) = scratch();
        let here = Here::from_env();
        let top = changed(&root, "repo", None);
        let (elsewhere, mark) = elsewhere(&root, &top, "repo");
        fs::write(top.join(".git/HEAD"), "broken\n").unwrap();
        for name in ["HEAD", "config", "hooks", "objects", "refs"] {
            let ran = Command::new("cp")
                .arg("-R")
                .arg(elsewhere.join(name))
                .arg(top.join(name))
                .status()
                .unwrap();
            assert!(ran.success());
        }
        git_in(
            &root,
            &top,
            &["--git-dir=.", "config", "core.bare", "false"],
        );
        git_in(
            &root,
            &top,
            &[
                "--git-dir=.",
                "config",
                "core.worktree",
                &top.display().to_string(),
            ],
        );
        assert_eq!(
            asked_args_in(&here, &strings(&["commit", "-m", "x"]), Some(&top)),
            Err(Refused::ElsewhereCommon)
        );
        // Not where `GIT_DIR` or `--git-dir` names the repository: that's asked for.
        let named = Here {
            git_dir_named: true,
            ..here.clone()
        };
        assert!(asked_args_in(&named, &strings(&["commit", "-m", "x"]), Some(&top)).is_ok());
        assert!(!mark.exists());
        Command::new("git")
            .arg("status")
            .current_dir(&top)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(mark.exists());
    }

    #[test]
    fn a_commondir_naming_itself_or_none_and_a_worktrees_naming_its_repository_are_as_ever() {
        let (_dir, root) = scratch();
        let here = Here::from_env();
        let own = changed(&root, "own", Some("./\n"));
        let none = changed(&root, "none", None);
        let tree = root.join("tree");
        git_in(
            &root,
            &none,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "try",
                &tree.display().to_string(),
            ],
        );
        for folder in [&own, &none, &tree] {
            assert!(
                own_args_in(
                    &here,
                    &strings(&["status", "--porcelain"]),
                    Some(folder),
                    None
                )
                .is_ok(),
                "{folder:?}"
            );
            assert!(
                asked_args_in(&here, &strings(&["commit", "-m", "x"]), Some(folder)).is_ok(),
                "{folder:?}"
            );
        }
        // A worktree whose own folder's commondir leads to another repository isn't.
        let (elsewhere, _) = elsewhere(&root, &none, "none");
        fs::write(
            none.join(".git/worktrees/tree/commondir"),
            format!("{}\n", elsewhere.display()),
        )
        .unwrap();
        assert_eq!(
            asked_args_in(&here, &strings(&["commit", "-m", "x"]), Some(&tree)),
            Err(Refused::ElsewhereCommon)
        );
        // Nor does one that makes a repository have any to ask about.
        let clone = strings(&[
            "clone",
            "--quiet",
            "--",
            &own.display().to_string(),
            &root.join("cloned").display().to_string(),
        ]);
        assert!(asked_args_in(&here, &clone, Some(&own)).is_ok());
        // A commondir that can't be read as a small regular file (a link to one git follows): refused.
        let named = root.join("named");
        fs::write(&named, format!("{}\n", elsewhere.display())).unwrap();
        fs::remove_file(none.join(".git/worktrees/tree/commondir")).unwrap();
        std::os::unix::fs::symlink(&named, none.join(".git/worktrees/tree/commondir")).unwrap();
        assert_eq!(
            asked_args_in(&here, &strings(&["commit", "-m", "x"]), Some(&tree)),
            Err(Refused::ElsewhereCommon)
        );
    }
}
