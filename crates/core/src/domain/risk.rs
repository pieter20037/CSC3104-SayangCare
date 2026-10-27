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
        let normalized = text
            .to_lowercase()
            .chars()
            .map(|character| match character {
                '\u{2018}' | '\u{2019}' => '\'',
                character if character.is_alphanumeric() || character == '\'' => character,
                _ => ' ',
            })
            .collect::<String>();
        let normalized = format!(
            " {} ",
            normalized.split_whitespace().collect::<Vec<_>>().join(" ")
        );
        let contains_phrase = |phrase: &str| normalized.contains(&format!(" {phrase} "));

        let immediate = [
            ("end my life", 5),
            ("want to die", 5),
            ("kill myself", 5),
            ("hurt myself", 5),
            ("hurting myself", 5),
            ("cut myself", 5),
            ("cutting myself", 5),
            ("harm myself", 5),
            ("harming myself", 5),
            ("self harm", 5),
            ("self harming", 5),
            ("selfharm", 5),
            ("suicidal", 5),
            ("want to end it", 5),
            ("want to jump off", 5),
            ("going to jump off", 5),
            ("about to jump off", 5),
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
            ("do not want to live", 4),
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
            if contains_phrase(marker) {
                score = score.saturating_add(*weight);
                evidence.push(*marker);
            }
        }

        let immediate_risk = immediate.iter().any(|(marker, _)| contains_phrase(marker));
        let level = if immediate_risk || score >= 9 {
            RiskLevel::CRISIS
        } else if score >= 4 {
            RiskLevel::HIGH
        } else if score >= 2 {
            RiskLevel::MODERATE
        } else if score > 0 {
            RiskLevel::ELEVATED
        } else {
            RiskLevel::LOW
        };
        let mut rationale = match level {
            RiskLevel::CRISIS => "immediate safety risk detected; urgent escalation required",
            RiskLevel::HIGH => {
                "multiple or strong distress cues detected; human review recommended"
            }
            RiskLevel::MODERATE => "distress cues detected; monitor and continue support",
            RiskLevel::ELEVATED => "mild distress cue detected; continue supportive check-in",
            _ => "no acute distress markers detected",
        }
        .to_string();

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

    #[test]
    fn weighted_distress_cues_use_intermediate_levels() {
        assert_eq!(
            RiskAssessment::from_text("I feel sad.").level,
            RiskLevel::ELEVATED
        );
        assert_eq!(
            RiskAssessment::from_text("I have been anxious.").level,
            RiskLevel::MODERATE
        );
        assert_eq!(
            RiskAssessment::from_text("I do not want to live.").level,
            RiskLevel::HIGH
        );
        assert_eq!(
            RiskAssessment::from_text("I want to kill myself.").level,
            RiskLevel::CRISIS
        );
    }

    #[test]
    fn screenshot_self_harm_statements_require_immediate_escalation() {
        for text in [
            "hello i am feeling very stressed out recently from school work and i am starting to cut myself with penknife.",
            "no i am not comfortable. im so depressed i really want to end it.",
            "I need help. can u refer me to a volunteer? I'm alone and im starting to have suicidal thoughts",
            "i am very stressed lately and starting to have negative thoughts about people around me. I cannot stand it and i want to jump off the building",
        ] {
            let assessment = RiskAssessment::from_text(text);
            assert_eq!(assessment.level, RiskLevel::CRISIS, "{text}");
            assert!(assessment.level.requires_immediate_escalation(), "{text}");
        }
    }

    #[test]
    fn recognizes_speech_spacing_punctuation_and_inflections() {
        for text in [
            "I am CUTTING   MYSELF.",
            "I am hurting myself",
            "I am harming myself",
            "I want to end\nmy life",
            "I\u{2019}m thinking about self\u{2011}harm",
            "I can't go on!",
            "I can\u{2019}t go on",
            "I am self-harming",
        ] {
            assert!(
                RiskAssessment::from_text(text)
                    .level
                    .requires_immediate_escalation(),
                "{text}"
            );
        }
    }

    #[test]
    fn ordinary_phrases_do_not_match_partial_risk_words() {
        for text in [
            "Can you tell me a story about a garden?",
            "I cut vegetables for dinner.",
            "I enjoyed the seaside today.",
            "I want to end the story here.",
        ] {
            assert_eq!(
                RiskAssessment::from_text(text).level,
                RiskLevel::LOW,
                "{text}"
            );
        }
        assert_eq!(
            RiskAssessment::from_text("I am stressed about school.").level,
            RiskLevel::ELEVATED
        );
    }
}
