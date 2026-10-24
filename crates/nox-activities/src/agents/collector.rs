//! Finds the agents' logs, reads them incrementally and builds `AgentsState`.
//!
//! Discovery runs every `RESCAN` seconds; between rescans only files touched
//! recently (or holding a live turn) are `stat`ed. The first pass reads the
//! last `KEEP_DAYS` of history without raising alerts.

use super::live::{self, Ctx, Event, Tracker};
use super::parse::{Entry, FileState};
use super::pricing::PriceList;
use super::registry::{self, Status};
use super::tail::{Cursor, ReadOutcome, discover};
use super::usage::{KEEP_DAYS, UsageStore, local_day};
use super::{claude, codex, copilot, limits, oauth, opencode};
use nox_core::model::Timestamp;
use nox_core::model::agents::{
    AccountLimits, AgentProvider, AgentSettings, AgentsState, BudgetStatus, LimitWindow,
    ProviderStatus, SessionPhase,
};
use nox_core::{Reading, Unavailable};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const RESCAN: f64 = 30.0;
/// Files modified this recently are checked on every tick.
const HOT: f64 = 15.0 * 60.0;
const MAX_SESSIONS: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    Live(Event),
    Limit {
        provider: AgentProvider,
        account: Option<String>,
        window: String,
        used: f64,
    },
    Budget {
        spent: f64,
        budget: f64,
    },
}

struct LogFile {
    cursor: Cursor,
    provider: AgentProvider,
    account: usize,
    state: FileState,
}

#[derive(Debug, Clone)]
struct Account {
    name: String,
    home: PathBuf,
}

#[derive(Default)]
struct OAuthState {
    last_try: Option<Timestamp>,
    result: Option<Result<Vec<LimitWindow>, String>>,
}

