//! Session cost resolution.
//!
//! Order of preference for every provider:
//! 1. **Wire cost** reported by the service (Grok `costUsdTicks`, …).
//!    Pi's `usage.cost.total` is a client estimate; an ambiguous zero does
//!    not override a known catalog rate.
//! 2. **Catalog estimate** from models.dev (or the cached catalog) using
//!    token counts × unit rates. Lookup may strip served-model suffixes
//!    (e.g. `grok-4.5-build` → `grok-4.5`) without rewriting stored ids.
//!
//! Storage and UI keep the original model string; only the pricing lookup
//! is allowed to alias.

mod cost;

pub use cost::{CostSource, RecordedCost, resolve_cost};

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Primary pricing data source — curated, per-provider, USD/1M tokens.
pub const PRICING_CATALOG_URL: &str = "https://models.dev/api.json";
pub const PRICING_CATALOG_JSON_KEY: &str = "pricing_catalog_json";
pub const PRICING_CATALOG_UPDATED_AT_KEY: &str = "pricing_catalog_updated_at";
pub const PRICING_CATALOG_MODEL_COUNT_KEY: &str = "pricing_catalog_model_count";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub input_above_threshold: Option<f64>,
    pub output_above_threshold: Option<f64>,
    pub cache_read_above_threshold: Option<f64>,
    pub cache_write_above_threshold: Option<f64>,
    pub threshold_tokens: Option<u64>,
}

/// Stored once per catalog key (~10k entries), so absent rates are omitted
/// rather than written as `null`; a missing field reads back as `None`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoteModelPricing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
}

pub type PricingCatalog = HashMap<String, RemoteModelPricing>;

pub fn normalize_model_key(model: &str) -> String {
    model.trim().to_lowercase()
}

/// Parse a cached PricingCatalog (our flattened format, stored in SQLite).
/// Automatically generates short-name aliases for provider-prefixed keys
/// (e.g. `"vendor/model-1"` → also inserts `"model-1"`) so that
/// exact-match lookup works without HashMap iteration.
pub fn parse_catalog(json: &str) -> Result<PricingCatalog, serde_json::Error> {
    let raw: std::collections::BTreeMap<String, RemoteModelPricing> = serde_json::from_str(json)?;
    let mut catalog: PricingCatalog = raw
        .iter()
        .map(|(name, pricing)| (normalize_model_key(name), pricing.clone()))
        .collect();
    let mut ordered: Vec<_> = raw.iter().collect();
    ordered.sort_by_key(|(name, _)| {
        PREFERRED_ALIAS_PROVIDERS
            .iter()
            .position(|provider| name.starts_with(&format!("{provider}/")))
            .unwrap_or(usize::MAX)
    });
    for (name, pricing) in ordered {
        let key = normalize_model_key(name);
        // Exact persisted keys always win; missing aliases have deterministic
        // provider precedence, independent of HashMap iteration order.
        if let Some((_, suffix)) = key.split_once('/')
            && !suffix.is_empty()
            && !catalog.contains_key(suffix)
        {
            catalog.insert(suffix.to_string(), pricing.clone());
        }
    }
    Ok(catalog)
}

// ── models.dev parsing ──────────────────────────────────────────────

#[derive(Deserialize)]
struct ModelsDevCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Deserialize)]
struct ModelsDevModel {
    cost: Option<ModelsDevCost>,
}

#[derive(Deserialize)]
struct ModelsDevProvider {
    // BTreeMap keeps model iteration deterministic so alias collisions
    // always resolve the same way across runs.
    models: Option<std::collections::BTreeMap<String, ModelsDevModel>>,
}

/// Skip non-text models when generating aliases.
const SKIP_KEYWORDS: &[&str] = &[
    "speech",
    "embedding",
    "image",
    "tts",
    "asr",
    "video",
    "ocr",
    "audio",
    "realtime",
    "vision",
];

/// Preferred providers when multiple catalog entries compete for the same
/// short alias. This only affects alias ownership, not runtime lookup logic.
/// First match wins.
const PREFERRED_ALIAS_PROVIDERS: &[&str] = &[
    "anthropic",
    "meta",
    "openai",
    "google",
    "xai",
    "deepseek",
    "moonshotai",
    "minimax",
    "zai",
    "alibaba",
    "stepfun",
    "xiaomi",
    "mistral",
    "cohere",
];

