//! Preparation and activation of a signed fleet release.

use std::{fs, io, path::Path, time::Duration};

use kuvo_store::{files, home::Home};
use serde::{Deserialize, Serialize};

use crate::{
    Command, Release, SELECTOR, execute,
    manifest::{ReleaseRef, SignedRelease, local_target, sha256},
    prepare_command::version_order,
    read_record, staging,
};

const STAGE_WAIT: i64 = 300_000;

#[derive(Clone)]
pub struct Candidate {
    signed: SignedRelease,
    envelope: String,
}

impl Candidate {
    pub fn verify(envelope: String, key: &str) -> io::Result<Self> {
        if envelope.len() as u64 > staging::MAX_ENVELOPE {
            return Err(io::Error::other("That release can't be read."));
        }
        let signed = SignedRelease::verify(envelope.as_bytes(), &[key])?;
        if !signed.spreads() {
            return Err(io::Error::other(
                "This release cannot be shared with your setup.",
            ));
        }
        Ok(Self { signed, envelope })
    }

    pub fn envelope(&self) -> &str {
        &self.envelope
    }
    pub fn version(&self) -> &str {
        self.signed.version()
    }
    pub fn old_enough_for_automatic_check(&self) -> bool {
        self.signed.released().is_some_and(|release| {
            jiff::Timestamp::now()
                .as_millisecond()
                .saturating_sub(release.as_millisecond())
                >= 3 * 86_400_000
        })
    }
    pub fn newer_than(&self, version: Option<&str>) -> bool {
        version_order(self.version()) > version.and_then(version_order)
    }

    pub fn matches_pack(&self, target: &str, digest: &str) -> bool {
        self.signed
            .pack_for(target)
            .is_ok_and(|pack| pack.sha256 == digest)
    }

    pub fn local(&self) -> io::Result<ReleaseRef> {
        Ok(self
            .signed
            .reference(self.signed.pack_for(local_target()?)?))
    }

    pub fn kept(home: &Home, key: &str) -> io::Result<Option<Self>> {
        files::text_of(&home.updates().join("release/boxes.json"))
            .map(|bytes| Self::verify(bytes, key))
            .transpose()
    }

    pub fn keep(&self, home: &Home) -> io::Result<()> {
        let path = home.updates().join("release/boxes.json");
        if files::read(&path)?.as_deref() == Some(self.envelope.as_bytes()) {
            return Ok(());
        }
        files::write_secret(&path, self.envelope.as_bytes())
    }

