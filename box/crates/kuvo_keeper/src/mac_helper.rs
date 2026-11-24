//! Launch the signed Helper already installed in a Mac's home.

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
const HELPER: &str = "Kuvo Helper.app";

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

    pub fn start(&self, home: &Home, app: &str) -> io::Result<&'static str> {
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
        if !owned {
            files::write_atomic(
                &self.plist,
                launch_plist(home.root(), &program, app).as_bytes(),
            )?;
            if loaded.status.success() && self.run(["bootout", &service])?.status.success() {
                for _ in 0..100 {
                    if !self.run(["print", &service])?.status.success() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            let mut registered = self.bootstrap(&domain, plist)?;
            for _ in 0..3 {
                if registered.status.success() {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
                registered = self.bootstrap(&domain, plist)?;
            }
            if registered.status.success() {
                return Ok("Kuvo Helper runs Kuvo on this Mac now, and at login.");
            }
            return self.open_if_disabled(&domain, &bundle, home, registered);
        }
        if !loaded.status.success() {
            let registered = self.bootstrap(&domain, plist)?;
            if !registered.status.success() {
                return self.open_if_disabled(&domain, &bundle, home, registered);
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
        let started = self.run(["kickstart", &service])?;
        if started.status.success() {
            Ok("Kuvo Helper runs Kuvo on this Mac.")
        } else {
            self.open_if_disabled(&domain, &bundle, home, started)
        }
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

    fn open_if_disabled(
        &self,
        domain: &str,
        bundle: &Path,
        home: &Home,
        failed: Output,
    ) -> io::Result<&'static str> {
        let disabled = self.run(["print-disabled", domain])?;
        let name = format!("\"{LABEL}\"");
        if disabled.status.success()
            && String::from_utf8_lossy(&disabled.stdout)
                .lines()
                .any(|line| {
                    line.split_once("=>").is_some_and(|(label, state)| {
                        label.trim() == name && matches!(state.trim(), "disabled" | "true")
                    })
                })
        {
            let opened = output(
                Command::new(&self.open)
                    .args(["-g", "-j", "-n"])
                    .arg(bundle)
                    .args(["--args", "keep", "--home"])
                    .arg(home.root()),
            )?;
            if opened.status.success() {
                return Ok(
                    "Kuvo Helper is turned off in System Settings › General › Login Items, so it was opened for now, to run Kuvo on this Mac until you log out. Turn it on there to start it at login.",
                );
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
