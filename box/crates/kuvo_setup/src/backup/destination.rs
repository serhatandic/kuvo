//! Archive locations and publish/readback operations.
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use kuvo_store::{declarations, files, locks::OwnedLock};
use serde::{Deserialize, Serialize};

use super::{Error, PointId, Result, Source, archive};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Destination {
    Disk {
        path: PathBuf,
    },
    Computer {
        computer: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        ssh: Option<String>,
    },
}

pub trait DestinationStore {
    fn list(&self) -> Result<Vec<PointId>>;
    fn put(&self, id: &PointId, archive: &Path) -> Result<()>;
    fn get(&self, id: &PointId, scratch: &Path) -> Result<PathBuf>;
    fn erase(&self, id: &PointId) -> Result<()>;
}

fn storage(kind: &str) -> (&'static str, &'static str) {
    let storage = declarations::data(kind)
        .and_then(|kind| kind.external)
        .expect("backup destination is declared");
    (
        storage.root,
        storage.archive_suffix.expect("archive suffix is declared"),
    )
}

fn points(names: impl IntoIterator<Item = String>, suffix: &str) -> Vec<PointId> {
    let mut points: Vec<PointId> = names
        .into_iter()
        .filter_map(|name| name.strip_suffix(suffix)?.to_owned().try_into().ok())
        .collect();
    points.sort_unstable_by(|a, b| b.cmp(a));
    points
}

pub struct DiskStore {
    root: PathBuf,
    selected: PathBuf,
    suffix: &'static str,
}

impl DiskStore {
    pub fn open(folder: &Path, home: &Path, source: &Source) -> Result<Self> {
        if !folder.is_absolute() {
            return Err(Error::Message(
                "Choose an enrolled backup computer or a folder on a connected disk.",
            ));
        }
        let folder = folder.canonicalize()?;
        let home = home.canonicalize()?;
        if folder.starts_with(&home) || home.starts_with(&folder) {
            return Err(Error::Message(
                "Choose a backup folder separate from the Kuvo folder.",
            ));
        }
        let (prefix, suffix) = storage("diskBackupArchives");
        let base = folder.join(prefix);
        let root = base.join(source.to_string());
        for part in [&base, &root] {
            match fs::create_dir(part) {
                Ok(()) => files::private_folder(part)?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error.into()),
            }
            if !fs::symlink_metadata(part)?.is_dir() || part.canonicalize()? != *part {
                return Err(Error::Message("The backup folder contains a link."));
            }
        }
        Ok(Self {
            root,
            selected: folder,
            suffix,
        })
    }

    fn at(&self, id: &PointId) -> PathBuf {
        self.root.join(format!("{id}{}", self.suffix))
    }

    pub fn folder(&self) -> &Path {
        &self.selected
    }
}

impl DestinationStore for DiskStore {
    fn list(&self) -> Result<Vec<PointId>> {
        let entries = fs::read_dir(&self.root)?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(points(entries, self.suffix))
    }

