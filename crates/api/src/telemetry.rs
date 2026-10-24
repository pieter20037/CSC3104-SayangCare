use prometheus::{
    HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sayangcare_core::domain::{Session, SessionState};

// Existing total request counter.
static HTTP_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);

// Requests grouped by HTTP status category.
static HTTP_2XX_TOTAL: AtomicU64 = AtomicU64::new(0);
static HTTP_3XX_TOTAL: AtomicU64 = AtomicU64::new(0);
static HTTP_4XX_TOTAL: AtomicU64 = AtomicU64::new(0);
static HTTP_5XX_TOTAL: AtomicU64 = AtomicU64::new(0);

// Cumulative histogram bucket boundaries (seconds).
const LATENCY_BUCKETS: [f64; 8] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0];

static LATENCY_COUNTS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

static LATENCY_COUNT: AtomicU64 = AtomicU64::new(0);

// Store total latency in microseconds.
static LATENCY_SUM_US: AtomicU64 = AtomicU64::new(0);

pub struct HttpRouteMetrics {
    registry: Registry,
    requests: IntCounterVec,
    duration: HistogramVec,
}

impl HttpRouteMetrics {
    fn new() -> Self {
        let registry = Registry::new();

        let requests = IntCounterVec::new(
            Opts::new(
                "sayangcare_http_requests_by_route_total",
                "HTTP requests grouped by route, method and status.",
            ),
            &["route", "method", "status"],
        )
        .expect("static HTTP counter");

        let duration = HistogramVec::new(
            HistogramOpts::new(
                "sayangcare_http_request_duration_by_route_seconds",
                "HTTP request duration grouped by route and method.",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0,
                2.5, 5.0, 10.0,
            ]),
            &["route", "method"],
        )
        .expect("static HTTP histogram");

        registry.register(Box::new(requests.clone()))
            .expect("register HTTP counter");

        registry.register(Box::new(duration.clone()))
            .expect("register HTTP histogram");

        Self {
            registry,
            requests,
            duration,
        }
    }

    fn render(&self) -> String {
        TextEncoder::new()
            .encode_to_string(&self.registry.gather())
            .expect("encode HTTP metrics")
    }
}

fn http_route_metrics() -> &'static HttpRouteMetrics {
    static METRICS: OnceLock<HttpRouteMetrics> = OnceLock::new();
    METRICS.get_or_init(HttpRouteMetrics::new)
}

// Operational Metrics functions takes the sessions retrieved from Redis and returns two numbers:
// active: sessions that are still active or recently updated
// backlog: sessions that are terminated or have been inactive long enough to qualify for archival/recovery.
pub struct OperationalMetrics {
    registry: Registry,
    active_sessions: IntGauge,
    archive_backlog: IntGauge,
    circuit_breaker_state: IntGaugeVec,
}

impl OperationalMetrics {
    fn new() -> Self {
        let registry = Registry::new();

        let active_sessions = IntGauge::new(
            "sayangcare_redis_active_sessions",
            "Number of active sessions currently stored in Redis.",
        )
        .expect("static active sessions gauge");

        let archive_backlog = IntGauge::new(
            "sayangcare_archive_backlog",
            "Number of Redis sessions eligible for archival or recovery.",
        )
        .expect("static archive backlog gauge");

        let circuit_breaker_state = IntGaugeVec::new(
            Opts::new(
                "sayangcare_circuit_breaker_state",
                "Current circuit breaker state: 0=Closed, 1=Open, 2=HalfOpen.",
            ),
            &["breaker"],
        )
        .expect("static circuit breaker state gauge");

        registry
            .register(Box::new(circuit_breaker_state.clone()))
            .expect("register circuit breaker state gauge");

        registry
            .register(Box::new(active_sessions.clone()))
            .expect("register active sessions gauge");

        registry
            .register(Box::new(archive_backlog.clone()))
            .expect("register archive backlog gauge");

        Self {
            registry,
            active_sessions,
            archive_backlog,
            circuit_breaker_state,
        }
    }

