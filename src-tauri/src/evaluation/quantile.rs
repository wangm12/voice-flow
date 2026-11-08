//! Shared finite, nonnegative latency quantiles for evaluation reports.

/// Linear interpolation at `p` over sorted rank position `(n - 1) * p`.
/// Invalid latency samples are excluded; an empty admitted sample is unavailable.
pub(crate) fn linear_quantile(samples: &[f64], p: f64) -> Option<f64> {
    debug_assert!((0.0..=1.0).contains(&p));
    let mut values: Vec<f64> = samples
        .iter()
        .copied()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let position = p * (values.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f64;
    Some(values[lower] + (values[upper] - values[lower]) * fraction)
}

#[cfg(test)]
mod tests {
    use super::linear_quantile;

    #[test]
    fn percentile_empty_is_none() {
        assert_eq!(linear_quantile(&[], 0.5), None);
    }

    #[test]
    fn evaluation_percentiles_use_linear_interpolation_and_filter_invalid_values() {
        assert_eq!(linear_quantile(&[8.0], 0.5), Some(8.0));
        assert_eq!(linear_quantile(&[100.0, 0.0, 20.0, 10.0], 0.5), Some(15.0));
        assert!((linear_quantile(&[100.0, 0.0, 20.0, 10.0], 0.95).unwrap() - 88.0).abs() < 1e-9);
        assert_eq!(linear_quantile(&[-1.0, f64::INFINITY, f64::NAN], 0.5), None);
        assert_eq!(linear_quantile(&[0.0, -1.0, 2.0], 0.5), Some(1.0));
    }
}