    fn put(&self, id: &PointId, input: &Path) -> Result<()> {
        let file = self.at(id);
        let _guard = OwnedLock::acquire(
            &self.root.join(format!("{id}{}.publishing", self.suffix)),
            Duration::from_millis(100),
        )?;
        match fs::symlink_metadata(&file) {
            Ok(_) => return Err(Error::Message("That backup point already exists.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        // No hard-link requirement on the removable disk. Every writer takes the
        // same on-disk publishing lock before testing the final name.
        let mut partial = tempfile::Builder::new()
            .suffix(".partial")
            .tempfile_in(&self.root)?;
        io::copy(&mut archive::regular(input)?, partial.as_file_mut())?;
        partial.as_file().sync_all()?;
        files::replace_synced(partial.path(), &file)?;
        Ok(())
    }

    fn get(&self, id: &PointId, _scratch: &Path) -> Result<PathBuf> {
        Ok(self.at(id))
    }

    fn erase(&self, id: &PointId) -> Result<()> {
        files::remove_file_synced(&self.at(id))?;
        Ok(())
    }
}

pub struct SshStore {
    program: PathBuf,
    scratch: PathBuf,
    target: String,
    known_hosts: String,
    root: String,
    suffix: &'static str,
}

impl SshStore {
    pub fn new(home: &Path, account: &Path, target: &str, source: &Source) -> Result<Self> {
        if !valid_ssh(target) {
            return Err(Error::Message(
                "Choose an enrolled backup computer or a folder on a connected disk.",
            ));
        }
        let yours = account.join(".ssh");
        let known_hosts = [
            yours.join("known_hosts"),
            yours.join("known_hosts2"),
            home.join("ssh/known_hosts"),
        ]
        .map(|path| {
            format!(
                "\"{}\"",
                path.to_string_lossy()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
            )
        })
        .join(" ");
        let (base, suffix) = storage("sshBackupArchives");
        let root = format!("base=\"$HOME/{base}\"\nroot=\"$base/{source}\"\n");
        let scratch = home.join("backup");
        files::private_folder(&scratch)?;
        Ok(Self {
            scratch,
            program: if cfg!(target_os = "macos") {
                "/usr/bin/ssh".into()
            } else {
                "ssh".into()
            },
            target: target.into(),
            known_hosts,
            root,
            suffix,
        })
    }

    /// The caller supplies the system SSH program (or the isolated scenario's SSH host).
    pub fn with_program(mut self, program: PathBuf) -> Self {
        self.program = program;
        self
    }

    fn command(
        &self,
        script: &str,
        input: Option<File>,
        output: Option<File>,
        scratch: &Path,
    ) -> Result<String> {
        let mut text = tempfile::tempfile_in(scratch)?;
        let command = format!(
            "exec sh -c 'kuvo_script=$(printf %s {} | base64 -d) || exit 127; eval \"$kuvo_script\"'",
            STANDARD.encode(script)
        );
        let mut child = Command::new(&self.program)
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "RemoteCommand=none",
                "-o",
                "RequestTTY=no",
                "-o",
                "ConnectTimeout=15",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ForwardX11=no",
                "-o",
                "UpdateHostKeys=no",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ControlMaster=no",
                "-o",
                "ControlPath=none",
            ])
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", self.known_hosts))
            .args(["--", &self.target, &command])
            .stdin(input.map_or_else(Stdio::null, Stdio::from))
            .stdout(Stdio::from(match output {
                Some(file) => file,
                None => text.try_clone()?,
            }))
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(45 * 60);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Message(
                        "Could not reach or use the backup folder over SSH.",
                    ));
                }
            }
        };
        if !status.success() {
            return Err(Error::Message(
                "Could not reach or use the backup folder over SSH.",
            ));
        }
        use io::{Seek, SeekFrom};
        text.seek(SeekFrom::Start(0))?;
        let mut stdout = String::new();
        text.take(4_000_000).read_to_string(&mut stdout)?;
        Ok(stdout)
    }

    fn run(
        &self,
        script: &str,
        create: bool,
        input: Option<File>,
        output: Option<File>,
        scratch: &Path,
    ) -> Result<String> {
        let dir = if create {
            "mkdir -p \"$root\""
        } else {
            "[ -d \"$root\" ] || exit 0"
        };
        self.command(&format!(
            "set -eu\numask 077\n{}[ ! -L \"$base\" ] && [ ! -L \"$root\" ] || exit 1\n{dir}\n{script}",
            self.root
        ), input, output, scratch)
    }

    fn name(&self, id: &PointId) -> String {
        format!("\"$root/{id}{}\"", self.suffix)
    }

    pub fn verify(&self, computer: &str, scratch: &Path) -> Result<()> {
        let result = self.command(r#"set -eu
export KUVO_HOME="$HOME/.kuvo-code" BUN_BE_BUN=1
if [ -x "$KUVO_HOME/bin/kuvo-box" ]; then
  exec "$KUVO_HOME/bin/kuvo-box" backup-identity
fi
version=$(sed -n 's/^V=\([a-zA-Z0-9._-]*\)$/\1/p' "$KUVO_HOME/bin/kuvo-start" | head -n 1)
[ -n "$version" ] && exec "$KUVO_HOME/engine/$version/opencode" --config=/dev/null --no-env-file "$KUVO_HOME/bin/kuvo-link.js" backup-identity"#, None, None, scratch);
        if result.is_ok_and(|answer| answer.trim() == computer) {
            Ok(())
        } else {
            Err(Error::Message(
                "SSH did not reach the selected Kuvo computer.",
            ))
        }
    }
}

pub(super) fn valid_ssh(value: &str) -> bool {
    let first = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    value.len() <= 255
        && value.bytes().next().is_some_and(first)
        && value
            .bytes()
            .all(|byte| first(byte) || b".@:[]-".contains(&byte))
}

impl DestinationStore for SshStore {
    fn list(&self) -> Result<Vec<PointId>> {
        // Listing is invoked by an operation that already owns the backup scratch
        // folder. The SSH response just contains names, never archive data.
        let script = format!(
            "for file in \"$root\"/*{0}; do [ -f \"$file\" ] && printf \"%s\\n\" \"${{file##*/}}\"; done\nexit 0",
            self.suffix
        );
        let answer = self.run(&script, false, None, None, &self.scratch)?;
        Ok(points(
            answer.trim().split('\n').map(str::to_owned),
            self.suffix,
        ))
    }

    fn put(&self, id: &PointId, input: &Path) -> Result<()> {
        let script = format!(
            r#"dest={}
tmp=$(mktemp "$root/.backup.XXXXXXXX")
trap 'rm -f "$tmp"' EXIT
cat > "$tmp"
sync
ln "$tmp" "$dest"
sync"#,
            self.name(id)
        );
        self.run(
            &script,
            true,
            Some(archive::regular(input)?),
            None,
            &self.scratch,
        )?;
        Ok(())
    }

    fn get(&self, id: &PointId, scratch: &Path) -> Result<PathBuf> {
        let file = scratch.join("received");
        let output = File::options().write(true).create_new(true).open(&file)?;
        let script = format!(
            "file={}; [ -f \"$file\" ] && [ ! -L \"$file\" ] && cat \"$file\"",
            self.name(id)
        );
        self.run(&script, false, None, Some(output), scratch)?;
        Ok(file)
    }

    fn erase(&self, id: &PointId) -> Result<()> {
        self.run(
            &format!("rm -f {}\nsync", self.name(id)),
            false,
            None,
            None,
            &self.scratch,
        )?;
        Ok(())
    }
}
