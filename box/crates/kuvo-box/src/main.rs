mod serve;

use std::{env, io, path::PathBuf, process::ExitCode};

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
    let mut args = env::args_os().skip(1);
    if !args.next().is_some_and(|mode| mode == "serve") || args.next().is_some() {
        return Err("usage: kuvo-box serve (set KUVO_HOME to a scratch home)".into());
    }
    let home = env::var_os("KUVO_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("KUVO_HOME must name an absolute scratch path"))?;
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(serve::run(&home))
}
