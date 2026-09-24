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

    async fn update_call(&self, call_sid: &str, params: &[(&str, &str)]) -> CoreResult<()> {
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
        let twiml = format!(
            r#"<Response><Play>{}</Play></Response>"#,
            audio_url
        );
        self.update_call(call_sid, &[("Twiml", twiml.as_str())]).await
    }

    async fn hangup(&self, call_sid: &str) -> CoreResult<()> {
        self.update_call(call_sid, &[("Status", "completed")]).await
    }

    async fn redirect_to_human(&self, call_sid: &str, volunteer_id: &str) -> CoreResult<()> {
        let twiml = format!(
            r#"<Response><Say>Connecting you to a volunteer now.</Say>
               <Dial><Client>{}</Client></Dial></Response>"#,
            volunteer_id
        );
        self.update_call(call_sid, &[("Twiml", twiml.as_str())]).await
    }
}