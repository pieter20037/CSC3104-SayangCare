use crate::breaker::AdaptiveThresholdPolicy;

/// Sentiment-aware policy: the more distressed the caller, the lower
/// (more sensitive) the thresholds become.
///
/// Rationale: a caller in crisis cannot tolerate a 3-second stall, so
/// we trip faster and fall back to buffered audio + human escalation.
pub struct SentimentAdaptivePolicy {
    pub base_error_rate: f64,
    pub base_latency_ms: u64,
    /// How aggressively distress tightens thresholds (0.0 = no effect).
    pub sensitivity: f32,
}

impl SentimentAdaptivePolicy {
    pub fn new(base_error_rate: f64, base_latency_ms: u64, sensitivity: f32) -> Self {
        Self {
            base_error_rate,
            base_latency_ms,
            sensitivity: sensitivity.clamp(0.0, 1.0),
        }
    }
}

impl AdaptiveThresholdPolicy for SentimentAdaptivePolicy {
    fn thresholds(&self, distress_severity: f32) -> (f64, u64) {
        let s = distress_severity.clamp(0.0, 1.0);
        // Scale factor in [1 - sensitivity, 1.0].
        let scale = 1.0 - (self.sensitivity * s);
        let err = (self.base_error_rate * scale as f64).clamp(0.01, 1.0);
        let lat = (self.base_latency_ms as f32 * scale).max(50.0) as u64;
        (err, lat)
    }
}
