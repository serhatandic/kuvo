//! Scheduling and the small memory floor for new background work. No idle polling:
//! callers wait only when they have a background start to make.

#[cfg(unix)]
use std::io;
use std::time::Duration;

#[cfg(unix)]
pub(super) fn lower_child() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let target = 20;
    #[cfg(target_os = "linux")]
    let target = 15;
    // SAFETY: these calls act on the calling child only. Never attempt to undo
    // a lower priority inherited from its parent (which needs privilege).
    unsafe {
        if libc::getpriority(libc::PRIO_PROCESS, 0) < target
            && libc::setpriority(libc::PRIO_PROCESS, 0, target) != 0
        {
            return Err(io::Error::last_os_error());
        }
        #[cfg(target_os = "linux")]
        {
            // IOPRIO_WHO_PROCESS = 1, IOPRIO_CLASS_IDLE = 3 (class starts at bit 13).
            // Some kernel/filesystem combinations do not allow I/O priority changes.
            let _ = libc::syscall(libc::SYS_ioprio_set, 1, 0, 3 << 13);
        }
    }
    Ok(())
}

/// Wait before starting another agent or heavy command; never stop existing
/// work. Call before starting the process's own handshake or execution timeout.
pub async fn wait_for_background_memory() {
    wait_while_critical(critical).await;
}

/// A start being held for memory can be withdrawn without waiting for pressure
/// to clear. Once admitted, the usual process owner handles its lifetime.
pub async fn wait_for_background_memory_or_cancel(cancelled: impl Future<Output = ()>) -> bool {
    wait_while_critical_or_cancel(critical, cancelled).await
}

async fn wait_while_critical_or_cancel(
    critical: impl FnMut() -> bool,
    cancelled: impl Future<Output = ()>,
) -> bool {
    tokio::select! {
        biased;
        () = cancelled => false,
        () = wait_while_critical(critical) => true,
    }
}

async fn wait_while_critical(mut critical: impl FnMut() -> bool) {
    while critical() {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(target_os = "macos")]
fn critical() -> bool {
    let mut pressure = 0_i32;
    let mut size = size_of_val(&pressure);
    // SAFETY: the sysctl writes an integer into a buffer of its exact advertised size.
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&mut pressure as *mut i32).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    result == 0 && size == size_of_val(&pressure) && pressure >= 4
}

#[cfg(target_os = "linux")]
fn critical() -> bool {
    // Linux has no system-wide normal/warning/critical enum. MemAvailable is
    // the kernel's estimate after reclaimable caches: leave a small 64 MiB floor.
    std::fs::read_to_string("/proc/meminfo").is_ok_and(|info| {
        info.lines()
            .find_map(|line| {
                line.strip_prefix("MemAvailable:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .is_some_and(|kb| kb < 64 * 1024)
    })
}

#[cfg(windows)]
fn critical() -> bool {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Memory::{
        CreateMemoryResourceNotification, LowMemoryResourceNotification,
        QueryMemoryResourceNotification,
    };
    // SAFETY: the returned handle is newly owned, or null on error.
    let handle = unsafe { CreateMemoryResourceNotification(LowMemoryResourceNotification) };
    if handle.is_null() {
        return false;
    }
    // SAFETY: take sole ownership of the notification handle.
    let _owner = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut low = 0;
    // SAFETY: the handle is alive and low is a writable BOOL.
    (unsafe { QueryMemoryResourceNotification(handle, &mut low) }) != 0 && low != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test(start_paused = true)]
    async fn background_start_waits_until_critical_pressure_clears() {
        let pressure = Arc::new(AtomicBool::new(true));
        let current = pressure.clone();
        let start = tokio::spawn(async move {
            wait_while_critical(|| current.load(Ordering::SeqCst)).await;
            // A real process, not a fake memory or launch result.
            let mut spawn = super::super::Spawn::new(
                std::env::current_exe().unwrap(),
                crate::platform::Environment::new(),
            );
            spawn.args.push("--list".into());
            spawn.priority = super::super::Priority::Background;
            let mut child = spawn.spawn().unwrap();
            child.wait().unwrap()
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(!start.is_finished(), "started under critical pressure");
        pressure.store(false, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), start)
                .await
                .expect("start did not resume after pressure cleared")
                .unwrap()
                .success()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_a_background_start_does_not_wait_for_memory_or_spawn() {
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let start = tokio::spawn(async move {
            wait_while_critical_or_cancel(|| true, async {
                let _ = cancelled.await;
            })
            .await
        });
        tokio::task::yield_now().await;
        assert!(!start.is_finished());
        let at = tokio::time::Instant::now();
        cancel.send(()).unwrap();
        assert!(
            !tokio::time::timeout(Duration::from_millis(1), start)
                .await
                .expect("start remained blocked by memory after cancellation")
                .unwrap(),
            "a cancelled start was admitted"
        );
        assert_eq!(
            tokio::time::Instant::now(),
            at,
            "cancellation waited for memory"
        );
    }
}
