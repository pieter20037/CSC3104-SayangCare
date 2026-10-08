use serde::{Deserialize, Serialize};

/// Risk level from 1 (low) to 5 (crisis).
/// Determines priority queue ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RiskLevel(pub u8);

impl RiskLevel {
    pub const LOW: RiskLevel = RiskLevel(1);
    pub const ELEVATED: RiskLevel = RiskLevel(2);
    pub const MODERATE: RiskLevel = RiskLevel(3);
    pub const HIGH: RiskLevel = RiskLevel(4);
    pub const CRISIS: RiskLevel = RiskLevel(5);

    pub fn new(v: u8) -> Self {
        Self(v.clamp(1, 5))
    }

    /// Whether this caller should skip the queue and go straight to a human.
    pub fn requires_immediate_escalation(&self) -> bool {
        *self >= RiskLevel::HIGH
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub rationale: String,
    pub assessed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl RiskAssessment {
    /// Weighted heuristic classifier for demo-grade distress detection.
    /// It intentionally goes beyond a single keyword match so the escalation
    /// policy is more realistic before a live LLM or clinician triage layer is added.
    pub fn from_text(text: &str) -> Self {
        let normalized = text.to_ascii_lowercase();

        let immediate = [
            ("end my life", 5),
            ("want to die", 5),
            ("kill myself", 5),
            ("hurt myself", 5),
            ("self-harm", 5),
            ("i am going to die", 5),
            ("suicide", 5),
            ("i can't go on", 4),
            ("no reason to live", 5),
            ("don't want to be here", 4),
        ];
        let high = [
            ("hopeless", 3),
            ("alone and scared", 3),
            ("isolated", 2),
            ("panic attack", 3),
            ("can't cope", 3),
            ("unsafe", 3),
            ("not safe", 3),
            ("desperate", 3),
            ("overwhelmed", 2),
            ("i feel trapped", 3),
            ("i don't want to live", 4),
        ];
        let moderate = [
            ("anxious", 2),
            ("depressed", 2),
            ("sad", 1),
            ("lonely", 1),
            ("crying", 1),
            ("stressed", 1),
            ("tired of living", 2),
            ("can't handle this", 2),
        ];

        let mut evidence = Vec::new();
        let mut score = 0u8;

        for (marker, weight) in immediate.iter().chain(high.iter()).chain(moderate.iter()) {
            if normalized.contains(marker) {
                score = score.saturating_add(*weight);
                evidence.push(*marker);
            }
        }

        let mut level = RiskLevel::LOW;
        let mut rationale = "no acute distress markers detected".to_string();

        if score >= 9
            || immediate
                .iter()
                .any(|(marker, _)| normalized.contains(marker))
        {
            level = RiskLevel::CRISIS;
            rationale = "immediate safety risk detected; urgent escalation required".to_string();
        } else if score >= 5 || high.iter().any(|(marker, _)| normalized.contains(marker)) {
            level = RiskLevel::HIGH;
            rationale = "high-risk distress markers detected; human review recommended".to_string();
        } else if score >= 2
            || moderate
                .iter()
                .any(|(marker, _)| normalized.contains(marker))
        {
            level = RiskLevel::MODERATE;
            rationale =
                "moderate distress markers detected; monitor and continue support".to_string();
        }

        if !evidence.is_empty() && level == RiskLevel::LOW {
            let sample = evidence
                .iter()
                .take(3)
                .map(|marker| marker.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            rationale = format!("risk cues detected: {sample}; triage level {}", level.0);
        }

        Self {
            level,
            rationale,
            assessed_at: Some(chrono::Utc::now()),
        }
    }
}

impl Default for RiskAssessment {
    fn default() -> Self {
        Self {
            level: RiskLevel::LOW,
            rationale: "initial assessment pending".into(),
            assessed_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RiskAssessment, RiskLevel};

    #[test]
    fn detects_immediate_escalation_keywords() {
        let assessment =
            RiskAssessment::from_text("I want to end my life and I feel completely hopeless");

        assert_eq!(assessment.level, RiskLevel::CRISIS);
        assert!(assessment.rationale.to_lowercase().contains("immediate"));
    }

    #[test]
    fn weighted_high_risk_phrase_escales_without_exact_match() {
        let assessment = RiskAssessment::from_text("I feel hopeless and trapped and I am unsafe.");

        assert_eq!(assessment.level, RiskLevel::HIGH);
        assert!(assessment.rationale.to_lowercase().contains("human review"));
    }
}
