#[cfg(target_os = "macos")]
mod install_media;
mod model_stars;
mod serve;
mod session_ownership;
mod setup_search;
#[cfg(target_os = "macos")]
mod sign_ins;
mod updates;
mod vault_asks;

use std::{env, io, path::PathBuf, process::ExitCode};

use kuvo_os::platform::{Account, Platform};
use kuvo_server::ssh;
use kuvo_store::home::Home;

fn main() -> ExitCode {
    match run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("kuvo-box: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let word = |index: usize| args.get(index).and_then(|part| part.to_str());
    if word(0) == Some("hold") {
        kuvo_agents::hold::run(&args[1..])?;
        return Ok(ExitCode::SUCCESS);
    }
    #[cfg(target_os = "macos")]
    if word(0) == Some("install-media") {
        return Ok(install_media::run(&args[1..]));
    }
    #[cfg(target_os = "macos")]
    if (word(0), args.len()) == (Some("helper-hand-over"), 1) {
        let home = Home::new(PathBuf::from(
            env::var_os("KUVO_HOME").ok_or("Expected KUVO_HOME")?,
        ))?;
        kuvo_keeper::mac_helper::run_handover(&home)?;
        return Ok(ExitCode::SUCCESS);
    }
    #[cfg(target_os = "macos")]
    if word(0) == Some("helper") {
        let home = Home::new(PathBuf::from(
            env::var_os("KUVO_HOME").ok_or("Expected KUVO_HOME")?,
        ))?;
        return Ok(ExitCode::from(kuvo_keeper::mac_helper::command(
            &home,
            &args[1..],
        )?));
    }
    if word(0) == Some("update") {
        return Ok(kuvo_keeper::prepare_command::run(&args[1..])?);
    }
    #[cfg(unix)]
    if (word(0), args.len()) == (Some("preserve-before-rust"), 1) {
        let home = Home::new(PathBuf::from(
            env::var_os("KUVO_HOME").ok_or("Expected KUVO_HOME")?,
        ))?;
        kuvo_setup::backup::Backups::new(home).preserve_before_rust()?;
        return Ok(ExitCode::SUCCESS);
    }
    if (word(0), word(1)) == (Some("box"), Some("labels")) {
        return kuvo_work::label_restore::run(&args[2..]);
    }
    if (word(0), word(1), args.len()) == (Some("selftest"), Some("--engine"), 3) {
        if word(2) != Some(kuvo_engine::pinned_version().as_str()) {
            return Err("The release names a different engine.".into());
        }
        println!("ok");
        return Ok(ExitCode::SUCCESS);
    }
    if word(0) == Some("rehome") {
        return rehome(&args[1..]);
    }
    if word(0) == Some("here") {
        return here(&args[1..]);
    }
    if word(0) == Some("install") && args.iter().any(|arg| arg == "--status") {
        return install_status(&args[1..]);
    }
    let identity = (word(0), args.len()) == (Some("backup-identity"), 1);
    let join = (word(0), word(1), args.len()) == (Some("install"), Some("--join"), 2);
    let ssh_enroll = match (word(0), word(1), args.len()) {
        (Some("ssh-enroll"), Some(name), 2) => Some(name),
        _ => None,
    };
    let seed = (word(0), word(1), args.len()) == (Some("box"), Some("seed"), 2);
    let wipe = (word(0), args.len()) == (Some("wipe-member"), 1);
    let helper_resident = (word(0), word(1), args.len()) == (Some("serve"), Some("--resident"), 2);
    let serve = (word(0), args.len()) == (Some("serve"), 1) || helper_resident;
    #[cfg(windows)]
    let startup = match (word(0), word(1), args.len()) {
        (Some("install"), Some("--startup"), 2) => Some(true),
        (Some("uninstall"), Some("--startup"), 2) => Some(false),
        _ => None,
    };
    #[cfg(not(windows))]
    let startup: Option<bool> = None;
    let uninstall = if word(0) == Some("uninstall") && startup.is_none() {
        if args[1..].iter().any(|word| word != "--check") {
            eprintln!("Usage: kuvo-box uninstall [--check]");
            return Ok(ExitCode::from(2));
        }
        Some(args.len() > 1)
    } else {
        None
    };
    let recovery = match (word(0), word(1), args.len()) {
        (Some("installer-recovery"), Some("issue"), 2) => Some(false),
        (Some("installer-recovery"), Some("apply"), 2) => Some(true),
        _ => None,
    };
    let keeper = match (word(0), word(1), args.len()) {
        (Some("keep"), _, 1) => Some(kuvo_keeper::Command::Run),
        (Some("keep"), Some("select"), 3) => Some(kuvo_keeper::Command::Select(
            word(2).ok_or("Expected a release target")?.parse()?,
        )),
        (Some("keep"), Some("pause"), 2) => Some(kuvo_keeper::Command::Pause),
        (Some("keep"), Some("off"), 2) => Some(kuvo_keeper::Command::Off),
        (Some("keep"), Some("handover"), 2) => Some(kuvo_keeper::Command::Handover),
        _ => None,
    };
    let probe = match (word(0), word(1), args.len()) {
        (Some("resident-info"), Some(pid), 2) => Some(pid.parse::<u32>()?),
        _ => None,
    };
    let supervised =
        cfg!(windows) && (word(0), word(1), args.len()) == (Some("serve"), Some("--supervised"), 2);
    let command = match (word(0), word(1), args.len()) {
        _ if serve || wipe => None,
        _ if join
            || ssh_enroll.is_some()
            || seed
            || recovery.is_some()
            || identity
            || uninstall.is_some()
            || startup.is_some()
            || keeper.is_some()
            || probe.is_some()
            || supervised =>
        {
            None
        }
        (Some("route"), _, 1) => Some(ssh::Command::Route {
            internal: false,
            internal_path: None,
        }),
        (Some("route"), Some("--internal"), 2 | 3) => Some(ssh::Command::Route {
            internal: true,
            internal_path: args.get(2).map(|path| path.to_string_lossy().into_owned()),
        }),
        (Some("route-to"), _, 1) => Some(ssh::Command::RouteTo),
        _ => {
            return Err(
                "usage: kuvo-box serve | keep [select <release> | handover | pause | off] | install --join | installer-recovery issue|apply | box seed | uninstall [--check] | wipe-member | route [--internal [path]] | route-to (set KUVO_HOME)"
                    .into(),
            );
        }
    };
    #[cfg(windows)]
    let home = {
        let account = Account::current()?;
        let mut environment: kuvo_os::platform::Environment = env::vars_os().collect();
        // The system profile and SID belong to the token, not an environment override.
        environment.retain(|key, _| !key.eq_ignore_ascii_case("USERPROFILE"));
        Home::resolve(&account, Platform::Windows, &environment)?
    };
    #[cfg(windows)]
    kuvo_os::windows::private_home(
        home.root(),
        !wipe && uninstall.is_none() && startup != Some(false),
    )?;
    #[cfg(not(windows))]
    let home = env::var_os("KUVO_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("KUVO_HOME must name an absolute scratch path"))?;
    #[cfg(not(windows))]
    let home = Home::new(home)?;
    if wipe || serve {
        match kuvo_setup::wipe::finish(home.root())? {
            true => return Ok(ExitCode::SUCCESS),
            false if wipe => return Ok(ExitCode::FAILURE),
            false => {}
        }
    }
    if let Some(check) = uninstall {
        let outcome = kuvo_setup::uninstall::run(&home, check);
        #[cfg(windows)]
        if !check
            && outcome.status == 0
            && (home.bin().join(kuvo_keeper::windows::STARTER).is_file()
                || !kuvo_keeper::prepare_command::is_scratch(&home)?)
        {
            kuvo_keeper::windows::StartupTask::for_home(home.root())?.remove()?;
        }
        println!("{}", outcome.value);
        if let Some(note) = outcome.note {
            eprintln!("{note}");
        }
        return Ok(ExitCode::from(outcome.status));
    }
    if identity {
        println!("{}", kuvo_setup::backup::ssh_identity(home.root())?);
        return Ok(ExitCode::SUCCESS);
    }
    if !join
        && ssh_enroll.is_none()
        && !seed
        && recovery.is_none()
        && keeper.is_none()
        && probe.is_none()
        && startup.is_none()
    {
        let platform = Platform::current();
        let account_home = Account::current()?.home_folder(platform, &env::vars_os().collect())?;
        kuvo_move::open(&home, platform, Some(&account_home), |_| {})?;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        #[cfg(windows)]
        if let Some(install) = startup {
            if install {
                kuvo_keeper::windows::stage_local_release(home.root(), &env::current_exe()?)?;
                kuvo_setup::installer::open_staged_home(&home)?;
                let task = start_windows(&home).await?;
                println!("{}", task.name());
            } else {
                kuvo_keeper::windows::StartupTask::for_home(home.root())?
                    .disable(&home)
                    .await?;
            }
            return Ok(());
        }
        if let Some(keeper) = keeper {
            kuvo_keeper::execute(&home, env::vars_os().collect(), keeper, shutdown()).await?;
            Ok(())
        } else if let Some(pid) = probe {
            let reply = kuvo_keeper::resident_info(home.root(), pid)
                .await?
                .ok_or("The resident has not answered for this start")?;
            println!("{reply}");
            Ok(())
        } else if seed {
            use std::io::Read as _;
            let mut input = Vec::new();
            io::stdin().lock().read_to_end(&mut input)?;
            let answer = kuvo_setup::seed::take(&home, &input).await;
            println!("{}", serde_json::to_string(&answer)?);
            Ok(())
        } else if let Some(apply) = recovery {
            if apply {
                let receipt = read_installer_input(514)?;
                let message = kuvo_setup::installer::apply_cancellation(&home, &receipt).await?;
                println!("{message}");
            } else {
                for receipt in kuvo_setup::installer::cancellation_receipts(&home)? {
                    println!("{receipt}");
                }
            }
            Ok(())
        } else if join || ssh_enroll.is_some() {
            let code = read_installer_input(4098)?;
            let name = match ssh_enroll {
                Some(name) => name.to_owned(),
                None => hostname::get()?.to_string_lossy().into_owned(),
            };
            if ssh_enroll.is_some() {
                let joined = kuvo_setup::installer::join_ssh(&home, name, &code).await?;
                let (key, identity) = kuvo_setup::installer::enrolled_ssh_identity(&home)?;
                #[cfg(not(windows))]
                start_ssh_resident(&home, &identity).await?;
                println!("{}", serde_json::json!({"setup": joined.setup, "primary": joined.primary, "key": key, "identity": identity}));
            } else {
                let joined = kuvo_setup::installer::join(&home, name, &code).await?;
                println!("{}", serde_json::to_string(&joined)?);
            }
            Ok(())
        } else if let Some(command) = command {
            tokio::select! {
                result = ssh::pipe(home.root(), command, tokio::io::stdin(), tokio::io::stdout()) => result?,
                () = shutdown() => {},
            }
            Ok(())
        } else {
            let (launch, helper_ended) = if helper_resident
                && std::fs::read_to_string(home.root().join("box/kind"))
                    .is_ok_and(|kind| kind.trim() == "mac")
            {
                let (input, received) = tokio::sync::oneshot::channel();
                let (ended, helper_ended) = tokio::sync::oneshot::channel();
                // Helper keeps stdin open while it supervises the resident. This read
                // must finish independently of the runtime that drains a local wipe.
                std::thread::Builder::new().name("mac-helper-stdin".into()).spawn(move || {
                    let mut stdin = io::stdin().lock();
                    let launch = kuvo_services::vault::Launch::read(&mut stdin);
                    if launch.is_ok() && input.send(launch).is_ok() {
                        let _ = io::copy(&mut stdin, &mut io::sink());
                    }
                    let _ = ended.send(());
                })?;
                (received.await.map_err(|_| io::Error::other("The Mac Helper's launch message is unreadable."))??, Some(helper_ended))
            } else {
                (kuvo_services::vault::Launch::default(), None)
            };
            serve::run(home.root(), launch, helper_ended, resident_shutdown(home.root(), supervised)).await
        }
    });
    if serve && result.is_ok() && kuvo_setup::wipe::pending(home.root())? {
        // Let resident tasks and blocking file writers finish before detaching their home.
        drop(runtime);
        return Ok(ExitCode::from(u8::from(!kuvo_setup::wipe::finish(
            home.root(),
        )?)));
    }
    // An addressed SSH exchange can finish while its stdin is still open. The
    // source is stdio, so Tokio's blocking read only ends when that SSH channel closes.
    runtime.shutdown_background();
    result.map(|()| ExitCode::SUCCESS)
}

#[cfg(windows)]
async fn start_windows(home: &Home) -> io::Result<kuvo_keeper::windows::StartupTask> {
    let task = kuvo_keeper::windows::StartupTask::for_home(home.root())?;
    kuvo_keeper::windows::prepare_first_release(home.root(), env!("CARGO_PKG_VERSION"))?;
    for marker in ["off", "paused"] {
        kuvo_store::files::remove_file_synced(&home.run().join(marker))?;
    }
    let registered = task.register()?;
    if let Err(error) = task.start_and_wait(home.root()).await {
        if registered && let Err(cleanup) = task.disable(home).await {
            return Err(io::Error::other(format!(
                "{error}. Kuvo couldn't remove the new task: {cleanup}"
            )));
        }
        return Err(error);
    }
    Ok(task)
}

#[cfg(not(windows))]
async fn start_ssh_resident(home: &Home, identity: &str) -> io::Result<()> {
    use kuvo_os::process::Spawn;
    use std::{fs, time::Duration};

    // cmd.exe tracks per-drive working directories in entries named `=C:`.
    // They are not process environment variables and the child gets an explicit cwd.
    let environment = env::vars_os()
        .filter(|(name, _)| !name.as_encoded_bytes().starts_with(b"="))
        .collect();
    let mut spawn = Spawn::new(env::current_exe()?, environment);
    spawn.args.push("serve".into());
    spawn.cwd = Some(home.root().to_owned());
    let mut child = spawn.spawn_recoverable(&format!("kuvo-ssh-{identity}"))?;
    let pid = child.identity().pid;
    let record = home.run().join("serve.json");
    let ready = async {
        loop {
            if child.has_exited()? {
                return Err(io::Error::other(
                    "Kuvo Code stopped before it started answering",
                ));
            }
            if fs::read(&record).is_ok_and(|bytes| {
                serde_json::from_slice::<kuvo_server::ServeRecord>(&bytes)
                    .is_ok_and(|record| record.pid == pid)
            }) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(25), ready)
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "Kuvo Code did not start on this computer",
            )
        })??;
    child.leave_running_on_drop();
    Ok(())
}

