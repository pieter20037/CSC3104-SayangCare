use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
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
// Internal state used when a Half-Open recovery probe is running.
const HALF_OPEN_PROBING: u8 = 3;

const STATE_BITS: u32 = 2;
const STATE_MASK: u64 = 0b11;

// Combines the generation and state into a single u64 for atomic updates.
fn pack_state(generation: u64, state: u8) -> u64 {
    (generation << STATE_BITS) | u64::from(state)
}

fn unpack_state(value: u64) -> (u64, u8) {
    let generation = value >> STATE_BITS;
    let state = (value & STATE_MASK) as u8;

    (generation, state)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPermit {
    Normal { generation: u64 },         // request entered while the circuit was Closed
    RecoveryProbe { generation: u64 },  // request reserved the Half-Open recovery slot
}

impl From<u8> for CircuitState {
    fn from(v: u8) -> Self {
        match v {
            1 => CircuitState::Open,
            2 | HALF_OPEN_PROBING => CircuitState::HalfOpen,
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
    packed_state: AtomicU64,
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
            packed_state: AtomicU64::new(pack_state(0, CircuitState::Closed as u8)),
            metrics: Mutex::new(WindowMetrics::default()),
            opened_at: RwLock::new(None),
            policy,
        }
    }

    pub fn state(&self) -> CircuitState {
        let packed = self.packed_state.load(Ordering::Acquire);
        let (_, state) = unpack_state(packed);

        CircuitState::from(state)
    }

    // Helper to read the combined state.
    fn current_state_and_generation(&self) -> (u64, u8) {
        unpack_state(self.packed_state.load(Ordering::Acquire))
    }

    /// Fast pre-check. Returns `Err` if circuit is open.
    pub fn acquire(&self) -> Result<(), sayangcare_core::CoreError> {
        self.acquire_with_permit().map(|_| ())
    }

    // To remember whether an LLM request is a normal request or a recovery test.
    pub fn acquire_with_permit(
        &self,
    ) -> Result<RequestPermit, sayangcare_core::CoreError> {
        let circuit_open = || {
            sayangcare_core::CoreError::CircuitOpen("llm-inference".into())
        };

        loop {
            let packed = self.packed_state.load(Ordering::Acquire);
            let (generation, state) = unpack_state(packed);

            match state {
                0 => {
                    return Ok(RequestPermit::Normal { generation });
                }

                1 => {
                    return Err(circuit_open());
                }

                2 => {
                    // Reserve the recovery slot without changing its generation.
                    let probing = pack_state(generation, HALF_OPEN_PROBING);

                    if self
                        .packed_state
                        .compare_exchange(
                            packed,
                            probing,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return Ok(RequestPermit::RecoveryProbe { generation });
                    }

                    // Another thread changed the state. Retry.
                    continue;
                }

                HALF_OPEN_PROBING => {
                    return Err(circuit_open());
                }

                _ => {
                    return Err(circuit_open());
                }
            }
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

    pub fn record_success_with_permit(
        &self,
        permit: RequestPermit,
        latency_ms: u64,
    ) {
        match permit {
            RequestPermit::Normal {
                generation: request_generation,
            } => {
                // Lock metrics before checking the circuit generation.
                let mut metrics = self.metrics.lock().unwrap();

                let current = self.packed_state.load(Ordering::Acquire);
                let (generation, state) = unpack_state(current);

                // Ignore results from an outdated generation
                // or requests that are no longer in Closed state.
                if request_generation != generation
                    || state != CircuitState::Closed as u8
                {
                    return;
                }

                metrics.record(
                    latency_ms,
                    true,
                    self.config.window_size as usize,
                );
            }

            RequestPermit::RecoveryProbe {
                generation: request_generation,
            } => {
                // Lock metrics before transitioning so the new
                // Closed state starts with a clean window.
                let mut metrics = self.metrics.lock().unwrap();

                let expected = pack_state(
                    request_generation,
                    HALF_OPEN_PROBING,
                );

                let next = pack_state(
                    request_generation,
                    CircuitState::Closed as u8,
                );

                // Close only if this exact recovery probe is still active.
                if self
                    .packed_state
                    .compare_exchange(
                        expected,
                        next,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    metrics.records.clear();
                }
            }
        }
    }

    /// CB-04: successful requests also evaluate sentiment-adjusted latency.
    pub async fn record_success_with_distress(
        &self,
        permit: RequestPermit,
        latency_ms: u64,
        distress_severity: f32,
    ) {
        let (error_threshold, latency_threshold) = self.policy.thresholds(distress_severity);
        if let RequestPermit::RecoveryProbe { generation } = permit {
            // A slow probe is not healthy enough to resume normal traffic.
            if latency_ms >= latency_threshold {
                self.trip_if_current(generation, HALF_OPEN_PROBING).await;
            } else {
                self.record_success_with_permit(permit, latency_ms);
            }
            return;
        }
        self.record_success_with_permit(permit, latency_ms);
        let RequestPermit::Normal { generation } = permit else {
            return;
        };
        let (total, error_rate, p95) = {
            let metrics = self.metrics.lock().unwrap();
            let (current_generation, state) = self.current_state_and_generation();
            if current_generation != generation || state != CircuitState::Closed as u8 {
                return;
            }
            metrics.snapshot()
        };
        if total >= self.config.min_requests
            && (error_rate >= error_threshold || p95 >= latency_threshold)
        {
            self.trip_if_current(generation, CircuitState::Closed as u8)
                .await;
        }
    }

    /// Record a failed call. May trip the breaker.
    pub async fn record_failure(&self, latency_ms: u64, distress_severity: f32) {
        let (total, error_rate, p95_latency) = {
            let mut metrics = self.metrics.lock().unwrap();

            metrics.record(latency_ms, false, self.config.window_size as usize);

            metrics.snapshot()
        };
        // Check if the circuit is Half-Open before applying the normal error-rate thresholds.
        if self.state() == CircuitState::HalfOpen {
            warn!("recovery probe failed; reopening circuit breaker");
            self.trip().await;
            return;
        }

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

    pub async fn record_failure_with_permit(
        &self,
        permit: RequestPermit,
        latency_ms: u64,
        distress_severity: f32,
    ) {
        let current = self.packed_state.load(Ordering::Acquire);
        let (generation, state) = unpack_state(current);

        match permit {
            RequestPermit::Normal {
                generation: request_generation,
            } => {
                let (total, error_rate, p95_latency) = {
                    // Lock metrics before checking the generation.
                    let mut metrics = self.metrics.lock().unwrap();

                    let current = self.packed_state.load(Ordering::Acquire);
                    let (generation, state) = unpack_state(current);

                    // Ignore failures from an outdated generation
                    // or requests that are no longer in Closed state.
                    if request_generation != generation
                        || state != CircuitState::Closed as u8
                    {
                        return;
                    }

                    metrics.record(
                        latency_ms,
                        false,
                        self.config.window_size as usize,
                    );

                    metrics.snapshot()
                };

                if total < self.config.min_requests {
                    return;
                }

                let (err_threshold, lat_threshold) =
                    self.policy.thresholds(distress_severity);

                if error_rate >= err_threshold || p95_latency >= lat_threshold {
                    warn!(
                        error_rate,
                        p95_latency,
                        err_threshold,
                        lat_threshold,
                        distress_severity,
                        "circuit breaker tripping"
                    );

                    self.trip_if_current(
                        request_generation,
                        CircuitState::Closed as u8,
                    )
                    .await;
                }
            }

            RequestPermit::RecoveryProbe {
                generation: request_generation,
            } => {
                // Only the current recovery probe can reopen the circuit.
                if request_generation != generation || state != HALF_OPEN_PROBING {
                    return;
                }

                if self
                    .trip_if_current(request_generation, HALF_OPEN_PROBING)
                    .await
                {
                    warn!("recovery probe failed; reopening circuit breaker");
                }
            }
        }
    }

    /// Reset the flag when the circuit opens again and records the start of the new cooldown.
    async fn trip(&self) {
        let mut opened_at = self.opened_at.write().await;

        loop {
            let current = self.packed_state.load(Ordering::Acquire);
            let (generation, state) = unpack_state(current);

            // The circuit is already Open.
            if state == CircuitState::Open as u8 {
                return;
            }

            let next_generation = (generation + 1) & ((1u64 << 62) - 1);

            let next = pack_state(
                next_generation,
                CircuitState::Open as u8,
            );

            if self
                .packed_state
                .compare_exchange(
                    current,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                *opened_at = Some(Utc::now());
                return;
            }
        }
    }

    // Conditional trip helper: accepts two values (expected_generation and expected_state)
    async fn trip_if_current(
        &self,
        expected_generation: u64,
        expected_state: u8,
    ) -> bool {
        let mut opened_at = self.opened_at.write().await;

        let expected = pack_state(expected_generation, expected_state);

        let next_generation =
            (expected_generation + 1) & ((1u64 << 62) - 1);

        let next = pack_state(
            next_generation,
            CircuitState::Open as u8,
        );

        if self
            .packed_state
            .compare_exchange(
                expected,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            *opened_at = Some(Utc::now());
            true
        } else {
            false
        }
    }

    /// Reset the breaker to Closed state and clear metrics.
    fn reset(&self) {
        loop {
            let current = self.packed_state.load(Ordering::Acquire);
            let (generation, state) = unpack_state(current);

            // Only an active recovery probe should lead to Closed.
            if state != HALF_OPEN_PROBING {
                return;
            }

            let next = pack_state(generation, CircuitState::Closed as u8);

            if self
                .packed_state
                .compare_exchange(
                    current,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                self.metrics.lock().unwrap().records.clear();
                return;
            }
        }
    }

    /// Background task: transition Open -> HalfOpen after cooldown.
    pub async fn tick(&self) {
        let opened_at = self.opened_at.read().await;

        let Some(opened) = *opened_at else {
            return;
        };

        let elapsed = (Utc::now() - opened).num_seconds();

        if elapsed < self.config.half_open_after_secs as i64 {
            return;
        }

        loop {
            let current = self.packed_state.load(Ordering::Acquire);
            let (generation, state) = unpack_state(current);

            // Only Open can transition to Half-Open.
            if state != CircuitState::Open as u8 {
                return;
            }

            let next = pack_state(
                generation,
                CircuitState::HalfOpen as u8,
            );

            if self
                .packed_state
                .compare_exchange(
                    current,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                info!("circuit breaker -> half-open");
                return;
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

    fn adaptive_test_breaker() -> CircuitBreaker {
        CircuitBreaker::new(
            BreakerConfig {
                base_error_rate: 0.25,
                base_latency_ms: 1000,
                min_requests: 1,
                half_open_after_secs: 0,
                window_size: 10,
            },
            Arc::new(crate::SentimentAdaptivePolicy::new(0.25, 1000, 0.4)),
        )
    }

    #[tokio::test]
    async fn successful_latency_trips_sooner_for_distressed_caller() {
        let calm = adaptive_test_breaker();
        let distressed = adaptive_test_breaker();
        calm.record_success_with_distress(calm.acquire_with_permit().unwrap(), 850, 0.0)
            .await;
        distressed
            .record_success_with_distress(distressed.acquire_with_permit().unwrap(), 850, 0.5)
            .await;
        assert_eq!(calm.state(), CircuitState::Closed);
        assert_eq!(distressed.state(), CircuitState::Open);
    }

    #[tokio::test]
    async fn slow_recovery_probe_reopens_and_fast_probe_closes() {
        let breaker = adaptive_test_breaker();
        breaker
            .record_failure_with_permit(breaker.acquire_with_permit().unwrap(), 100, 0.0)
            .await;
        breaker.tick().await;
        let probe = breaker.acquire_with_permit().unwrap();
        breaker.record_success_with_distress(probe, 850, 0.5).await;
        assert_eq!(breaker.state(), CircuitState::Open);
        breaker.tick().await;
        let probe = breaker.acquire_with_permit().unwrap();
        breaker.record_success_with_distress(probe, 100, 0.5).await;
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn stale_success_with_distress_cannot_trip_new_generation() {
        let breaker = adaptive_test_breaker();
        let old = breaker.acquire_with_permit().unwrap();
        breaker.record_failure_with_permit(old, 100, 0.0).await;
        breaker.tick().await;
        let probe = breaker.acquire_with_permit().unwrap();
        breaker.record_success_with_distress(probe, 100, 0.0).await;
        breaker.record_success_with_distress(old, 2000, 1.0).await;
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

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

    // This test ensures that only the first request can reserve the Half-Open recovery probe.
    #[test]
    fn half_open_allows_only_one_recovery_probe() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Simulate the breaker entering Half-Open.
        breaker.packed_state.store(
            pack_state(0, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // First caller should be allowed.
        assert!(breaker.acquire().is_ok());

        // Second caller should be blocked.
        assert!(breaker.acquire().is_err());

        // Third caller should also be blocked.
        assert!(breaker.acquire().is_err());
    }

    // This test verifies that the CB returns to Closed state after a successful recovery request.
    #[test]
    fn successful_recovery_probe_closes_circuit() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Simulate the circuit entering Half-Open.
        breaker.packed_state.store(
            pack_state(0, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // Allow one recovery request.
        assert!(breaker.acquire().is_ok());

        // Simulate a successful LLM response.
        breaker.record_success(150);

        // The circuit should return to Closed.
        assert_eq!(breaker.state(), CircuitState::Closed);

        // Normal requests should be allowed again.
        assert!(breaker.acquire().is_ok());
    }

    // Using #[tokio::test] because record_failure() is async and requires .await.
    #[tokio::test]
    async fn failed_recovery_probe_reopens_circuit() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Simulate entering Half-Open.
        breaker.packed_state.store(
            pack_state(0, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // Allow the recovery request.
        assert!(breaker.acquire().is_ok());

        // Simulate the LLM failing during recovery.
        breaker.record_failure(150, 0.25).await;

        // The circuit should reopen immediately.
        assert_eq!(breaker.state(), CircuitState::Open);

        // Further requests should be blocked.
        assert!(breaker.acquire().is_err());

        // A new cooldown timestamp should be recorded.
        assert!(breaker.opened_at.read().await.is_some());
    }

    #[tokio::test]
    async fn failed_recovery_allows_another_probe_after_cooldown() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 0,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Simulate Half-Open.
        breaker.packed_state.store(
            pack_state(0, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // First recovery request is allowed.
        assert!(breaker.acquire().is_ok());

        // Internal state should indicate a probe is running.
        assert_eq!(
            unpack_state(breaker.packed_state.load(Ordering::Acquire)).1,
            HALF_OPEN_PROBING
        );

        // Simulate failed recovery.
        breaker.record_failure(150, 0.25).await;

        assert_eq!(breaker.state(), CircuitState::Open);

        // Cooldown is zero seconds for testing.
        breaker.tick().await;

        assert_eq!(breaker.state(), CircuitState::HalfOpen);

        // A new recovery request should be allowed.
        assert!(breaker.acquire().is_ok());

        assert_eq!(
            unpack_state(breaker.packed_state.load(Ordering::Acquire)).1,
            HALF_OPEN_PROBING
        );
    }

    // Concurrency test: to check whether the CB can handle multiple callers trying to access the LLM at the same time.
    #[test]
    fn concurrent_half_open_requests_allow_only_one_probe() {
        use std::sync::Barrier;
        use std::thread;

        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = Arc::new(CircuitBreaker::new(
            config,
            Arc::new(TestPolicy),
        ));

        // Simulate entering Half-Open.
        breaker.packed_state.store(
            pack_state(0, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        let num_callers = 10;
        let barrier = Arc::new(Barrier::new(num_callers));

        let handles: Vec<_> = (0..num_callers)
            .map(|_| {
                let breaker = Arc::clone(&breaker);
                let barrier = Arc::clone(&barrier);

                thread::spawn(move || {
                    // All threads begin attempting recovery together.
                    barrier.wait();
                    breaker.acquire().is_ok()
                })
            })
            .collect();

        let allowed = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|&success| success)
            .count();

        // Exactly one caller should acquire the recovery slot.
        assert_eq!(allowed, 1);

        // The internal state should indicate a probe in progress.
        assert_eq!(
            unpack_state(breaker.packed_state.load(Ordering::Acquire)).1,
            HALF_OPEN_PROBING
        );
    }

    #[test]
    fn acquire_returns_correct_request_permit() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Normal request while Closed.
        assert_eq!(
            breaker.acquire_with_permit().unwrap(),
            RequestPermit::Normal { generation: 0 }
        );

        // Simulate Half-Open.
        breaker.packed_state.store(
            pack_state(0, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // First request becomes the recovery probe.
        assert_eq!(
            breaker.acquire_with_permit().unwrap(),
            RequestPermit::RecoveryProbe { generation: 0 }
        );

        // Second request must be blocked.
        assert!(breaker.acquire_with_permit().is_err());
    }

    // To verify that packing and unpacking preserves both the generation and state correctly.
    #[test]
    fn packed_state_preserves_generation_and_state() {
        let generation = 42;

        for state in 0..=HALF_OPEN_PROBING {
            let packed = pack_state(generation, state);
            let (decoded_generation, decoded_state) = unpack_state(packed);

            assert_eq!(decoded_generation, generation);
            assert_eq!(decoded_state, state);
        }
    }

    #[test]
    fn old_normal_request_must_not_close_half_open_circuit() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Caller A starts while the circuit is Closed.
        let old_permit = breaker.acquire_with_permit().unwrap();

        assert_eq!(
            old_permit,
            RequestPermit::Normal { generation: 0 }
        );

        // Simulate a later recovery cycle.
        breaker.packed_state.store(
            pack_state(1, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // Caller B reserves the recovery slot.
        let recovery_permit = breaker.acquire_with_permit().unwrap();

        assert_eq!(
            recovery_permit,
            RequestPermit::RecoveryProbe { generation: 1 }
        );

        // Caller A's old LLM response finally succeeds.
        breaker.record_success_with_permit(old_permit, 150);

        // Caller A must NOT close Caller B's recovery attempt.
        assert_eq!(
            breaker.state(),
            CircuitState::HalfOpen
        );
    }

    #[tokio::test]
    async fn old_normal_failure_must_not_reopen_half_open_circuit() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Caller A starts while the circuit is Closed.
        let old_permit = breaker.acquire_with_permit().unwrap();

        assert_eq!(
            old_permit,
            RequestPermit::Normal { generation: 0 }
        );

        // Simulate the circuit entering a newer recovery cycle.
        breaker.packed_state.store(
            pack_state(1, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // Caller B reserves the recovery slot.
        let recovery_permit = breaker.acquire_with_permit().unwrap();

        assert_eq!(
            recovery_permit,
            RequestPermit::RecoveryProbe { generation: 1 }
        );

        // Caller A's old request finally fails.
        breaker
            .record_failure_with_permit(old_permit, 150, 0.25)
            .await;

        // Caller A must not reopen Caller B's recovery attempt.
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn valid_recovery_probe_closes_circuit_with_permit() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Simulate Half-Open in generation 1.
        breaker.packed_state.store(
            pack_state(1, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // Reserve the recovery probe.
        let permit = breaker.acquire_with_permit().unwrap();

        assert_eq!(
            permit,
            RequestPermit::RecoveryProbe { generation: 1 }
        );

        // The legitimate recovery probe succeeds.
        breaker.record_success_with_permit(permit, 150);

        // The circuit should return to Closed.
        assert_eq!(breaker.state(), CircuitState::Closed);

        // Normal requests should be allowed again.
        assert!(breaker.acquire_with_permit().is_ok());
    }

    // Test if the helper (trip_if_current) rejects outdated failures.
    #[tokio::test]
    async fn outdated_failure_cannot_trip_new_generation() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Simulate a newer circuit generation that has recovered.
        breaker.packed_state.store(
            pack_state(2, CircuitState::Closed as u8),
            Ordering::Release,
        );

        // An old failure from generation 1 tries to trip the circuit.
        let tripped = breaker
            .trip_if_current(1, CircuitState::Closed as u8)
            .await;

        // The outdated failure must be rejected.
        assert!(!tripped);

        // The circuit must remain Closed in generation 2.
        assert_eq!(breaker.state(), CircuitState::Closed);

        let (generation, _) = breaker.current_state_and_generation();
        assert_eq!(generation, 2);
    }

    // This test simulates the state transitions directly rather than using concurrent threads, so it's a focused regression test for
    // stale recovery processes.
    #[test]
    fn outdated_recovery_probe_cannot_close_new_generation() {
        struct TestPolicy;

        impl AdaptiveThresholdPolicy for TestPolicy {
            fn thresholds(&self, _distress_severity: f32) -> (f64, u64) {
                (0.5, 1000)
            }
        }

        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 1000,
            min_requests: 3,
            half_open_after_secs: 5,
            window_size: 10,
        };

        let breaker = CircuitBreaker::new(config, Arc::new(TestPolicy));

        // Generation 1 enters Half-Open.
        breaker.packed_state.store(
            pack_state(1, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // Caller A reserves the recovery probe for generation 1.
        let old_permit = breaker.acquire_with_permit().unwrap();

        assert_eq!(
            old_permit,
            RequestPermit::RecoveryProbe { generation: 1 }
        );

        // Simulate a newer recovery cycle in generation 2.
        breaker.packed_state.store(
            pack_state(2, CircuitState::HalfOpen as u8),
            Ordering::Release,
        );

        // Caller B reserves the new recovery probe.
        let new_permit = breaker.acquire_with_permit().unwrap();

        assert_eq!(
            new_permit,
            RequestPermit::RecoveryProbe { generation: 2 }
        );

        // Caller A's outdated recovery request finally succeeds.
        breaker.record_success_with_permit(old_permit, 150);

        // Caller A must NOT close Caller B's recovery attempt.
        assert_eq!(breaker.state(), CircuitState::HalfOpen);

        assert_eq!(
            unpack_state(breaker.packed_state.load(Ordering::Acquire)),
            (2, HALF_OPEN_PROBING)
        );

        // Caller B succeeds and legitimately closes the circuit.
        breaker.record_success_with_permit(new_permit, 150);

        assert_eq!(breaker.state(), CircuitState::Closed);
    }
}
