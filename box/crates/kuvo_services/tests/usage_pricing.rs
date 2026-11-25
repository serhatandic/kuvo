// Public pricing snapshots: no network. Unknown models must not be given fabricated prices.
use kuvo_services::usage::pricing::{Pricing, Tokens};
#[test]
fn fallback_options_require_a_real_exact_price() {
    let pricing = Pricing::load(None).unwrap();
    assert!(pricing.fallback_rates("gpt-5.4", "codex").is_some());
    assert!(
        pricing
            .fallback_rates("this-is-not-a-model", "codex")
            .is_none()
    );
    assert!(pricing.resolve("this-is-not-a-model").is_none());
}
#[test]
fn cache_writes_and_fast_rates_count_differently() {
    let pricing = Pricing::load(None).unwrap();
    let rates = pricing.resolve("claude-opus-4-6").unwrap();
    let base = rates.cost(
        &Tokens {
            input: 1000,
            ..Default::default()
        },
        true,
    );
    let hour = rates.cost(
        &Tokens {
            cache_write_1h: 1000,
            ..Default::default()
        },
        true,
    );
    assert!((hour - base * 2.0).abs() < 1e-9);
    let fast = rates.cost(
        &Tokens {
            input: 1000,
            fast: true,
            ..Default::default()
        },
        true,
    );
    assert!((fast - base * rates.fast).abs() < 1e-9);
}
