use std::sync::atomic::{AtomicU64, Ordering};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

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
}
