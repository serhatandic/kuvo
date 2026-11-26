// Accepted readings are the source for both rings and warnings. No provider or wall clock.
use kuvo_services::usage::readings::{Milestone, Source, Trail, Window};
use std::collections::BTreeSet;
const MIN: u64 = 60_000;
const START: u64 = 1_000_000_000;
const RESET: u64 = START + 300 * MIN;
fn reading(at: u64, used: f64, reset: u64) -> Window {
    Window {
        id: "primary".into(),
        used,
        minutes: Some(300),
        resets_at: Some(reset),
        model: None,
        name: None,
        amount: None,
        read: at,
        source: Source::Usage,
    }
}
fn toggles() -> BTreeSet<Milestone> {
    [
        Milestone::UnderTenPercent,
        Milestone::HealthyToClose,
        Milestone::CloseToRunningOut,
    ]
    .into()
}
#[test]
fn a_new_read_of_an_old_window_and_a_single_fake_refill_do_not_replace_the_reading() {
    let mut trail = Trail::new(reading(START + 120 * MIN, 0.8, RESET)).unwrap();
    assert_eq!(
        trail.observe(
            reading(START + 121 * MIN, 0.99, RESET - 60 * MIN),
            &toggles()
        ),
        None
    );
    assert_eq!(
        trail.observe(
            reading(START + 122 * MIN, 0.0, RESET + 122 * MIN),
            &toggles()
        ),
        None
    );
    assert_eq!(trail.current.used, 0.8);
    assert_eq!(trail.current.resets_at, Some(RESET));
    assert_eq!(
        trail.observe(reading(START + 123 * MIN, 0.81, RESET), &toggles()),
        Some(vec![])
    );
    assert_eq!(trail.current.used, 0.81);
}
#[test]
fn noisy_readings_do_not_rearm_the_warning_in_the_same_window() {
    let mut trail = Trail::new(reading(START + 160 * MIN, 0.88, RESET)).unwrap();
    assert_eq!(
        trail.observe(reading(START + 161 * MIN, 0.92, RESET), &toggles()),
        Some(vec![Milestone::UnderTenPercent])
    );
    assert_eq!(
        trail.observe(reading(START + 162 * MIN, 0.90, RESET + MIN), &toggles()),
        Some(vec![])
    );
    assert_eq!(
        trail.observe(reading(START + 163 * MIN, 0.93, RESET + MIN), &toggles()),
        Some(vec![])
    );
    assert_eq!(trail.current.resets_at, Some(RESET));
}
#[test]
fn new_window_has_no_projection_from_old_samples() {
    let mut trail = Trail::new(reading(START + 240 * MIN, 0.7, RESET)).unwrap();
    trail.observe(reading(START + 250 * MIN, 0.8, RESET), &toggles());
    trail.observe(reading(START + 260 * MIN, 0.9, RESET), &toggles());
    assert!(trail.runs_out());
    assert_eq!(
        trail.observe(reading(RESET + MIN, 0.01, RESET + 300 * MIN), &toggles()),
        Some(vec![])
    );
    assert!(!trail.projected());
    assert_eq!(trail.current.used, 0.01);
}
#[test]
fn a_corroborated_recovery_rearms_a_real_crossing_without_accepting_old_peer_state() {
    let mut trail = Trail::new(reading(START + 160 * MIN, 0.88, RESET)).unwrap();
    let milestone = Milestone::UnderTenPercent;
    assert_eq!(
        trail.observe(reading(START + 161 * MIN, 0.92, RESET), &toggles()),
        Some(vec![milestone])
    );
    let old = trail.clone();
    assert_eq!(trail.occurrence(milestone), 1);
    assert_eq!(
        trail.observe(reading(START + 162 * MIN, 0.84, RESET), &toggles()),
        None
    );
    assert_eq!(
        trail.observe(reading(START + 163 * MIN, 0.84, RESET), &toggles()),
        Some(vec![])
    );
    trail.merge_fired(&old);
    assert_eq!(
        trail.observe(reading(START + 164 * MIN, 0.91, RESET), &toggles()),
        Some(vec![milestone])
    );
    assert_eq!(trail.occurrence(milestone), 2);
}
#[test]
fn an_off_toggle_does_not_consume_an_unannounced_crossing() {
    let mut trail = Trail::new(reading(START + 160 * MIN, 0.89, RESET)).unwrap();
    assert_eq!(
        trail.observe(reading(START + 161 * MIN, 0.91, RESET), &BTreeSet::new()),
        Some(vec![])
    );
    assert_eq!(
        trail.observe(reading(START + 162 * MIN, 0.92, RESET), &toggles()),
        Some(vec![Milestone::UnderTenPercent])
    );
}