pub struct Collector {
    home: PathBuf,
    prices: &'static PriceList,
    files: HashMap<PathBuf, LogFile>,
    oc_db: Option<(opencode::Database, FileState)>,
    oc_files: HashMap<PathBuf, SystemTime>,
    oc_state: FileState,
    usage: UsageStore,
    live: Tracker,
    limits: HashMap<(AgentProvider, usize), (Vec<LimitWindow>, Timestamp)>,
    plans: HashMap<(AgentProvider, usize), (String, Timestamp)>,
    credits: HashMap<usize, (String, Timestamp)>,
    codex_accounts: Vec<Account>,
    registry: HashMap<String, registry::Entry>,
    prev_status: HashMap<String, Status>,
    last_rescan: Option<Timestamp>,
    /// The first full pass is done: transitions are news from now on.
    started: bool,
    claude_app: Option<(SystemTime, Vec<limits::Sample>)>,
    claude_profile: Option<(SystemTime, Profile)>,
    oauth: OAuthState,
    limit_prev: HashMap<String, f64>,
    budget_fired_day: Option<i64>,
    first_snapshot: bool,
    pid_alive: fn(i32) -> bool,
    /// `$CODEX_HOME` captured at creation (tests clear it).
    codex_home_env: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
struct Profile {
    org: Option<String>,
    plan: Option<String>,
}

fn mtime_secs(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn system_time(t: Timestamp) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs_f64(t.max(0.0))
}

fn is_jsonl(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

impl Collector {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            prices: PriceList::embedded(),
            files: HashMap::new(),
            oc_db: None,
            oc_files: HashMap::new(),
            oc_state: FileState::default(),
            usage: UsageStore::default(),
            live: Tracker::default(),
            limits: HashMap::new(),
            plans: HashMap::new(),
            credits: HashMap::new(),
            codex_accounts: Vec::new(),
            registry: HashMap::new(),
            prev_status: HashMap::new(),
            last_rescan: None,
            started: false,
            claude_app: None,
            claude_profile: None,
            oauth: OAuthState::default(),
            limit_prev: HashMap::new(),
            budget_fired_day: None,
            first_snapshot: true,
            pid_alive: registry::pid_alive,
            codex_home_env: std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()),
        }
    }

    /// Test isolation: injected process liveness, and no `$CODEX_HOME`
    /// from the environment running the tests.
    #[cfg(test)]
    pub fn isolated(mut self, alive: fn(i32) -> bool) -> Self {
        self.pid_alive = alive;
        self.codex_home_env = None;
        self
    }

    pub fn prices_updated(&self) -> &str {
        &self.prices.updated
    }

    fn claude_roots(&self) -> [PathBuf; 2] {
        [
            self.home.join(".claude/projects"),
            self.home.join(".config/claude/projects"),
        ]
    }

    fn refresh_accounts(&mut self, s: &AgentSettings) {
        let default = self
            .codex_home_env
            .clone()
            .unwrap_or_else(|| self.home.join(".codex"));
        let mut v = vec![Account {
            name: String::new(),
            home: default.clone(),
        }];
        let mut seen: HashSet<PathBuf> = HashSet::new();
        seen.insert(std::fs::canonicalize(&default).unwrap_or(default));
        let mut add = |name: String, path: PathBuf, v: &mut Vec<Account>| {
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if path.join("sessions").is_dir() && seen.insert(canon) && v.len() < 9 {
                v.push(Account { name, home: path });
            }
        };
        for h in &s.codex_homes {
            let p = PathBuf::from(&h.path);
            if p.is_absolute() {
                add(h.name.chars().take(40).collect(), p, &mut v);
            }
        }
        if let Ok(rd) = std::fs::read_dir(&self.home) {
            let mut extra: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(".codex-") && n.len() > 7)
                })
                .collect();
            extra.sort();
            for p in extra {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("")[7..].to_string();
                let mut c = name.chars();
                let pretty = c
                    .next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default();
                add(pretty, p, &mut v);
            }
        }
        self.codex_accounts = v;
    }

    /// Discovers log files; new ones are read right away.
    fn rescan(&mut self, s: &AgentSettings, now: Timestamp, out: &mut Vec<AgentEvent>) {
        self.refresh_accounts(s);
        let since = system_time(now - (KEEP_DAYS as f64 + 1.0) * 86_400.0);
        let mut found: Vec<(PathBuf, AgentProvider, usize)> = Vec::new();
        let on = |p: AgentProvider| s.enabled.contains(&p);
        if on(AgentProvider::Claude) {
            for root in self.claude_roots() {
                let mut v = Vec::new();
                discover(&root, 3, since, &is_jsonl, &mut v);
                found.extend(v.into_iter().map(|p| (p, AgentProvider::Claude, 0)));
            }
        }
        if on(AgentProvider::Codex) {
            let accept = |p: &Path| {
                is_jsonl(p)
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("rollout-"))
            };
            for (i, a) in self.codex_accounts.iter().enumerate() {
                let mut v = Vec::new();
                discover(&a.home.join("sessions"), 4, since, &accept, &mut v);
                discover(&a.home.join("archived_sessions"), 1, since, &accept, &mut v);
                found.extend(v.into_iter().map(|p| (p, AgentProvider::Codex, i)));
            }
        }
        if on(AgentProvider::Copilot) {
            let accept = |p: &Path| p.file_name().and_then(|n| n.to_str()) == Some("events.jsonl");
            let mut v = Vec::new();
            discover(
                &self.home.join(".copilot/session-state"),
                1,
                since,
                &accept,
                &mut v,
            );
            found.extend(v.into_iter().map(|p| (p, AgentProvider::Copilot, 0)));
        }
        for (path, provider, account) in found {
            if self.files.contains_key(&path) {
                continue;
            }
            self.files.insert(
                path.clone(),
                LogFile {
                    cursor: Cursor::new(path.clone()),
                    provider,
                    account,
                    state: FileState::default(),
                },
            );
            self.read_file(&path, now, out);
        }
        // Files deleted or disabled since: forget them.
        self.files
            .retain(|p, f| s.enabled.contains(&f.provider) && p.exists());
        if on(AgentProvider::OpenCode) {
            self.rescan_opencode(now, out);
        }
        self.last_rescan = Some(now);
    }

    fn rescan_opencode(&mut self, now: Timestamp, out: &mut Vec<AgentEvent>) {
        let base = self.home.join(".local/share/opencode");
        let db = base.join("opencode.db");
        if db.is_file() && self.oc_db.is_none() {
            self.oc_db = Some((opencode::Database::new(db), FileState::default()));
        }
        let since = system_time(now - (KEEP_DAYS as f64 + 1.0) * 86_400.0);
        let mut v = Vec::new();
        let accept = |p: &Path| p.extension().and_then(|e| e.to_str()) == Some("json");
        discover(&base.join("storage/message"), 1, since, &accept, &mut v);
        for p in v {
            let m = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            if m.is_none() || self.oc_files.get(&p) == m.as_ref() {
                continue;
            }
            if let Some(m) = m {
                self.oc_files.insert(p.clone(), m);
            }
            if let Some(rec) = opencode::read_message_file(&p) {
                let mut st = std::mem::take(&mut self.oc_state);
                let entries = opencode::parse(&rec, &mut st, now);
                self.apply(&entries, AgentProvider::OpenCode, 0, &st, now, out);
                self.oc_state = st;
            }
        }
        if self.oc_files.len() > 20_000 {
            self.oc_files.clear();
        }
    }

    fn poll_opencode_db(&mut self, now: Timestamp, out: &mut Vec<AgentEvent>) {
        let Some((mut db, mut st)) = self.oc_db.take() else {
            return;
        };
        let since_ms = ((now - (KEEP_DAYS as f64 + 1.0) * 86_400.0) * 1000.0) as i64;
        let mut batches: Vec<(Vec<Entry>, FileState)> = Vec::new();
        db.read_new(since_ms, |rec| {
            let e = opencode::parse(rec, &mut st, now);
            if !e.is_empty() {
                batches.push((e, st.clone_light()));
            }
        });
        for (e, s) in batches {
            self.apply(&e, AgentProvider::OpenCode, 0, &s, now, out);
        }
        self.oc_db = Some((db, st));
    }

    fn read_file(&mut self, path: &Path, now: Timestamp, out: &mut Vec<AgentEvent>) {
        let Some(mut f) = self.files.remove(path) else {
            return;
        };
        let parser: fn(&[u8], &mut FileState, Timestamp) -> Vec<Entry> = match f.provider {
            AgentProvider::Claude => claude::parse,
            AgentProvider::Codex => codex::parse,
            AgentProvider::Copilot => copilot::parse,
            AgentProvider::OpenCode => {
                self.files.insert(path.to_path_buf(), f);
                return;
            }
        };
        let (provider, account) = (f.provider, f.account);
        let mut pending: Vec<(Vec<Entry>, FileState)> = Vec::new();
        let state = &mut f.state;
        let outcome = f.cursor.read_new(|line, skipped| {
            let entries = if skipped {
                // Too long to decode: for Claude it still says work goes on.
                if provider == AgentProvider::Claude && state.turn_open {
                    vec![Entry::TurnActive(now)]
                } else {
                    Vec::new()
                }
            } else {
                parser(line, state, now)
            };
            if !entries.is_empty() {
                pending.push((entries, state.clone_light()));
            }
        });
        if let ReadOutcome::Read { restarted: true } = outcome {
            f.state = FileState::default();
        }
        for (e, s) in pending {
            self.apply(&e, provider, account, &s, now, out);
        }
        if outcome != ReadOutcome::Gone {
            self.files.insert(path.to_path_buf(), f);
        }
    }

    fn apply(
        &mut self,
        entries: &[Entry],
        provider: AgentProvider,
        account: usize,
        st: &FileState,
        now: Timestamp,
        out: &mut Vec<AgentEvent>,
    ) {
        let today = local_day(now);
        let ctx = Ctx {
            provider,
            account,
            session: &st.session,
            project: &st.project,
            model: &st.model,
            reports: self.started,
            now,
        };
        for e in entries {
            match e {
                Entry::Usage {
                    key,
                    at,
                    model,
                    billable,
                } => {
                    if let Some(added) = self.usage.add(
                        key.clone(),
                        (provider, account),
                        *at,
                        model,
                        *billable,
                        today,
                    ) {
                        self.live.add_usage(&ctx, *at, &added.tokens, added.cost);
                    }
                }
                Entry::Limits { windows, at } => {
                    let k = (provider, account);
                    if self.limits.get(&k).is_none_or(|(_, t)| *t <= *at) {
                        self.limits.insert(k, (windows.clone(), *at));
                    }
                }
                Entry::Plan { plan, at } => {
                    let k = (provider, account);
                    if self.plans.get(&k).is_none_or(|(_, t)| *t <= *at) {
                        self.plans.insert(k, (plan.clone(), *at));
                    }
                }
                Entry::Credits { text, at } => {
                    if self.credits.get(&account).is_none_or(|(_, t)| *t <= *at) {
                        self.credits.insert(account, (text.clone(), *at));
                    }
                }
                other => {
                    if let Some(ev) = self.live.apply(other, &ctx) {
                        out.push(AgentEvent::Live(ev));
                    }
                }
            }
        }
    }

    fn read_registry(&mut self, now: Timestamp, out: &mut Vec<AgentEvent>) {
        let reg = registry::read(&self.home.join(".claude/sessions"), &self.pid_alive);
        for (sid, e) in &reg {
            let prev = self.prev_status.get(sid).copied();
            if self.started && e.status == Status::Waiting && prev != Some(Status::Waiting) {
                let project = self
                    .live
                    .sessions
                    .get(&(AgentProvider::Claude, sid.clone()))
                    .map(|l| l.project.clone())
                    .filter(|p| !p.is_empty())
                    .or_else(|| e.cwd.as_deref().map(super::parse::project_name))
                    .unwrap_or_default();
                out.push(AgentEvent::Live(Event::NeedsInput {
                    provider: AgentProvider::Claude,
                    project,
                    what: registry::waiting_label(e.waiting_for.as_deref()),
                }));
            }
            // A live process keeps its session visible even when quiet.
            if let Some(l) = self
                .live
                .sessions
                .get_mut(&(AgentProvider::Claude, sid.clone()))
            {
                l.last_event = l.last_event.max(now - live::VISIBLE_FOR + 60.0);
            }
        }
        self.prev_status = reg.iter().map(|(k, e)| (k.clone(), e.status)).collect();
        self.registry = reg;
    }

    fn claude_app_limits(&mut self, now: Timestamp) -> Vec<LimitWindow> {
        let p = self
            .home
            .join("Library/Application Support/Claude/plan-usage-history.json");
        let Some(m) = std::fs::metadata(&p).and_then(|m| m.modified()).ok() else {
            self.claude_app = None;
            return Vec::new();
        };
        if self.claude_app.as_ref().is_none_or(|(t, _)| *t != m) {
            let s = std::fs::read(&p)
                .ok()
                .and_then(|b| limits::samples(&b))
                .unwrap_or_default();
            self.claude_app = Some((m, s));
        }
        let org = self.profile().org;
        self.claude_app
            .as_ref()
            .map(|(_, s)| limits::from_samples(s, now, org.as_deref()))
            .unwrap_or_default()
    }

    /// Organisation and plan from `~/.claude.json` (only these fields).
    fn profile(&mut self) -> Profile {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct File {
            oauth_account: Option<Acct>,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Acct {
            organization_uuid: Option<String>,
            organization_type: Option<String>,
            organization_rate_limit_tier: Option<String>,
        }
        let p = self.home.join(".claude.json");
        let Some(m) = std::fs::metadata(&p)
            .ok()
            .filter(|m| m.len() < 32 << 20)
            .and_then(|m| m.modified().ok())
        else {
            return Profile::default();
        };
        if let Some((t, prof)) = &self.claude_profile
            && *t == m
        {
            return prof.clone();
        }
        let prof = std::fs::read(&p)
            .ok()
            .and_then(|b| serde_json::from_slice::<File>(&b).ok())
            .and_then(|f| f.oauth_account)
            .map(|a| Profile {
                org: a.organization_uuid,
                plan: self.prices.claude_plan(&format!(
                    "{} {}",
                    a.organization_rate_limit_tier.unwrap_or_default(),
                    a.organization_type.unwrap_or_default()
                )),
            })
            .unwrap_or_default();
        self.claude_profile = Some((m, prof.clone()));
        prof
    }

    /// One collection pass. Returns alert-worthy events.
    pub fn tick(&mut self, s: &AgentSettings, now: Timestamp) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let rescan = self
            .last_rescan
            .is_none_or(|t| now - t >= RESCAN || now < t);
        if rescan {
            self.rescan(s, now, &mut out);
        }
        // Between rescans only recently written files (or live turns) are stat'ed.
        let due: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, f)| {
                rescan
                    || f.state.turn_open
                    || f.cursor.modified.is_some_and(|m| now - mtime_secs(m) < HOT)
            })
            .filter(|(_, f)| f.cursor.changed() == Some(true))
            .map(|(p, _)| p.clone())
            .collect();
        for p in due {
            self.read_file(&p, now, &mut out);
        }
        if s.enabled.contains(&AgentProvider::OpenCode) {
            self.poll_opencode_db(now, &mut out);
        }
        if s.enabled.contains(&AgentProvider::Claude) {
            self.read_registry(now, &mut out);
            if s.allow_limit_queries {
                let due = match (&self.oauth.last_try, &self.oauth.result) {
                    (None, _) => true,
                    (Some(t), Some(Ok(_))) => now - t >= oauth::REFRESH,
                    (Some(t), _) => now - t >= oauth::RETRY,
                };
                if due {
                    self.oauth.last_try = Some(now);
                    let r = oauth::fetch(now);
                    if let Err(e) = &r {
                        log::info!("agents: Claude limits query failed: {e}");
                    }
                    self.oauth.result = Some(r);
                }
            } else {
                self.oauth = OAuthState::default();
            }
        }
        let today = local_day(now);
        self.usage.prune(today, false);
        let registry = &self.registry;
        self.live.forget_old(now, &|l| registry.contains_key(&l.id));
        self.started = true;
        out
    }

    /// Something is working or waiting: poll faster.
    pub fn busy(&self, now: Timestamp) -> bool {
        self.live
            .sessions
            .values()
            .any(|l| l.turn_open && now - l.last_event < live::IDLE_TURN)
            || self
                .registry
                .values()
                .any(|e| matches!(e.status, Status::Busy | Status::Waiting))
    }

    fn sessions(
        &self,
        provider: AgentProvider,
        now: Timestamp,
    ) -> Vec<nox_core::model::agents::AgentSession> {
        let mut v: Vec<_> = self
            .live
            .sessions
            .values()
            .filter(|l| l.provider == provider)
            .filter_map(|l| {
                let reg = (provider == AgentProvider::Claude)
                    .then(|| self.registry.get(&l.id))
                    .flatten();
                if reg.is_none() && now - l.last_event >= live::VISIBLE_FOR {
                    return None;
                }
                let (phase, activity) = live::phase(l, reg, now);
                let account = (l.account > 0)
                    .then(|| self.codex_accounts.get(l.account).map(|a| a.name.clone()))
                    .flatten();
                Some(live::to_model(l, phase, activity, account))
            })
            .collect();
        let rank = |p: SessionPhase| match p {
            SessionPhase::WaitingForUser => 0,
            SessionPhase::Working => 1,
            SessionPhase::Finished => 2,
            SessionPhase::Idle => 3,
        };
        v.sort_by(|a, b| {
            rank(a.phase)
                .cmp(&rank(b.phase))
                .then(b.last_event_at.total_cmp(&a.last_event_at))
        });
        v.truncate(MAX_SESSIONS);
        v
    }

    fn limit_events(
        &mut self,
        provider: AgentProvider,
        account: Option<&str>,
        windows: &[LimitWindow],
        threshold: f64,
        out: &mut Vec<AgentEvent>,
    ) {
        for w in windows {
            let Reading::Measured(used) = w.used_percent else {
                continue;
            };
            // One key per window: after a renewal its usage falls below the
            // threshold again, so the next crossing alerts again. (The renewal
            // time is not part of the key: Codex reports it with seconds of
            // jitter between readings.)
            let key = format!("{provider:?}:{}:{}", account.unwrap_or(""), w.name);
            let prev = self.limit_prev.insert(key, used);
            if prev.is_some_and(|p| p < threshold) && used >= threshold {
                out.push(AgentEvent::Limit {
                    provider,
                    account: account.map(str::to_string),
                    window: w.name.clone(),
                    used,
                });
            }
        }
        if self.limit_prev.len() > 512 {
            self.limit_prev.clear();
        }
    }

    pub fn snapshot(
        &mut self,
        s: &AgentSettings,
        now: Timestamp,
    ) -> (AgentsState, Vec<AgentEvent>) {
        let mut events = Vec::new();
        let today = local_day(now);
        let period = s.period;
        let days = i64::from(period.days());
        let mut providers = Vec::new();
        let mut spent_today = 0.0;
        for p in AgentProvider::ALL {
            if !s.enabled.contains(&p) {
                continue;
            }
            let detected = self.detected(p);
            let default_acct = (p == AgentProvider::Codex).then_some(0);
            let t_today = self.usage.totals(p, default_acct, today, 1);
            let t_period = self.usage.totals(p, default_acct, today, days);
            spent_today += self.usage.totals(p, None, today, 1).cost;
            fn missing<T>(p: AgentProvider) -> Reading<T> {
                Reading::Unavailable(Unavailable::SourceMissing(source_label(p).into()))
            }
            let (tokens_today, cost_today, tokens_period, cost_period) = if detected {
                (
                    Reading::Measured(t_today.tokens.to_usage()),
                    Reading::Estimated(t_today.cost),
                    Reading::Measured(t_period.tokens.to_usage()),
                    Reading::Estimated(t_period.cost),
                )
            } else {
                (missing(p), missing(p), missing(p), missing(p))
            };
            let mut notes: Vec<String> = Vec::new();
            let mut plan = None;
            let limits: Reading<Vec<LimitWindow>> = match p {
                AgentProvider::Claude => {
                    plan = self.profile().plan;
                    let oauth = s
                        .allow_limit_queries
                        .then(|| self.oauth.result.clone())
                        .flatten();
                    match oauth {
                        Some(Ok(w)) => Reading::Measured(limits::expire(&w, now)),
                        other => {
                            if let Some(Err(e)) = other {
                                notes.push(format!("Requête des limites : {e}"));
                            }
                            let app = self.claude_app_limits(now);
                            if !app.is_empty() {
                                notes.push(
                                    "Limites lues dans l’historique de l’app Claude ; réinitialisations estimées."
                                        .into(),
                                );
                                Reading::Measured(app)
                            } else if s.allow_limit_queries {
                                Reading::Unavailable(Unavailable::Pending)
                            } else {
                                Reading::Unavailable(Unavailable::Error(
                                    "Ni l’historique de l’app Claude ni la requête OAuth (désactivée dans les réglages)".into(),
                                ))
                            }
                        }
                    }
                }
                AgentProvider::Codex => {
                    if let Some((c, _)) = self.credits.get(&0) {
                        notes.push(c.clone());
                    }
                    plan = self
                        .plans
                        .get(&(p, 0))
                        .and_then(|(pl, _)| self.prices.codex_plan(pl));
                    match self.limits.get(&(p, 0)) {
                        Some((w, _)) => Reading::Measured(limits::expire(w, now)),
                        None => Reading::Unavailable(Unavailable::SourceMissing(
                            "Lecture des limites dans les journaux Codex".into(),
                        )),
                    }
                }
                AgentProvider::OpenCode => Reading::Unavailable(Unavailable::NotSupported),
                AgentProvider::Copilot => {
                    notes.push(
                        "Jetons connus à la fin de chaque session (session.shutdown).".into(),
                    );
                    Reading::Unavailable(Unavailable::NotSupported)
                }
            };
            if let Reading::Measured(w) = &limits {
                let w = w.clone();
                self.limit_events(p, None, &w, s.limit_threshold_percent, &mut events);
            }
            if t_period.unpriced > 0 {
                notes.push(format!(
                    "{} réponse(s) d’un modèle sans prix connu, exclues du coût.",
                    t_period.unpriced
                ));
            }
            let accounts = if p == AgentProvider::Codex {
                self.codex_account_cards(today, now, s.limit_threshold_percent, &mut events)
            } else {
                Vec::new()
            };
            providers.push(ProviderStatus {
                provider: p,
                detected,
                sessions: self.sessions(p, now),
                limits,
                tokens_today,
                cost_today_usd: cost_today,
                note: (!notes.is_empty()).then(|| notes.join(" ")),
                tokens_period,
                cost_period_usd: cost_period,
                unpriced_requests: t_period.unpriced,
                plan,
                banked_resets: if p == AgentProvider::Codex {
                    Reading::Unavailable(Unavailable::Error(
                        "Connues seulement du serveur de l’app Codex, que Nox ne lance pas".into(),
                    ))
                } else {
                    Reading::Unavailable(Unavailable::NotSupported)
                },
                accounts,
            });
        }
        let budget = (s.daily_budget_usd > 0.0).then_some(BudgetStatus {
            daily_usd: s.daily_budget_usd,
            spent_today_usd: Reading::Estimated(spent_today),
        });
        if s.daily_budget_usd > 0.0 && spent_today >= s.daily_budget_usd {
            if self.budget_fired_day != Some(today) && !self.first_snapshot {
                events.push(AgentEvent::Budget {
                    spent: spent_today,
                    budget: s.daily_budget_usd,
                });
            }
            self.budget_fired_day = Some(today);
        }
        self.first_snapshot = false;
        let mut order = s.card_order.clone();
        order.retain(|c| !s.hidden_cards.contains(c));
        (
            AgentsState {
                providers,
                period,
                budget,
                prices_updated: self.prices.updated.clone(),
                card_order: order,
            },
            events,
        )
    }

    fn codex_account_cards(
        &mut self,
        today: i64,
        now: Timestamp,
        threshold: f64,
        events: &mut Vec<AgentEvent>,
    ) -> Vec<AccountLimits> {
        let accts: Vec<(usize, String)> = self
            .codex_accounts
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, a)| (i, a.name.clone()))
            .collect();
        accts
            .into_iter()
            .map(|(i, name)| {
                let w = self
                    .limits
                    .get(&(AgentProvider::Codex, i))
                    .map(|(w, _)| limits::expire(w, now));
                if let Some(w) = &w {
                    self.limit_events(AgentProvider::Codex, Some(&name), w, threshold, events);
                }
                AccountLimits {
                    limits: match w {
                        Some(w) => Reading::Measured(w),
                        None => Reading::Unavailable(Unavailable::Pending),
                    },
                    plan: self
                        .plans
                        .get(&(AgentProvider::Codex, i))
                        .and_then(|(pl, _)| self.prices.codex_plan(pl)),
                    tokens_today: Reading::Measured(
                        self.usage
                            .totals(AgentProvider::Codex, Some(i), today, 1)
                            .tokens
                            .to_usage(),
                    ),
                    name,
                }
            })
            .collect()
    }

    fn detected(&self, p: AgentProvider) -> bool {
        if self.usage.any(p) || self.live.sessions.keys().any(|(q, _)| *q == p) {
            return true;
        }
        match p {
            AgentProvider::Claude => self.claude_roots().iter().any(|r| r.is_dir()),
            AgentProvider::Codex => self
                .codex_accounts
                .iter()
                .any(|a| a.home.join("sessions").is_dir()),
            AgentProvider::OpenCode => self.home.join(".local/share/opencode").is_dir(),
            AgentProvider::Copilot => self.home.join(".copilot/session-state").is_dir(),
        }
    }

    /// The pid behind a live Claude session (for focusing its terminal).
    pub fn session_pid(&self, id: &str) -> Option<i32> {
        self.registry.get(id).map(|e| e.pid)
    }

    /// Aggregates for diagnostics (agents_dump): files read, records kept.
    pub fn stats(&self) -> (usize, usize, usize) {
        (self.files.len(), self.usage.len(), self.live.sessions.len())
    }
}

