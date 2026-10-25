use async_trait::async_trait;
use reqwest::Client;
use sayangcare_core::ports::TelephonyProvider;
use sayangcare_core::{CoreError, CoreResult};
use serde::Deserialize;
use std::time::Duration;

const REST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct TwilioProvider {
    client: Client,
    account_sid: String,
    auth_token: String,
    base_url: String,
}

#[derive(Deserialize)]
struct CreateCallResponse {
    sid: String,
}

impl TwilioProvider {
    pub fn new(account_sid: String, auth_token: String) -> Self {
        Self {
            client: Client::new(),
            account_sid,
            auth_token,
            base_url: "https://api.twilio.com/2010-04-01".into(),
        }
    }

    fn has_live_credentials(&self) -> bool {
        let sid = self.account_sid.trim();
        let token = self.auth_token.trim();

        !sid.is_empty()
            && !token.is_empty()
            && !sid.eq_ignore_ascii_case("ACtest")
            && !token.eq_ignore_ascii_case("test")
    }

    async fn update_call(&self, call_sid: &str, params: &[(&str, &str)]) -> CoreResult<()> {
        if !self.has_live_credentials() {
            return Err(CoreError::Telephony(
                "Twilio live handoff is not configured. Set real SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID and SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN values before redirecting a call.".to_string(),
            ));
        }

        let url = format!(
            "{}/Accounts/{}/Calls/{}.json",
            self.base_url, self.account_sid, call_sid
        );
        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.account_sid, Some(&self.auth_token))
            .form(params)
            .timeout(REST_TIMEOUT)
            .send()
            .await
            .map_err(|e| CoreError::Telephony(format!("twilio: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.map_err(|error| {
                CoreError::Telephony(format!("read Twilio HTTP {status} error response: {error}"))
            })?;
            return Err(CoreError::Telephony(format!(
                "twilio HTTP {status}: {body}"
            )));
        }
        Ok(())
    }

    async fn create_call(&self, params: &[(&str, &str)]) -> CoreResult<String> {
        if !self.has_live_credentials() {
            return Err(CoreError::Telephony(
                "Twilio outbound calling is not configured. Set real Account SID and Auth Token credentials.".to_string(),
            ));
        }

        let url = format!("{}/Accounts/{}/Calls.json", self.base_url, self.account_sid);
        let response = self
            .client
            .post(&url)
            .basic_auth(&self.account_sid, Some(&self.auth_token))
            .form(params)
            .timeout(REST_TIMEOUT)
            .send()
            .await
            .map_err(|error| CoreError::Telephony(format!("twilio outbound call: {error}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.map_err(|error| {
                CoreError::Telephony(format!("read Twilio HTTP {status} error response: {error}"))
            })?;
            return Err(CoreError::Telephony(format!(
                "Twilio outbound call returned HTTP {status}: {body}"
            )));
        }

        let call = response
            .json::<CreateCallResponse>()
            .await
            .map_err(|error| {
                CoreError::Telephony(format!("decode Twilio call response: {error}"))
            })?;
        if call.sid.trim().is_empty() {
            return Err(CoreError::Telephony(
                "Twilio returned an empty outbound Call SID".to_string(),
            ));
        }
        Ok(call.sid)
    }
}