#[test]
fn projected_crossings_only_follow_accepted_samples_in_the_same_window() {
    let mut trail = Trail::new(reading(START + 120 * MIN, 0.1, RESET)).unwrap();
    let all = toggles();
    for (minute, used) in [(130, 0.12), (140, 0.14)] {
        assert_eq!(
            trail.observe(reading(START + minute * MIN, used, RESET), &all),
            Some(vec![])
        );
    }
    assert_eq!(
        trail.observe(reading(START + 160 * MIN, 0.285, RESET), &all),
        Some(vec![Milestone::HealthyToClose])
    );
    assert_eq!(
        trail.observe(reading(START + 170 * MIN, 0.4, RESET), &all),
        Some(vec![Milestone::CloseToRunningOut])
    );
    assert_eq!(
        trail.observe(reading(START + 180 * MIN, 0.5, RESET), &all),
        Some(vec![])
    );
    assert_eq!(
        trail.observe(reading(START + 181 * MIN, 0.99, RESET - 60 * MIN), &all),
        None
    );
    assert_eq!(trail.occurrence(Milestone::HealthyToClose), 1);
    assert_eq!(trail.occurrence(Milestone::CloseToRunningOut), 1);
}

#[test]
fn hidden_metrics_reprime_after_restart_but_keep_occurrence_ids() {
    let mut trail = Trail::new(reading(START + 160 * MIN, 0.89, RESET)).unwrap();
    let all = toggles();
    let almost = Milestone::UnderTenPercent;
    assert_eq!(
        trail.observe_visible(reading(START + 161 * MIN, 0.91, RESET), Some(&all)),
        Some(vec![almost])
    );
    // Two consistent recoveries while hidden, then the first visible reading
    // is a baseline rather than a replay of the old warning.
    trail.observe_visible(reading(START + 162 * MIN, 0.82, RESET), None);
    trail.observe_visible(reading(START + 163 * MIN, 0.82, RESET), None);
    assert!(!trail.notice_state().primed);
    let mut trail: Trail = serde_json::from_slice(&serde_json::to_vec(&trail).unwrap()).unwrap();
    assert_eq!(
        trail.observe_visible(reading(START + 164 * MIN, 0.82, RESET), Some(&all)),
        Some(vec![])
    );
    assert_eq!(trail.occurrence(almost), 1);
    assert_eq!(
        trail.observe_visible(reading(START + 165 * MIN, 0.92, RESET), Some(&all)),
        Some(vec![almost])
    );
    assert_eq!(trail.occurrence(almost), 2);
}

#[test]
fn cold_start_and_unknown_cadence_do_not_invent_a_projection() {
    let all = toggles();
    let mut first = reading(START + 160 * MIN, 0.95, RESET);
    first.minutes = None;
    first.resets_at = None;
    let mut trail = Trail::new(first.clone()).unwrap();
    assert_eq!(trail.occurrence(Milestone::UnderTenPercent), 0);
    first.read += MIN;
    first.used = 0.98;
    assert_eq!(trail.observe(first, &all), Some(vec![]));
    assert!(!trail.projected());
    assert!(!trail.runs_out());
}
