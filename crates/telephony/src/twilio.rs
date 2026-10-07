use async_trait::async_trait;
use reqwest::Client;
use sayangcare_core::ports::TelephonyProvider;
use sayangcare_core::{CoreError, CoreResult};

pub struct TwilioProvider {
    client: Client,
    account_sid: String,
    auth_token: String,
    base_url: String,
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
            .send()
            .await
            .map_err(|e| CoreError::Telephony(format!("twilio: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoreError::Telephony(format!("twilio {}: {body}", url)));
        }
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::TwilioProvider;

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
}
