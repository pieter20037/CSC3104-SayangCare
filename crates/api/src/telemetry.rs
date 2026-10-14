use std::sync::atomic::{AtomicU64, Ordering};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static HTTP_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);

pub fn record_http_request() {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn prometheus_metrics() -> String {
    render_prometheus_metrics(HTTP_REQUESTS_TOTAL.load(Ordering::Relaxed))
}

fn render_prometheus_metrics(requests_total: u64) -> String {
    format!(
        "# HELP sayangcare_http_requests_total Total HTTP requests served by this API process.\n# TYPE sayangcare_http_requests_total counter\nsayangcare_http_requests_total {requests_total}\n"
    )
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
    use super::render_prometheus_metrics;

    #[test]
    fn renders_http_request_counter_in_prometheus_format() {
        assert_eq!(
            render_prometheus_metrics(7),
            "# HELP sayangcare_http_requests_total Total HTTP requests served by this API process.\n# TYPE sayangcare_http_requests_total counter\nsayangcare_http_requests_total 7\n"
        );
    }
}
