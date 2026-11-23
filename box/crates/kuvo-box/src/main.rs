mod model_stars;
mod serve;
mod session_ownership;

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
    let identity = (word(0), args.len()) == (Some("backup-identity"), 1);
    let join = (word(0), word(1), args.len()) == (Some("install"), Some("--join"), 2);
    let command = match (word(0), word(1), args.len()) {
        (Some("serve"), _, 1) => None,
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
            serve::run(home.root()).await
        }
    });
    // An addressed SSH exchange can finish while its stdin is still open. The
    // source is stdio, so Tokio's blocking read only ends when that SSH channel closes.
    runtime.shutdown_background();
    result.map(|()| ExitCode::SUCCESS)
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
