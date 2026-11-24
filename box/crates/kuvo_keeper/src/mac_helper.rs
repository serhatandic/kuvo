//! Launch the signed Helper already installed in a Mac's home.

mod handover;

pub use handover::{run_handover, watch_handover};

use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use kuvo_os::platform::Account;
use kuvo_store::{files, home::Home};

const LABEL: &str = "run.kuvo.code.machine";
const WATCH_LABEL: &str = "run.kuvo.code.machine.permissions";
const HELPER: &str = "Kuvo Helper.app";
const AT_LOGIN: &str = "Kuvo Helper starts Kuvo on this Mac at login.";

pub fn pause(home: &Home) -> io::Result<()> {
    let keeper = home.bin().join("kuvo-start");
    if !keeper.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "its keeper is missing.",
        ));
    }
    let account = home
        .root()
        .parent()
        .filter(|folder| home.root() == folder.join(".kuvo-code"))
        .ok_or_else(|| io::Error::other("its keeper does not run from this folder."))?;
    let mut task = kuvo_os::process::Spawn::new("/bin/sh", std::env::vars_os().collect());
    task.env.insert("HOME".into(), account.as_os_str().into());
    task.cwd = Some(home.root().to_owned());
    task.args = vec![keeper.into_os_string(), "pause".into()];
    task.spawn()?.leave_running_on_drop();
    Ok(())
}

/// Inspect or unregister the login job for the account's own Mac installation.
/// A scratch profile can only use launch tools inside that profile.
pub fn uninstall(home: &Home, check: bool) -> io::Result<Option<bool>> {
    let account = Account::current()?;
    let profile = account
        .home_folder(
            kuvo_os::platform::Platform::Mac,
            &std::env::vars_os().collect(),
        )?
        .canonicalize()?;
    if home.root().canonicalize()? != profile.join(".kuvo-code") {
        return Ok(None);
    }
    let real = profile == account.home.canonicalize()?;
    let Some(agent) = LaunchAgent::for_account(&profile, real) else {
        return Ok(None);
    };
    let mac = files::text_of(&home.root().join("box/kind"))
        .as_deref()
        .map(str::trim)
        == Some("mac");
    agent.unregister(home, &profile, WATCH_LABEL, "watch-machine", check)?;
    let mode = if mac { "keep" } else { "keep-machine" };
    agent
        .unregister(home, &profile, LABEL, mode, check)
        .map(Some)
}

#[derive(Clone, Copy)]
pub enum Started {
    Installed,
    Kicked,
    OpenedForLoginItems,
    OpenedAfterRefusal,
}

impl Started {
    pub fn message(self) -> &'static str {
        match self {
            Self::Installed => "Kuvo Helper runs Kuvo on this Mac now, and at login.",
            Self::Kicked => "Kuvo Helper runs Kuvo on this Mac.",
            Self::OpenedForLoginItems => {
                "Kuvo Helper is turned off in System Settings › General › Login Items, so it was opened for now, to run Kuvo on this Mac until you log out. Turn it on there to start it at login."
            }
            Self::OpenedAfterRefusal => {
                "Kuvo Helper runs the new version now, opened until you log out: launchd didn't start it."
            }
        }
    }
}

pub struct LaunchAgent {
    launchctl: PathBuf,
    open: PathBuf,
    plist: PathBuf,
}

impl LaunchAgent {
    pub fn for_account(profile: &Path, real: bool) -> Option<Self> {
        let library = profile.join("Library");
        let agents = library.join("LaunchAgents");
        let (launchctl, open) = if real {
            ("/bin/launchctl".into(), "/usr/bin/open".into())
        } else {
            let actual = Account::current().ok()?.home.canonicalize().ok()?;
            if profile == actual
                || [&library, &agents].into_iter().any(|folder| {
                    fs::symlink_metadata(folder).is_ok()
                        && !folder
                            .canonicalize()
                            .is_ok_and(|folder| folder.starts_with(profile))
                })
            {
                return None;
            }
            let tools = PathBuf::from(std::env::var_os("KUVO_TEST_UNINSTALL_TOOLS")?);
            let tools = tools.canonicalize().ok()?;
            if !tools.starts_with(profile) {
                return None;
            }
            let launchctl = tools.join("launchctl");
            let open = tools.join("open");
            if !fs::symlink_metadata(&launchctl).is_ok_and(|meta| meta.is_file())
                || fs::symlink_metadata(&open).is_ok_and(|meta| !meta.is_file())
            {
                return None;
            }
            (launchctl, open)
        };
        Some(Self {
            launchctl,
            open,
            plist: agents.join(format!("{LABEL}.plist")),
        })
    }

