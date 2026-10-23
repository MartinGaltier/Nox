use super::Timestamp;
use crate::reading::Reading;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
    /// SF Symbol for cards and alerts.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Claude => "sparkle",
            Self::Codex => "chevron.left.forwardslash.chevron.right",
            Self::OpenCode => "terminal",
            Self::Copilot => "infinity",
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
    /// Start of the current (or last) turn.
    #[serde(default)]
    pub turn_started_at: Option<Timestamp>,
    /// Tokens of the current (or last) turn, from the logs.
    #[serde(default)]
    pub tokens: TokenUsage,
    /// API-equivalent value of the current turn (always an estimate).
    #[serde(default)]
    pub cost_usd: Reading<f64>,
    /// Named account (Codex home) when not the default one.
    #[serde(default)]
    pub account: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LimitKind {
    /// Rolling session window (5 h).
    Session,
    Weekly,
    #[default]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LimitSource {
    /// Written by the agent into its own session log (Codex `rate_limits`).
    #[default]
    SessionLog,
    /// Saved on this Mac by the Claude app (`plan-usage-history.json`).
    ClaudeApp,
    /// Asked of the account with the CLI's OAuth token (opt-in only).
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    /// e.g. "5 h", "Semaine", "Opus semaine".
    pub name: String,
    /// 0..100
    pub used_percent: Reading<f64>,
    pub resets_at: Option<Timestamp>,
    #[serde(default)]
    pub kind: LimitKind,
    #[serde(default)]
    pub window_minutes: Option<u32>,
    /// Model family the window applies to ("Opus"); `None` = everything.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub source: LimitSource,
    /// When the provider reported this value.
    #[serde(default)]
    pub observed_at: Timestamp,
    /// `resets_at` was inferred (e.g. from the Claude app history), not read.
    #[serde(default)]
    pub resets_estimated: bool,
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
    pub fn add(&mut self, o: &TokenUsage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
}

/// A second Codex home (`CODEX_HOME`) with its own account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountLimits {
    pub name: String,
    pub limits: Reading<Vec<LimitWindow>>,
    pub plan: Option<String>,
    pub tokens_today: Reading<TokenUsage>,
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
    /// Tokens over `AgentsState::period`.
    #[serde(default)]
    pub tokens_period: Reading<TokenUsage>,
    #[serde(default)]
    pub cost_period_usd: Reading<f64>,
    /// Responses whose model has no known price (left out of costs).
    #[serde(default)]
    pub unpriced_requests: u64,
    /// Subscription named by the local files ("Plus", "Max 5×").
    #[serde(default)]
    pub plan: Option<String>,
    /// Codex banked resets (only the Codex app server knows them).
    #[serde(default)]
    pub banked_resets: Reading<u32>,
    /// Extra Codex homes.
    #[serde(default)]
    pub accounts: Vec<AccountLimits>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AgentPeriod {
    #[default]
    Today,
    Week,
    Month,
}

impl AgentPeriod {
    pub fn days(self) -> u32 {
        match self {
            Self::Today => 1,
            Self::Week => 7,
            Self::Month => 30,
        }
    }
    pub fn label_fr(self) -> &'static str {
        match self {
            Self::Today => "Aujourd’hui",
            Self::Week => "7 jours",
            Self::Month => "30 jours",
        }
    }
}

/// Cards of the Agents page, in user order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentCard {
    Live,
    Limits,
    Spend,
    Budget,
    Accounts,
}

impl AgentCard {
    pub const DEFAULT_ORDER: [AgentCard; 5] = [
        Self::Live,
        Self::Limits,
        Self::Spend,
        Self::Budget,
        Self::Accounts,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetStatus {
    pub daily_usd: f64,
    /// API-equivalent spend today across providers (estimate).
    pub spent_today_usd: Reading<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentsState {
    pub providers: Vec<ProviderStatus>,
    #[serde(default)]
    pub period: AgentPeriod,
    #[serde(default)]
    pub budget: Option<BudgetStatus>,
    /// Review date of the embedded price table ("2026-10-09").
    #[serde(default)]
    pub prices_updated: String,
    #[serde(default)]
    pub card_order: Vec<AgentCard>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentCommand {
    Refresh,
    /// Focus the terminal/app that runs the session.
    FocusSession(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexHome {
    pub name: String,
    /// Absolute path of the `CODEX_HOME` folder.
    pub path: String,
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
    pub finish_alert: bool,
    pub needs_input_alert: bool,
    pub limit_alert: bool,
    /// Alert once when a window's usage crosses this percentage.
    pub limit_threshold_percent: f64,
    /// 0 = no budget.
    pub daily_budget_usd: f64,
    pub period: AgentPeriod,
    /// Extra Codex homes; `~/.codex-*` folders are also found automatically.
    pub codex_homes: Vec<CodexHome>,
    pub card_order: Vec<AgentCard>,
    pub hidden_cards: Vec<AgentCard>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            enabled: AgentProvider::ALL.to_vec(),
            finish_alert_min_secs: 30,
            allow_limit_queries: false,
            finish_alert: true,
            needs_input_alert: true,
            limit_alert: true,
            limit_threshold_percent: 80.0,
            daily_budget_usd: 0.0,
            period: AgentPeriod::Today,
            codex_homes: Vec::new(),
            card_order: AgentCard::DEFAULT_ORDER.to_vec(),
            hidden_cards: Vec::new(),
        }
    }
}