fn source_label(p: AgentProvider) -> &'static str {
    match p {
        AgentProvider::Claude => "Journaux Claude Code",
        AgentProvider::Codex => "Journaux Codex",
        AgentProvider::OpenCode => "Données OpenCode",
        AgentProvider::Copilot => "Journaux GitHub Copilot",
    }
}

impl FileState {
    /// The fields entries are attributed with (not the per-file maps).
    pub fn clone_light(&self) -> FileState {
        FileState {
            session: self.session.clone(),
            project: self.project.clone(),
            model: self.model.clone(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nox_core::model::agents::{LimitKind, SessionPhase};
    use std::io::Write;

    /// "YYYY-MM-DDTHH:MM:SS.000Z" for an epoch second (test helper).
    pub fn iso(t: f64) -> String {
        let secs = t.floor() as i64;
        let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }

    fn append(p: &Path, line: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap();
        writeln!(f, "{line}").unwrap();
    }

    /// Synthetic home with one Claude session and one Codex rollout.
    pub fn fake_home(tag: &str, now: f64) -> PathBuf {
        let home = std::env::temp_dir().join(format!("nox-home-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let claude = home.join(".claude/projects/-tmp-demo/s1.jsonl");
        append(
            &claude,
            &format!(
                r#"{{"type":"user","sessionId":"s1","cwd":"/tmp/demo","timestamp":"{}","message":{{"role":"user","content":"hi"}}}}"#,
                iso(now - 120.0)
            ),
        );
        append(
            &claude,
            &format!(
                r#"{{"type":"assistant","sessionId":"s1","timestamp":"{}","requestId":"r1","message":{{"id":"m1","model":"claude-opus-5-5","stop_reason":"tool_use","usage":{{"input_tokens":1000,"output_tokens":100}},"content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{}}}}]}}}}"#,
                iso(now - 100.0)
            ),
        );
        let codex = home.join(".codex/sessions/2026/10/24/rollout-x.jsonl");
        append(
            &codex,
            &format!(
                r#"{{"timestamp":"{}","type":"session_meta","payload":{{"id":"th1","cwd":"/tmp/cx"}}}}"#,
                iso(now - 90.0)
            ),
        );
        append(
            &codex,
            &format!(
                r#"{{"timestamp":"{}","type":"turn_context","payload":{{"model":"gpt-5.4"}}}}"#,
                iso(now - 89.0)
            ),
        );
        append(&codex, &codex_count(now - 80.0, 70.0, 500));
        home
    }

    /// Renewal time shared by every reading of the fixture's window.
    pub const RESETS_AT: i64 = 4_102_444_800; // 2100-01-01

    pub fn codex_count(at: f64, used: f64, total: u64) -> String {
        codex_count_resets(at, used, total, RESETS_AT)
    }

    pub fn codex_count_resets(at: f64, used: f64, total: u64, resets: i64) -> String {
        format!(
            r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{total},"output_tokens":10}}}},"rate_limits":{{"limit_id":"codex","plan_type":"plus","primary":{{"used_percent":{used},"window_minutes":300,"resets_at":{}}}}}}}}}"#,
            iso(at),
            resets
        )
    }

    #[test]
    fn end_to_end_on_synthetic_home() {
        let now = nox_core::model::now();
        let home = fake_home("e2e", now);
        let mut c = Collector::new(home.clone()).isolated(|pid| pid == 4242);
        let s = AgentSettings::default();

        assert!(c.tick(&s, now).is_empty(), "history is not news");
        let (st, ev) = c.snapshot(&s, now);
        assert!(ev.is_empty());
        let claude = &st.providers[0];
        assert!(claude.detected);
        assert_eq!(claude.sessions.len(), 1);
        assert_eq!(claude.sessions[0].phase, SessionPhase::Working);
        assert_eq!(claude.sessions[0].activity.as_deref(), Some("Edit"));
        assert_eq!(claude.sessions[0].project.as_deref(), Some("demo"));
        assert_eq!(claude.sessions[0].model.as_deref(), Some("Opus 5.5"));
        assert_eq!(claude.tokens_today.value().unwrap().total(), 1100);
        let cost = *claude.cost_today_usd.value().unwrap();
        assert!(claude.cost_today_usd.is_estimated());
        assert!((cost - (1000.0 * 4.0 + 100.0 * 20.0) / 1e6).abs() < 1e-12);
        let codex = &st.providers[1];
        let w = codex.limits.value().unwrap();
        assert_eq!(w[0].kind, LimitKind::Session);
        assert_eq!(w[0].used_percent, Reading::Measured(70.0));
        assert_eq!(codex.plan.as_deref(), Some("Plus"));
        assert_eq!(codex.tokens_today.value().unwrap().total(), 510);
        assert!(!st.providers[2].detected, "no OpenCode here");
        assert_eq!(st.prices_updated, "2026-10-09");

        // The Claude turn finishes: one Finished event.
        let claude_log = home.join(".claude/projects/-tmp-demo/s1.jsonl");
        append(
            &claude_log,
            &format!(
                r#"{{"type":"assistant","sessionId":"s1","timestamp":"{}","requestId":"r2","message":{{"id":"m2","model":"claude-opus-5-5","stop_reason":"end_turn","usage":{{"input_tokens":10,"output_tokens":5}}}}}}"#,
                iso(now - 1.0)
            ),
        );
        // Codex crosses the 80 % threshold.
        append(
            &home.join(".codex/sessions/2026/10/24/rollout-x.jsonl"),
            &codex_count(now - 1.0, 85.0, 900),
        );
        // A Claude process asks for permission.
        append(
            &home.join(".claude/sessions/4242.json"),
            r#"{"pid":4242,"sessionId":"s1","status":"waiting","waitingFor":"permission"}"#,
        );
        let mut ev = c.tick(&s, now + 1.0);
        let (st, more) = c.snapshot(&s, now + 1.0);
        ev.extend(more);
        assert!(
            ev.iter().any(|e| matches!(e, AgentEvent::Live(Event::Finished { duration, .. }) if (*duration - 119.0).abs() < 1.0)),
            "{ev:?}"
        );
        assert!(
            ev.iter()
                .any(|e| matches!(e, AgentEvent::Live(Event::NeedsInput { .. }))),
            "{ev:?}"
        );
        assert!(
            ev.iter()
                .any(|e| matches!(e, AgentEvent::Limit { used, .. } if *used == 85.0)),
            "{ev:?}"
        );
        assert_eq!(
            st.providers[0].sessions[0].phase,
            SessionPhase::WaitingForUser
        );
        assert_eq!(st.providers[1].tokens_today.value().unwrap().total(), 910);

        // Nothing new: no event, identical state.
        let ev = c.tick(&s, now + 2.0);
        assert!(ev.is_empty(), "{ev:?}");
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn disabled_providers_are_not_read() {
        let now = nox_core::model::now();
        let home = fake_home("off", now);
        let mut c = Collector::new(home.clone()).isolated(|_| false);
        let s = AgentSettings {
            enabled: vec![AgentProvider::Codex],
            ..Default::default()
        };
        c.tick(&s, now);
        let (st, _) = c.snapshot(&s, now);
        assert_eq!(st.providers.len(), 1);
        assert_eq!(st.providers[0].provider, AgentProvider::Codex);
        assert!(
            c.files
                .keys()
                .all(|p| !p.to_string_lossy().contains(".claude"))
        );
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn dead_pid_registry_entry_is_ignored() {
        let now = nox_core::model::now();
        let home = fake_home("deadpid", now);
        // Left behind by a crashed CLI: its process is gone.
        append(
            &home.join(".claude/sessions/4242.json"),
            r#"{"pid":4242,"sessionId":"s1","status":"waiting","waitingFor":"permission"}"#,
        );
        let mut c = Collector::new(home.clone()).isolated(|_| false);
        let s = AgentSettings::default();
        c.tick(&s, now);
        let ev = c.tick(&s, now + 1.0);
        assert!(
            !ev.iter()
                .any(|e| matches!(e, AgentEvent::Live(Event::NeedsInput { .. }))),
            "{ev:?}"
        );
        let (st, _) = c.snapshot(&s, now + 1.0);
        let phase = st.providers[0].sessions[0].phase;
        assert_eq!(
            phase,
            SessionPhase::Working,
            "log heuristics, not the stale registry"
        );
        assert_eq!(c.session_pid("s1"), None);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn codex_home_from_environment_is_not_used_in_tests() {
        let c = Collector::new(PathBuf::from("/nonexistent")).isolated(|_| false);
        assert!(c.codex_home_env.is_none());
    }

    #[test]
    fn limit_alert_survives_renewal_jitter_and_rearms_after_reset() {
        let now = nox_core::model::now();
        let home = fake_home("limits", now);
        let log = home.join(".codex/sessions/2026/10/24/rollout-x.jsonl");
        let mut c = Collector::new(home.clone()).isolated(|_| false);
        let s = AgentSettings::default();
        let crossed = |c: &mut Collector, t: f64| {
            c.tick(&s, t);
            c.snapshot(&s, t)
                .1
                .iter()
                .filter(|e| matches!(e, AgentEvent::Limit { .. }))
                .count()
        };
        assert_eq!(crossed(&mut c, now), 0, "70 % at start: below threshold");
        // Same window, renewal time a few seconds off (Codex jitter).
        append(
            &log,
            &codex_count_resets(now - 0.5, 85.0, 900, RESETS_AT + 7),
        );
        assert_eq!(crossed(&mut c, now + 1.0), 1);
        append(
            &log,
            &codex_count_resets(now - 0.4, 90.0, 950, RESETS_AT - 3),
        );
        assert_eq!(crossed(&mut c, now + 2.0), 0, "once per crossing");
        // After a renewal usage restarts low, then crosses again.
        append(
            &log,
            &codex_count_resets(now - 0.3, 5.0, 960, RESETS_AT + 18_000),
        );
        assert_eq!(crossed(&mut c, now + 3.0), 0);
        append(
            &log,
            &codex_count_resets(now - 0.2, 81.0, 970, RESETS_AT + 18_000),
        );
        assert_eq!(crossed(&mut c, now + 4.0), 1);
        let _ = std::fs::remove_dir_all(home);
    }
}