    fn render(&self) -> String {
        TextEncoder::new()
            .encode_to_string(&self.registry.gather())
            .expect("encode operational metrics")
    }
}

fn operational_metrics() -> &'static OperationalMetrics {
    static METRICS: OnceLock<OperationalMetrics> = OnceLock::new();
    METRICS.get_or_init(OperationalMetrics::new)
}

pub fn record_session_metrics(active: i64, backlog: i64) {
    let metrics = operational_metrics();

    metrics.active_sessions.set(active);
    metrics.archive_backlog.set(backlog);
}

pub fn record_circuit_breaker_state(
    breaker: &str,
    state: sayangcare_circuit_breaker::CircuitState,
) {
    use sayangcare_circuit_breaker::CircuitState;

    let value = match state {
        CircuitState::Closed => 0,
        CircuitState::Open => 1,
        CircuitState::HalfOpen => 2,
    };

    operational_metrics()
        .circuit_breaker_state
        .with_label_values(&[breaker])
        .set(value);
}

const SESSION_INACTIVITY_MINUTES: i64 = 5;

pub fn classify_sessions(
    sessions: &[Session],
    now: DateTime<Utc>,
) -> (i64, i64) {
    let cutoff = now - ChronoDuration::minutes(SESSION_INACTIVITY_MINUTES);

    let mut active = 0_i64;
    let mut backlog = 0_i64;

    for session in sessions {
        match session.state {
            SessionState::Completed
            | SessionState::Failed
            | SessionState::Abandoned => {
                backlog += 1;
            }

            SessionState::Initiated
            | SessionState::Active
            | SessionState::Degraded
            | SessionState::Escalated => {
                if session.updated_at < cutoff {
                    backlog += 1;
                } else {
                    active += 1;
                }
            }
        }
    }

    (active, backlog)
}

pub fn record_http_route_request(
    route: &str,
    method: &str,
    status: u16,
    duration_seconds: f64,
) {
    let metrics = http_route_metrics();

    let status_label = status.to_string();

    metrics
        .requests
        .with_label_values(&[route, method, &status_label])
        .inc();

    if valid_duration(duration_seconds) {
        metrics
            .duration
            .with_label_values(&[route, method])
            .observe(duration_seconds);
    }
}

fn status_category(status: u16) -> Option<&'static str> {
    match status {
        200..=299 => Some("2xx"),
        300..=399 => Some("3xx"),
        400..=499 => Some("4xx"),
        500..=599 => Some("5xx"),
        _ => None,
    }
}

// Histogram helper
fn valid_duration(duration_seconds: f64) -> bool {
    duration_seconds.is_finite() && duration_seconds >= 0.0
}

fn bucket_contains(duration_seconds: f64, boundary: f64) -> bool {
    duration_seconds <= boundary
}

