use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub turns: Vec<Turn>,
}

impl Default for Transcript {
    fn default() -> Self {
        Self { turns: Vec::new() }
    }
}

impl Transcript {
    pub fn push(&mut self, turn: Turn) {
        self.turns.push(turn);
    }

    /// Recent context window for LLM calls (last N turns).
    pub fn recent_context(&self, n: usize) -> &[Turn] {
        let start = self.turns.len().saturating_sub(n);
        &self.turns[start..]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub speaker: Speaker,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    Caller,
    Assistant,
    HumanVolunteer,
}
