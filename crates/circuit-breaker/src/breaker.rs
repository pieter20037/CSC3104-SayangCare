use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
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
    total: AtomicU64,
    errors: AtomicU64,
    total_latency_ms: AtomicU64,
    opened_at: RwLock<Option<DateTime<Utc>>>,
    policy: Arc<dyn AdaptiveThresholdPolicy>,
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
            total: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
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
        self.total.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
        if self.state() == CircuitState::HalfOpen {
            // Successful probe -> close.
            self.reset();
        }
    }

    /// Record a failed call. May trip the breaker.
    pub async fn record_failure(&self, latency_ms: u64, distress_severity: f32) {
        self.total.fetch_add(1, Ordering::Relaxed);
        self.errors.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);

        if self.state() != CircuitState::Closed {
            return;
        }

        let total = self.total.load(Ordering::Relaxed);
        if total < self.config.min_requests {
            return;
        }

        let errors = self.errors.load(Ordering::Relaxed);
        let error_rate = errors as f64 / total as f64;
        let avg_latency = self.total_latency_ms.load(Ordering::Relaxed) / total.max(1);

        let (err_threshold, lat_threshold) = self.policy.thresholds(distress_severity);

        if error_rate >= err_threshold || avg_latency >= lat_threshold {
            warn!(
                error_rate,
                avg_latency,
                err_threshold,
                lat_threshold,
                distress_severity,
                "circuit breaker tripping"
            );
            self.trip().await;
        }
    }

    async fn trip(&self) {
        self.state.store(CircuitState::Open as u8, Ordering::Relaxed);
        *self.opened_at.write().await = Some(Utc::now());
    }

    fn reset(&self) {
        self.state.store(CircuitState::Closed as u8, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);
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

    pub fn snapshot(&self) -> BreakerSnapshot {
        let total = self.total.load(Ordering::Relaxed);
        let errors = self.errors.load(Ordering::Relaxed);
        BreakerSnapshot {
            state: self.state(),
            error_rate: if total == 0 { 0.0 } else { errors as f64 / total as f64 },
            p95_latency_ms: self.total_latency_ms.load(Ordering::Relaxed) / total.max(1),
            total_requests: total,
            opened_at: *self.opened_at.blocking_read(),
            last_adapted_threshold: 0.0,
        }
    }
}