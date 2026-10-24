use crate::perf_metrics::{self, StageTimer};
use sayangcare_core::{config::PerformanceConfig, CoreError, CoreResult};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{timeout_at, Duration, Instant};

pub struct AiSlots(pub Arc<Semaphore>);
impl AiSlots {
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(Semaphore::new(limit)))
    }
    pub fn try_enter(&self) -> Option<AiSlot> {
        let permit = self.0.clone().try_acquire_owned().ok()?;
        perf_metrics::metrics().ai_in_flight.inc();
        Some(AiSlot { _permit: permit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test(start_paused = true)]
    async fn shared_deadline_leaves_persistence_reserve() {
        let config = PerformanceConfig {
            response_budget_ms: 100,
            persistence_reserve_ms: 20,
            ..Default::default()
        };
        let budget = TurnBudget::new("test", &config);
        let result = budget
            .ai("sentiment", 500, async {
                tokio::time::sleep(Duration::from_millis(60)).await;
                Ok(())
            })
            .await;
        assert!(result.is_ok());
        let started = Instant::now();
        assert!(budget
            .ai("reply", 500, async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(())
            })
            .await
            .is_err());
        assert!(started.elapsed() <= Duration::from_millis(21));
        assert!(budget
            .storage("storage_write", async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(())
            })
            .await
            .is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_drops_work_and_returns_slot() {
        struct OnDrop(Arc<AtomicBool>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let slots = AiSlots::new(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let slot = slots.try_enter().unwrap();
        assert!(slots.try_enter().is_none());
        let budget = TurnBudget::new("test", &PerformanceConfig::default());
        let result: CoreResult<()> = budget
            .ai("reply", 10, async move {
                let _drop = OnDrop(flag);
                let _slot = slot;
                std::future::pending().await
            })
            .await;
        assert!(result.is_err());
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(slots.try_enter().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn expired_budget_does_not_start_dependency_work() {
        let budget = TurnBudget::new("test", &PerformanceConfig::default());
        tokio::time::sleep(Duration::from_secs(6)).await;
        let started = AtomicBool::new(false);
        let result = budget
            .storage("storage_read", async {
                started.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await;
        assert!(result.is_err());
        assert!(!started.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn slow_alert_is_bounded_and_session_write_retains_reserve() {
        let config = PerformanceConfig {
            response_budget_ms: 100,
            persistence_reserve_ms: 30,
            coordination_timeout_ms: 10,
            ..Default::default()
        };
        let budget = TurnBudget::new("test", &config);
        let started = Instant::now();
        assert!(budget
            .auxiliary("alerts", std::future::pending::<CoreResult<()>>())
            .await
            .is_err());
        assert!(started.elapsed() <= Duration::from_millis(11));
        assert!(budget
            .storage("storage_write", async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(())
            })
            .await
            .is_ok());
    }

    #[test]
    fn invalid_limits_are_rejected() {
        assert!(PerformanceConfig {
            max_ai_turns: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(PerformanceConfig {
            persistence_reserve_ms: 5000,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(PerformanceConfig::default().validate().is_ok());
    }
}
pub struct AiSlot {
    _permit: OwnedSemaphorePermit,
}
impl Drop for AiSlot {
    fn drop(&mut self) {
        perf_metrics::metrics().ai_in_flight.dec();
    }
}

/// One absolute application deadline; AI leaves a reserve for session persistence.
#[derive(Clone)]
pub struct TurnBudget {
    pub path: &'static str,
    pub deadline: Instant,
    pub ai_deadline: Instant,
    pub limits: PerformanceConfig,
}
impl TurnBudget {
    pub fn new(path: &'static str, limits: &PerformanceConfig) -> Self {
        let deadline = Instant::now() + Duration::from_millis(limits.response_budget_ms);
        Self {
            path,
            deadline,
            ai_deadline: deadline - Duration::from_millis(limits.persistence_reserve_ms),
            limits: limits.clone(),
        }
    }
    pub fn ai_expired(&self) -> bool {
        Instant::now() >= self.ai_deadline
    }
    pub async fn stage<T>(
        &self,
        stage: &'static str,
        deadline: Instant,
        work: impl Future<Output = CoreResult<T>>,
    ) -> CoreResult<T> {
        let mut timer = StageTimer::new(self.path, stage);
        if Instant::now() >= deadline {
            timer.outcome("timeout");
            return Err(CoreError::DeadlineExceeded(stage));
        }
        match timeout_at(deadline, work).await {
            Ok(result) => {
                timer.outcome(if result.is_ok() { "ok" } else { "error" });
                result
            }
            Err(_) => {
                timer.outcome("timeout");
                Err(CoreError::DeadlineExceeded(stage))
            }
        }
    }
    pub async fn ai<T>(
        &self,
        stage: &'static str,
        cap_ms: u64,
        work: impl Future<Output = CoreResult<T>>,
    ) -> CoreResult<T> {
        let deadline = self
            .ai_deadline
            .min(Instant::now() + Duration::from_millis(cap_ms));
        self.stage(stage, deadline, work).await
    }
    pub async fn storage<T>(
        &self,
        stage: &'static str,
        work: impl Future<Output = CoreResult<T>>,
    ) -> CoreResult<T> {
        self.stage(stage, self.deadline, work).await
    }

    /// Bound best-effort alerts/queue work while preserving session-write time.
    pub async fn auxiliary<T>(
        &self,
        stage: &'static str,
        work: impl Future<Output = CoreResult<T>>,
    ) -> CoreResult<T> {
        self.ai(stage, self.limits.coordination_timeout_ms, work)
            .await
    }
}