fn read_installer_input(limit: u64) -> io::Result<String> {
    use std::io::{BufRead as _, Read as _};
    let mut input = String::new();
    io::stdin().lock().take(limit).read_line(&mut input)?;
    Ok(input)
}

fn here(args: &[std::ffi::OsString]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use std::path::Path;

    let root = match args {
        [root] => Path::new(root),
        _ => Path::new(""),
    };
    let app = env::var_os("KUVO_HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let engine = env::var_os("KUVO_ENGINE_DATA").map(PathBuf::from);
    let account =
        Account::current()?.home_folder(Platform::current(), &env::vars_os().collect())?;
    match kuvo_setup::locate(&account, root, &app, engine.as_deref()) {
        Ok(found) => {
            println!("{}", serde_json::to_string(&found)?);
            Ok(ExitCode::SUCCESS)
        }
        Err(message) => {
            eprintln!("{message}");
            Ok(ExitCode::from(2))
        }
    }
}

async fn resident_shutdown(home: &std::path::Path, supervised: bool) {
    tokio::select! {
        () = shutdown() => {},
        () = kuvo_keeper::wait_for_stop(home), if supervised => {},
    }
}

fn rehome(args: &[std::ffi::OsString]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use std::{ffi::OsStr, path::Path};

    let usage = "usage: kuvo-box rehome <root> --status | --resume | --settle | --sign-ins | --back | --new [--check | --release <folder> [--keeps-awake on|off]] | --run --app <bundle> --release <folder> --engine <folder>";
    let invalid = || {
        eprintln!("{usage}");
        Ok(ExitCode::from(2))
    };
    let Some((root, options)) = args.split_first() else {
        return invalid();
    };
    let name = Path::new(root);
    if name.file_name() != Some(root.as_os_str()) {
        return invalid();
    }
    let account = Account::current()?;
    let folder = account.home_folder(Platform::current(), &env::vars_os().collect())?;
    let target = folder.join(root);
    if options == [OsStr::new("--new"), OsStr::new("--check")]
        || options == [OsStr::new("--check"), OsStr::new("--new")]
    {
        #[cfg(target_os = "macos")]
        {
            let app = env::var_os("KUVO_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from);
            let data = env::var_os("KUVO_ENGINE_DATA")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from);
            let answer = kuvo_move::mac::new_readiness(
                &Home::new(target)?,
                app.as_deref(),
                data.as_deref(),
                &account.home,
            );
            println!("{}", serde_json::to_string(&answer)?);
        }
        #[cfg(not(target_os = "macos"))]
        println!(
            "{}",
            serde_json::json!({"ready":false,"made":false,"why":"Kuvo on this machine is in its own home already: there's nothing to move."})
        );
        return Ok(ExitCode::SUCCESS);
    }
    if options == [OsStr::new("--status")] {
        println!("{}", kuvo_move::rehome_status(&Home::new(target)?));
        return Ok(ExitCode::SUCCESS);
    }
    #[cfg(target_os = "macos")]
    if options == [OsStr::new("--sign-ins")] {
        let result = kuvo_move::mac::sign_ins(&Home::new(target)?, &account.home)?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(ExitCode::from(result.exit_code()));
    }
    #[cfg(target_os = "macos")]
    if let Some(words) = options.strip_prefix(&[std::ffi::OsString::from("--new")]) {
        let mut words = words.iter();
        let mut release = None;
        let mut keeps_awake = None;
        while let Some(word) = words.next() {
            match word.to_str() {
                Some("--release") if release.is_none() => {
                    let Some(path) = words.next() else {
                        return invalid();
                    };
                    release = Some(std::path::absolute(path)?);
                }
                Some("--keeps-awake") if keeps_awake.is_none() => {
                    match words.next().and_then(|value| value.to_str()) {
                        Some("on") => keeps_awake = Some(true),
                        Some("off") => keeps_awake = Some(false),
                        _ => return invalid(),
                    }
                }
                _ => return invalid(),
            }
        }
        let app = env::var_os("KUVO_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let data = env::var_os("KUVO_ENGINE_DATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let outcome = kuvo_move::mac::new_home(
            &Home::new(target)?,
            app.as_deref(),
            data.as_deref(),
            &account.home,
            release.as_deref(),
            keeps_awake == Some(false),
        );
        println!("{}", serde_json::to_string(&outcome)?);
        return Ok(ExitCode::from(outcome.exit_code()));
    }
    #[cfg(target_os = "macos")]
    if options == [OsStr::new("--resume")] {
        let outcome = kuvo_move::mac::resume(&Home::new(target)?, &folder, &account.home)?;
        println!("{}", serde_json::to_string(&outcome)?);
        return Ok(ExitCode::from(outcome.exit_code()));
    }
    #[cfg(target_os = "macos")]
    if options == [OsStr::new("--back")] {
        let outcome = kuvo_move::mac::back(&Home::new(target)?)?;
        println!("{}", serde_json::to_string(&outcome)?);
        return Ok(ExitCode::from(outcome.exit_code()));
    }
    #[cfg(target_os = "macos")]
    if options == [OsStr::new("--settle")] {
        let selected = env::var_os("KUVO_HOME")
            .filter(|path| !path.is_empty())
            .map_or_else(|| folder.join(".kuvo-code"), PathBuf::from);
        let located = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.into());
        if located(&selected) != located(&target) {
            return invalid();
        }
        let outcome = kuvo_move::mac::settle(&Home::new(target)?, &folder, &account.home)?;
        println!("{}", serde_json::to_string(&outcome)?);
        return Ok(ExitCode::from(outcome.exit_code()));
    }
    let mut options = options.iter();
    let mut run = false;
    let (mut app, mut release, mut engine) = (None, None, None);
    while let Some(word) = options.next() {
        let target = match word.to_str() {
            Some("--run") if !run => {
                run = true;
                continue;
            }
            Some("--app") if app.is_none() => &mut app,
            Some("--release") if release.is_none() => &mut release,
            Some("--engine") if engine.is_none() => &mut engine,
            _ => return invalid(),
        };
        *target = options
            .next()
            .map(Path::new)
            .filter(|path| path.is_absolute());
        if target.is_none() {
            return invalid();
        }
    }
    let (true, Some(app), Some(release), Some(engine)) = (run, app, release, engine) else {
        return invalid();
    };
    let Some(home) = env::var_os("KUVO_HOME")
        .filter(|path| path != OsStr::new(""))
        .map(PathBuf::from)
    else {
        return invalid();
    };
    if !home.is_absolute()
        || home
            .canonicalize()
            .ok()
            .zip(target.canonicalize().ok())
            .is_none_or(|(home, target)| home != target)
    {
        return invalid();
    }
    #[cfg(target_os = "macos")]
    {
        let home = Home::new(target)?;
        let outcome = kuvo_move::mac::Run {
            home: &home,
            account: &folder,
            system_account: &account.home,
            bundle: app,
            release,
            engine,
        }
        .execute(|_| {});
        println!("{}", serde_json::to_string(&outcome)?);
        Ok(ExitCode::from(outcome.exit_code()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, release, engine);
        println!(
            "{}",
            serde_json::json!({"result":"refused","message":
            "Kuvo on this machine is in its own home already: there's nothing to move.",
            "status":kuvo_move::rehome_status(&Home::new(target)?)})
        );
        Ok(ExitCode::FAILURE)
    }
}

fn install_status(args: &[std::ffi::OsString]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use std::{ffi::OsStr, path::Path};

    let usage = || {
        eprintln!("usage: kuvo-box install --status --root <name> [--app <bundle>]");
        Ok(ExitCode::from(2))
    };
    let mut words = args.iter().filter(|word| *word != "--status");
    let (mut root, mut bundle) = (None, None);
    while let Some(word) = words.next() {
        let slot = match word.to_str() {
            Some("--root") if root.is_none() => &mut root,
            Some("--app") if bundle.is_none() => &mut bundle,
            _ => return usage(),
        };
        *slot = words.next().map(Path::new);
        if slot.is_none() {
            return usage();
        }
    }
    let Some(root) = root.filter(|root| root.file_name() == Some(root.as_os_str())) else {
        return usage();
    };
    let platform = Platform::current();
    let account = Account::current()?;
    let folder = account.home_folder(platform, &env::vars_os().collect())?;
    let home = Home::new(folder.join(root))?;
    let selected = env::var_os("KUVO_HOME")
        .filter(|path| path != OsStr::new(""))
        .map_or_else(|| folder.join(".kuvo-code"), PathBuf::from);
    let located = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.into());
    if located(home.root()) != located(&selected) {
        return usage();
    }
    #[cfg(target_os = "macos")]
    let source = {
        let Some(id) = bundle
            .filter(|path| path.is_absolute())
            .and_then(|app| kuvo_move::mac::bundle_id(app).ok())
        else {
            println!(
                "{}",
                serde_json::json!({
                    "result":"refused","message":"Kuvo couldn't tell which app started it. Open Kuvo again."
                })
            );
            return Ok(ExitCode::FAILURE);
        };
        let real = located(home.root()) == located(&account.home.join(".kuvo-code"));
        if real != (id == "run.kuvo.code") {
            let message = if real {
                "Only Kuvo itself sets up ~/.kuvo-code: a copy of it keeps a folder of its own."
            } else {
                "Kuvo itself sets up ~/.kuvo-code only."
            };
            println!(
                "{}",
                serde_json::json!({"result":"refused","message":message})
            );
            return Ok(ExitCode::FAILURE);
        }
        Some(folder.join("Library/Application Support").join(id))
    };
    #[cfg(not(target_os = "macos"))]
    let source: Option<PathBuf> = {
        let _ = bundle;
        None
    };
    let helper_ready = {
        #[cfg(target_os = "macos")]
        {
            let profile = located(&folder);
            let real = located(home.root()) == located(&account.home.join(".kuvo-code"));
            home.bin()
                .join("kuvo-box")
                .is_file()
                .then(|| kuvo_keeper::mac_helper::LaunchAgent::for_account(&profile, real))
                .flatten()
                .and_then(|agent| agent.ready(&home).ok())
        }
        #[cfg(not(target_os = "macos"))]
        None
    };
    let judge = kuvo_move::install::judge(&home, source.as_deref(), platform, helper_ready)?;
    if judge.verdict == kuvo_move::install::Verdict::Stuck
        && judge.status["step"].is_null()
        && let Some(message) = judge.status["failed"]["message"].as_str()
    {
        println!(
            "{}",
            serde_json::json!({"result":"refused","message":message})
        );
        return Ok(ExitCode::FAILURE);
    }
    let mut reply = serde_json::to_value(judge)?;
    if reply["verdict"] == "two" {
        reply["message"] = "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: both stay as they are.".into();
    }
    println!("{reply}");
    Ok(ExitCode::SUCCESS)
}

async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