    pub fn start(&self, home: &Home, app: &str) -> io::Result<Started> {
        self.start_in(home, app, false)
    }

    fn automatic(home: &Home, guarded: bool) -> io::Result<()> {
        if guarded
            && ["paused", "off"]
                .iter()
                .any(|mark| home.run().join(mark).exists())
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Kuvo on this Mac was stopped as it went: nothing was started.",
            ));
        }
        Ok(())
    }

    fn start_in(&self, home: &Home, app: &str, automatic: bool) -> io::Result<Started> {
        Self::automatic(home, automatic)?;
        let bundle = home.bin().join(HELPER);
        let program = bundle.join("Contents/MacOS/Kuvo Helper");
        if !program.is_file()
            || files::text_of(&home.root().join("box/kind"))
                .as_deref()
                .map(str::trim)
                != Some("mac")
        {
            return Err(io::Error::other(
                "Kuvo Helper isn't in Kuvo's bin folder: its release brings it.",
            ));
        }
        let domain = domain()?;
        let service = format!("{domain}/{LABEL}");
        let plist = self.plist.as_os_str();
        let loaded = self.run(["print", &service])?;
        let owned = self.owns(&program);
        Self::automatic(home, automatic)?;
        if !owned {
            self.write_job(home, &program, app)?;
            let registered = self.register(home, &domain, loaded.status.success(), automatic)?;
            if registered.status.success() {
                return Ok(Started::Installed);
            }
            return self.open_after_refusal(&domain, &bundle, home, registered, automatic);
        }
        if !loaded.status.success() {
            let registered = self.bootstrap(&domain, plist)?;
            if !registered.status.success() {
                return self.open_after_refusal(&domain, &bundle, home, registered, automatic);
            }
        }
        if home.run().join("paused").exists() {
            for _ in 0..100 {
                let status = self.run(["print", &service])?;
                if !String::from_utf8_lossy(&status.stdout)
                    .lines()
                    .any(|line| line.trim_start().starts_with("pid = "))
                {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        Self::automatic(home, automatic)?;
        let started = self.run(["kickstart", &service])?;
        if started.status.success() {
            Ok(Started::Kicked)
        } else {
            self.open_after_refusal(&domain, &bundle, home, started, automatic)
        }
    }

    fn write_job(&self, home: &Home, program: &Path, app: &str) -> io::Result<()> {
        files::write_atomic(
            &self.plist,
            launch_plist(home.root(), program, app).as_bytes(),
        )
    }

    fn register(
        &self,
        home: &Home,
        domain: &str,
        bootout: bool,
        automatic: bool,
    ) -> io::Result<Output> {
        let service = format!("{domain}/{LABEL}");
        if bootout && self.run(["bootout", &service])?.status.success() {
            for _ in 0..100 {
                if !self.run(["print", &service])?.status.success() {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        Self::automatic(home, automatic)?;
        let mut registered = self.bootstrap(domain, self.plist.as_os_str())?;
        for _ in 0..3 {
            if registered.status.success() {
                break;
            }
            thread::sleep(Duration::from_secs(1));
            Self::automatic(home, automatic)?;
            registered = self.bootstrap(domain, self.plist.as_os_str())?;
        }
        Ok(registered)
    }

    pub fn ready(&self, home: &Home) -> io::Result<bool> {
        let program = home.bin().join(HELPER).join("Contents/MacOS/Kuvo Helper");
        if !program.is_file() || !self.owns(&program) {
            return Ok(false);
        }
        let service = format!("{}/{LABEL}", domain()?);
        Ok(self.run(["print", &service])?.status.success())
    }

    fn owns(&self, program: &Path) -> bool {
        files::text_of(&self.plist).is_some_and(|text| {
            text.contains(&format!(
                "<string>{}</string>",
                xml(&program.to_string_lossy())
            ))
        })
    }

    fn unregister(
        &self,
        home: &Home,
        profile: &Path,
        label: &str,
        mode: &str,
        check: bool,
    ) -> io::Result<bool> {
        let plist = self.plist.with_file_name(format!("{label}.plist"));
        let exists = match fs::symlink_metadata(&plist) {
            Ok(entry) => {
                let program = home.bin().join(HELPER).join("Contents/MacOS/Kuvo Helper");
                let matches = |key, expected| Self::job_field(&plist, key, expected);
                if !entry.is_file()
                    || !plist.canonicalize()?.starts_with(profile)
                    || !matches("Label", label.as_ref())?
                    || !matches("ProgramArguments.0", program.as_os_str())?
                    || !matches("ProgramArguments.1", mode.as_ref())?
                    || !matches("ProgramArguments.2", "--home".as_ref())?
                    || !matches("ProgramArguments.3", home.root().as_os_str())?
                {
                    return Err(io::Error::other(
                        "Kuvo couldn’t confirm that the login job belongs to this installation.",
                    ));
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        if check {
            return Ok(exists);
        }
        let service = format!("{}/{label}", domain()?);
        let mut status = self.run(["print", &service])?;
        if exists && status.status.success() && self.run(["bootout", &service])?.status.success() {
            for _ in 0..100 {
                status = self.run(["print", &service])?;
                if job_missing(&status) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        if !job_missing(&status) {
            return Err(io::Error::other(
                "Kuvo couldn’t confirm that its Helper login job had stopped.",
            ));
        }
        files::remove_file_synced(&plist)?;
        Ok(exists)
    }

    fn job_field(plist: &Path, key: &str, expected: &OsStr) -> io::Result<bool> {
        let value = output(
            Command::new("/usr/bin/plutil")
                .args(["-extract", key, "raw", "-o", "-"])
                .arg(plist),
        )?;
        Ok(value.status.success()
            && value.stdout.strip_suffix(b"\n").unwrap_or(&value.stdout)
                == expected.as_encoded_bytes())
    }

    fn run(&self, args: [&str; 2]) -> io::Result<Output> {
        output(Command::new(&self.launchctl).args(args))
    }

    fn bootstrap(&self, domain: &str, plist: &OsStr) -> io::Result<Output> {
        output(Command::new(&self.launchctl).args([
            OsStr::new("bootstrap"),
            domain.as_ref(),
            plist,
        ]))
    }

    fn open_after_refusal(
        &self,
        domain: &str,
        bundle: &Path,
        home: &Home,
        failed: Output,
        automatic: bool,
    ) -> io::Result<Started> {
        let disabled = self.run(["print-disabled", domain])?;
        let name = format!("\"{LABEL}\"");
        let disabled = disabled.status.success()
            && String::from_utf8_lossy(&disabled.stdout)
                .lines()
                .any(|line| {
                    line.split_once("=>").is_some_and(|(label, state)| {
                        label.trim() == name && matches!(state.trim(), "disabled" | "true")
                    })
                });
        if disabled || automatic {
            Self::automatic(home, automatic)?;
            let opened = output(
                Command::new(&self.open)
                    .args(["-g", "-j", "-n"])
                    .arg(bundle)
                    .args(["--args", "keep", "--home"])
                    .arg(home.root()),
            )?;
            if opened.status.success() {
                if automatic && !disabled {
                    return Ok(Started::OpenedAfterRefusal);
                }
                return Ok(Started::OpenedForLoginItems);
            }
            return Err(io::Error::other(
                String::from_utf8_lossy(&opened.stderr).trim().to_owned(),
            ));
        }
        Err(io::Error::other(
            String::from_utf8_lossy(&failed.stderr).trim().to_owned(),
        ))
    }
}

fn job_missing(status: &Output) -> bool {
    status.status.code() == Some(113)
        || (!status.status.success()
            && String::from_utf8_lossy(&status.stderr)
                .to_ascii_lowercase()
                .contains("could not find service"))
}

pub fn command(home: &Home, args: &[std::ffi::OsString]) -> io::Result<u8> {
    use serde_json::json;

    let usage = || {
        eprintln!(
            "Usage: kuvo-box helper install [--load] [--app <bundle ID>] | start [--if-missing] [--app <bundle ID>] | status"
        );
        Ok(2)
    };
    let mut args = args.iter().map(|arg| arg.to_str());
    let Some(Some(action @ ("install" | "start" | "status"))) = args.next() else {
        return usage();
    };
    let mut app = None;
    let mut flag = false;
    while let Some(arg) = args.next() {
        match arg {
            Some("--app") if action != "status" && app.is_none() => {
                app = args.next().flatten().filter(|id| {
                    id.starts_with(|c: char| c.is_ascii_alphanumeric())
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                });
                if app.is_none() {
                    return usage();
                }
            }
            Some("--load") if action == "install" && !flag => flag = true,
            Some("--if-missing") if action == "start" && !flag => flag = true,
            _ => return usage(),
        }
    }
    let app = app.unwrap_or("run.kuvo.code");
    let account = Account::current()?;
    let profile = account
        .home_folder(
            kuvo_os::platform::Platform::Mac,
            &std::env::vars_os().collect(),
        )?
        .canonicalize()?;
    let real = profile == account.home.canonicalize()?;
    let root = profile.join(".kuvo-code");
    let own = root
        .canonicalize()
        .is_ok_and(|root| home.root().canonicalize().is_ok_and(|home| home == root))
        && files::text_of(&home.root().join("box/kind"))
            .as_deref()
            .map(str::trim)
            == Some("mac");
    let tools = own
        .then(|| LaunchAgent::for_account(&profile, real))
        .flatten();
    let program = home.bin().join(HELPER).join("Contents/MacOS/Kuvo Helper");
    if action == "status" {
        let owns = tools.as_ref().is_some_and(|tools| tools.owns(&program));
        let status = tools.as_ref().and_then(|tools| {
            let service = format!("{}/{LABEL}", domain().ok()?);
            tools.run(["print", &service]).ok()
        });
        let loaded = status.as_ref().map(|answer| answer.status.success());
        let running = status.and_then(|answer| {
            if !answer.status.success() {
                return Some(false);
            }
            String::from_utf8_lossy(&answer.stdout)
                .lines()
                .find_map(|line| {
                    line.trim()
                        .strip_prefix("state = ")
                        .map(|state| state == "running")
                })
        });
        println!(
            "{}",
            json!({"helper":program.is_file(),"agent":owns,"loaded":loaded,"running":running,
            "handOver":handover::status(home)?})
        );
        return Ok(0);
    }
    let refused = if !own {
        Some("Only Kuvo on a Mac in its own folder runs under Kuvo Helper.")
    } else if tools.is_none() {
        Some("Only Kuvo in ~/.kuvo-code starts at login.")
    } else if !program.is_file() {
        Some("Kuvo Helper isn't in Kuvo's bin folder: its release brings it.")
    } else {
        None
    };
    if let Some(message) = refused {
        let reply = if action == "install" {
            json!({"written":false,"loaded":false,"message":message})
        } else {
            json!({"started":false,"mac":own,"message":message})
        };
        println!("{reply}");
        return Ok(1);
    }
    let tools = tools.expect("the command has launch tools");
    if action == "install" {
        tools.write_job(home, &program, app)?;
        if !flag {
            println!(
                "{}",
                json!({"written":true,"loaded":false,"message":AT_LOGIN})
            );
            return Ok(0);
        }
        let registered = tools.register(home, &domain()?, true, false)?;
        let loaded = registered.status.success();
        let message = if loaded {
            "Kuvo Helper runs Kuvo on this Mac now, and at login.".into()
        } else {
            format!(
                "Kuvo Helper starts Kuvo on this Mac at login, but launchd didn't start it now: {}.",
                String::from_utf8_lossy(&registered.stderr).trim()
            )
        };
        println!(
            "{}",
            json!({"written":true,"loaded":loaded,"message":message})
        );
        return Ok(u8::from(!loaded));
    }
    if flag && tools.owns(&program) {
        println!(
            "{}",
            json!({"started":false,"mac":true,"left":true,
            "message":"Kuvo Helper's launch agent is in place: it's left as it is."})
        );
        return Ok(0);
    }
    let (answer, code) = match tools.start(home, app) {
        Ok(Started::OpenedForLoginItems) => (
            json!({"started":true,"mac":true,"disabled":true,"opened":true,
            "message":Started::OpenedForLoginItems.message()}),
            0,
        ),
        Ok(started) => (
            json!({"started":true,"mac":true,"message":started.message()}),
            0,
        ),
        Err(error) => (
            json!({"started":false,"mac":true,"message":error.to_string()}),
            1,
        ),
    };
    println!("{answer}");
    Ok(code)
}

fn domain() -> io::Result<String> {
    let uid = output(Command::new("/usr/bin/id").arg("-u"))?;
    let uid = String::from_utf8_lossy(&uid.stdout);
    let uid: u32 = uid.trim().parse().map_err(io::Error::other)?;
    Ok(format!("gui/{uid}"))
}

fn output(command: &mut Command) -> io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Kuvo Helper's launch tools took too long to answer.",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn launch_plist(home: &Path, program: &Path, app: &str) -> String {
    let home = xml(&home.to_string_lossy());
    let program = xml(&program.to_string_lossy());
    let app = xml(app);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{LABEL}</string>
<key>ProgramArguments</key><array><string>{program}</string><string>keep</string><string>--home</string><string>{home}</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
<key>AbandonProcessGroup</key><true/>
<key>AssociatedBundleIdentifiers</key><array><string>{app}</string></array>
</dict></plist>
"#
    )
}
