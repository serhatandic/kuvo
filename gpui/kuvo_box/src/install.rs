//! Running the box's installer, the one the app carries (PROTOCOL.md, "Installing this computer's
//! box"): `sh <bundle>/Contents/Resources/box-release/kuvo-install status|new|move --app <bundle>
//! [--root <name>]`, its JSON lines read as they come.
//!
//! The box decides everything: what this computer holds (`verdict`), what to run next (`verb`),
//! and every word shown. This only runs what it's told and hands on what was said.
//!
//! One thing is the app's own, checked before the installer is ever started (the installer checks
//! it again): which build may install at all. A build that isn't run from a bundle carrying a
//! release (`cargo run`) only attaches to a box that's there; Kuvo Code itself (`run.kuvo.code`)
//! installs into `~/.kuvo-code` alone, and a copy only into a root of its own.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// Kuvo Code's own bundle ID: the only one that installs into the real `~/.kuvo-code`.
pub const REAL_APP: &str = "run.kuvo.code";
/// The box's home in your home folder, for Kuvo Code itself.
pub const REAL_ROOT: &str = ".kuvo-code";

/// What an app bundle carries for this Mac's box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    /// The app's bundle (`…/Kuvo Code.app`), which the installer is told (`--app`): the box reads
    /// its ID from it, and finds `kuvo-keys` in it.
    pub bundle: PathBuf,
    /// Its bundle ID.
    pub id: String,
    /// The box's release: `boxes.json`, the pack, and `kuvo-install` beside them.
    pub release: PathBuf,
    pub installer: PathBuf,
    /// The one-time Keychain hand-over tool, where the bundle has it (`Contents/MacOS/kuvo-keys`).
    /// Only the box's move runs it, never the app.
    pub keys: Option<PathBuf>,
}

/// What the bundle around `executable` carries (`<bundle>/Contents/MacOS/<executable>`); none for
/// a program that isn't in a bundle, or a bundle with no ID or no release in it.
pub fn carried(executable: &Path) -> Option<Carried> {
    let contents = executable.parent().filter(|folder| folder.file_name().is_some_and(|name| name == "MacOS"))?.parent()?;
    let bundle = contents.parent()?;
    if contents.file_name()? != "Contents" || bundle.extension()? != "app" {
        return None;
    }
    let id = bundle_id(&std::fs::read_to_string(contents.join("Info.plist")).ok()?)?;
    let release = contents.join("Resources/box-release");
    let installer = release.join("kuvo-install");
    if !installer.is_file() || !release.join("boxes.json").is_file() {
        return None;
    }
    let keys = Some(contents.join("MacOS/kuvo-keys")).filter(|keys| keys.is_file());
    Some(Carried { bundle: bundle.to_owned(), id, release, installer, keys })
}

/// `CFBundleIdentifier` in an Info.plist written as XML (as every build of ours writes it).
fn bundle_id(plist: &str) -> Option<String> {
    let after = &plist[plist.find("<key>CFBundleIdentifier</key>")? + "<key>CFBundleIdentifier</key>".len()..];
    let value = &after[after.find("<string>")? + "<string>".len()..];
    let id = value[..value.find("</string>")?].trim();
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')).then(|| id.to_owned())
}

/// Why this build installs nothing, in words to show as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlyAttaches(pub String);

/// The installer, for a build that may run it.
#[derive(Debug, Clone)]
pub struct Installer {
    carried: Carried,
    root: String,
    shell: PathBuf,
}

/// One of the installer's verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Status,
    New,
    Move,
}

impl Verb {
    pub fn word(self) -> &'static str {
        match self {
            Verb::Status => "status",
            Verb::New => "new",
            Verb::Move => "move",
        }
    }
}

/// The installer's last line: its answer, as the box said it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Answer {
    /// `done`, `nothing`, `refused`, `failed` or `waits`; none for `status`'s own answer.
    pub result: Option<String>,
    pub verdict: Option<String>,
    /// What to run next, as `status` names it.
    pub verb: Option<String>,
    /// Words to show as they are.
    pub message: Option<String>,
    /// Whether what was made runs now.
    pub started: Option<bool>,
    /// How a move stands (`kuvo/box/rehome`'s answer).
    pub status: Value,
}

/// What came of one run.
#[derive(Debug, Clone, PartialEq)]
pub enum Ran {
    Answered { exit: i32, answer: Answer },
    /// It couldn't be run, or said nothing that can be read: an app's fault, never the user's.
    Broken(String),
}