/// Parse models.dev API response into a flat PricingCatalog.
///
/// Every provider/model with usable pricing (including explicit zero) is indexed as
/// `provider_id/model_id`. For preferred providers a short-name alias
/// (just `model_id`) is also inserted so that e.g. `claude-opus-4-6`
/// resolves directly without a prefix.
pub fn parse_models_dev(json: &str) -> Result<PricingCatalog, serde_json::Error> {
    let providers: std::collections::BTreeMap<String, ModelsDevProvider> =
        serde_json::from_str(json)?;
    let mut catalog = PricingCatalog::new();

    // Process preferred providers first so their short aliases win; the
    // BTreeMap's id ordering breaks the remaining ties deterministically.
    let mut ordered: Vec<_> = providers.iter().collect();
    ordered.sort_by_key(|(id, _)| {
        PREFERRED_ALIAS_PROVIDERS
            .iter()
            .position(|&candidate| candidate == id.as_str())
            .unwrap_or(usize::MAX)
    });

    for (provider_id, provider) in &ordered {
        let models = match &provider.models {
            Some(m) => m,
            None => continue,
        };
        for (model_id, model) in models {
            let cost = match &model.cost {
                Some(c) => c,
                None => continue,
            };
            let input = cost.input.filter(|v| v.is_finite() && *v >= 0.0);
            let output = cost.output.filter(|v| v.is_finite() && *v >= 0.0);
            if input.is_none() || output.is_none() {
                continue;
            }

            let pricing = RemoteModelPricing {
                input_cost_per_token: input.map(|v| v / 1_000_000.0),
                output_cost_per_token: output.map(|v| v / 1_000_000.0),
                cache_read_input_token_cost: cost
                    .cache_read
                    .filter(|&v| v > 0.0)
                    .map(|v| v / 1_000_000.0),
                cache_creation_input_token_cost: cost
                    .cache_write
                    .filter(|&v| v > 0.0)
                    .map(|v| v / 1_000_000.0),
                input_cost_per_token_above_200k_tokens: None,
                output_cost_per_token_above_200k_tokens: None,
                cache_read_input_token_cost_above_200k_tokens: None,
                cache_creation_input_token_cost_above_200k_tokens: None,
            };

            let key = normalize_model_key(&format!("{provider_id}/{model_id}"));
            catalog.insert(key.clone(), pricing.clone());

            // Short-name alias — first preferred provider wins
            let short = normalize_model_key(model_id);
            if short != key
                && !catalog.contains_key(&short)
                && !SKIP_KEYWORDS.iter().any(|kw| short.contains(kw))
            {
                catalog.insert(short, pricing);
            }
        }
    }

    Ok(catalog)
}

pub fn count_models_dev_models(json: &str) -> Result<u64, serde_json::Error> {
    let providers: HashMap<String, ModelsDevProvider> = serde_json::from_str(json)?;
    let count = providers
        .values()
        .filter_map(|provider| provider.models.as_ref())
        .map(|models| models.len() as u64)
        .sum();
    Ok(count)
}

fn push_unique(targets: &mut Vec<String>, candidate: String) {
    if !targets.contains(&candidate) {
        targets.push(candidate);
    }
}

/// A dated served revision may use its undated catalog entry. Numeric model
/// versions and price tiers (mini, fast, pro, etc.) are distinct models.
fn strip_trailing_version_segment(model: &str) -> Option<String> {
    let (prefix, suffix) = model.rsplit_once('-')?;
    if suffix.len() == 8 && suffix.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(prefix.to_string());
    }
    None
}

/// Grok Build's served identifier uses the selected model's catalog rates.
fn strip_variant_suffix(model: &str) -> Option<String> {
    let lower = model.to_lowercase();
    let base = lower.strip_suffix("-build")?;
    let short = base.rsplit('/').next()?;
    short.starts_with("grok-").then(|| base.to_string())
}

fn model_match_variants(model: &str) -> Vec<String> {
    let normalized = normalize_model_key(model);
    let mut variants = vec![normalized.clone()];

    // provider/model -> model
    if let Some((_, suffix)) = normalized.rsplit_once('/') {
        push_unique(&mut variants, suffix.to_string());
    }

    // Keep stripping trailing revision/version segments until stable.
    let mut idx = 0;
    while idx < variants.len() {
        if let Some(stripped) = strip_trailing_version_segment(&variants[idx]) {
            push_unique(&mut variants, stripped);
        }
        idx += 1;
    }

    variants
}

