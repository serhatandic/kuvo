use std::sync::{Arc, atomic::AtomicBool};

use tokio::sync::oneshot;

use super::{Destination, Input, Job, Progress, Ran, SeedTransfer};
use crate::ssh::{
    AddError,
    account_scripts::{BAD_ACCOUNT, BUSY, LOST, locked_script, new_token, owns, release_script},
};

const RECOVER: i32 = 76;
const OLD: i32 = 77;

/// A marker reserves the account across SSH commands. Each writer holds the
/// marker's file lock until it exits, including writers left behind by SSH.
pub(in crate::ssh) struct Reservation<'a> {
    remote: &'a Destination,
    job: &'a Job,
    token: String,
    settled: bool,
}

impl Destination {
    pub(in crate::ssh) async fn reserve<'a>(
        &'a self,
        job: &'a Job,
    ) -> Result<Option<Reservation<'a>>, String> {
        if self.target.os == "windows" {
            return Ok(None);
        }
        let token = new_token().map_err(|_| "Couldn’t reserve the SSH account.")?;
        job.reservations.lock().unwrap().insert(token.clone());
        let reservation = Reservation {
            remote: self,
            job,
            token,
            settled: true,
        };
        reservation.acquire().await?;
        Ok(Some(reservation))
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.job.reservations.lock().unwrap().remove(&self.token);
    }
}

impl Reservation<'_> {
    pub(in crate::ssh) fn token(&self) -> &str {
        &self.token
    }

    async fn acquire(&self) -> Result<(), String> {
        let ran = self.claim(None).await?;
        match ran.status.code() {
            Some(0) => return Ok(()),
            Some(BUSY) => return Err(self.busy()),
            Some(OLD) => {
                let name = self.remote.name();
                return Err(format!(
                    "An older version of Kuvo Code left an unfinished change on {name}. Restart {name} to end that change, then sign in to the same OS account and run rm -f ~/.kuvo-code-adding before trying again."
                ));
            }
            Some(RECOVER) => {}
            _ => return Err(self.cannot_prepare(&ran)),
        }
        let previous = String::from_utf8_lossy(&ran.stdout);
        let previous = previous.trim();
        if !valid_token(previous) {
            return Err(
                "SSH setup reservation format is unsupported. Update Kuvo and retry.".into(),
            );
        }
        if self.job.reservations.lock().unwrap().contains(previous) {
            return Err(self.busy());
        }
        let (answer, answered) = oneshot::channel();
        self.job
            .send(Progress::RecoverAccount(
                self.remote.name().to_owned(),
                answer,
            ))
            .await?;
        answered.await.map_err(|_| "The Add was stopped.")?;
        self.job.admit().await?;
        let ran = self.claim(Some(previous)).await?;
        match ran.status.code() {
            Some(0) => Ok(()),
            Some(BUSY) => Err(self.busy()),
            Some(LOST) => Err(format!(
                "The account on {} changed while you were deciding. Try again.",
                self.remote.name()
            )),
            _ => Err(self.cannot_prepare(&ran)),
        }
    }

    async fn claim(&self, previous: Option<&str>) -> Result<Ran, String> {
        let script = claim_script(&self.remote.root, &self.token, previous);
        self.remote.shell(self.job, &script, Vec::new(), 30).await
    }

    pub(in crate::ssh) async fn run(
        &mut self,
        script: &str,
        input: Vec<u8>,
        timeout: u64,
    ) -> Result<Ran, String> {
        self.run_locked(script, Input::Bytes(input), timeout, self.job.stop.clone())
            .await
    }

    pub(in crate::ssh) async fn run_seed(
        &mut self,
        script: &str,
        input: SeedTransfer,
        timeout: u64,
    ) -> Result<Ran, String> {
        // Once this writer starts, wait for its result before rolling back or releasing.
        self.run_locked(
            script,
            Input::Seed(input),
            timeout,
            Arc::new(AtomicBool::new(false)),
        )
        .await
    }

    pub(in crate::ssh) async fn run_cleanup(
        &mut self,
        script: &str,
        timeout: u64,
    ) -> Result<Ran, String> {
        self.run_locked(
            script,
            Input::Bytes(Vec::new()),
            timeout,
            Arc::new(AtomicBool::new(false)),
        )
        .await
    }

    pub(in crate::ssh) fn can_cleanup(&self) -> bool {
        self.settled
    }

    async fn run_locked(
        &mut self,
        script: &str,
        input: Input,
        timeout: u64,
        stop: Arc<AtomicBool>,
    ) -> Result<Ran, String> {
        if !self.settled {
            return Err(self.interrupted());
        }
        self.settled = false;
        let script = locked_script(
            &self.remote.root,
            5,
            &format!("{} || exit {LOST}\n{script}", owns(&self.token)),
        );
        let ran = self
            .remote
            .shell_input(stop, &script, input, timeout)
            .await
            .map_err(|_| self.interrupted())?;
        let Some(code) = ran.status.code().filter(|code| *code != 255) else {
            return Err(self.interrupted());
        };
        self.settled = true;
        match code {
            BUSY => Err(self.busy()),
            LOST => Err(format!(
                "The account on {} was cleared for another attempt. Try again.",
                self.remote.name()
            )),
            BAD_ACCOUNT => Err(self.cannot_prepare(&ran)),
            _ => Ok(ran),
        }
    }

    pub(in crate::ssh) async fn release(self, failure: Option<&AddError>) -> Result<(), String> {
        if !self.settled {
            return Ok(());
        }
        let script = release_script(&self.remote.root, &self.token);
        let released = self
            .remote
            .shell_with_stop(Arc::new(AtomicBool::new(false)), &script, Vec::new(), 15)
            .await;
        if released.as_ref().is_ok_and(|ran| ran.status.success()) {
            return Ok(());
        }
        match &released {
            Ok(ran) => self.log("clear the SSH reservation", ran),
            Err(error) => eprintln!(
                "Kuvo couldn't clear the SSH reservation on {}: {error}",
                self.remote.name()
            ),
        }
        let name = self.remote.name();
        let detail = format!(
            "couldn’t clear the account on {name} for the next change. The next time you connect, Kuvo Code can check and offer to clear it."
        );
        Err(match failure {
            Some(failure) if failure.message != "The Add was stopped." => {
                format!("{} Kuvo Code {detail}", failure.message)
            }
            Some(_) => format!("Kuvo Code {detail}"),
            None => format!("Kuvo Code finished the change but {detail}"),
        })
    }

    fn busy(&self) -> String {
        format!(
            "Another Kuvo Code change is in progress on {}. Try again once it finishes.",
            self.remote.name()
        )
    }

    fn interrupted(&self) -> String {
        format!(
            "Kuvo Code lost contact with {} during a change. Try again to check whether it finished.",
            self.remote.name()
        )
    }

    fn cannot_prepare(&self, ran: &Ran) -> String {
        self.log("prepare the SSH account", ran);
        format!(
            "Kuvo Code couldn’t prepare the OS account on {}. Check that you can write to your home folder there, then try again.",
            self.remote.name()
        )
    }

    fn log(&self, action: &str, ran: &Ran) {
        let status = ran
            .status
            .code()
            .map_or_else(|| "null".into(), |code| code.to_string());
        eprintln!(
            "Kuvo couldn't {action} on {} (status {status}): {}",
            self.remote.name(),
            ran.stderr.lines().last().unwrap_or_default()
        );
    }
}