/// What the installer is doing, for the window to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Asking what this computer holds.
    Asking,
    /// Running what `status` named, for a computer that holds `verdict`.
    Running { verb: Verb, verdict: Option<String> },
    /// A step of it, in the box's words where it sends any (a move's steps: `title`, and where it
    /// is among them, `index` of `of`).
    // BOX-NEEDED: the installer's own first steps (`checking`, `engine`) have no words, so the
    // window says only that it's setting up. `{"step", "title"}` would be drawn as it is.
    Step(Step),
}

/// A step, as the box says it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Step {
    pub step: String,
    pub title: Option<String>,
    /// Where it is among the steps (from 1), and how many there are.
    pub index: Option<u32>,
    pub of: Option<u32>,
}

impl Step {
    /// A step out of what the box said: a line of the installer's, or a move's `progress`.
    fn said(said: &Value) -> Option<Step> {
        let count = |name: &str| said.get(name).and_then(Value::as_u64).and_then(|count| u32::try_from(count).ok());
        Some(Step { step: said.get("step")?.as_str()?.to_owned(), title: said.get("title").and_then(Value::as_str).map(str::to_owned), index: count("index"), of: count("of") })
    }
}

/// How setting up ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Nothing to run: the app attaches to what's here.
    Attach,
    /// A box was made or started: its server is up in a moment.
    Started,
    /// Made, but it doesn't run (a demo copy's home): said in `message`.
    NotStarted,
    /// Refused or failed, in the box's words.
    Stopped,
    /// Another move holds the lock.
    Waits,
    Broken,
}

/// What came of `status` and the verb it named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub end: End,
    /// What this computer holds, where the box judged it.
    pub verdict: Option<String>,
    /// What was run after `status`.
    pub ran: Option<Verb>,
    pub message: Option<String>,
    /// Where a move stands that's still at work in a process of its own (`status.progress`): one
    /// an app that quit left running, to draw while it's waited for.
    pub progress: Option<Step>,
}

impl Outcome {
    /// An outcome with no move still at work behind it.
    pub fn new(end: End, verdict: Option<String>, ran: Option<Verb>, message: Option<String>) -> Outcome {
        Outcome { end, verdict, ran, message, progress: None }
    }
}