// ── Pricing lookup ──────────────────────────────────────────────────

fn model_pricing_from_remote(remote: &RemoteModelPricing) -> Option<ModelPricing> {
    let input = remote.input_cost_per_token?;
    let output = remote.output_cost_per_token?;
    let cache_read = remote.cache_read_input_token_cost.unwrap_or(0.0);
    let cache_write = remote.cache_creation_input_token_cost.unwrap_or(0.0);

    Some(ModelPricing {
        input,
        output,
        cache_read,
        cache_write,
        input_above_threshold: remote.input_cost_per_token_above_200k_tokens,
        output_above_threshold: remote.output_cost_per_token_above_200k_tokens,
        cache_read_above_threshold: remote.cache_read_input_token_cost_above_200k_tokens,
        cache_write_above_threshold: remote.cache_creation_input_token_cost_above_200k_tokens,
        threshold_tokens: remote
            .input_cost_per_token_above_200k_tokens
            .or(remote.output_cost_per_token_above_200k_tokens)
            .or(remote.cache_read_input_token_cost_above_200k_tokens)
            .or(remote.cache_creation_input_token_cost_above_200k_tokens)
            .map(|_| 200_000),
    })
}

/// Exact-match only lookup: query → catalog key.
/// No HashMap iteration, no fuzzy matching.  All fuzziness (version
/// stripping, variant suffix stripping) lives in `lookup_pricing`'s
/// candidate pipeline so the resolution order is explicit and
/// deterministic regardless of HashMap seed.
fn lookup_exact(catalog: &PricingCatalog, model: &str) -> Option<ModelPricing> {
    let normalized = normalize_model_key(model);
    catalog.get(&normalized).and_then(model_pricing_from_remote)
}

pub fn lookup_pricing(catalog: Option<&PricingCatalog>, model: &str) -> Option<ModelPricing> {
    let catalog = catalog?;

    // 1. Exact key / version-stripped variants.
    for candidate in model_match_variants(model) {
        if let Some(pricing) = lookup_exact(catalog, &candidate) {
            return Some(pricing);
        }
    }

    // 2. Resolve only the documented Grok Build served-id alias.
    let normalized = normalize_model_key(model);
    if let Some(base) = strip_variant_suffix(&normalized) {
        for candidate in model_match_variants(&base) {
            if let Some(pricing) = lookup_exact(catalog, &candidate) {
                return Some(pricing);
            }
        }
    }

    None
}

