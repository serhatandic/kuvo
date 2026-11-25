//! Debug-only resident timer fire counters. A caller records completed timer branches,
//! not timer registration or ordinary events. No periodic reporting task.

macro_rules! timers {
    ($($timer:ident => $label:literal),+ $(,)?) => {
        #[derive(Clone, Copy)]
        #[repr(usize)]
        pub enum Timer { $($timer),+ }
        #[cfg(debug_assertions)]
        const LABELS: &[&str] = &[$($label),+];
    }
}

timers! {
    KeeperDeadline => "keeper.deadline",
    KeeperStopFallback => "keeper.stop-fallback",
    SetupDeadline => "setup.deadline",
    FileWatchGrace => "file-watch.grace",
    AgentProgramCheck => "agents.program-check",
    BackupDaily => "backup.daily",
    BackupPrune => "backup.engine-copy-expiry",
    EngineHealth => "engine.health",
    VaultAutopilot => "vault.autopilot-question",
    WireSse => "wire.agent-sse",
    WireHttp => "wire.http-processing",
    WireLine => "wire.line-heartbeat",
    LineRetention => "wire.line-retention",
    PeerTrust => "peers.trust-expiry",
    WorkBackgroundExpiry => "work.background-expiry",
    AgentAck => "agents.holder-ack",
    AgentTurnDeadline => "agents.turn-deadline",
    AgentTerminalList => "agents.terminal-list",
    AgentTurnRetry => "agents.turn-retry",
    AgentAskReplay => "agents.ask-replay-expiry",
    MoveTransferHeartbeat => "moves.transfer-heartbeat",
    RemoteWorkRows => "remote-work.rows",
    AgentSignInExpiry => "agents.sign-in-expiry",
    CodexServerExpiry => "agents.codex-server-expiry",
    AgentHolderOutput => "agents.holder-output",
    AgentHolderLiveness => "agents.holder-liveness",
    AgentBackground => "agents.background",
    AgentOutputFollow => "agents.output-follow",
    LibraryLegacyCopy => "library.legacy-copy",
    LibraryFollow => "library.follow",
    ArtifactsFollow => "artifacts.follow",
    MarketplaceRecovery => "marketplace.recovery",
    SessionInventoryRecovery => "sessions.inventory-recovery",
    EngineReconnect => "engine.reconnect",
    EngineLifetime => "engine.lifetime",
    SettingsFollow => "settings.follow",
    UpdateDeadline => "updates.deadline",
    ReleaseHolderWait => "updates.holder-wait",
    WakesDue => "wakes.due",
    WakesFollow => "wakes.follow",
    ApprovalGrace => "approval.grace",
    ApprovalRetry => "approval.retry",
    PeerCopyHeartbeat => "peers.copy-heartbeat",
    PeerMoveExpiry => "peers.move-expiry",
    PeerMoveAnnounce => "peers.move-announce",
    PeerWaiting => "peers.waiting",
    RelayReconnect => "relay.reconnect",
    RelayHeartbeat => "relay.heartbeat",
    LettersRetry => "letters.retry",
    WorkQueueExchange => "work-queue.exchange",
    WorkQueueFollow => "work-queue.follow",
    RemoteWorkRound => "remote-work.round",
    MoveRound => "moves.round",
    DeliveryRecovery => "delivery.recovery",
    DeliveryFollow => "delivery.follow",
    BotsHeld => "bots.held",
    BotsFollow => "bots.follow",
    WorkFollow => "work.follow",
    WorkAtWorkFollow => "work.at-work-follow",
    WorkKeepAwake => "work.keep-awake",
    WorkAsksFollow => "work.asks-follow",
    EngineQueueFollow => "engine.queue-follow",
    EngineWaitingFollow => "engine.waiting-follow",
    EngineAsksFollow => "engine.asks-follow",
    GitFollow => "git.follow",
    VaultSshAgents => "vault.ssh-agents",
    SkillsDaily => "skills.daily",
    UsagePeerHeartbeat => "usage.peer-deadline",
    UsageHistoryDeadline => "usage.history-peer-deadline",
    UsageClientDemand => "usage.client-demand",
    UsageResetDeadline => "usage.reset-deadline",
    UsageInsightsRefresh => "usage.insights-refresh",
    UsagePriceFeed => "usage.price-feed",
    UsageLimitHeartbeat => "usage.limit-heartbeat",
    UsageDemand => "usage.demand",
    NumberForwardWait => "work-queue.forward-wait",
    BackupRelayRetry => "backup.relay-retry",
    InstallerJoinRetry => "setup.installer-join-retry",
}