impl Installer {
    /// The installer for the app at `executable`, or why this build installs nothing. `demo_root`:
    /// the root a copy of the app was given (`KUVO_DEMO_MACHINE_ROOT`: a folder's name in your
    /// home folder).
    pub fn for_app(executable: &Path, demo_root: Option<&str>) -> Result<Installer, OnlyAttaches> {
        let Some(carried) = carried(executable) else {
            return Err(OnlyAttaches("This build of Kuvo doesn’t set up a computer. It connects to Kuvo where it’s already set up.".into()));
        };
        let root = match (carried.id == REAL_APP, demo_root) {
            (true, None) => REAL_ROOT.to_owned(),
            (true, Some(_)) => return Err(OnlyAttaches("Kuvo Code itself sets up in ~/.kuvo-code only.".into())),
            (false, Some(root)) if root != REAL_ROOT && !root.is_empty() && root != "." && root != ".." && root.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) => root.to_owned(),
            (false, _) => return Err(OnlyAttaches("This copy of Kuvo doesn’t set up a computer: only Kuvo Code itself does, or a copy given a folder of its own.".into())),
        };
        Ok(Installer { carried, root, shell: PathBuf::from("/bin/sh") })
    }

    pub fn carried(&self) -> &Carried {
        &self.carried
    }

    /// The box's home, as a folder's name in your home folder.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Runs one verb to its end, handing each step on as it's said.
    pub fn run(&self, verb: Verb, on: &mut dyn FnMut(Progress)) -> Ran {
        let mut command = Command::new(&self.shell);
        command.arg(&self.carried.installer).arg(verb.word());
        // (A Mac's installer is told its app; elsewhere there's no bundle to name.)
        if cfg!(target_os = "macos") {
            command.arg("--app").arg(&self.carried.bundle);
        }
        if self.root != REAL_ROOT {
            command.arg("--root").arg(&self.root);
        }
        // Nothing this app was started with reaches the box's own commands (the script drops them
        // too): a shell's, an older app's.
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KUVO_") {
                command.env_remove(name);
            }
        }
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return Ran::Broken(format!("Its installer couldn’t be started ({error}).")),
        };
        // Its stderr is read beside its lines, so neither waits on the other.
        let errors = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut said = String::new();
                let _ = stderr.read_to_string(&mut said);
                said
            })
        });
        let mut answer = None;
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(Value::Object(said)) = serde_json::from_str::<Value>(&line) else { continue };
                // (An answer names its result or its verdict; a step neither.)
                if said.get("result").is_none()
                    && said.get("verdict").is_none()
                    && let Some(step) = Step::said(&Value::Object(said.clone()))
                {
                    on(Progress::Step(step));
                    continue;
                }
                let word = |name: &str| said.get(name).and_then(Value::as_str).map(str::to_owned);
                answer = Some(Answer {
                    result: word("result"),
                    verdict: word("verdict"),
                    verb: word("verb"),
                    message: word("message").filter(|message| !message.trim().is_empty()),
                    started: said.get("started").and_then(Value::as_bool),
                    status: said.get("status").cloned().unwrap_or(Value::Null),
                });
            }
        }
        let exit = child.wait().ok().and_then(|status| status.code());
        let errors = errors.and_then(|reading| reading.join().ok()).unwrap_or_default();
        let last = errors.lines().rev().find(|line| !line.trim().is_empty()).map(|line| line.trim().to_owned());
        match (exit, answer) {
            // (Exit 2: not asked as its usage says. It prints no JSON then.)
            (Some(exit), Some(answer)) if exit != 2 && (answer.result.is_some() || answer.verdict.is_some()) => Ran::Answered { exit, answer },
            (exit, _) => Ran::Broken(match (last, exit) {
                (Some(last), _) => format!("Its installer said: {last}"),
                (None, Some(exit)) => format!("Its installer ended without an answer (exit {exit})."),
                (None, None) => "Its installer was stopped before it answered.".into(),
            }),
        }
    }

    /// Asks what this computer holds, then runs what the box names, if anything.
    pub fn set_up(&self, on: &mut dyn FnMut(Progress)) -> Outcome {
        on(Progress::Asking);
        let (exit, status) = match self.run(Verb::Status, on) {
            Ran::Answered { exit, answer } => (exit, answer),
            Ran::Broken(why) => return Outcome { end: End::Broken, verdict: None, ran: None, message: Some(why), progress: None },
        };
        if let Some(end) = stopped(exit, &status) {
            return Outcome { end, verdict: status.verdict, ran: None, message: status.message, progress: None };
        }
        let verb = match status.verb.as_deref() {
            None => {
                let waits = status.verdict.as_deref() == Some("waits");
                let progress = waits.then(|| status.status.get("progress").and_then(Step::said)).flatten();
                return Outcome { end: if waits { End::Waits } else { End::Attach }, verdict: status.verdict, ran: None, message: status.message, progress };
            }
            Some("new") => Verb::New,
            Some("move") => Verb::Move,
            // (A release newer than this app would name only what this app carries: its own.)
            Some(other) => return Outcome { end: End::Broken, verdict: status.verdict, ran: None, message: Some(format!("Its installer asked for “{other}”, which this app doesn’t know.")), progress: None },
        };
        on(Progress::Running { verb, verdict: status.verdict.clone() });
        let (exit, answer) = match self.run(verb, on) {
            Ran::Answered { exit, answer } => (exit, answer),
            Ran::Broken(why) => return Outcome { end: End::Broken, verdict: status.verdict, ran: Some(verb), message: Some(why), progress: None },
        };
        let verdict = answer.verdict.clone().or(status.verdict);
        let end = match stopped(exit, &answer) {
            Some(end) => end,
            None if answer.started == Some(false) => End::NotStarted,
            None => End::Started,
        };
        Outcome { end, verdict, ran: Some(verb), message: answer.message, progress: None }
    }
}

impl crate::BoxClient {
    /// Offers this box the release an app on its computer carries (`kuvo/box/update {from}`,
    /// PROTOCOL.md, "Updates"): it takes one newer than itself at its next quiet moment, and
    /// refuses any other (`409`).
    pub fn offer_release(&self, folder: &Path) -> crate::Result<Value> {
        self.value(&crate::transport::Request::post("kuvo/box/update", serde_json::json!({ "from": folder })))
    }
}