    pub fn is_prepared(&self, home: &Home) -> io::Result<bool> {
        let release = self.local()?;
        let target = release.target()?;
        if read_record::<ReleaseRef>(&home.run().join("fleet-ready.json"))?.as_ref()
            != Some(&release)
        {
            return Ok(false);
        }
        if files::text_of(&home.run().join("update-cannot")).is_some_and(|text| {
            let mut words = text.split_whitespace().skip(1);
            words.next() == Some(&release.version)
                && words
                    .next()
                    .is_some_and(|digest| release.sha256.starts_with(digest))
        }) {
            return Ok(false);
        }
        match target.validate(home.root()) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub async fn prepare(self, home: Home, fetcher: Fetcher) -> io::Result<()> {
        let target = local_target()?;
        let pack = self.signed.pack_for(target)?;
        let bytes = download(
            &fetcher.0,
            &pack.url,
            staging::MAX_PACK,
            Duration::from_secs(300),
        )
        .await?;
        tokio::task::spawn_blocking(move || {
            staging::prepare(&home, &self.signed, target, &bytes)?;
            Ok(())
        })
        .await
        .map_err(io::Error::other)?
    }

    pub fn primary_may_switch(
        &self,
        home: &Home,
        now: i64,
        ready_count: usize,
        reachable: usize,
    ) -> io::Result<bool> {
        #[derive(Deserialize, Serialize)]
        struct Clock {
            release: String,
            at: i64,
        }
        let path = home.run().join("fleet-stage-clock.json");
        let release = sha256(self.envelope.as_bytes());
        let local = self.is_prepared(home)?;
        if !local && ready_count == 0 {
            return Ok(false);
        }
        let clock = match read_record::<Clock>(&path)? {
            Some(clock) if clock.release == release => clock,
            _ => {
                let clock = Clock { release, at: now };
                files::write_secret(&path, &serde_json::to_vec(&clock)?)?;
                clock
            }
        };
        Ok(local && (ready_count == reachable || now.saturating_sub(clock.at) >= STAGE_WAIT))
    }

    pub async fn activate(&self, home: &Home) -> io::Result<()> {
        if !self.is_prepared(home)? || stopped(home) || home.run().join("previous").exists() {
            return Ok(());
        }
        if let Some(engine) = read_record::<serde_json::Value>(&home.run().join("engine.json"))?
            && engine["version"].as_str() != Some(self.signed.engine())
        {
            return Err(io::Error::other(
                "The update is waiting for an engine handover.",
            ));
        }
        let release = self.local()?.target()?;
        files::write_secret(
            &release.folder(home.root()).join("boxes.json"),
            self.envelope.as_bytes(),
        )?;
        if home.root().join(SELECTOR).is_file() {
            execute(
                home,
                Default::default(),
                Command::Select(release),
                std::future::pending(),
            )
            .await
        } else {
            shell_switch(home, release).await
        }
    }
}

pub fn stopped(home: &Home) -> bool {
    ["off", "paused", "stopped"]
        .iter()
        .any(|name| home.run().join(name).exists())
}

pub fn installed(home: &Home) -> io::Result<(Option<String>, Option<Release>)> {
    let selected = Release::read(&home.root().join(SELECTOR))?
        .or_else(|| fs::read_link(home.bin()).ok()?.to_str()?.parse().ok());
    let working = Release::read(&home.run().join("previous"))?.or(selected);
    let version = working
        .as_ref()
        .map(|working| working.version().to_owned())
        .or_else(|| {
            files::text_of(&home.bin().join("box-version")).map(|version| version.trim().to_owned())
        });
    Ok((version, working))
}

pub fn installed_envelope(home: &Home, release: &Release) -> Option<String> {
    files::text_of(&release.folder(home.root()).join("boxes.json"))
}

#[derive(Default, Clone)]
pub struct Fetcher(reqwest::Client);

impl Fetcher {
    pub async fn candidate(&self, url: &str, key: &str) -> io::Result<Candidate> {
        let bytes = download(&self.0, url, staging::MAX_ENVELOPE, Duration::from_secs(30)).await?;
        Candidate::verify(String::from_utf8(bytes).map_err(io::Error::other)?, key)
    }
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    max: u64,
    timeout: Duration,
) -> io::Result<Vec<u8>> {
    for wait in [Duration::from_secs(5), Duration::from_secs(20)] {
        match download_once(client, url, max, timeout).await {
            Err(DownloadError::Request(error)) if retry_download(&error) => {
                tokio::time::sleep(wait).await;
            }
            result => return result.map_err(DownloadError::into_io),
        }
    }
    download_once(client, url, max, timeout)
        .await
        .map_err(DownloadError::into_io)
}

fn retry_download(error: &reqwest::Error) -> bool {
    error.status().map_or(!error.is_builder(), |status| {
        status.is_server_error()
            || matches!(
                status,
                reqwest::StatusCode::REQUEST_TIMEOUT | reqwest::StatusCode::TOO_MANY_REQUESTS
            )
    })
}

enum DownloadError {
    Request(reqwest::Error),
    TooLarge,
}

impl DownloadError {
    fn into_io(self) -> io::Error {
        match self {
            Self::Request(source) => io::Error::other(source),
            Self::TooLarge => io::Error::other("It's larger than a release can be."),
        }
    }
}

async fn download_once(
    client: &reqwest::Client,
    url: &str,
    max: u64,
    timeout: Duration,
) -> Result<Vec<u8>, DownloadError> {
    let mut response = client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(DownloadError::Request)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(DownloadError::Request)? {
        if (bytes.len() + chunk.len()) as u64 > max {
            return Err(DownloadError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(unix)]
async fn shell_switch(home: &Home, target: Release) -> io::Result<()> {
    // The detached keeper command must outlive the outgoing resident. Only
    // ask to keep workers after the keeper itself has confirmed compatibility.
    let root = home
        .root()
        .parent()
        .filter(|root| home.root() == root.join(".kuvo-code"))
        .ok_or_else(|| io::Error::other("This keeper doesn’t run from Kuvo’s home folder."))?;
    let current = fs::read_link(home.bin())?;
    let current: Release = current
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid bin link"))?
        .parse()?;
    if current == target {
        return Ok(());
    }
    let keeper = home.root().join(target.to_string()).join("kuvo-start");
    let checked = std::process::Command::new("/bin/sh")
        .arg(&keeper)
        .args(["switch-engine", &target.to_string()])
        .env("HOME", root)
        .output()?;
    if !checked.status.success() || checked.stdout != b"keep\n" {
        return Err(io::Error::other(
            "The update is waiting for an engine handover.",
        ));
    }
    let previous = home.run().join("previous");
    files::write_new(&previous, current.to_string().as_bytes())?;
    let mut command = kuvo_os::process::Spawn::new("/bin/sh", std::env::vars_os().collect());
    command
        .env
        .insert("HOME".into(), root.as_os_str().to_owned());
    command.cwd = Some(home.root().to_owned());
    command.args = vec![
        keeper.into_os_string(),
        "stop".into(),
        "--switch".into(),
        target.to_string().into(),
        "keep".into(),
    ];
    match command.spawn() {
        Ok(mut child) => child.leave_running_on_drop(),
        Err(error) => {
            files::remove_file_synced(&previous)?;
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(not(unix))]
async fn shell_switch(_home: &Home, _target: Release) -> io::Result<()> {
    Err(io::Error::other(
        "The keeper has not selected its first release.",
    ))
}

pub fn checked_home(home: &Home, profile: &Path) -> bool {
    home.root() == profile.join(".kuvo-code")
}
