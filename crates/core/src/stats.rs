//! Small statistics helpers shared by the perf harnesses.

/// Nearest-rank percentile over an ascending-sorted slice.
/// `p` is in `[0, 1]`; returns 0.0 for an empty slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let p = p.clamp(0.0, 1.0);
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

/// Convenience: min, p50, p90, p99, max over an unsorted sample.
pub fn five_number_summary(mut sample: Vec<f64>) -> (f64, f64, f64, f64, f64) {
    sample.sort_by(|a, b| a.total_cmp(b));
    (
        percentile(&sample, 0.0),
        percentile(&sample, 0.5),
        percentile(&sample, 0.9),
        percentile(&sample, 0.99),
        percentile(&sample, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_of_sorted_sample() {
        // 1..=100 ascending
        let sample: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&sample, 0.0), 1.0);
        assert_eq!(percentile(&sample, 0.5), 50.0);
        assert_eq!(percentile(&sample, 0.9), 90.0);
        assert_eq!(percentile(&sample, 0.99), 99.0);
        assert_eq!(percentile(&sample, 1.0), 100.0);
    }

    #[test]
    fn percentile_edges() {
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[7.0], 0.5), 7.0);
        assert_eq!(percentile(&[1.0, 2.0], 5.0), 2.0); // clamped p
    }

    #[test]
    fn summary_sorts_its_input() {
        let (min, p50, _p90, _p99, max) = five_number_summary(vec![3.0, 1.0, 2.0]);
        assert_eq!((min, p50, max), (1.0, 2.0, 3.0));
    }
}
