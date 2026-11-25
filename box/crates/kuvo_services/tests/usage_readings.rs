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
