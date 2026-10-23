use super::Timestamp;
use crate::reading::Reading;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentProvider {
    Claude,
    Codex,
    OpenCode,
    Copilot,
}

impl AgentProvider {
    pub const ALL: [AgentProvider; 4] = [Self::Claude, Self::Codex, Self::OpenCode, Self::Copilot];
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Copilot => "GitHub Copilot",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionPhase {
    /// Model is generating or a tool is running.
    Working,
    /// Waiting for the user (permission prompt / question).
    WaitingForUser,
    Idle,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub provider: AgentProvider,
    pub project: Option<String>,
    pub model: Option<String>,
    pub phase: SessionPhase,
    /// Short description of current activity (tool name, last action).
    pub activity: Option<String>,
    pub started_at: Timestamp,
    pub last_event_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    /// e.g. "5 h", "Semaine", "Opus semaine".
    pub name: String,
    /// 0..100
    pub used_percent: Reading<f64>,
    pub resets_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub provider: AgentProvider,
    /// Local data found (CLI/app installed and used).
    pub detected: bool,
    pub sessions: Vec<AgentSession>,
    pub limits: Reading<Vec<LimitWindow>>,
    /// Tokens today, read from local logs.
    pub tokens_today: Reading<TokenUsage>,
    /// API-equivalent value in USD (always an estimate).
    pub cost_today_usd: Reading<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentsState {
    pub providers: Vec<ProviderStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentCommand {
    Refresh,
    /// Focus the terminal/app that runs the session.
    FocusSession(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub enabled: Vec<AgentProvider>,
    /// Alert when a session that worked at least this long finishes.
    pub finish_alert_min_secs: u64,
    /// Allow reading OAuth tokens already stored by the CLIs to query plan
    /// limits (never copied or persisted by Nox). Off = local logs only.
    pub allow_limit_queries: bool,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            enabled: AgentProvider::ALL.to_vec(),
            finish_alert_min_secs: 30,
            allow_limit_queries: false,
        }
    }
}
