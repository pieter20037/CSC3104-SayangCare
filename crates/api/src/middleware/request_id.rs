//! Injects a stable X-Request-Id header into every request/response.
//! Useful for correlating logs across API pods during chaos tests.

use actix_web::{
    body::MessageBody,
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    http::header::{HeaderName, HeaderValue},
    Error, HttpMessage,
};
use futures::future::LocalBoxFuture;
use std::future::{ready, Ready};
use std::time::Instant;
use uuid::Uuid;

const HEADER: &str = "x-request-id";

#[derive(Default)]
pub struct RequestId;

impl<S, B> Transform<S, ServiceRequest> for RequestId
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = RequestIdMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(RequestIdMiddleware { service }))
    }
}

pub struct RequestIdMiddleware<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for RequestIdMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let path = req.path().to_owned();
        let method = req.method().as_str().to_owned();
        let mut turn_timer = if path == "/twilio/gather" {
            Some(crate::perf_metrics::StageTimer::new("twilio", "total"))
        } else if path.starts_with("/api/v1/calls/") && path.ends_with("/turn") {
            Some(crate::perf_metrics::StageTimer::new("json", "total"))
        } else {
            None
        };
        let incoming = req
            .headers()
            .get(HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let rid = incoming.unwrap_or_else(|| Uuid::new_v4().to_string());

        // Make it available to handlers via extensions.
        req.extensions_mut().insert(RequestIdValue(rid.clone()));

        let start = Instant::now();
        let fut = self.service.call(req);

        Box::pin(async move {
            let response = fut.await;
            if let Some(timer) = turn_timer.as_mut() {
                timer.outcome(match &response {
                    Ok(response) if response.status().is_success() => "ok",
                    Ok(response) if response.status().is_client_error() => "client_error",
                    _ => "error",
                });
            }
            let elapsed = start.elapsed().as_secs_f64();

            let route = match &response {
                Ok(res) => res.request().match_pattern(),
                Err(_) => None,
            };

            let route = route.unwrap_or_else(|| "unmatched".to_string());

            if path != "/api/v1/metrics" {
                let status = match &response {
                    Ok(res) => res.status().as_u16(),
                    Err(err) => err.as_response_error().status_code().as_u16(),
                };

                // Preserve the existing HTTP metrics.
                crate::telemetry::record_http_request(status, elapsed);

                // Record the new route-level HTTP metrics.
                crate::telemetry::record_http_route_request(&route, &method, status, elapsed);
            }

            let mut res = response?;

            if let Ok(value) = HeaderValue::from_str(&rid) {
                res.headers_mut()
                    .insert(HeaderName::from_static(HEADER), value);
            }

            Ok(res)
        })
    }
}

#[derive(Debug, Clone)]
pub struct RequestIdValue(pub String);
