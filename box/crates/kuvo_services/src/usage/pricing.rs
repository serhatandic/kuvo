//! Model price resolution against the shipped catalogs and the last fetched copies.
//! Unknown models stay unknown, never zero-cost estimates.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rates {
    #[serde(rename = "i")]
    pub input: f64,
    #[serde(rename = "o")]
    pub output: f64,
    #[serde(rename = "cw")]
    pub cache_write: f64,
    #[serde(rename = "cr")]
    pub cache_read: f64,
    #[serde(rename = "ia")]
    pub input_above: Option<f64>,
    #[serde(rename = "oa")]
    pub output_above: Option<f64>,
    #[serde(rename = "cwa")]
    pub cache_write_above: Option<f64>,
    #[serde(rename = "cra")]
    pub cache_read_above: Option<f64>,
    #[serde(rename = "cre", default = "yes")]
    pub cache_read_explicit: bool,
    #[serde(default = "one")]
    pub fast: f64,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    #[serde(default)]
    pub fast: bool,
}
impl Rates {
    pub fn cost(&self, tokens: &Tokens, long_context: bool) -> f64 {
        let prompt = tokens
            .input
            .saturating_add(tokens.cache_write_5m)
            .saturating_add(tokens.cache_write_1h)
            .saturating_add(tokens.cache_read);
        let above = long_context && prompt > 200_000;
        let pick = |base: f64, tier: Option<f64>| if above { tier.unwrap_or(base) } else { base };
        let input = pick(self.input, self.input_above);
        let total = tokens.input as f64 * input
            + tokens.output as f64 * pick(self.output, self.output_above)
            + tokens.cache_write_5m as f64 * pick(self.cache_write, self.cache_write_above)
            + tokens.cache_write_1h as f64 * input * 2.0
            + tokens.cache_read as f64 * pick(self.cache_read, self.cache_read_above);
        total / 1_000_000.0 * if tokens.fast { self.fast } else { 1.0 }
    }
    fn scaled(mut self, factor: f64) -> Self {
        self.input *= factor;
        self.output *= factor;
        self.cache_write *= factor;
        self.cache_read *= factor;
        for value in [
            &mut self.input_above,
            &mut self.output_above,
            &mut self.cache_write_above,
            &mut self.cache_read_above,
        ]
        .into_iter()
        .flatten()
        {
            *value *= factor;
        }
        self.fast = 1.0;
        self
    }
}
#[derive(Deserialize)]
struct Compact {
    models: BTreeMap<String, Rates>,
}
#[derive(Deserialize)]
struct OwnRates {
    input_per_million: f64,
    output_per_million: f64,
    cache_write_per_million: Option<f64>,
    cache_read_per_million: Option<f64>,
}
#[derive(Deserialize)]
struct Alias {
    pattern: String,
    canonical: String,
}
#[derive(Deserialize)]
struct SupplementFile {
    pricing: BTreeMap<String, OwnRates>,
    alias_rules: Vec<Alias>,
    #[serde(default)]
    fast_multipliers: BTreeMap<String, f64>,
    #[serde(default)]
    fallback_models: BTreeMap<String, Vec<String>>,
    updated_at: Option<String>,
}
pub(super) fn validate_supplement(value: &serde_json::Value) -> Result<(), String> {
    let supplement: SupplementFile =
        serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    let valid = |n: f64| n.is_finite() && n >= 0.;
    if !supplement.pricing.values().all(|r| {
        valid(r.input_per_million)
            && valid(r.output_per_million)
            && r.cache_read_per_million.is_none_or(valid)
            && r.cache_write_per_million.is_none_or(valid)
    }) || !supplement.fast_multipliers.values().copied().all(valid)
    {
        return Err("Invalid price supplement rates".into());
    }
    Ok(())
}
pub struct Pricing {
    own: BTreeMap<String, Rates>,
    primary: BTreeMap<String, Rates>,
    secondary: BTreeMap<String, Rates>,
    aliases: Vec<(regex::Regex, String)>,
    fallback: BTreeMap<String, Vec<String>>,
    multipliers: BTreeMap<String, f64>,
}
const LITELLM: &str = include_str!("../../../../src/usage/pricing/pricing_litellm_snapshot.json");
const MODELS: &str = include_str!("../../../../src/usage/pricing/pricing_models_dev_snapshot.json");
const SUPPLEMENT: &str = include_str!("../../../../src/usage/pricing/pricing_supplement.json");

