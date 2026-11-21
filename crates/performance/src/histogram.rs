/// Fixed-length log-scale duration histogram (76 buckets).
///
/// - Bucket `0`: `[0.0, 0.5)` ms
/// - Bucket `i` (`1..=74`): `[0.5 * 1.2^(i-1), 0.5 * 1.2^i)` ms (up to ~361,602 ms ≈ 6 min)
/// - Bucket `75`: `[0.5 * 1.2^74, +inf)` ms (overflow)
///
/// Gives ≤10% relative error across 6 orders of magnitude while fitting in 608 bytes per 1-minute row.
pub const NUM_HISTOGRAM_BUCKETS: usize = 76;
const BASE_MS: f64 = 0.5;
const GROWTH_FACTOR: f64 = 1.2;

pub type HistogramBuckets = [i64; NUM_HISTOGRAM_BUCKETS];

/// Compute the bucket index for a duration in milliseconds.
pub fn bucket_index_for_ms(duration_ms: f64) -> usize {
    if !duration_ms.is_finite() || duration_ms < BASE_MS {
        return 0;
    }
    let ratio = duration_ms / BASE_MS;
    let raw = ratio.ln() / GROWTH_FACTOR.ln();
    if !raw.is_finite() || raw < 0.0 {
        return 0;
    }
    let idx = (raw.floor() as usize) + 1;
    idx.min(NUM_HISTOGRAM_BUCKETS - 1)
}

/// Lower bound (in ms) of bucket `idx`.
pub fn bucket_lower_ms(idx: usize) -> f64 {
    if idx == 0 {
        0.0
    } else {
        BASE_MS * GROWTH_FACTOR.powi((idx - 1) as i32)
    }
}

/// Upper bound (in ms) of bucket `idx` (overflow bucket uses `1.2x` of its lower bound as fallback).
pub fn bucket_upper_ms(idx: usize) -> f64 {
    if idx == 0 {
        BASE_MS
    } else {
        BASE_MS * GROWTH_FACTOR.powi(idx as i32)
    }
}

/// Record a single observed duration (ms) into the histogram.
pub fn observe_duration(buckets: &mut HistogramBuckets, duration_ms: f64) {
    let clamped = if duration_ms.is_finite() && duration_ms >= 0.0 {
        duration_ms
    } else {
        0.0
    };
    let idx = bucket_index_for_ms(clamped);
    buckets[idx] = buckets[idx].saturating_add(1);
}

/// Element-wise merge of `other` into `into`.
pub fn merge_histograms(into: &mut HistogramBuckets, other: &[i64]) {
    for (slot, val) in into.iter_mut().zip(other.iter()) {
        *slot = slot.saturating_add((*val).max(0));
    }
}

/// Estimate quantile `q` (`0.0..=1.0`) from log-scale histogram buckets using linear interpolation
/// within the target bucket, clamped to `[min_ms, max_ms]`.
pub fn estimate_percentile(buckets: &[i64], min_ms: f64, max_ms: f64, quantile: f64) -> f64 {
    let q = quantile.clamp(0.0, 1.0);
    let total: i64 = buckets.iter().copied().filter(|&c| c > 0).sum();
    if total <= 0 {
        return 0.0;
    }

    let safe_min = if min_ms.is_finite() && min_ms >= 0.0 {
        min_ms
    } else {
        0.0
    };
    let safe_max = if max_ms.is_finite() && max_ms >= safe_min {
        max_ms
    } else {
        safe_min
    };

    if total == 1 || (safe_max - safe_min).abs() < f64::EPSILON {
        return round_two_decimals(safe_min);
    }

    let target_rank = (total as f64) * q;
    let mut cumulative = 0i64;

    for (idx, &raw_count) in buckets.iter().enumerate() {
        let count = raw_count.max(0);
        if count == 0 {
            continue;
        }
        let next_cum = cumulative + count;
        if (next_cum as f64) >= target_rank || next_cum == total {
            let lower = bucket_lower_ms(idx).max(safe_min);
            let upper = if idx == NUM_HISTOGRAM_BUCKETS - 1 {
                safe_max.max(lower)
            } else {
                bucket_upper_ms(idx).min(safe_max).max(lower)
            };
            let fraction = ((target_rank - cumulative as f64) / (count as f64)).clamp(0.0, 1.0);
            let est = lower + (upper - lower) * fraction;
            let clamped = est.clamp(safe_min, safe_max);
            return round_two_decimals(clamped);
        }
        cumulative = next_cum;
    }

    round_two_decimals(safe_max)
}

pub fn round_two_decimals(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_indices_are_monotonic_and_bounded() {
        assert_eq!(bucket_index_for_ms(0.0), 0);
        assert_eq!(bucket_index_for_ms(0.49), 0);
        assert_eq!(bucket_index_for_ms(0.5), 1);
        assert!(bucket_index_for_ms(10.0) < bucket_index_for_ms(100.0));
        assert!(bucket_index_for_ms(100.0) < bucket_index_for_ms(1000.0));
        assert_eq!(bucket_index_for_ms(10_000_000.0), NUM_HISTOGRAM_BUCKETS - 1);
    }

    #[test]
    fn estimates_p50_p95_p99_accurately_within_tolerance() {
        let mut h = [0i64; NUM_HISTOGRAM_BUCKETS];
        // 1000 samples linearly from 1.0ms..=1000.0ms
        for i in 1..=1000 {
            observe_duration(&mut h, i as f64);
        }

        let p50 = estimate_percentile(&h, 1.0, 1000.0, 0.50);
        let p95 = estimate_percentile(&h, 1.0, 1000.0, 0.95);
        let p99 = estimate_percentile(&h, 1.0, 1000.0, 0.99);

        assert!((p50 - 500.0).abs() <= 55.0, "p50={p50}");
        assert!((p95 - 950.0).abs() <= 95.0, "p95={p95}");
        assert!((p99 - 990.0).abs() <= 95.0, "p99={p99}");
        assert!(p50 <= p95 && p95 <= p99);
    }

    #[test]
    fn merging_histograms_matches_single_pass_observations() {
        let mut h1 = [0i64; NUM_HISTOGRAM_BUCKETS];
        let mut h2 = [0i64; NUM_HISTOGRAM_BUCKETS];
        let mut combined = [0i64; NUM_HISTOGRAM_BUCKETS];

        for d in [12.5, 45.0, 120.0, 850.0] {
            observe_duration(&mut h1, d);
            observe_duration(&mut combined, d);
        }
        for d in [5.0, 60.0, 220.0, 1500.0] {
            observe_duration(&mut h2, d);
            observe_duration(&mut combined, d);
        }

        merge_histograms(&mut h1, &h2);
        assert_eq!(h1, combined);
    }
}