pub fn record_http_request(status: u16, duration_seconds: f64) {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);

    match status_category(status) {
        Some("2xx") => {
            HTTP_2XX_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        Some("3xx") => {
            HTTP_3XX_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        Some("4xx") => {
            HTTP_4XX_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        Some("5xx") => {
            HTTP_5XX_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }

    if valid_duration(duration_seconds) {
        for (index, boundary) in LATENCY_BUCKETS.iter().enumerate() {
            if bucket_contains(duration_seconds, *boundary) {
                LATENCY_COUNTS[index].fetch_add(1, Ordering::Relaxed);
            }
        }

        LATENCY_COUNT.fetch_add(1, Ordering::Relaxed);

        let micros = (duration_seconds * 1_000_000.0) as u64;
        LATENCY_SUM_US.fetch_add(micros, Ordering::Relaxed);
    }
}

pub fn prometheus_metrics() -> String {
    let mut output = format!(
        "# HELP sayangcare_http_requests_total Total HTTP requests served by this API process.\n\
         # TYPE sayangcare_http_requests_total counter\n\
         sayangcare_http_requests_total {}\n",
        HTTP_REQUESTS_TOTAL.load(Ordering::Relaxed)
    );

    output.push_str(
        "# HELP sayangcare_http_requests_by_status_total HTTP requests by status category.\n\
         # TYPE sayangcare_http_requests_by_status_total counter\n",
    );

    for (status, counter) in [
        ("2xx", &HTTP_2XX_TOTAL),
        ("3xx", &HTTP_3XX_TOTAL),
        ("4xx", &HTTP_4XX_TOTAL),
        ("5xx", &HTTP_5XX_TOTAL),
    ] {
        output.push_str(&format!(
            "sayangcare_http_requests_by_status_total{{status=\"{}\"}} {}\n",
            status,
            counter.load(Ordering::Relaxed)
        ));
    }

    output.push_str(
        "# HELP sayangcare_http_request_duration_seconds HTTP request duration in seconds.\n\
         # TYPE sayangcare_http_request_duration_seconds histogram\n",
    );

    for (index, boundary) in LATENCY_BUCKETS.iter().enumerate() {
        output.push_str(&format!(
            "sayangcare_http_request_duration_seconds_bucket{{le=\"{}\"}} {}\n",
            boundary,
            LATENCY_COUNTS[index].load(Ordering::Relaxed)
        ));
    }

    let count = LATENCY_COUNT.load(Ordering::Relaxed);

    output.push_str(&format!(
        "sayangcare_http_request_duration_seconds_bucket{{le=\"+Inf\"}} {}\n",
        count
    ));

    output.push_str(&format!(
        "sayangcare_http_request_duration_seconds_sum {}\n",
        LATENCY_SUM_US.load(Ordering::Relaxed) as f64 / 1_000_000.0
    ));

    output.push_str(&format!(
        "sayangcare_http_request_duration_seconds_count {}\n",
        count
    ));

    output.push_str(&http_route_metrics().render());
    output.push_str(&operational_metrics().render());
    output.push_str(&crate::perf_metrics::metrics().render());
    output
}

pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sayangcare=debug,actix_web=info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true).with_thread_ids(true))
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_prometheus_metrics() {
        let output = prometheus_metrics();

        assert!(output.contains("sayangcare_http_requests_total"));
        assert!(output.contains("sayangcare_http_requests_by_status_total"));
        assert!(output.contains("sayangcare_http_request_duration_seconds_bucket"));
        assert!(output.contains("sayangcare_http_request_duration_seconds_sum"));
        assert!(output.contains("sayangcare_http_request_duration_seconds_count"));
    }

    #[test]
    fn classifies_http_status_codes_correctly() {
        assert_eq!(status_category(200), Some("2xx"));
        assert_eq!(status_category(201), Some("2xx"));
        assert_eq!(status_category(302), Some("3xx"));
        assert_eq!(status_category(404), Some("4xx"));
        assert_eq!(status_category(500), Some("5xx"));
        assert_eq!(status_category(503), Some("5xx"));
        assert_eq!(status_category(199), None);
    }

    #[test]
    fn histogram_buckets_are_cumulative() {
        let duration = 0.020; // 20 milliseconds

        assert!(!bucket_contains(duration, 0.005));
        assert!(!bucket_contains(duration, 0.010));

        assert!(bucket_contains(duration, 0.025));
        assert!(bucket_contains(duration, 0.050));
        assert!(bucket_contains(duration, 0.100));

        // Exactly on a bucket boundary should be included.
        assert!(bucket_contains(0.025, 0.025));
    }

    #[test]
    fn rejects_invalid_request_durations() {
        assert!(valid_duration(0.0));
        assert!(valid_duration(0.020));

        assert!(!valid_duration(-1.0));
        assert!(!valid_duration(f64::NAN));
        assert!(!valid_duration(f64::INFINITY));
        assert!(!valid_duration(f64::NEG_INFINITY));
    }

    #[test]
    fn perf_metrics_preserve_api04_export_contract() {
        let mut timer = crate::perf_metrics::StageTimer::new("json", "total");
        timer.outcome("ok");
        drop(timer);
        let output = prometheus_metrics();
        assert!(output.contains("sayangcare_caller_turn_stage_duration_seconds_bucket"));
        assert!(output.contains("sayangcare_http_requests_by_status_total"));
        assert!(output.contains("sayangcare_http_request_duration_seconds_bucket"));
        assert_eq!(
            output
                .matches("# TYPE sayangcare_http_requests_total counter")
                .count(),
            1
        );
    }

    #[test]
    fn route_metrics_record_status_method_and_route() {
        let metrics = HttpRouteMetrics::new();

        metrics
            .requests
            .with_label_values(&["/api/v1/health", "GET", "200"])
            .inc();

        let output = metrics.render();

        assert!(output.contains(
            "sayangcare_http_requests_by_route_total{method=\"GET\",route=\"/api/v1/health\",status=\"200\"} 1"
        ));
    }

    #[test]
    fn route_histogram_records_duration() {
        let metrics = HttpRouteMetrics::new();

        metrics
            .duration
            .with_label_values(&["/api/v1/health", "GET"])
            .observe(0.020);

        let output = metrics.render();

        assert!(output.contains(
            "sayangcare_http_request_duration_by_route_seconds_bucket"
        ));
        assert!(output.contains("le=\"0.025\""));
        assert!(output.contains(
            "sayangcare_http_request_duration_by_route_seconds_count"
        ));
    }

    #[test]
    fn operational_gauges_export_expected_values() {
        let metrics = OperationalMetrics::new();

        metrics.active_sessions.set(3);
        metrics.archive_backlog.set(2);

        let output = metrics.render();

        assert!(output.contains("sayangcare_redis_active_sessions 3"));
        assert!(output.contains("sayangcare_archive_backlog 2"));
    }

    #[test]
    fn classifies_active_and_archive_backlog_sessions() {
        use sayangcare_core::domain::CallerId;

        let now = Utc::now();

        let make_session = |state: SessionState, minutes_ago: i64| {
            let caller = CallerId {
                phone_number: "+6500000000".to_string(),
                display_name: None,
            };

            let mut session = Session::new(caller);
            session.state = state;
            session.updated_at = now - ChronoDuration::minutes(minutes_ago);
            session
        };

        let sessions = vec![
            // Recently updated sessions count as active.
            make_session(SessionState::Initiated, 1),
            make_session(SessionState::Active, 2),
            make_session(SessionState::Degraded, 3),
            make_session(SessionState::Escalated, 1),

            // Terminal sessions count towards archive backlog.
            make_session(SessionState::Completed, 0),
            make_session(SessionState::Failed, 0),
            make_session(SessionState::Abandoned, 0),

            // Inactive sessions are eligible for archival.
            make_session(SessionState::Active, 10),
            make_session(SessionState::Escalated, 6),

            // Exactly five minutes old is not yet past the cutoff.
            make_session(SessionState::Active, 5),
        ];

        let (active, backlog) = classify_sessions(&sessions, now);

        assert_eq!(active, 5);
        assert_eq!(backlog, 5);
    }

    #[test]
    fn circuit_breaker_gauge_exports_states() {
        use sayangcare_circuit_breaker::CircuitState;

        let metrics = OperationalMetrics::new();

        metrics
            .circuit_breaker_state
            .with_label_values(&["llm-inference"])
            .set(0);

        let output = metrics.render();

        assert!(output.contains(
            "sayangcare_circuit_breaker_state{breaker=\"llm-inference\"} 0"
        ));

        metrics
            .circuit_breaker_state
            .with_label_values(&["llm-inference"])
            .set(1);

        let output = metrics.render();

        assert!(output.contains(
            "sayangcare_circuit_breaker_state{breaker=\"llm-inference\"} 1"
        ));

        metrics
            .circuit_breaker_state
            .with_label_values(&["llm-inference"])
            .set(2);

        let output = metrics.render();

        assert!(output.contains(
            "sayangcare_circuit_breaker_state{breaker=\"llm-inference\"} 2"
        ));

        // Confirm the public circuit-breaker state values remain consistent.
        assert_eq!(CircuitState::Closed as u8, 0);
        assert_eq!(CircuitState::Open as u8, 1);
        assert_eq!(CircuitState::HalfOpen as u8, 2);
    }
}
