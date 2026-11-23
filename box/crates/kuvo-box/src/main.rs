mod serve;

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
    if word(0) == Some("rehome") {
        return rehome(&args[1..]);
    }
    if word(0) == Some("install") && args.iter().any(|arg| arg == "--status") {
        return install_status(&args[1..]);
    }
    let identity = (word(0), args.len()) == (Some("backup-identity"), 1);
    let join = (word(0), word(1), args.len()) == (Some("install"), Some("--join"), 2);
    let command = match (word(0), word(1), args.len()) {
        (Some("serve"), None, 1) | (Some("serve"), Some("--resident"), 2) => None,
        _ if join || identity => None,
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
                "usage: kuvo-box serve | install --join | route [--internal [path]] | route-to (set KUVO_HOME)"
                    .into(),
            );
        }
    };
    let home = env::var_os("KUVO_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("KUVO_HOME must name an absolute scratch path"))?;
    let home = Home::new(home)?;
    if identity {
        println!("{}", kuvo_setup::backup::ssh_identity(home.root())?);
        return Ok(ExitCode::SUCCESS);
    }
    if !join {
        let platform = Platform::current();
        let account_home = Account::current()?.home_folder(platform, &env::vars_os().collect())?;
        kuvo_move::open(&home, platform, Some(&account_home), |_| {})?;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        if join {
            use std::io::{BufRead as _, Read as _};
            let mut code = String::new();
            io::stdin().lock().take(4098).read_line(&mut code)?;
            let name = hostname::get()?.to_string_lossy().into_owned();
            let joined = kuvo_setup::installer::join(&home, name, &code).await?;
            println!("{}", serde_json::to_string(&joined)?);
            Ok(())
        } else if let Some(command) = command {
            tokio::select! {
                result = ssh::pipe(home.root(), command, tokio::io::stdin(), tokio::io::stdout()) => result?,
                () = shutdown() => {},
            }
            Ok(())
        } else {
            let (launch, helper_ended) = if word(1) == Some("--resident")
                && std::fs::read_to_string(home.root().join("box/kind"))
                    .is_ok_and(|kind| kind.trim() == "mac")
            {
                let (input, received) = tokio::sync::oneshot::channel();
                let (ended, helper_ended) = tokio::sync::oneshot::channel();
                tokio::task::spawn_blocking(move || {
                    let mut stdin = io::stdin().lock();
                    let launch = kuvo_services::vault::Launch::read(&mut stdin);
                    if launch.is_ok() && input.send(launch).is_ok() {
                        let _ = io::copy(&mut stdin, &mut io::sink());
                    }
                    let _ = ended.send(());
                });
                (received.await.map_err(|_| io::Error::other("The Mac Helper's launch message is unreadable."))??, Some(helper_ended))
            } else {
                (kuvo_services::vault::Launch::default(), None)
            };
            serve::run(home.root(), launch, helper_ended).await
        }
    });
    // An addressed SSH exchange can finish while its stdin is still open. The
    // source is stdio, so Tokio's blocking read only ends when that SSH channel closes.
    runtime.shutdown_background();
    result.map(|()| ExitCode::SUCCESS)
}

fn rehome(args: &[std::ffi::OsString]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use std::{ffi::OsStr, path::Path};

    let usage = "usage: kuvo-box rehome <root> --status | --resume | --settle | --back | --new [--check | --release <folder> [--keeps-awake on|off]] | --run --app <bundle> --release <folder> --engine <folder>";
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
    let judge = kuvo_move::install::judge(&home, source.as_deref(), platform, None)?;
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