#[cfg(debug_assertions)]
static FIRES: [std::sync::atomic::AtomicU64; LABELS.len()] =
    [const { std::sync::atomic::AtomicU64::new(0) }; LABELS.len()];

#[inline(always)]
pub fn fired(timer: Timer) {
    #[cfg(debug_assertions)]
    FIRES[timer as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(debug_assertions))]
    let _ = timer;
}

#[cfg(debug_assertions)]
pub fn snapshot() -> std::collections::BTreeMap<&'static str, u64> {
    use std::sync::atomic::Ordering;
    LABELS
        .iter()
        .zip(&FIRES)
        .map(|(&name, counter)| (name, counter.load(Ordering::Relaxed)))
        .collect()
}

// Use counted intervals in resident-started loops and actual timer branches.
// In release an interval is exactly Tokio's interval, not a wrapper/counter.
#[cfg(debug_assertions)]
pub struct Interval {
    inner: tokio::time::Interval,
    timer: Timer,
}
#[cfg(not(debug_assertions))]
pub type Interval = tokio::time::Interval;

#[cfg(debug_assertions)]
impl Interval {
    pub async fn tick(&mut self) -> tokio::time::Instant {
        let at = self.inner.tick().await;
        fired(self.timer);
        at
    }
    pub fn set_missed_tick_behavior(&mut self, behavior: tokio::time::MissedTickBehavior) {
        self.inner.set_missed_tick_behavior(behavior);
    }
    pub fn reset_immediately(&mut self) {
        self.inner.reset_immediately();
    }
}
#[inline]
pub fn interval(timer: Timer, period: std::time::Duration) -> Interval {
    let inner = tokio::time::interval(period);
    #[cfg(debug_assertions)]
    {
        Interval { inner, timer }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = timer;
        inner
    }
}
#[inline]
pub fn interval_at(
    timer: Timer,
    start: tokio::time::Instant,
    period: std::time::Duration,
) -> Interval {
    let inner = tokio::time::interval_at(start, period);
    #[cfg(debug_assertions)]
    {
        Interval { inner, timer }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = timer;
        inner
    }
}
#[cfg(debug_assertions)]
pub fn sleep(timer: Timer, duration: std::time::Duration) -> impl std::future::Future<Output = ()> {
    let inner = tokio::time::sleep(duration);
    async move {
        inner.await;
        fired(timer);
    }
}
#[cfg(not(debug_assertions))]
#[inline(always)]
pub fn sleep(_: Timer, duration: std::time::Duration) -> tokio::time::Sleep {
    tokio::time::sleep(duration)
}
#[cfg(debug_assertions)]
pub fn sleep_until(
    timer: Timer,
    deadline: tokio::time::Instant,
) -> impl std::future::Future<Output = ()> {
    let inner = tokio::time::sleep_until(deadline);
    async move {
        inner.await;
        fired(timer);
    }
}
#[cfg(not(debug_assertions))]
#[inline(always)]
pub fn sleep_until(_: Timer, deadline: tokio::time::Instant) -> tokio::time::Sleep {
    tokio::time::sleep_until(deadline)
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn only_completed_timer_futures_count() {
        let before = snapshot()["wire.http-processing"];
        let mut tick = interval(Timer::WireHttp, Duration::from_secs(1));
        tick.tick().await;
        tokio::select! {
            biased;
            _ = std::future::ready(()) => {},
            _ = sleep(Timer::WireHttp, Duration::from_secs(1)) => {},
        }
        assert_eq!(snapshot()["wire.http-processing"], before + 1);
        sleep(Timer::WireHttp, Duration::from_secs(1)).await;
        tick.tick().await;
        assert_eq!(snapshot()["wire.http-processing"], before + 3);
    }
}
