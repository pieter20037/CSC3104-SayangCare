use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CircuitState {
    Closed = 0,
    Open = 1,
    HalfOpen = 2,
}

impl From<u8> for CircuitState {
    fn from(v: u8) -> Self {
        match v {
            1 => CircuitState::Open,
            2 => CircuitState::HalfOpen,
            _ => CircuitState::Closed,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BreakerConfig {
    pub base_error_rate: f64,
    pub base_latency_ms: u64,
    pub min_requests: u64,
    pub half_open_after_secs: u64,
    pub window_size: u64,
}

#[derive(Debug, Clone)]
pub struct BreakerSnapshot {
    pub state: CircuitState,
    pub error_rate: f64,
    pub p95_latency_ms: u64,
    pub total_requests: u64,
    pub opened_at: Option<DateTime<Utc>>,
    pub last_adapted_threshold: f64,
}

/// Rolling-window circuit breaker with adaptive thresholds.
///
/// The "sentiment-aware" twist: `SentimentAdaptivePolicy` computes the
/// effective error/latency thresholds at evaluation time based on the
/// caller's current distress severity. A distressed caller gets a
/// *more conservative* (lower) threshold so we trip faster and preserve
/// the human connection.
pub struct CircuitBreaker {
    config: BreakerConfig,
    state: AtomicU8,
    // Simple sliding counters; a production impl would use a ring buffer.
    metrics: Mutex<WindowMetrics>,
    opened_at: RwLock<Option<DateTime<Utc>>>,
    policy: Arc<dyn AdaptiveThresholdPolicy>,
}

#[derive(Debug, Clone, Copy)]
struct RequestRecord {
    latency_ms: u64,
    success: bool,
}

#[derive(Debug, Default)]
struct WindowMetrics {
    records: VecDeque<RequestRecord>,
}

impl WindowMetrics {
    fn record(&mut self, latency_ms: u64, success: bool, capacity: usize) {
        if capacity == 0 {
            return;
        }

        self.records.push_back(RequestRecord {
            latency_ms,
            success,
        });

        while self.records.len() > capacity {
            self.records.pop_front();
        }
    }

    fn snapshot(&self) -> (u64, f64, u64) {
        let total = self.records.len();

        if total == 0 {
            return (0, 0.0, 0);
        }

        let failures = self.records.iter().filter(|r| !r.success).count();
        let error_rate = failures as f64 / total as f64;

        let mut latencies: Vec<u64> = self.records.iter().map(|r| r.latency_ms).collect();

        latencies.sort_unstable();

        let rank = (total * 95).div_ceil(100);
        let p95 = latencies[rank.saturating_sub(1)];

        (total as u64, error_rate, p95)
    }
}

#[async_trait::async_trait]
pub trait AdaptiveThresholdPolicy: Send + Sync {
    /// Return (error_rate_threshold, latency_threshold_ms) given distress severity in [0,1].
    fn thresholds(&self, distress_severity: f32) -> (f64, u64);
}

impl CircuitBreaker {
    pub fn new(config: BreakerConfig, policy: Arc<dyn AdaptiveThresholdPolicy>) -> Self {
        Self {
            config,
            state: AtomicU8::new(CircuitState::Closed as u8),
            metrics: Mutex::new(WindowMetrics::default()),
            opened_at: RwLock::new(None),
            policy,
        }
    }

    pub fn state(&self) -> CircuitState {
        CircuitState::from(self.state.load(Ordering::Relaxed))
    }

    /// Fast pre-check. Returns `Err` if circuit is open.
    pub fn acquire(&self) -> Result<(), sayangcare_core::CoreError> {
        match self.state() {
            CircuitState::Closed | CircuitState::HalfOpen => Ok(()),
            CircuitState::Open => Err(sayangcare_core::CoreError::CircuitOpen(
                "llm-inference".into(),
            )),
        }
    }

    /// Record a successful call.
    pub fn record_success(&self, latency_ms: u64) {
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.record(latency_ms, true, self.config.window_size as usize);
        }

        if self.state() == CircuitState::HalfOpen {
            self.reset();
        }
    }

    /// Record a failed call. May trip the breaker.
    pub async fn record_failure(&self, latency_ms: u64, distress_severity: f32) {
        let (total, error_rate, p95_latency) = {
            let mut metrics = self.metrics.lock().unwrap();

            metrics.record(latency_ms, false, self.config.window_size as usize);

            metrics.snapshot()
        };

        if self.state() != CircuitState::Closed {
            return;
        }

        if total < self.config.min_requests {
            return;
        }

        let (err_threshold, lat_threshold) = self.policy.thresholds(distress_severity);

        if error_rate >= err_threshold || p95_latency >= lat_threshold {
            warn!(
                error_rate,
                p95_latency,
                err_threshold,
                lat_threshold,
                distress_severity,
                "circuit breaker tripping"
            );

            self.trip().await;
        }
    }

    async fn trip(&self) {
        self.state
            .store(CircuitState::Open as u8, Ordering::Relaxed);
        *self.opened_at.write().await = Some(Utc::now());
    }

    /// Reset the breaker to Closed state and clear metrics.
    fn reset(&self) {
        self.state
            .store(CircuitState::Closed as u8, Ordering::Relaxed);

        self.metrics.lock().unwrap().records.clear();
    }

    /// Background task: transition Open -> HalfOpen after cooldown.
    pub async fn tick(&self) {
        if self.state() != CircuitState::Open {
            return;
        }
        let opened = *self.opened_at.read().await;
        if let Some(t) = opened {
            let elapsed = (Utc::now() - t).num_seconds() as u64;
            if elapsed >= self.config.half_open_after_secs {
                info!("circuit breaker -> half-open");
                self.state
                    .store(CircuitState::HalfOpen as u8, Ordering::Relaxed);
            }
        }
    }

    /// Return a snapshot of the current breaker state and metrics.
    pub fn snapshot(&self) -> BreakerSnapshot {
        let (total, error_rate, p95_latency) = self.metrics.lock().unwrap().snapshot();

        BreakerSnapshot {
            state: self.state(),
            error_rate,
            p95_latency_ms: p95_latency,
            total_requests: total,
            opened_at: *self.opened_at.blocking_read(),
            last_adapted_threshold: 0.0,
        }
    }
}

/// A simple adaptive policy that scales thresholds based on distress severity.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_window_discards_old_requests() {
        let mut metrics = WindowMetrics::default();

        metrics.record(100, true, 3);
        metrics.record(200, false, 3);
        metrics.record(300, true, 3);
        metrics.record(400, true, 3);

        let (total, error_rate, p95) = metrics.snapshot();

        assert_eq!(total, 3);
        assert!((error_rate - 1.0 / 3.0).abs() < 0.0001);
        assert_eq!(p95, 400);
    }

