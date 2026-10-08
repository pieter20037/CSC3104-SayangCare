use async_trait::async_trait;
use reqwest::Client;
use sayangcare_core::config::AiConfig;
use sayangcare_core::domain::{
    DistressMarker, RiskAssessment, SentimentSignal, Speaker, Transcript,
};
use sayangcare_core::ports::InferenceService;
use sayangcare_core::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const SYSTEM_PROMPT: &str = "You are SayangCare, a brief, compassionate conversational support assistant. Reply in the same language as the caller's most recent message. Do not switch languages or translate their message; if the language is unclear, use English. Respond directly to what the caller said, avoid canned introductions, and ask at most one gentle follow-up question. You are not a clinician and must not diagnose or promise confidentiality. Never shame or argue with the caller. If the caller indicates immediate self-harm or danger, acknowledge them, encourage contacting local emergency services or a trusted nearby person now, and keep the response short. A separate deterministic safety classifier handles escalation; do not claim a human has joined or that escalation happened.";

#[derive(Clone)]
pub struct GroqInference {
    client: Client,
    api_key: String,
    model: String,
    endpoint: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    max_tokens: u16,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct SentimentResult {
    valence: f32,
    arousal: f32,
    #[serde(default)]
    distress_markers: Vec<String>,
}

impl GroqInference {
    pub fn new(config: &AiConfig) -> CoreResult<Option<Self>> {
        let Some(api_key) = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
        else {
            return Ok(None);
        };

        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.max(1)))
            .build()
            .map_err(|error| CoreError::Inference(format!("HTTP client setup failed: {error}")))?;

        Ok(Some(Self {
            client,
            api_key: api_key.to_string(),
            model: config.model.clone(),
            endpoint: config.base_url.clone(),
        }))
    }

    async fn complete(
        &self,
        messages: Vec<ChatMessage<'_>>,
        max_tokens: u16,
    ) -> CoreResult<String> {
        let request = ChatRequest {
            model: &self.model,
            messages,
            temperature: 0.3,
            max_tokens,
        };
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .map_err(|error| CoreError::Inference(format!("Groq request failed: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CoreError::Inference(format!(
                "Groq returned HTTP {status}: {}",
                body.chars().take(800).collect::<String>()
            )));
        }

        let result: ChatResponse = response
            .json()
            .await
            .map_err(|error| CoreError::Inference(format!("invalid Groq response: {error}")))?;
        result
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .map(|content| content.trim().to_string())
            .filter(|content| !content.is_empty())
            .ok_or_else(|| CoreError::Inference("Groq returned no assistant text".to_string()))
    }
}

#[async_trait]
impl InferenceService for GroqInference {
    async fn generate_reply(&self, context: &Transcript) -> CoreResult<String> {
        let mut messages = vec![ChatMessage {
            role: "system",
            content: SYSTEM_PROMPT,
        }];
        let recent = context.recent_context(12);
        for turn in recent {
            let role = match turn.speaker {
                Speaker::Caller => "user",
                Speaker::Assistant | Speaker::HumanVolunteer => "assistant",
            };
            messages.push(ChatMessage {
                role,
                content: &turn.text,
            });
        }
        self.complete(messages, 180).await
    }

    async fn score_sentiment(&self, text: &str) -> CoreResult<SentimentSignal> {
        let instruction = format!(
            "Analyze this caller utterance for sentiment. Return only JSON with valence (number -1 to 1), arousal (number 0 to 1), and distress_markers (array chosen only from hopelessness, isolation, self_harm, panic, grief). Utterance: {text}"
        );
        let messages = vec![
            ChatMessage {
                role: "system",
                content: "Return valid JSON only. Do not provide a diagnosis.",
            },
            ChatMessage {
                role: "user",
                content: &instruction,
            },
        ];
        let response = self.complete(messages, 100).await?;
        let parsed: SentimentResult = serde_json::from_str(&response).map_err(|error| {
            CoreError::Inference(format!("invalid sentiment JSON from Groq: {error}"))
        })?;
        let distress_markers = parsed
            .distress_markers
            .iter()
            .filter_map(|marker| match marker.as_str() {
                "hopelessness" => Some(DistressMarker::Hopelessness),
                "isolation" => Some(DistressMarker::Isolation),
                "self_harm" => Some(DistressMarker::SelfHarm),
                "panic" => Some(DistressMarker::Panic),
                "grief" => Some(DistressMarker::Grief),
                _ => None,
            })
            .collect();
        Ok(SentimentSignal {
            valence: parsed.valence.clamp(-1.0, 1.0),
            arousal: parsed.arousal.clamp(0.0, 1.0),
            distress_markers,
            captured_at: chrono::Utc::now(),
        })
    }

    async fn assess_risk(
        &self,
        context: &Transcript,
        _sentiment: &SentimentSignal,
    ) -> CoreResult<RiskAssessment> {
        let caller_text = context
            .turns
            .iter()
            .filter(|turn| turn.speaker == Speaker::Caller)
            .map(|turn| turn.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(RiskAssessment::from_text(&caller_text))
    }
}

#[cfg(test)]
mod tests {
    use super::GroqInference;
    use sayangcare_core::config::AiConfig;

    #[test]
    fn missing_key_disables_provider_without_breaking_local_startup() {
        let provider = GroqInference::new(&AiConfig::default()).unwrap();
        assert!(provider.is_none());
    }
}