#[async_trait]
impl TelephonyProvider for TwilioProvider {
    async fn play_audio(&self, call_sid: &str, audio_url: &str) -> CoreResult<()> {
        let twiml = format!(r#"<Response><Play>{}</Play></Response>"#, audio_url);
        self.update_call(call_sid, &[("Twiml", twiml.as_str())])
            .await
    }

    async fn hangup(&self, call_sid: &str) -> CoreResult<()> {
        self.update_call(call_sid, &[("Status", "completed")]).await
    }

    async fn redirect_to_human(&self, call_sid: &str, volunteer_id: &str) -> CoreResult<()> {
        let volunteer_phone = if volunteer_id == "volunteer-aisha" {
            "+6590001001"
        } else if volunteer_id == "volunteer-noor" {
            "+6590001002"
        } else if volunteer_id == "volunteer-ibrahim" {
            "+6590001003"
        } else {
            volunteer_id
        };

        let twiml = format!(
            r#"<Response><Say>Connecting you to a volunteer now.</Say>
               <Dial>{}</Dial></Response>"#,
            volunteer_phone
        );
        self.update_call(call_sid, &[("Twiml", twiml.as_str())])
            .await
    }

    async fn register_status_callback(&self, call_sid: &str, callback_url: &str) -> CoreResult<()> {
        self.update_call(
            call_sid,
            &[
                ("StatusCallback", callback_url),
                ("StatusCallbackMethod", "POST"),
                ("StatusCallbackEvent", "completed"),
            ],
        )
        .await
    }

    async fn move_call_to_conference(&self, call_sid: &str, conference: &str) -> CoreResult<()> {
        let room = xml_escape(conference);
        let twiml = format!(
            r#"<Response><Dial><Conference startConferenceOnEnter="true" endConferenceOnExit="true">{room}</Conference></Dial></Response>"#
        );
        self.update_call(call_sid, &[("Twiml", twiml.as_str())])
            .await
    }

    async fn create_outbound_call(
        &self,
        to_number: &str,
        from_number: &str,
        twiml_url: &str,
    ) -> CoreResult<String> {
        if to_number.trim().is_empty()
            || from_number.trim().is_empty()
            || twiml_url.trim().is_empty()
        {
            return Err(CoreError::Telephony(
                "Twilio outbound call requires non-empty To, From, and TwiML URL values"
                    .to_string(),
            ));
        }
        self.create_call(&[
            ("To", to_number),
            ("From", from_number),
            ("Url", twiml_url),
            ("Method", "POST"),
        ])
        .await
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::TwilioProvider;
    use sayangcare_core::ports::TelephonyProvider;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn stub(status: &str, body: &str) -> (TwilioProvider, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let request = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let read = socket.read(&mut buffer).await.unwrap();
                    assert!(read > 0, "connection closed before request body");
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .expect("form content length");
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                socket.write_all(response.as_bytes()).await.unwrap();
                String::from_utf8(bytes).unwrap()
            })
            .await
            .expect("local Twilio stub timed out")
        });
        let mut provider = TwilioProvider::new("AC123".into(), "token".into());
        provider.base_url = format!("http://{address}/2010-04-01");
        provider.client = reqwest::Client::builder().no_proxy().build().unwrap();
        (provider, request)
    }

    #[test]
    fn demo_twilio_credentials_are_rejected() {
        let provider = TwilioProvider::new("ACtest".to_string(), "test".to_string());
        assert!(!provider.has_live_credentials());
    }

    #[test]
    fn real_twilio_credentials_are_accepted() {
        let provider = TwilioProvider::new("AC123".to_string(), "secret-token".to_string());
        assert!(provider.has_live_credentials());
    }

    #[tokio::test]
    async fn outbound_request_uses_basic_auth_and_twilio_form_contract() {
        let (provider, request) = stub("201 Created", r#"{"sid":"CAlocaltest"}"#).await;
        let sid = provider
            .create_outbound_call(
                "+6591234567",
                "+16501234567",
                "https://example.test/connect",
            )
            .await
            .unwrap();
        assert_eq!(sid, "CAlocaltest");
        let request = request.await.unwrap();
        let headers = request.to_ascii_lowercase();
        assert!(request.starts_with("POST /2010-04-01/Accounts/AC123/Calls.json HTTP/1.1"));
        assert!(headers.contains("authorization: basic qumxmjm6dg9rzw4="));
        assert!(headers.contains("content-type: application/x-www-form-urlencoded"));
        assert!(request.contains("To=%2B6591234567"));
        assert!(request.contains("From=%2B16501234567"));
        assert!(request.contains("Url=https%3A%2F%2Fexample.test%2Fconnect"));
        assert!(request.contains("Method=POST"));
    }

    #[tokio::test]
    async fn outbound_http_error_is_not_reported_as_success() {
        let (provider, request) = stub("401 Unauthorized", r#"{"message":"denied"}"#).await;
        let error = provider
            .create_outbound_call(
                "+6591234567",
                "+16501234567",
                "https://example.test/connect",
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("401"));
        assert!(error.contains("denied"));
        request.await.unwrap();
    }

    #[tokio::test]
    async fn outbound_response_requires_a_nonempty_sid() {
        for body in [
            r#"{"sid":""}"#,
            r#"{"sid":"  "}"#,
            r#"{"message":"missing sid"}"#,
            "not json",
        ] {
            let (provider, request) = stub("201 Created", body).await;
            assert!(provider
                .create_outbound_call(
                    "+6591234567",
                    "+16501234567",
                    "https://example.test/connect"
                )
                .await
                .is_err());
            request.await.unwrap();
        }
    }

    #[tokio::test]
    async fn outbound_rejects_unconfigured_credentials_and_empty_fields() {
        let provider = TwilioProvider::new("ACtest".into(), "test".into());
        let error = provider
            .create_outbound_call(
                "+6591234567",
                "+16501234567",
                "https://example.test/connect",
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not configured"));
        for (to, from, url) in [
            (" ", "+16501234567", "https://example.test/connect"),
            ("+6591234567", "", "https://example.test/connect"),
            ("+6591234567", "+16501234567", " "),
        ] {
            let error = provider
                .create_outbound_call(to, from, url)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("requires non-empty"));
        }
    }
}
