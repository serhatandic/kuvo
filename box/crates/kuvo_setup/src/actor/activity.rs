use std::time::Duration;

use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

use crate::storage::Files;

const MARKER: &str = "run/link.seen";

#[derive(Clone, Copy)]
pub(super) enum Activity {
    Seen,
    Forget,
}

pub(super) type Sender = mpsc::Sender<(Activity, oneshot::Sender<()>)>;

pub(super) async fn record(sender: &Sender, activity: Activity) {
    let (reply, done) = oneshot::channel();
    if sender.send((activity, reply)).await.is_ok() {
        let _ = done.await;
    }
}

pub(super) async fn track(
    files: Files,
    mut requests: mpsc::Receiver<(Activity, oneshot::Sender<()>)>,
) {
    let mut last = None;
    while let Some((activity, done)) = requests.recv().await {
        let now = Instant::now();
        let result = match activity {
            Activity::Seen
                if last.is_none_or(|time| now.duration_since(time) >= Duration::from_secs(300)) =>
            {
                last = Some(now);
                files.write_bytes(MARKER, b"")
            }
            Activity::Seen => Ok(()),
            Activity::Forget => {
                last = None;
                files.remove(MARKER)
            }
        };
        if let Err(error) = result {
            eprintln!("recording phone activity: {error}");
        }
        let _ = done.send(());
    }
}
