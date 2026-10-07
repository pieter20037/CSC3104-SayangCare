use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Raw sentiment scores from the LLM / classifier.
/// Range is -1.0 (very negative) to 1.0 (very positive).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentimentSignal {
    pub valence: f32,
    pub arousal: f32,
    pub distress_markers: Vec<DistressMarker>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistressMarker {
    Hopelessness,
    Isolation,
    SelfHarm,
    Panic,
    Grief,
}

impl SentimentSignal {
    /// Aggregate distress severity in [0.0, 1.0], used by the circuit breaker
    /// to adapt degradation thresholds.
    pub fn distress_severity(&self) -> f32 {
        let valence_component = ((1.0 - self.valence) / 2.0).clamp(0.0, 1.0);
        let marker_weight = (self.distress_markers.len() as f32 * 0.2).min(1.0);
        (valence_component * 0.6 + marker_weight * 0.4).clamp(0.0, 1.0)
    }
}
