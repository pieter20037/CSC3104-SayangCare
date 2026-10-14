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
    /// Continuous distress estimate in [0.0, 1.0] for adapting breaker
    /// thresholds. Neutral valence alone contributes no distress.
    pub fn distress_severity(&self) -> f32 {
        let negative_valence = (-self.valence).clamp(0.0, 1.0);
        let arousal = self.arousal.clamp(0.0, 1.0);
        let affective_distress = negative_valence * (0.75 + 0.25 * arousal);
        let marker_distress = self
            .distress_markers
            .iter()
            .map(|marker| match marker {
                DistressMarker::Hopelessness => 0.8,
                DistressMarker::Isolation => 0.45,
                DistressMarker::SelfHarm => 1.0,
                DistressMarker::Panic => 0.7,
                DistressMarker::Grief => 0.55,
            })
            .fold(0.0_f32, f32::max);

        (1.0 - (1.0 - affective_distress) * (1.0 - marker_distress)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{DistressMarker, SentimentSignal};

    fn signal(
        valence: f32,
        arousal: f32,
        distress_markers: Vec<DistressMarker>,
    ) -> SentimentSignal {
        SentimentSignal {
            valence,
            arousal,
            distress_markers,
            captured_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn neutral_or_positive_valence_without_markers_is_not_distress() {
        assert_eq!(signal(0.0, 0.5, vec![]).distress_severity(), 0.0);
        assert_eq!(signal(0.8, 0.9, vec![]).distress_severity(), 0.0);
    }

    #[test]
    fn negative_valence_and_arousal_produce_a_continuous_score() {
        let low_arousal = signal(-0.6, 0.0, vec![]).distress_severity();
        let high_arousal = signal(-0.6, 1.0, vec![]).distress_severity();

        assert!(low_arousal > 0.0);
        assert!(high_arousal > low_arousal);
        assert!(high_arousal < 1.0);
    }

    #[test]
    fn markers_add_weighted_distress_without_counting_every_marker_equally() {
        let isolation = signal(0.0, 0.0, vec![DistressMarker::Isolation]).distress_severity();
        let hopelessness = signal(0.0, 0.0, vec![DistressMarker::Hopelessness]).distress_severity();
        let self_harm = signal(0.0, 0.0, vec![DistressMarker::SelfHarm]).distress_severity();

        assert!(isolation < hopelessness);
        assert!(hopelessness < self_harm);
        assert_eq!(self_harm, 1.0);
    }
}
