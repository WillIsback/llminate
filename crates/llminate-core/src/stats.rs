//! Latency/tokens statistics for `llminate bench`.

/// One measured request.
#[derive(Debug, Clone, Copy)]
pub struct BenchRow {
    pub latency_ms: u64,
    pub completion_tokens: u64,
}

/// Aggregated benchmark statistics.
#[derive(Debug, Clone)]
pub struct BenchStats {
    latencies: Vec<u64>,
    completion_tokens: u64,
}

impl BenchStats {
    /// Build stats from raw rows.
    pub fn new(rows: Vec<BenchRow>) -> Self {
        let mut latencies: Vec<u64> = rows.iter().map(|r| r.latency_ms).collect();
        latencies.sort_unstable();
        let completion_tokens = rows.iter().map(|r| r.completion_tokens).sum();
        Self {
            latencies,
            completion_tokens,
        }
    }

    /// Number of requests.
    pub fn n(&self) -> usize {
        self.latencies.len()
    }

    /// Nearest-rank percentile (0.0..=1.0), clamped like the original shell script.
    pub fn percentile(&self, q: f64) -> u64 {
        if self.latencies.is_empty() {
            return 0;
        }
        let idx = ((q * self.n() as f64) as usize).min(self.n() - 1);
        self.latencies[idx]
    }

    /// Median latency.
    pub fn p50(&self) -> u64 {
        self.percentile(0.50)
    }

    /// 95th percentile latency.
    pub fn p95(&self) -> u64 {
        self.percentile(0.95)
    }

    /// Minimum latency.
    pub fn min(&self) -> u64 {
        self.latencies.first().copied().unwrap_or(0)
    }

    /// Maximum latency.
    pub fn max(&self) -> u64 {
        self.latencies.last().copied().unwrap_or(0)
    }

    /// Sum of completion tokens.
    pub fn completion_tokens(&self) -> u64 {
        self.completion_tokens
    }

    /// Approximate generation throughput (completion tokens / wall-clock seconds).
    pub fn tokens_per_second(&self, wall_secs: f64) -> f64 {
        if wall_secs <= 0.0 {
            return 0.0;
        }
        self.completion_tokens as f64 / wall_secs
    }

    /// Requests per second over the measured wall-clock window.
    pub fn requests_per_second(&self, wall_secs: f64) -> f64 {
        if wall_secs <= 0.0 {
            return 0.0;
        }
        self.n() as f64 / wall_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lat: &[u64], tok: &[u64]) -> Vec<BenchRow> {
        lat.iter()
            .zip(tok)
            .map(|(&latency_ms, &completion_tokens)| BenchRow {
                latency_ms,
                completion_tokens,
            })
            .collect()
    }

    #[test]
    fn percentiles_are_nearest_rank_of_sorted() {
        let s = BenchStats::new(rows(&[10, 20, 30, 40, 50], &[1, 1, 1, 1, 1]));
        assert_eq!(s.n(), 5);
        assert_eq!(s.min(), 10);
        assert_eq!(s.max(), 50);
        assert_eq!(s.p50(), 30); // idx = 0.5*5 = 2
        assert_eq!(s.p95(), 50); // idx = 4
    }

    #[test]
    fn rates_use_wall_clock() {
        let s = BenchStats::new(rows(&[100, 200], &[10, 30]));
        assert_eq!(s.completion_tokens(), 40);
        assert!((s.tokens_per_second(2.0) - 20.0).abs() < 1e-9);
        assert!((s.requests_per_second(2.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn empty_is_safe() {
        let s = BenchStats::new(vec![]);
        assert_eq!(s.n(), 0);
        assert_eq!(s.p50(), 0);
        assert_eq!(s.tokens_per_second(1.0), 0.0);
    }
}
