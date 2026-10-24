//! PERF-01/02 collectors appended to API-04's existing Prometheus exporter.
use prometheus::{
    HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};
use std::sync::OnceLock;
use std::time::Instant;

pub struct Metrics {
    pub registry: Registry,
    pub stages: HistogramVec,
    pub fallbacks: IntCounterVec,
    pub ai_in_flight: IntGauge,
    pub pg_size: IntGauge,
    pub pg_idle: IntGauge,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let stages = HistogramVec::new(
            HistogramOpts::new(
                "sayangcare_caller_turn_stage_duration_seconds",
                "Caller turn stage durations; total includes all measured stages.",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.2, 0.5, 0.8, 1.0, 2.0, 3.0, 4.0, 5.0, 8.0,
                12.0, 24.0,
            ]),
            &["path", "stage", "outcome"],
        )
        .expect("static histogram");
        let fallbacks = IntCounterVec::new(
            Opts::new(
                "sayangcare_ai_fallback_total",
                "AI fallback decisions by bounded reason.",
            ),
            &["reason"],
        )
        .expect("static counter");
        let ai_in_flight = IntGauge::new(
            "sayangcare_ai_turns_in_flight",
            "AI turn slots occupied in this API process.",
        )
        .unwrap();
        let pg_size = IntGauge::new(
            "sayangcare_postgres_pool_connections",
            "Connections in this API process pool at last scrape.",
        )
        .unwrap();
        let pg_idle = IntGauge::new(
            "sayangcare_postgres_pool_idle",
            "Idle connections in this API process pool at last scrape.",
        )
        .unwrap();
        registry.register(Box::new(stages.clone())).unwrap();
        registry.register(Box::new(fallbacks.clone())).unwrap();
        registry.register(Box::new(ai_in_flight.clone())).unwrap();
        registry.register(Box::new(pg_size.clone())).unwrap();
        registry.register(Box::new(pg_idle.clone())).unwrap();
        Self {
            registry,
            stages,
            fallbacks,
            ai_in_flight,
            pg_size,
            pg_idle,
        }
    }

    pub fn render(&self) -> String {
        TextEncoder::new()
            .encode_to_string(&self.registry.gather())
            .expect("encode registered metrics")
    }
}

pub fn metrics() -> &'static Metrics {
    static METRICS: OnceLock<Metrics> = OnceLock::new();
    METRICS.get_or_init(Metrics::new)
}

pub struct StageTimer {
    started: Instant,
    path: &'static str,
    stage: &'static str,
    outcome: &'static str,
}

impl StageTimer {
    pub fn new(path: &'static str, stage: &'static str) -> Self {
        Self {
            started: Instant::now(),
            path,
            stage,
            outcome: "cancelled",
        }
    }
    pub fn outcome(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        let seconds = self.started.elapsed().as_secs_f64();
        metrics()
            .stages
            .with_label_values(&[self.path, self.stage, self.outcome])
            .observe(seconds);
        tracing::debug!(
            path = self.path,
            stage = self.stage,
            outcome = self.outcome,
            seconds,
            "caller turn timing"
        );
    }
}

pub fn fallback(reason: &'static str) {
    metrics().fallbacks.with_label_values(&[reason]).inc();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_is_extensible_and_histograms_export_buckets() {
        let metrics = Metrics::new();
        metrics
            .stages
            .with_label_values(&["json", "reply", "ok"])
            .observe(0.15);
        let api04 =
            prometheus::IntCounter::new("example_api04_requests_total", "API04 integration test")
                .unwrap();
        metrics.registry.register(Box::new(api04.clone())).unwrap();
        api04.inc();
        let text = metrics.render();
        assert!(text.contains("sayangcare_caller_turn_stage_duration_seconds_bucket"));
        assert!(text.contains("stage=\"reply\""));
        assert!(text.contains("le=\"+Inf\""));
        assert!(text.contains("example_api04_requests_total 1"));
    }
}
