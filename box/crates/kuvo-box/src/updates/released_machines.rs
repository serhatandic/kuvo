//! A released 0.7 machine has no release mail reader. Its main sends the signed
//! installer over the address it saved while setting up that account.

use std::{io, time::Duration};

use kuvo_keeper::fleet::{self, Candidate, Fetcher, InstallerMedia};
use kuvo_setup::{SavedMachine, SetupHandle, with_saved_machine};
use kuvo_store::{files, home::Home};
use serde_json::Value;
use tokio::{task::JoinHandle, time::Instant};

pub struct Checks {
    next: Instant,
    every: Duration,
    task: Option<JoinHandle<io::Result<()>>>,
}

impl Checks {
    pub fn new(scratch: bool) -> Self {
        let (delay, every) = if scratch {
            (Duration::ZERO, Duration::from_secs(2))
        } else {
            (Duration::from_secs(20), Duration::from_secs(15 * 60))
        };
        Self {
            next: Instant::now() + delay,
            every,
            task: None,
        }
    }

    pub async fn tick(
        &mut self,
        home: &Home,
        release: Option<&Candidate>,
        fetcher: &Fetcher,
        setup: &SetupHandle,
        standalone: bool,
    ) {
        if self.task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        if let Some(task) = self.task.take() {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("Checking the released machines: {error}"),
                Err(error) => eprintln!("The released-machine check stopped: {error}"),
            }
        }
        if Instant::now() >= self.next
            && home.root().join("machines.json").is_file()
            && let Some(release) = release
        {
            self.next = Instant::now() + self.every;
            self.task = Some(tokio::spawn(round(
                home.clone(),
                release.clone(),
                fetcher.clone(),
                setup.clone(),
                standalone,
            )));
        }
    }
}

impl Drop for Checks {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

const PROBE: &str = r#"
set -eu
K="$HOME/.kuvo-code"
[ "$(uname -s)" = Linux ] || exit 0
[ -d "$K" ] && [ ! -L "$K" ] && [ ! -e "$K/run/off" ] || exit 0
if [ -L "$K/bin" ]; then
    # An attempt that reached selection can still owe the keeper start.
    J="$K/updates/install/released-machine.json"
    [ -f "$J" ] && [ ! -L "$J" ] || exit 0
elif [ -f "$K/bin/box-version" ]; then
    case "$(cat "$K/bin/box-version")" in 0.7.2 | 0.8.0-beta.1) ;; *) exit 0 ;; esac
else
    [ -f "$K/bin/kuvo-start" ] || [ -f "$K/run/link.json" ] || exit 0
fi
case "$(uname -m)" in
    x86_64) printf 'x86_64-unknown-linux-musl\n' ;;
    aarch64) printf 'aarch64-unknown-linux-musl\n' ;;
esac
"#;

async fn round(
    home: Home,
    release: Candidate,
    fetcher: Fetcher,
    setup: SetupHandle,
    standalone: bool,
) -> io::Result<()> {
    let Some(bytes) = files::read(&home.root().join("machines.json"))? else {
        return Ok(());
    };
    let records: Vec<Value> = serde_json::from_slice(&bytes)?;
    for record in records {
        let Some(destination) = record["destination"].as_str() else {
            continue;
        };
        if fleet::stopped(&home) || !standalone && !setup.may_switch_fleet().await {
            break;
        }
        let result = upgrade(&home, destination, &release, &fetcher, &setup, standalone).await;
        if let Err(error) = result {
            eprintln!("Checking the released machine {destination}: {error}");
        }
    }
    Ok(())
}

async fn upgrade(
    home: &Home,
    destination: &str,
    release: &Candidate,
    fetcher: &Fetcher,
    setup: &SetupHandle,
    standalone: bool,
) -> io::Result<()> {
    let Some(target) = with_saved_machine(home, destination, probe).await? else {
        return Ok(());
    };
    // A slow release download must never keep the account reserved.
    let media = release.installer_media(home, target, fetcher).await?;
    with_saved_machine(home, destination, async |machine| {
        if probe(machine).await? != Some(target)
            || fleet::stopped(home)
            || !standalone && !setup.may_switch_fleet().await
        {
            return Ok(());
        }
        let script = install_script(machine.identity(), &media);
        let installed = machine
            .run(&script, media.archive, Duration::from_secs(1200))
            .await?;
        if installed.status.success() || installed.status.code() == Some(4) {
            return Ok(());
        }
        let reply: Value = serde_json::from_slice(&installed.stdout).unwrap_or_default();
        let message = reply["message"]
            .as_str()
            .unwrap_or("The released machine couldn't install its signed update.");
        Err(io::Error::other(message.to_owned()))
    })
    .await
}

async fn probe(machine: &mut SavedMachine) -> io::Result<Option<&'static str>> {
    let probe = machine
        .run(PROBE, Vec::new(), Duration::from_secs(60))
        .await?;
    if !probe.status.success() {
        return Err(io::Error::other(
            "The released machine couldn't be checked.",
        ));
    }
    Ok(
        match std::str::from_utf8(&probe.stdout)
            .map_err(io::Error::other)?
            .trim()
        {
            "x86_64-unknown-linux-musl" => Some("x86_64-unknown-linux-musl"),
            "aarch64-unknown-linux-musl" => Some("aarch64-unknown-linux-musl"),
            _ => None,
        },
    )
}

fn install_script(identity: &str, media: &InstallerMedia) -> String {
    // The transport checked the saved machine ID. The pack name and digest
    // have a restricted alphabet and came from the verified manifest.
    format!(
        r#"set -eu
umask 077
K="$HOME/.kuvo-code"
I="$K/updates/install"
[ ! -L "$K" ] && [ ! -L "$K/updates" ] && [ ! -L "$I" ] || exit 78
mkdir -p "$I"
T=$(mktemp -d "$I/.incoming.XXXXXXXX") || exit 1
trap 'rm -rf -- "$T"; rmdir "$I" 2>/dev/null || true' EXIT
trap 'exit 1' HUP INT TERM
tar xf - -C "$T" boxes.json kuvo-install '{pack}'
sum=$(sha256sum <"$T/{pack}")
[ "${{sum%% *}}" = '{digest}' ] || exit 1
tar xOzf "$T/{pack}" bin/kuvo-box >"$T/kuvo-install-bin" 2>/dev/null ||
    tar xOzf "$T/{pack}" ./bin/kuvo-box >"$T/kuvo-install-bin"
chmod 700 "$T/kuvo-install-bin"
sh "$T/kuvo-install" released-machine --identity '{identity}'
"#,
        pack = media.pack_name,
        digest = media.pack_sha256,
    )
}
