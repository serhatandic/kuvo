#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    use kuvo_os::process::{Spawn, Stdio};
    use std::{
        env, fs,
        io::{self, Write as _},
        process,
    };

    fn run() -> io::Result<i32> {
        let own = env::current_exe()?;
        let (bin, home) = own
            .parent()
            .and_then(|bin| Some((bin, bin.parent()?)))
            .ok_or_else(|| {
                io::Error::other("Kuvo's starter must be in its installation's bin folder")
            })?;
        kuvo_os::windows::private_home(home, false)?;
        let run = home.join("run");
        // A recurring task repairs a crashed keeper. A deliberate stop stays stopped.
        if run.join("off").try_exists()? || run.join("paused").try_exists()? {
            return Ok(0);
        }
        fs::create_dir_all(&run)?;
        let mut log = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(run.join("keeper.log"))?;
        let mut command = Spawn::new(
            bin.join("kuvo-box.exe"),
            env::vars_os()
                .filter(|(key, _)| {
                    !key.as_encoded_bytes().starts_with(b"=")
                        && !key.eq_ignore_ascii_case("KUVO_HOME")
                })
                .collect(),
        );
        command
            .env
            .insert("KUVO_HOME".into(), home.as_os_str().to_owned());
        command.args.push("keep".into());
        command.cwd = Some(home.to_owned());
        command.stdout = Stdio::File(log.try_clone()?);
        command.stderr = Stdio::File(log.try_clone()?);
        let exit = command
            .spawn_resident(&format!("kuvo-starter-{}", process::id()))
            .and_then(|mut keeper| keeper.wait())
            .and_then(|status| {
                status
                    .code()
                    .ok_or_else(|| io::Error::other("Kuvo's keeper has no exit code"))
            });
        if let Err(error) = &exit {
            let _ = writeln!(log, "Kuvo starter: {error}");
        }
        exit
    }

    process::exit(run().unwrap_or(1));
}

#[cfg(not(windows))]
fn main() {
    std::process::exit(1);
}
