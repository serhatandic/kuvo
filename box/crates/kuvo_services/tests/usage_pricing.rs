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

#[test]
fn the_public_price_catalog_matches_the_usagekit_fixture_aliases_and_rates() {
    let pricing = Pricing::load(None).unwrap();
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../test/fixtures/usage/pricing-expected.json"
    ))
    .unwrap();
    for (model, want) in corpus.as_object().unwrap() {
        let found = pricing.resolve(model);
        if want.is_null() {
            assert!(found.is_none(), "{model}");
            continue;
        }
        let found = found.unwrap_or_else(|| panic!("Missing price for {model}"));
        let got = serde_json::json!({"input":found.input,"output":found.output,"cacheWrite":found.cache_write,"cacheRead":found.cache_read,"inputAbove":found.input_above,"outputAbove":found.output_above,"cacheWriteAbove":found.cache_write_above,"cacheReadAbove":found.cache_read_above,"fast":found.fast});
        for (field, expected) in want.as_object().unwrap() {
            let actual = &got[field];
            if let Some(expected) = expected.as_f64() {
                let actual = actual.as_f64().unwrap_or_else(|| panic!("{model} {field}"));
                assert_eq!(
                    actual.to_bits(),
                    expected.to_bits(),
                    "{model} {field}: {actual} != {expected}"
                );
            } else {
                assert_eq!(actual, expected, "{model} {field}");
            }
        }
    }
}