fn cached<T: serde::de::DeserializeOwned>(folder: Option<&Path>, name: &str) -> Option<T> {
    serde_json::from_slice(&std::fs::read(folder?.join(name)).ok()?).ok()
}
impl Pricing {
    pub fn load(folder: Option<&Path>) -> Result<Self, serde_json::Error> {
        let mut primary: Compact = serde_json::from_str(LITELLM)?;
        let mut secondary: Compact = serde_json::from_str(MODELS)?;
        if let Some(other) = cached::<Compact>(folder, "litellm.json") {
            primary.models.extend(other.models);
        }
        if let Some(other) = cached::<Compact>(folder, "models_dev.json") {
            secondary.models.extend(other.models);
        }
        let bundled: SupplementFile = serde_json::from_str(SUPPLEMENT)?;
        let supplement = cached::<SupplementFile>(folder, "supplement.json")
            .filter(|other| other.updated_at >= bundled.updated_at);
        let mut fallback = bundled.fallback_models.clone();
        let supplement = supplement.unwrap_or(bundled);
        fallback.extend(supplement.fallback_models);
        let own = supplement
            .pricing
            .into_iter()
            .map(|(model, r)| {
                let input = r.input_per_million;
                let rates = Rates {
                    input,
                    output: r.output_per_million,
                    cache_write: r.cache_write_per_million.unwrap_or(input),
                    cache_read: r.cache_read_per_million.unwrap_or(input * 0.1),
                    input_above: None,
                    output_above: None,
                    cache_write_above: None,
                    cache_read_above: None,
                    cache_read_explicit: r.cache_read_per_million.is_some(),
                    fast: supplement
                        .fast_multipliers
                        .get(&model)
                        .copied()
                        .unwrap_or(1.0),
                };
                (model, rates)
            })
            .collect();
        let aliases = supplement
            .alias_rules
            .into_iter()
            .filter_map(|a| Some((regex::Regex::new(&a.pattern).ok()?, a.canonical)))
            .collect();
        Ok(Self {
            own,
            primary: primary.models,
            secondary: secondary.models,
            aliases,
            fallback,
            multipliers: supplement.fast_multipliers,
        })
    }
    fn exact(&self, name: &str) -> Option<&Rates> {
        self.own
            .get(name)
            .or_else(|| self.primary.get(name))
            .or_else(|| self.secondary.get(name))
    }
    pub fn fallback_choices(&self, provider: &str) -> Vec<String> {
        self.fallback
            .get(provider)
            .into_iter()
            .flatten()
            .filter(|name| self.fallback_rates(name, provider).is_some())
            .cloned()
            .collect()
    }
    pub fn fallback_rates(&self, model: &str, provider: &str) -> Option<&Rates> {
        if !self.fallback.get(provider)?.iter().any(|m| m == model) {
            return None;
        }
        self.exact(model).filter(|r| {
            r.input > 0.0
                && r.output > 0.0
                && r.cache_read >= 0.0
                && r.input.is_finite()
                && r.output.is_finite()
                && r.cache_read.is_finite()
        })
    }
    pub fn canonical_name<'a>(&'a self, model: &'a str) -> &'a str {
        self.aliases
            .iter()
            .find(|(pattern, _)| pattern.is_match(model))
            .map_or(model, |(_, canonical)| canonical.as_str())
    }
    pub fn resolve(&self, model: &str) -> Option<Rates> {
        self.lookup(self.canonical_name(model))
            .or_else(|| self.lookup(model))
    }
    fn fuzzy(&self, model: &str) -> Option<(&str, &Rates)> {
        self.primary
            .iter()
            .filter(|(key, _)| matches_key(key, model))
            .max_by(|(a, _), (b, _)| a.len().cmp(&b.len()).then(b.cmp(a)))
            .map(|(k, v)| (k.as_str(), v))
    }
    fn multiplier(&self, model: &str) -> Option<f64> {
        self.multipliers.get(model).copied().or_else(|| {
            let model = normalize(model);
            for part in model.split(['/', ':']) {
                for (base, multiplier) in &self.multipliers {
                    let key = normalize(base);
                    if let Some(at) = part.rfind(&key) {
                        let rest = &part[at + key.len()..];
                        if rest.is_empty() || rest.starts_with('-') {
                            return Some(*multiplier);
                        }
                    }
                }
            }
            None
        })
    }
    fn lookup(&self, name: &str) -> Option<Rates> {
        if let Some(rate) = self.own.get(name).or_else(|| self.primary.get(name)) {
            return Some(rate.clone());
        }
        if let Some(base) = name.strip_suffix("-fast") {
            if let Some((key, rate)) = self
                .own
                .get(base)
                .or_else(|| self.primary.get(base))
                .map(|r| (base, r))
                .or_else(|| self.fuzzy(base))
                .or_else(|| self.secondary.get(base).map(|r| (base, r)))
            {
                let mult = if rate.fast != 1.0 {
                    Some(rate.fast)
                } else {
                    self.multiplier(key).or_else(|| self.multiplier(base))
                };
                if let Some(mult) = mult {
                    return Some(rate.clone().scaled(mult));
                }
            }
        } else if let Some((_, rate)) = self.fuzzy(name) {
            return Some(rate.clone());
        }
        self.secondary.get(name).cloned()
    }
}
fn normalize(model: &str) -> String {
    model.replace(['.', '@'], "-")
}
fn contains_key(string: &str, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    string.match_indices(key).any(|(start, _)| {
        if start > 0 && string.as_bytes()[start - 1].is_ascii_alphanumeric() {
            return false;
        }
        let suffix = &string.as_bytes()[start + key.len()..];
        let Some(first) = suffix.first() else {
            return true;
        };
        if first.is_ascii_alphanumeric() {
            return false;
        }
        if !key.as_bytes().last().is_some_and(u8::is_ascii_digit) || !matches!(first, b'-' | b'.') {
            return true;
        }
        let digits = suffix[1..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        digits == 0
            || digits == 8
                && suffix
                    .get(1 + digits)
                    .is_none_or(|b| !b.is_ascii_alphanumeric())
    })
}
fn matches_key(a: &str, b: &str) -> bool {
    contains_key(a, b)
        || contains_key(b, a)
        || contains_key(&normalize(a), &normalize(b))
        || contains_key(&normalize(b), &normalize(a))
}
