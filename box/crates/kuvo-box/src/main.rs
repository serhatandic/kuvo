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
    if word(0) == Some("here") {
        return here(&args[1..]);
    }
    let join = (word(0), word(1), args.len()) == (Some("install"), Some("--join"), 2);
    let ssh_enroll = match (word(0), word(1), args.len()) {
        (Some("ssh-enroll"), Some(name), 2) => Some(name),
        _ => None,
    };
    let seed = (word(0), word(1), args.len()) == (Some("box"), Some("seed"), 2);
    let recovery = match (word(0), word(1), args.len()) {
        (Some("installer-recovery"), Some("issue"), 2) => Some(false),
        (Some("installer-recovery"), Some("apply"), 2) => Some(true),
        _ => None,
    };
    let command = match (word(0), word(1), args.len()) {
        (Some("serve"), _, 1) => None,
        _ if join || ssh_enroll.is_some() || seed || recovery.is_some() => None,
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
                "usage: kuvo-box serve | install --join | installer-recovery issue|apply | box seed | route [--internal [path]] | route-to (set KUVO_HOME)"
                    .into(),
            );
        }
    };
    let home = env::var_os("KUVO_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("KUVO_HOME must name an absolute scratch path"))?;
    let home = Home::new(home)?;
    if !join && ssh_enroll.is_none() && !seed && recovery.is_none() {
        let platform = Platform::current();
        let account_home = Account::current()?.home_folder(platform, &env::vars_os().collect())?;
        kuvo_move::open(&home, platform, Some(&account_home), |_| {})?;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        if seed {
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
                start_ssh_resident(&home).await?;
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
            serve::run(home.root()).await
        }
    });
    // An addressed SSH exchange can finish while its stdin is still open. The
    // source is stdio, so Tokio's blocking read only ends when that SSH channel closes.
    runtime.shutdown_background();
    result.map(|()| ExitCode::SUCCESS)
}

async fn start_ssh_resident(home: &Home) -> io::Result<()> {
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
    let mut child = spawn.spawn()?;
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

fn rehome(args: &[std::ffi::OsString]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use std::{ffi::OsStr, path::Path};

    let usage =
        "usage: kuvo-box rehome <root> --run --app <bundle> --release <folder> --engine <folder>";
    let Some((root, options)) = args.split_first() else {
        return Err(usage.into());
    };
    let name = Path::new(root);
    if name.file_name() != Some(root.as_os_str()) {
        return Err(usage.into());
    }
    let mut options = options.iter();
    let mut run = false;
    let (mut app, mut release, mut engine) = (None, None, None);
    while let Some(word) = options.next() {
        let target = match word.to_str().ok_or(usage)? {
            "--run" if !run => {
                run = true;
                continue;
            }
            "--app" if app.is_none() => &mut app,
            "--release" if release.is_none() => &mut release,
            "--engine" if engine.is_none() => &mut engine,
            _ => return Err(usage.into()),
        };
        *target = Some(
            options
                .next()
                .map(Path::new)
                .filter(|path| path.is_absolute())
                .ok_or(usage)?,
        );
    }
    let (true, Some(app), Some(release), Some(engine)) = (run, app, release, engine) else {
        return Err(usage.into());
    };
    let account = Account::current()?;
    let folder = account.home_folder(Platform::current(), &env::vars_os().collect())?;
    let target = folder.join(root);
    let home = env::var_os("KUVO_HOME")
        .filter(|path| path != OsStr::new(""))
        .map(PathBuf::from)
        .ok_or("KUVO_HOME must name the destination of the app move")?;
    if !home.is_absolute() || home.canonicalize()? != target.canonicalize()? {
        return Err("KUVO_HOME must name the destination of the app move".into());
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
            "Kuvo on this machine is in its own home already: there's nothing to move."})
        );
        Ok(ExitCode::FAILURE)
    }
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