/// Whether an answer says it went no further: another move holds the lock (exit 4), or it was
/// refused or failed. None for one that's done, did nothing, or only judged.
fn stopped(exit: i32, answer: &Answer) -> Option<End> {
    match answer.result.as_deref() {
        Some("waits") => Some(End::Waits),
        _ if exit == 4 => Some(End::Waits),
        Some("done" | "nothing") if exit == 0 => None,
        None if exit == 0 => None,
        _ => Some(End::Stopped),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_s_id_is_read_from_its_plist() {
        let plist = "<plist><dict>\n<key>CFBundleExecutable</key><string>kuvo</string>\n<key>CFBundleIdentifier</key>\n\t<string>run.kuvo.code</string></dict></plist>";
        assert_eq!(bundle_id(plist).as_deref(), Some("run.kuvo.code"));
        // (Nothing is taken for an ID that couldn't be one.)
        for plist in ["", "bplist00", "<key>CFBundleIdentifier</key><string></string>", "<key>CFBundleIdentifier</key><string>a b; rm</string>"] {
            assert_eq!(bundle_id(plist), None, "{plist}");
        }
    }
}

/// A stand-in app bundle for tests and shots: laid out as the real one, its installer a shell
/// script that says what the test tells it to and notes how it was asked. Nothing real is run,
/// fetched or written outside its folder.
#[cfg(feature = "stand-in")]
pub mod stand_in {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    const SCRIPT: &str = r#"#!/bin/sh
D=$(cd "$(dirname "$0")" && pwd -P)
echo "$*" >>"$D/asked"
env | sed -n 's/^\(KUVO_[A-Za-z0-9_]*\)=.*/\1/p' >>"$D/env"
n=$(grep -c -e "^$1 " -e "^$1\$" "$D/asked")
f="$D/said/$1.$n"
[ -f "$f" ] || f="$D/said/$1"
[ -f "$f.stderr" ] && cat "$f.stderr" >&2
[ -f "$f" ] || { echo "Usage: sh kuvo-install status|new|move [--app <bundle>] [--root <name>]" >&2; exit 2; }
cat "$f"
exit "$(cat "$f.exit" 2>/dev/null || echo 0)"
"#;

    pub struct Bundle {
        pub path: PathBuf,
    }

    impl Bundle {
        /// A bundle named `Kuvo Code.app` in `folder`, with bundle ID `id`, carrying a release
        /// whose installer is the stand-in, and a stand-in `kuvo-keys`.
        pub fn make(folder: &Path, id: &str) -> Bundle {
            let path = folder.join("Kuvo Code.app");
            let release = path.join("Contents/Resources/box-release");
            fs::create_dir_all(release.join("said")).unwrap();
            fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
            fs::write(path.join("Contents/Info.plist"), format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\t<key>CFBundleIdentifier</key>\n\t<string>{id}</string>\n</dict></plist>\n")).unwrap();
            fs::write(release.join("boxes.json"), "{}").unwrap();
            for (file, text) in [(release.join("kuvo-install"), SCRIPT), (path.join("Contents/MacOS/kuvo"), "#!/bin/sh\nexit 0\n"), (path.join("Contents/MacOS/kuvo-keys"), "#!/bin/sh\nexit 2\n")] {
                fs::write(&file, text).unwrap();
                fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
            }
            Bundle { path }
        }

        /// The app's own program in it.
        pub fn executable(&self) -> PathBuf {
            self.path.join("Contents/MacOS/kuvo")
        }

        fn release(&self) -> PathBuf {
            self.path.join("Contents/Resources/box-release")
        }

        /// What the installer says when asked `verb`: these lines, then this exit. `nth`: only
        /// the nth time it's asked that (from 1); none: every time not said otherwise.
        pub fn says(&self, verb: &str, nth: Option<usize>, exit: i32, lines: &[&str]) -> &Bundle {
            let name = nth.map_or_else(|| verb.to_owned(), |nth| format!("{verb}.{nth}"));
            let said = self.release().join("said");
            fs::write(said.join(&name), lines.iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
            fs::write(said.join(format!("{name}.exit")), exit.to_string()).unwrap();
            self
        }

        /// What it also says on standard error when asked `verb`.
        pub fn complains(&self, verb: &str, words: &str) -> &Bundle {
            fs::write(self.release().join("said").join(format!("{verb}.stderr")), format!("{words}\n")).unwrap();
            self
        }

        /// How it was asked, each time: its arguments on a line.
        pub fn asked(&self) -> Vec<String> {
            fs::read_to_string(self.release().join("asked")).unwrap_or_default().lines().map(str::to_owned).collect()
        }

        /// Every `KUVO_*` variable it was ever started with.
        pub fn kuvo_variables(&self) -> Vec<String> {
            fs::read_to_string(self.release().join("env")).unwrap_or_default().lines().map(str::to_owned).collect()
        }
    }
}