    #[test]
    fn empty_window_returns_zero_metrics() {
        let metrics = WindowMetrics::default();

        assert_eq!(metrics.snapshot(), (0, 0.0, 0));
    }

    #[test]
    fn p95_is_not_average_latency() {
        let mut metrics = WindowMetrics::default();

        for _ in 0..19 {
            metrics.record(100, true, 20);
        }

        metrics.record(2000, true, 20);

        let (total, error_rate, p95) = metrics.snapshot();

        assert_eq!(total, 20);
        assert_eq!(error_rate, 0.0);
        assert_eq!(p95, 100);
    }
}

/// unit test for error rate updates when sliding window discards old requests.
#[test]
fn error_rate_updates_when_window_slides() {
    let mut metrics = WindowMetrics::default();

    // Two failures out of four requests.
    metrics.record(100, false, 4);
    metrics.record(150, false, 4);
    metrics.record(120, true, 4);
    metrics.record(130, true, 4);

    let (total, error_rate, _) = metrics.snapshot();

    assert_eq!(total, 4);
    assert!((error_rate - 0.5).abs() < 0.0001);

    // Add two successes. The two oldest failures
    // should be removed from the sliding window.
    metrics.record(110, true, 4);
    metrics.record(140, true, 4);

    let (total, error_rate, _) = metrics.snapshot();

    assert_eq!(total, 4);
    assert_eq!(error_rate, 0.0);
}
