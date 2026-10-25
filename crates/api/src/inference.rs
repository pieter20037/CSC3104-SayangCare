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
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

impl<'a> ChatRequest<'a> {
    fn new(model: &'a str, messages: Vec<ChatMessage<'a>>, max_tokens: u16, json: bool) -> Self {
        let reasoning_model = model.starts_with("openai/gpt-oss-");
        Self {
            model,
            messages,
            temperature: 0.3,
            max_tokens,
            reasoning_effort: reasoning_model.then_some("low"),
            include_reasoning: reasoning_model.then_some(false),
            response_format: json.then_some(ResponseFormat {
                kind: "json_object",
            }),
        }
    }
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
    #[serde(default)]
    finish_reason: Option<String>,
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
        json: bool,
    ) -> CoreResult<String> {
        let request = ChatRequest::new(&self.model, messages, max_tokens, json);
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
        let choice = result
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| CoreError::Inference("Groq returned no choices".to_string()))?;
        if choice.finish_reason.as_deref() == Some("length") {
            return Err(CoreError::Inference(
                "Groq exhausted its output-token budget before completing the response".into(),
            ));
        }
        choice
            .message
            .content
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
        self.complete(messages, 512, false).await
    }

    async fn score_sentiment(&self, text: &str) -> CoreResult<SentimentSignal> {
        let instruction = format!(
            "Estimate only the affect expressed in the caller's utterance; this is not a diagnosis or the separate safety-risk decision. Return exactly one JSON object with numeric valence, numeric arousal, and distress_markers containing only labels from hopelessness, isolation, self_harm, panic, grief. Use valence from -1.00 (strongly negative) through -0.50 (clearly negative), 0.00 (neutral or balanced), +0.50 (clearly positive), to +1.00 (strongly positive). Use arousal from 0.00 (very calm or low energy), 0.50 (moderate activation), to 1.00 (extremely activated). Choose intermediate decimal values when warranted; do not round every utterance to an endpoint. Add a distress marker only when the caller expresses that meaning, not when it is negated, hypothetical, or merely quoted. Do not infer self-harm from negative sentiment alone. Utterance: <caller_text>{text}</caller_text>"
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
        let response = self.complete(messages, 512, true).await?;
        let parsed: SentimentResult = serde_json::from_str(&response).map_err(|error| {
            CoreError::Inference(format!("invalid sentiment JSON from Groq: {error}"))
        })?;
        if !parsed.valence.is_finite() || !parsed.arousal.is_finite() {
            return Err(CoreError::Inference(
                "Groq sentiment contains non-finite values".into(),
            ));
        }
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

    #[test]
    fn reasoning_sentiment_request_reserves_completion_and_requests_json() {
        let value = serde_json::to_value(super::ChatRequest::new(
            "openai/gpt-oss-20b",
            vec![],
            512,
            true,
        ))
        .unwrap();
        assert_eq!(value["reasoning_effort"], "low");
        assert_eq!(value["include_reasoning"], false);
        assert_eq!(value["response_format"]["type"], "json_object");
        assert_eq!(value["max_tokens"], 512);
    }

    #[test]
    fn nonreasoning_models_do_not_receive_reasoning_specific_options() {
        let value = serde_json::to_value(super::ChatRequest::new("allam-2-7b", vec![], 512, false))
            .unwrap();
        assert!(value.get("reasoning_effort").is_none());
        assert!(value.get("include_reasoning").is_none());
        assert!(value.get("response_format").is_none());
    }
}
