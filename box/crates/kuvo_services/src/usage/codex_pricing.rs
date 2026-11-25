//! Codex prices its native token buckets by the maker's tier and historical model.
use super::{
    native_logs::Counts,
    pricing::{Pricing, Rates},
};
const FREE_SINCE: i64 = 1791244800000;
const REVIEW: [(i64, &str); 8] = [
    (1783555200000, "gpt-5.6-luna"),
    (1776902400000, "gpt-5.5"),
    (1772668800000, "gpt-5.4"),
    (1770249600000, "gpt-5.3-codex"),
    (1765411200000, "gpt-5.2-codex"),
    (1762992000000, "gpt-5.1-codex"),
    (1757894400000, "gpt-5-codex"),
    (1754524800000, "gpt-5"),
];
fn date_base(model: &str) -> &str {
    for size in [11, 9] {
        if model.len() <= size {
            continue;
        }
        let Some(suffix) = model.get(model.len() - size..) else {
            continue;
        };
        let b = suffix.as_bytes();
        let date = if size == 11 {
            b[0] == b'-'
                && b[5] == b'-'
                && b[8] == b'-'
                && b.iter()
                    .enumerate()
                    .all(|(i, n)| matches!(i, 0 | 5 | 8) || n.is_ascii_digit())
        } else {
            b[0] == b'-' && b[1..].iter().all(u8::is_ascii_digit)
        };
        if date {
            return &model[..model.len() - size];
        }
    }
    model
}
fn long(base: &str) -> Option<(f64, f64, f64)> {
    Some(match base {
        "gpt-5.4" => (5., 22.5, 0.5),
        "gpt-5.4-pro" => (60., 270., 60.),
        "gpt-5.5" => (10., 45., 1.),
        "gpt-5.5-pro" => (60., 270., 60.),
        "gpt-5.6-sol" => (8., 30., 0.8),
        "gpt-5.6-terra" => (4., 18., 0.4),
        "gpt-5.6-luna" => (0.4, 1.8, 0.04),
        "gpt-6-astra" => (20., 75., 2.),
        "gpt-6.1-sol" => (4., 15., 0.2),
        "gpt-6-sol" => (4., 15., 0.4),
        "gpt-6-luna" => (0.2, 0.75, 0.02),
        _ => return None,
    })
}
fn cost(rates: Rates, model: &str, counts: &Counts, fast: bool, ultrafast: bool) -> f64 {
    let base = date_base(model);
    let total = counts
        .input
        .saturating_add(counts.cache_read)
        .saturating_add(counts.cache_write_5m)
        .saturating_add(counts.cache_write_1h);
    let above = total
        > if long(base).is_some() {
            272_000
        } else {
            200_000
        };
    let (input, output, mut cached) = if let Some((i, o, c)) = long(base) {
        if total > 272_000 {
            (i, o, c)
        } else {
            (rates.input, rates.output, rates.cache_read)
        }
    } else if total > 200_000 {
        (
            rates.input_above.unwrap_or(rates.input),
            rates.output_above.unwrap_or(rates.output),
            rates.cache_read_above.unwrap_or(rates.cache_read),
        )
    } else {
        (rates.input, rates.output, rates.cache_read)
    };
    if matches!(base, "gpt-5.4-pro" | "gpt-5.5-pro") || !rates.cache_read_explicit {
        cached = input;
    }
    let multiplier = if ultrafast && base == "gpt-6-astra" {
        6.
    } else if fast || ultrafast {
        if matches!(base, "gpt-5.5" | "gpt-5.5-pro") {
            2.5
        } else if long(base).is_some() {
            2.
        } else if rates.fast == 1. {
            2.
        } else {
            rates.fast
        }
    } else {
        1.
    };
    let creation = if above {
        rates.cache_write_above.unwrap_or(rates.cache_write)
    } else {
        rates.cache_write
    };
    ((counts.input as f64 * input) / 1e6
        + (counts.output as f64 * output) / 1e6
        + (counts.cache_write_5m as f64 * creation) / 1e6
        + (counts.cache_write_1h as f64 * input * 2.0) / 1e6
        + (counts.cache_read as f64 * cached) / 1e6)
        * multiplier
}
pub struct Price {
    pub cost: Option<f64>,
    pub unknown: bool,
    pub fallback: Option<String>,
}
pub fn price(
    pricing: &Pricing,
    model: &str,
    at: i64,
    counts: &Counts,
    fallback: Option<&str>,
) -> Price {
    if model == "codex-auto-review" && at >= FREE_SINCE {
        return Price {
            cost: Some(0.),
            unknown: false,
            fallback: None,
        };
    }
    let model = if model == "codex-auto-review" {
        REVIEW
            .iter()
            .find(|(day, _)| at >= *day)
            .map(|(_, name)| *name)
            .unwrap_or("gpt-5")
    } else if model == "gpt-reserve" {
        "gpt-5.6-luna"
    } else {
        model
    };
    let canonical = pricing.canonical_name(model);
    let (base, alias) = canonical
        .strip_suffix("-fast")
        .map_or((canonical, false), |base| (base, true));
    let rates = pricing.resolve(base);
    let base_rates = rates.is_some();
    if let Some(rates) = rates.or_else(|| pricing.resolve(model)) {
        return Price {
            cost: Some(cost(
                rates,
                base,
                counts,
                if alias { base_rates } else { counts.fast },
                counts.ultrafast && (!alias || base_rates),
            )),
            unknown: false,
            fallback: None,
        };
    }
    let estimated = fallback.and_then(|name| {
        pricing
            .fallback_rates(name, "codex")
            .map(|r| (name, r.clone()))
    });
    Price {
        cost: estimated.as_ref().map(|(name, r)| {
            cost(
                r.clone(),
                name,
                counts,
                alias || counts.fast,
                counts.ultrafast,
            )
        }),
        unknown: counts.total.unwrap_or(counts.processed()) > 0,
        fallback: estimated.map(|(name, _)| name.into()),
    }
}
