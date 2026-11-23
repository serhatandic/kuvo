mod serve;

use std::{env, io, path::PathBuf, process::ExitCode};

use kuvo_server::ssh;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kuvo-box: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let word = |index: usize| args.get(index).and_then(|part| part.to_str());
    let command = match (word(0), word(1), args.len()) {
        (Some("serve"), _, 1) => None,
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
                "usage: kuvo-box serve | route [--internal [path]] | route-to (set KUVO_HOME)"
                    .into(),
            );
        }
    };
    let home = env::var_os("KUVO_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("KUVO_HOME must name an absolute scratch path"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        if let Some(command) = command {
            tokio::select! {
                result = ssh::pipe(&home, command, tokio::io::stdin(), tokio::io::stdout()) => result?,
                () = shutdown() => {},
            }
            Ok(())
        } else {
            serve::run(&home).await
        }
    });
    // An addressed SSH exchange can finish while its stdin is still open. The
    // source is stdio, so Tokio's blocking read only ends when that SSH channel closes.
    runtime.shutdown_background();
    result
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
