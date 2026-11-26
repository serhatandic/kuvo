//! Test-only signal snapshot of this process's timer fire counts.
//! Whole module gated `#[cfg(all(unix, debug_assertions))]` in kuvo_store/lib.rs.
use std::{ffi::OsStr, io, path::Path};
use tokio::{
    signal::unix::{SignalKind, signal},
    task::JoinHandle,
};

pub struct Report(JoinHandle<()>);
impl Drop for Report {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn start(home: &Path) -> io::Result<Option<Report>> {
    if std::env::var_os("KUVO_TEST_TIMER_REPORT").as_deref() != Some(OsStr::new("1")) {
        return Ok(None);
    }
    let mut requested = signal(SignalKind::user_defined1())?;
    let folder = home.join("run/timer-fires");
    crate::files::create_private_dirs(&folder)?;
    let pid = std::process::id();
    let file = folder.join(format!("{pid}.json"));
    Ok(Some(Report(tokio::spawn(async move {
        let mut sequence = 0u64;
        while requested.recv().await.is_some() {
            sequence = sequence.wrapping_add(1);
            let sample = serde_json::json!({"pid":pid,"sequence":sequence,"timers":kuvo_os::timer_fires::snapshot()});
            if let Err(error) = crate::files::write_secret(
                &file,
                &serde_json::to_vec(&sample).expect("timer snapshot serializes"),
            ) {
                eprintln!("Writing debug timer snapshot: {error}");
            }
        }
    }))))
}