pub(in crate::ssh) fn claim_script(root: &str, token: &str, previous: Option<&str>) -> String {
    let continuation = match previous {
        Some(previous) => format!(
            r#"
  [ "$prior" = '{previous}' ] || exit {BUSY}
  stage=$(mktemp -d "$A.recovery.XXXXXXXX") || exit {BAD_ACCOUNT}
  trap 'rm -f "$stage/reservation"; rmdir "$stage"' EXIT
  ln -s '{token}' "$stage/reservation" || exit {BAD_ACCOUNT}
  case "$(uname -s)" in
    Darwin) mv -fh "$stage/reservation" "$A" ;;
    *) mv -fT "$stage/reservation" "$A" ;;
  esac || exit {BAD_ACCOUNT}
  exit 0
else
  exit {LOST}"#,
        ),
        None => format!(r#"printf '%s\n' "$prior"; exit {RECOVER}"#),
    };
    locked_script(
        root,
        0,
        &format!(
            r#"
if claimed; then
  prior=$(readlink "$A") || exit {BAD_ACCOUNT}
  case "$prior" in guarded-*) ;; *) exit {OLD} ;; esac
  {continuation}
fi
ln -s '{token}' "$A" || exit {BAD_ACCOUNT}"#
        ),
    )
}

pub(crate) fn valid_token(token: &str) -> bool {
    token.strip_prefix("guarded-").is_some_and(|suffix| {
        suffix.len() == 48
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
