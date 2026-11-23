//! Claude Code's credentials belong to its account, outside Kuvo Helper's
//! Keychain namespace. Use the same security tool as the TypeScript move.
use std::{
    env, io,
    io::Write as _,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use kuvo_os::process::{Spawn, Stdio};
use zeroize::Zeroizing;

use super::{GenericPassword, KeychainItems};
use kuvo_store::home::Home;

pub struct ClaudeKeychain {
    file: Option<PathBuf>,
}

impl ClaudeKeychain {
    pub fn for_test_file(file: PathBuf) -> io::Result<Self> {
        if !file.is_absolute() || !file.is_file() {
            return Err(io::Error::other("The test Keychain file is missing."));
        }
        Ok(Self { file: Some(file) })
    }

    pub(super) fn for_move(home: &Home, system_account: &Path) -> io::Result<Option<Self>> {
        if super::is_account_home(home, system_account).unwrap_or(false) {
            return Ok(Some(Self { file: None }));
        }
        env::var_os("KUVO_TEST_REHOME_KEYCHAIN")
            .filter(|file| !file.is_empty())
            .map(|file| Self::for_test_file(file.into()))
            .transpose()
    }

    fn command(&self, args: &[&str]) -> io::Result<(bool, Zeroizing<Vec<u8>>, bool)> {
        let mut args: Vec<_> = args.iter().map(Into::into).collect();
        args.extend(self.file.iter().map(|file| file.as_os_str().to_owned()));
        run(args, None)
    }
}

impl KeychainItems for ClaudeKeychain {
    fn read(&self, service: &str) -> io::Result<Option<GenericPassword>> {
        let (found, details, _) = self.command(&["find-generic-password", "-s", service])?;
        if !found {
            return Ok(None);
        }
        let account = std::str::from_utf8(&details)
            .ok()
            .and_then(|details| details.lines().find_map(account))
            .ok_or_else(read_error)?;
        let (found, mut value, _) =
            self.command(&["find-generic-password", "-s", service, "-a", &account, "-w"])?;
        if !found {
            return Err(read_error());
        }
        if value.last() == Some(&b'\n') {
            value.pop();
        }
        Ok(Some(GenericPassword {
            account,
            value: std::mem::take(&mut *value),
        }))
    }

    fn write(&self, service: &str, item: &GenericPassword) -> io::Result<()> {
        let mut line = Zeroizing::new(String::from("add-generic-password -U -a "));
        quoted(&mut line, &item.account)?;
        line.push_str(" -s ");
        quoted(&mut line, service)?;
        line.push_str(" -X ");
        let encoded = Zeroizing::new(hex::encode(&item.value));
        line.push_str(&encoded);
        if let Some(file) = &self.file {
            line.push(' ');
            quoted(
                &mut line,
                file.to_str()
                    .ok_or_else(|| io::Error::other("The test Keychain path isn't UTF-8."))?,
            )?;
        }
        line.push('\n');
        let (written, _, refused) = run(vec!["-i".into()], Some(line))?;
        if !written || refused {
            return Err(io::Error::other(
                "Claude Code's sign-in couldn't be written.",
            ));
        }
        Ok(())
    }

    fn delete(&self, service: &str, _account: &str) -> io::Result<()> {
        self.command(&["delete-generic-password", "-s", service])?;
        Ok(())
    }
}

fn account(line: &str) -> Option<String> {
    let printed = line.trim_start().strip_prefix("\"acct\"<blob>=")?;
    if let Some(hex) = printed.strip_prefix("0x") {
        let hex = hex.split_ascii_whitespace().next()?;
        return String::from_utf8(hex::decode(hex).ok()?).ok();
    }
    let mut chars = printed.strip_prefix('"')?.chars();
    let mut found = String::new();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(found),
            '\\' => {
                found.push('\\');
                found.push(chars.next()?);
            }
            _ => found.push(character),
        }
    }
    None
}

fn read_error() -> io::Error {
    io::Error::other("Claude Code's sign-in couldn't be read.")
}

fn quoted(out: &mut String, text: &str) -> io::Result<()> {
    if text.contains(['\n', '\r', '\0']) {
        return Err(io::Error::other("Invalid Keychain item name."));
    }
    out.push('"');
    for character in text.chars() {
        if matches!(character, '"' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    Ok(())
}

fn run(
    args: Vec<std::ffi::OsString>,
    input: Option<Zeroizing<String>>,
) -> io::Result<(bool, Zeroizing<Vec<u8>>, bool)> {
    let mut command = Spawn::new("/usr/bin/security", env::vars_os().collect());
    command.args = args;
    command.stdout = Stdio::Pipe;
    command.stderr = Stdio::Pipe;
    if input.is_some() {
        command.stdin = Stdio::Pipe;
    }
    let mut child = command.spawn()?;
    let output = child.stdout.take().expect("security stdout");
    let errors = child.stderr.take().expect("security stderr");
    let output = thread::spawn(move || read(output));
    let errors = thread::spawn(move || read(errors));
    let writer = input.map(|input| {
        let mut stdin = child.stdin.take().expect("security stdin");
        thread::spawn(move || stdin.write_all(input.as_bytes()))
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    while !child.has_exited()? {
        if Instant::now() >= deadline {
            child.stop(Duration::from_secs(1), |_| {})?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Claude Code's Keychain operation timed out.",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let success = child.wait()?.success();
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| io::Error::other("Keychain command failed."))??;
    }
    let output = output.join().map_err(|_| read_error())??;
    let errors = errors.join().map_err(|_| read_error())??;
    let refused = errors
        .windows(5)
        .any(|word| word.eq_ignore_ascii_case(b"error") || word.eq_ignore_ascii_case(b"usage"));
    Ok((success, output, refused))
}

fn read(mut pipe: impl io::Read) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(Vec::new());
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}
