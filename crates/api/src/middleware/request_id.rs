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
use uuid::Uuid;
use std::time::Instant;

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
            let elapsed = start.elapsed().as_secs_f64();

            if path != "/api/v1/metrics" {
                let status = match &response {
                    Ok(res) => res.status().as_u16(),
                    Err(err) => err.as_response_error().status_code().as_u16(),
                };

                crate::telemetry::record_http_request(status, elapsed);
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
