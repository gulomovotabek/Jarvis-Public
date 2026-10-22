use std::sync::atomic::{AtomicU64, Ordering};

/// Fixed-bucket latency histogram with lock-free writes.
///
/// Buckets are in milliseconds and chosen to cover the shape of Claude turn
/// latencies (hundreds of ms to tens of seconds): 50ms → 60s+. The final
/// bucket (`+Inf`) captures everything above the last explicit bound.
pub struct LatencyHistogram {
    /// Upper bound (inclusive) of each bucket, in milliseconds.
    /// The last bucket has no upper bound (`u64::MAX`).
    bounds: Vec<u64>,
    /// Atomic counter per bucket — same length as `bounds`.
    buckets: Vec<AtomicU64>,
    count: AtomicU64,
    sum_ms: AtomicU64,
    max_ms: AtomicU64,
}

impl LatencyHistogram {
    pub fn new() -> Self {
        // Covers Claude turns from ~50ms (prompt-cache hit) to multi-minute hangs.
        let bounds = vec![
            50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 20_000, 40_000, 60_000,
            u64::MAX,
        ];
        let buckets = (0..bounds.len()).map(|_| AtomicU64::new(0)).collect();
        Self {
            bounds,
            buckets,
            count: AtomicU64::new(0),
            sum_ms: AtomicU64::new(0),
            max_ms: AtomicU64::new(0),
        }
    }

    pub fn record(&self, duration_ms: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_ms.fetch_add(duration_ms, Ordering::Relaxed);
        // Update max via CAS — cheap under low contention and we don't need
        // monotonicity guarantees across readers.
        let mut cur = self.max_ms.load(Ordering::Relaxed);
        while duration_ms > cur {
            match self.max_ms.compare_exchange_weak(
                cur,
                duration_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
        for (i, &bound) in self.bounds.iter().enumerate() {
            if duration_ms <= bound {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    /// Approximate percentile (ms) from cumulative bucket counts. Returns the
    /// upper bound of the bucket where the cumulative count first crosses the
    /// requested quantile. `+Inf` bucket reports as `max_ms`.
    fn percentile(&self, q: f64) -> u64 {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return 0;
        }
        let target = (q * count as f64).ceil() as u64;
        let mut cum = 0u64;
        for (i, &bound) in self.bounds.iter().enumerate() {
            cum += self.buckets[i].load(Ordering::Relaxed);
            if cum >= target {
                return if bound == u64::MAX {
                    self.max_ms.load(Ordering::Relaxed)
                } else {
                    bound
                };
            }
        }
        self.max_ms.load(Ordering::Relaxed)
    }

    pub fn to_json(&self) -> serde_json::Value {
        let count = self.count.load(Ordering::Relaxed);
        let sum = self.sum_ms.load(Ordering::Relaxed);
        let max = self.max_ms.load(Ordering::Relaxed);
        let mean = if count == 0 { 0 } else { sum / count };
        let p50 = self.percentile(0.50);
        let p90 = self.percentile(0.90);
        let p99 = self.percentile(0.99);
        let mut buckets = serde_json::Map::new();
        for (i, &bound) in self.bounds.iter().enumerate() {
            let label = if bound == u64::MAX {
                "+Inf".to_string()
            } else {
                format!("le_{bound}ms")
            };
            buckets.insert(label, serde_json::json!(self.buckets[i].load(Ordering::Relaxed)));
        }
        serde_json::json!({
            "count": count,
            "sum_ms": sum,
            "mean_ms": mean,
            "max_ms": max,
            "p50_ms": p50,
            "p90_ms": p90,
            "p99_ms": p99,
            "buckets": buckets,
        })
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BotMetrics {
    pub messages_received: AtomicU64,
    pub messages_responded: AtomicU64,
    pub messages_dropped_stale: AtomicU64,
    pub claude_calls: AtomicU64,
    pub claude_errors: AtomicU64,
    pub last_message_at: AtomicU64,
    pub last_response_at: AtomicU64,
    /// End-to-end Claude turn latency histogram (batcher perspective:
    /// from send_message call to result returned).
    pub claude_turn_duration: LatencyHistogram,
}

impl BotMetrics {
    pub fn new() -> Self {
        Self {
            messages_received: AtomicU64::new(0),
            messages_responded: AtomicU64::new(0),
            messages_dropped_stale: AtomicU64::new(0),
            claude_calls: AtomicU64::new(0),
            claude_errors: AtomicU64::new(0),
            last_message_at: AtomicU64::new(0),
            last_response_at: AtomicU64::new(0),
            claude_turn_duration: LatencyHistogram::new(),
        }
    }

    pub fn inc(&self, counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_now(&self, field: &AtomicU64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        field.store(now, Ordering::Relaxed);
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "messages_received": self.messages_received.load(Ordering::Relaxed),
            "messages_responded": self.messages_responded.load(Ordering::Relaxed),
            "messages_dropped_stale": self.messages_dropped_stale.load(Ordering::Relaxed),
            "claude_calls": self.claude_calls.load(Ordering::Relaxed),
            "claude_errors": self.claude_errors.load(Ordering::Relaxed),
            "last_message_at": self.last_message_at.load(Ordering::Relaxed),
            "last_response_at": self.last_response_at.load(Ordering::Relaxed),
            "claude_turn_duration": self.claude_turn_duration.to_json(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_sum_to_count() {
        let h = LatencyHistogram::new();
        h.record(30);
        h.record(200);
        h.record(1_500);
        h.record(100_000);

        let json = h.to_json();
        assert_eq!(json["count"], 4);
        // sum of all per-bucket counts must equal total count
        let buckets = json["buckets"].as_object().unwrap();
        let total: u64 = buckets.values().filter_map(|v| v.as_u64()).sum();
        assert_eq!(total, 4);
    }

    #[test]
    fn histogram_records_max_and_mean() {
        let h = LatencyHistogram::new();
        h.record(100);
        h.record(200);
        h.record(300);
        let json = h.to_json();
        assert_eq!(json["max_ms"], 300);
        assert_eq!(json["mean_ms"], 200);
        assert_eq!(json["sum_ms"], 600);
    }

    #[test]
    fn histogram_catches_overflow_in_inf_bucket() {
        let h = LatencyHistogram::new();
        h.record(u64::MAX / 2);
        let json = h.to_json();
        assert_eq!(json["buckets"]["+Inf"], 1);
    }

    #[test]
    fn histogram_percentiles() {
        let h = LatencyHistogram::new();
        for _ in 0..90 {
            h.record(40);
        } // le_50ms
        for _ in 0..9 {
            h.record(2000);
        } // le_2500ms
        h.record(50_000); // le_60000ms
        let j = h.to_json();
        assert_eq!(j["p50_ms"], 50);
        assert_eq!(j["p90_ms"], 50);
        assert_eq!(j["p99_ms"], 2500);
    }

    #[test]
    fn histogram_bucket_boundary_inclusive() {
        let h = LatencyHistogram::new();
        h.record(50); // right at le_50ms boundary
        h.record(51); // should land in le_100ms
        let json = h.to_json();
        assert_eq!(json["buckets"]["le_50ms"], 1);
        assert_eq!(json["buckets"]["le_100ms"], 1);
    }
}
