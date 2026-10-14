use actix_web::HttpRequest;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::collections::HashMap;

type HmacSha1 = Hmac<Sha1>;

/// Validate form-encoded Twilio webhooks against their externally visible URL.
pub fn verify_twilio_signature(
    request: &HttpRequest,
    form: &HashMap<String, String>,
    public_base_url: &str,
    auth_token: &str,
) -> bool {
    if auth_token.trim().is_empty() {
        tracing::warn!("Twilio signature validation failed: auth token is empty");
        return false;
    }
    let Some(signature) = request
        .headers()
        .get("X-Twilio-Signature")
        .and_then(|value| value.to_str().ok())
    else {
        tracing::warn!("Twilio signature validation failed: signature header missing or invalid");
        return false;
    };
    let Ok(signature) = STANDARD.decode(signature) else {
        tracing::warn!("Twilio signature validation failed: signature header is not standard base64");
        return false;
    };

    let signed_url = format!("{}{}", public_base_url.trim_end_matches('/'), request.uri());
    let mut parameters: Vec<_> = form.iter().collect();
    parameters.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    let parameter_names: Vec<_> = parameters.iter().map(|(key, _)| key.as_str()).collect();

    let Ok(mut mac) = HmacSha1::new_from_slice(auth_token.as_bytes()) else {
        return false;
    };
    mac.update(signed_url.as_bytes());
    for (key, value) in parameters {
        mac.update(key.as_bytes());
        mac.update(value.as_bytes());
    }
    let valid = mac.verify_slice(&signature).is_ok();
    if !valid {
        tracing::warn!(
            signed_url,
            parameter_names = ?parameter_names,
            "Twilio signature mismatch"
        );
    }
    valid
}
