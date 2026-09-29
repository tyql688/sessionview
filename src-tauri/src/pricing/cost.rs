use super::{PricingCatalog, estimate_cost_with_catalog, lookup_pricing};

/// A bill reported by the service and an SDK's local estimate have different
/// authority. In particular, SDKs also write zero when their rate is unknown.
#[derive(Clone, Copy, Debug)]
pub enum RecordedCost {
    Reported(f64),
    Estimate(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostSource {
    Reported,
    Estimated,
    Unpriced,
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedCost {
    pub usd: f64,
    pub source: CostSource,
}

pub fn resolve_cost(
    recorded: Option<RecordedCost>,
    catalog: Option<&PricingCatalog>,
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> ResolvedCost {
    match recorded {
        Some(RecordedCost::Reported(usd)) if usd.is_finite() && usd >= 0.0 => {
            return ResolvedCost {
                usd,
                source: CostSource::Reported,
            };
        }
        // Keep positive client estimates: they can carry channel-specific
        // rates/discounts absent from the public catalog. Zero is ambiguous.
        Some(RecordedCost::Estimate(usd)) if usd.is_finite() && usd > 0.0 => {
            return ResolvedCost {
                usd,
                source: CostSource::Estimated,
            };
        }
        Some(RecordedCost::Reported(usd) | RecordedCost::Estimate(usd))
            if !usd.is_finite() || usd < 0.0 =>
        {
            log::warn!("ignoring invalid recorded cost for model {model}");
        }
        _ => {}
    }
    if lookup_pricing(catalog, model).is_some() {
        return ResolvedCost {
            usd: estimate_cost_with_catalog(catalog, model, input, output, cache_read, cache_write),
            source: CostSource::Estimated,
        };
    }
    ResolvedCost {
        usd: 0.0,
        source: CostSource::Unpriced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::parse_catalog;

    #[test]
    fn zero_client_estimate_uses_catalog_but_reported_zero_remains_authoritative() {
        let catalog = parse_catalog(r#"{"meta/muse-spark-1.3":{"input_cost_per_token":0.00000125,"output_cost_per_token":0.00000425,"cache_read_input_token_cost":0.00000015}}"#).unwrap();
        let estimate = resolve_cost(
            Some(RecordedCost::Estimate(0.0)),
            Some(&catalog),
            "meta/muse-spark-1.3",
            1_000_000,
            1_000_000,
            1_000_000,
            0,
        );
        assert_eq!(estimate.source, CostSource::Estimated);
        assert!((estimate.usd - 5.65).abs() < 1e-10);
        let free = resolve_cost(
            Some(RecordedCost::Reported(0.0)),
            Some(&catalog),
            "meta/muse-spark-1.3",
            1_000_000,
            1_000_000,
            0,
            0,
        );
        assert_eq!(free.source, CostSource::Reported);
        assert_eq!(free.usd, 0.0);
    }

    #[test]
    fn absent_prices_and_zero_client_estimates_are_unpriced() {
        for recorded in [None, Some(RecordedCost::Estimate(0.0))] {
            assert_eq!(
                resolve_cost(recorded, None, "unknown", 100, 10, 0, 0).source,
                CostSource::Unpriced
            );
        }
        let saved = resolve_cost(
            Some(RecordedCost::Estimate(0.42)),
            None,
            "custom",
            100,
            10,
            0,
            0,
        );
        assert_eq!(saved.source, CostSource::Estimated);
        assert_eq!(saved.usd, 0.42);
    }
}