pub fn estimate_cost_with_catalog(
    catalog: Option<&PricingCatalog>,
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> f64 {
    let Some(p) = lookup_pricing(catalog, model) else {
        return 0.0;
    };
    component_cost(input, p.input, p.input_above_threshold, p.threshold_tokens)
        + component_cost(
            output,
            p.output,
            p.output_above_threshold,
            p.threshold_tokens,
        )
        + component_cost(
            cache_read,
            p.cache_read,
            p.cache_read_above_threshold,
            p.threshold_tokens,
        )
        + component_cost(
            cache_write,
            p.cache_write,
            p.cache_write_above_threshold,
            p.threshold_tokens,
        )
}

fn component_cost(
    tokens: u64,
    base_price: f64,
    above_threshold_price: Option<f64>,
    threshold_tokens: Option<u64>,
) -> f64 {
    if tokens == 0 {
        return 0.0;
    }
    match (above_threshold_price, threshold_tokens) {
        (Some(above), Some(threshold)) if tokens > threshold => {
            let below = threshold as f64 * base_price;
            let above_tokens = (tokens - threshold) as f64 * above;
            below + above_tokens
        }
        _ => tokens as f64 * base_price,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        count_models_dev_models, estimate_cost_with_catalog, parse_catalog, parse_models_dev,
    };

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn stored_catalog_omits_absent_rates_and_reads_back_identically() {
        let catalog = parse_models_dev(
            r#"{"vendor":{"models":{"model-a":{"cost":{"input":2.5,"output":15,"cache_read":0.25}}}}}"#,
        )
        .unwrap();
        let stored = serde_json::to_string(&catalog).unwrap();
        assert!(!stored.contains("null"), "{stored}");

        let reread = parse_catalog(&stored).unwrap();
        let legacy = parse_catalog(
            r#"{"vendor/model-a":{"input_cost_per_token":2.5e-6,"output_cost_per_token":1.5e-5,"cache_read_input_token_cost":2.5e-7,"cache_creation_input_token_cost":null,"input_cost_per_token_above_200k_tokens":null}}"#,
        )
        .unwrap();
        for model in ["vendor/model-a", "model-a"] {
            let original = super::lookup_pricing(Some(&catalog), model);
            assert!(original.is_some());
            assert_eq!(super::lookup_pricing(Some(&reread), model), original);
            assert_eq!(super::lookup_pricing(Some(&legacy), model), original);
        }
    }

    #[test]
    fn cached_aliases_preserve_exact_keys_and_prefer_the_model_vendor() {
        let raw = r#"{
            "meta/model-a":{"input_cost_per_token":1,"output_cost_per_token":2},
            "gateway/model-a":{"input_cost_per_token":9,"output_cost_per_token":9},
            "model-b":{"input_cost_per_token":3,"output_cost_per_token":4},
            "gateway/model-b":{"input_cost_per_token":8,"output_cost_per_token":8}
        }"#;
        for _ in 0..20 {
            let catalog = parse_catalog(raw).unwrap();
            assert_eq!(
                super::lookup_pricing(Some(&catalog), "model-a")
                    .unwrap()
                    .input,
                1.0
            );
            assert_eq!(
                super::lookup_pricing(Some(&catalog), "model-b")
                    .unwrap()
                    .input,
                3.0
            );
        }
    }

    #[test]
    fn parse_catalog_and_lookup_exact_model() {
        let catalog = parse_catalog(
            r#"{"gpt-5.4":{"input_cost_per_token":2.5e-6,"output_cost_per_token":15e-6,"cache_read_input_token_cost":2.5e-7}}"#,
        )
        .expect("catalog");
        let pricing = super::lookup_pricing(Some(&catalog), "gpt-5.4").expect("pricing");
        assert_close(pricing.input, 2.5e-6);
        assert_close(pricing.output, 15.0e-6);
        assert_close(pricing.cache_read, 0.25e-6);
        assert_eq!(pricing.threshold_tokens, None);
    }

    #[test]
    fn lookup_pricing_strips_build_suffix_for_grok_served_ids() {
        let catalog = parse_catalog(
            r#"{"grok-4.5":{"input_cost_per_token":2e-6,"output_cost_per_token":6e-6,"cache_read_input_token_cost":3e-7}}"#,
        )
        .expect("catalog");
        let pricing = super::lookup_pricing(Some(&catalog), "grok-4.5-build").expect("pricing");
        assert_close(pricing.input, 2e-6);
        assert_close(pricing.output, 6e-6);
        // Exact catalog id still wins.
        let exact = super::lookup_pricing(Some(&catalog), "grok-4.5").expect("exact");
        assert_close(exact.input, 2e-6);
    }

    #[test]
    fn resolve_cost_usd_prefers_wire_over_catalog() {
        let catalog =
            parse_catalog(r#"{"m":{"input_cost_per_token":1.0,"output_cost_per_token":1.0}}"#)
                .expect("catalog");
        // Catalog would estimate 200 if used (100+100 tokens * $1/token).
        let wire = super::resolve_cost(
            Some(super::RecordedCost::Reported(0.42)),
            Some(&catalog),
            "m",
            100,
            100,
            0,
            0,
        );
        assert_close(wire.usd, 0.42);
        let estimated = super::resolve_cost(None, Some(&catalog), "m", 100, 100, 0, 0);
        assert_close(estimated.usd, 200.0);
        // Explicit zero is still wire-authoritative.
        let free = super::resolve_cost(
            Some(super::RecordedCost::Reported(0.0)),
            Some(&catalog),
            "m",
            100,
            100,
            0,
            0,
        );
        assert_close(free.usd, 0.0);
    }

    #[test]
    fn lookup_pricing_does_not_borrow_prices_from_other_tiers() {
        let catalog = parse_catalog(
            r#"{"gpt-5.4":{"input_cost_per_token":0.0000025,"output_cost_per_token":0.000015}}"#,
        )
        .unwrap();
        for model in [
            "gpt-5.4-fast",
            "gpt-5.4-mini",
            "gpt-5.4-pro",
            "gpt-5.4-preview",
        ] {
            assert!(super::lookup_pricing(Some(&catalog), model).is_none());
        }
    }

    #[test]
    fn parse_models_dev_creates_short_aliases() {
        let json = r#"{
            "moonshotai": {
                "models": {
                    "kimi-k2.5": {
                        "cost": {"input": 0.6, "output": 3.0, "cache_read": 0.1}
                    }
                }
            },
            "deepseek": {
                "models": {
                    "deepseek-chat": {
                        "cost": {"input": 0.28, "output": 0.42, "cache_read": 0.028}
                    }
                }
            }
        }"#;
        let catalog = parse_models_dev(json).expect("catalog");

        // Full keys exist
        assert!(catalog.contains_key("moonshotai/kimi-k2.5"));
        assert!(catalog.contains_key("deepseek/deepseek-chat"));

        // Short aliases exist
        assert!(catalog.contains_key("kimi-k2.5"));
        assert!(catalog.contains_key("deepseek-chat"));

        // Pricing is correct (converted from USD/1M to per-token)
        let cost = estimate_cost_with_catalog(Some(&catalog), "kimi-k2.5", 1_000_000, 0, 0, 0);
        assert_close(cost, 0.6);

        let cost = estimate_cost_with_catalog(Some(&catalog), "deepseek-chat", 1_000_000, 0, 0, 0);
        assert_close(cost, 0.28);
    }

    #[test]
    fn parse_models_dev_resolves_nonpreferred_aliases_deterministically() {
        let json = r#"{
            "zeta-provider": {
                "models": {
                    "shared-model": {"cost": {"input": 9.0, "output": 9.0}}
                }
            },
            "alpha-provider": {
                "models": {
                    "shared-model": {"cost": {"input": 1.0, "output": 1.0}}
                }
            }
        }"#;
        let catalog = parse_models_dev(json).expect("catalog");
        let cost = estimate_cost_with_catalog(Some(&catalog), "shared-model", 1_000_000, 0, 0, 0);
        assert_close(cost, 1.0);
    }

    #[test]
    fn lookup_pricing_does_not_borrow_an_older_minor_versions_price() {
        let json = r#"{
            "zai": {
                "models": {
                    "glm-5": {
                        "cost": {"input": 1.0, "output": 3.2, "cache_read": 0.2}
                    }
                }
            }
        }"#;
        let catalog = parse_models_dev(json).expect("catalog");

        assert!(super::lookup_pricing(Some(&catalog), "glm-5.1").is_none());
    }

    #[test]
    fn parse_models_dev_keeps_explicit_free_models() {
        let json = r#"{
            "zai": {
                "models": {
                    "glm-4.7-flash": {
                        "cost": {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0}
                    },
                    "glm-5": {
                        "cost": {"input": 1.0, "output": 3.2}
                    }
                }
            }
        }"#;
        let catalog = parse_models_dev(json).expect("catalog");
        assert!(catalog.contains_key("zai/glm-4.7-flash"));
        let free = super::resolve_cost(None, Some(&catalog), "zai/glm-4.7-flash", 100, 50, 20, 0);
        assert_eq!(free.source, super::CostSource::Estimated);
        assert_eq!(free.usd, 0.0);
        assert!(catalog.contains_key("zai/glm-5"));
    }

    #[test]
    fn count_models_dev_models_ignores_alias_expansion() {
        let json = r#"{
            "anthropic": {
                "models": {
                    "claude-opus-4-6": {"cost": {"input": 5.0, "output": 25.0}},
                    "claude-sonnet-4-5": {"cost": {"input": 3.0, "output": 15.0}}
                }
            },
            "moonshotai": {
                "models": {
                    "kimi-k2.5": {"cost": {"input": 0.6, "output": 3.0}}
                }
            }
        }"#;

        assert_eq!(count_models_dev_models(json).expect("count"), 3);
        assert!(parse_models_dev(json).expect("catalog").len() > 3);
    }

    #[test]
    fn estimate_cost_handles_tiered_pricing() {
        let catalog = parse_catalog(
            r#"{"claude-sonnet-4-5":{"input_cost_per_token":3e-6,"output_cost_per_token":15e-6,"cache_read_input_token_cost":3e-7,"cache_creation_input_token_cost":3.75e-6,"input_cost_per_token_above_200k_tokens":6e-6,"output_cost_per_token_above_200k_tokens":22.5e-6,"cache_read_input_token_cost_above_200k_tokens":6e-7,"cache_creation_input_token_cost_above_200k_tokens":7.5e-6}}"#,
        )
        .expect("catalog");
        let cost = estimate_cost_with_catalog(
            Some(&catalog),
            "claude-sonnet-4-5",
            300_000,
            250_000,
            250_000,
            300_000,
        );
        let expected = (200_000.0 * 3e-6)
            + (100_000.0 * 6e-6)
            + (200_000.0 * 15e-6)
            + (50_000.0 * 22.5e-6)
            + (200_000.0 * 0.3e-6)
            + (50_000.0 * 0.6e-6)
            + (200_000.0 * 3.75e-6)
            + (100_000.0 * 7.5e-6);
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn estimate_cost_uses_all_four_components() {
        let json = r#"{
            "anthropic": {
                "models": {
                    "claude-opus-4-6": {
                        "cost": {"input": 5.0, "output": 25.0, "cache_read": 0.5, "cache_write": 6.25}
                    }
                }
            }
        }"#;
        let catalog = parse_models_dev(json).expect("catalog");
        let cost = estimate_cost_with_catalog(Some(&catalog), "claude-opus-4-6", 100, 50, 200, 10);
        let expected = (100.0 * 5e-6) + (50.0 * 25e-6) + (200.0 * 0.5e-6) + (10.0 * 6.25e-6);
        assert_close(cost, expected);
    }

    #[test]
    fn missing_cache_pricing_defaults_to_zero() {
        let catalog = parse_catalog(
            r#"{"test-model":{"input_cost_per_token":5e-6,"output_cost_per_token":25e-6}}"#,
        )
        .expect("catalog");
        // cache_read=200, cache_write=100 should contribute $0 when pricing is missing
        let cost = estimate_cost_with_catalog(Some(&catalog), "test-model", 100, 50, 200, 100);
        let expected = (100.0 * 5e-6) + (50.0 * 25e-6);
        assert_close(cost, expected);
    }

    #[test]
    fn lookup_pricing_matches_versioned_model_names() {
        let json = r#"{
            "anthropic": {
                "models": {
                    "claude-sonnet-4-5": {
                        "cost": {"input": 3.0, "output": 15.0}
                    }
                }
            }
        }"#;
        let catalog = parse_models_dev(json).expect("catalog");

        let cost = estimate_cost_with_catalog(
            Some(&catalog),
            "claude-sonnet-4-5-20250514",
            1_000_000,
            0,
            0,
            0,
        );

        assert_close(cost, 3.0);
    }

    #[test]
    fn lookup_pricing_matches_provider_suffix_without_alias() {
        let catalog = parse_catalog(
            r#"{"vendor/model-1":{"input_cost_per_token":4e-6,"output_cost_per_token":12e-6}}"#,
        )
        .expect("catalog");

        let cost =
            estimate_cost_with_catalog(Some(&catalog), "model-1-20250514", 1_000_000, 0, 0, 0);

        assert_close(cost, 4.0);
    }

    #[test]
    fn lookup_prefers_most_specific_model_over_shorter_key() {
        // "gpt-5.4-fast" must match "gpt-5.4" ($2.5/M input), not "gpt-5" ($1.25/M input).
        let json = r#"{
            "openai": {
                "models": {
                    "gpt-5": {
                        "cost": {"input": 1.25, "output": 10, "cache_read": 0.125}
                    },
                    "gpt-5.4": {
                        "cost": {"input": 2.5, "output": 15, "cache_read": 0.25}
                    }
                }
            }
        }"#;
        let catalog = parse_models_dev(json).expect("catalog");

        // Direct lookup — sanity check
        let cost_54 = estimate_cost_with_catalog(Some(&catalog), "gpt-5.4", 1_000_000, 0, 0, 0);
        assert_close(cost_54, 2.5);

        let cost_5 = estimate_cost_with_catalog(Some(&catalog), "gpt-5", 1_000_000, 0, 0, 0);
        assert_close(cost_5, 1.25);

        // Neither a different tier nor an older version is a known rate.
        assert!(super::lookup_pricing(Some(&catalog), "gpt-5.4-fast").is_none());
        assert!(super::lookup_pricing(Some(&catalog), "gpt-5.5").is_none());
    }
}
