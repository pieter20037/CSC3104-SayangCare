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

impl Default for RiskAssessment {
    fn default() -> Self {
        Self {
            level: RiskLevel::LOW,
            rationale: "initial assessment pending".into(),
            assessed_at: None,
        }
    }
}