use crate::swarm::config::SwarmHubConfig;
use codex_protocol::ThreadId;
use serde::Deserialize;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmLoungeEntry {
    pub text: String,
    pub author_thread_id: Option<String>,
    pub created_at_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmVote {
    pub id: String,
    pub topic: String,
    pub options: Vec<String>,
    pub created_at_unix_ms: u128,
    pub votes: Vec<SwarmVoteCast>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmVoteCast {
    pub option: String,
    pub weight: i32,
    pub voter_thread_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmTimerState {
    pub label: Option<String>,
    pub duration_ms: Option<u64>,
    pub started_at_unix_ms: Option<u128>,
    pub running: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmLeakEntry {
    pub id: String,
    pub label: String,
    pub value: String,
    pub context: Option<String>,
    pub severity: Option<String>,
    pub created_at_unix_ms: u128,
    pub source_thread_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SwarmLeakTracker {
    pub entries: Vec<SwarmLeakEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmTaskEntry {
    pub id: String,
    pub title: String,
    pub status: String,
    pub owner_thread_id: Option<String>,
    pub notes: Option<String>,
    pub created_at_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmEvidenceEntry {
    pub id: String,
    pub summary: String,
    pub severity: Option<String>,
    pub source: Option<String>,
    pub created_at_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmDecisionEntry {
    pub id: String,
    pub summary: String,
    pub rationale: Option<String>,
    pub created_at_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmArtifactEntry {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub created_at_unix_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmHubState {
    pub lounge: VecDeque<SwarmLoungeEntry>,
    pub votes: Vec<SwarmVote>,
    pub timer: SwarmTimerState,
    pub leak_tracker: SwarmLeakTracker,
    pub leak_tracker_path: Option<PathBuf>,
    pub tasks: Vec<SwarmTaskEntry>,
    pub evidence: Vec<SwarmEvidenceEntry>,
    pub decisions: Vec<SwarmDecisionEntry>,
    pub artifacts: Vec<SwarmArtifactEntry>,
}

impl Default for SwarmHubState {
    fn default() -> Self {
        Self {
            lounge: VecDeque::new(),
            votes: Vec::new(),
            timer: SwarmTimerState {
                label: None,
                duration_ms: None,
                started_at_unix_ms: None,
                running: false,
            },
            leak_tracker: SwarmLeakTracker::default(),
            leak_tracker_path: None,
            tasks: Vec::new(),
            evidence: Vec::new(),
            decisions: Vec::new(),
            artifacts: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct SwarmHub {
    state: Arc<RwLock<SwarmHubState>>,
    storage: Arc<SwarmHubStorage>,
}

#[derive(Default)]
struct SwarmHubStorage {
    codex_home: Option<PathBuf>,
}

impl SwarmHub {
    pub fn new(codex_home: PathBuf) -> Self {
        Self {
            state: Arc::new(RwLock::new(SwarmHubState::default())),
            storage: Arc::new(SwarmHubStorage {
                codex_home: Some(codex_home),
            }),
        }
    }

    pub async fn apply_config(&self, config: &SwarmHubConfig) {
        let mut state = self.state.write().await;
        if let Some(path) = config.leak_tracker_path.clone() {
            state.leak_tracker_path = Some(path);
        } else if state.leak_tracker_path.is_none()
            && let Some(storage_dir) = config.storage_dir.clone()
        {
            state.leak_tracker_path = Some(storage_dir.join("leak_tracker.json"));
        }
    }

    pub async fn snapshot(&self) -> SwarmHubState {
        self.state.read().await.clone()
    }

    pub async fn lounge_append(&self, entry: SwarmLoungeEntry) {
        let mut state = self.state.write().await;
        state.lounge.push_back(entry);
        if state.lounge.len() > 500 {
            state.lounge.pop_front();
        }
    }

    pub async fn lounge_clear(&self) {
        let mut state = self.state.write().await;
        state.lounge.clear();
    }

    pub async fn upsert_vote(&self, vote: SwarmVote) {
        let mut state = self.state.write().await;
        if let Some(existing) = state.votes.iter_mut().find(|v| v.id == vote.id) {
            *existing = vote;
        } else {
            state.votes.push(vote);
        }
    }

    pub async fn set_timer(&self, timer: SwarmTimerState) {
        let mut state = self.state.write().await;
        state.timer = timer;
    }

    pub async fn leak_tracker_set_path(&self, path: PathBuf, load_existing: bool) {
        let mut state = self.state.write().await;
        state.leak_tracker_path = Some(path.clone());
        if load_existing {
            if let Ok(contents) = tokio::fs::read_to_string(&path).await
                && let Ok(parsed) = serde_json::from_str::<SwarmLeakTracker>(&contents)
            {
                state.leak_tracker = parsed;
            }
        }
    }

    pub async fn leak_tracker_add(&self, entry: SwarmLeakEntry) -> Result<(), String> {
        let mut state = self.state.write().await;
        state.leak_tracker.entries.push(entry);
        self.persist_leak_tracker(&state).await
    }

    pub async fn leak_tracker_clear(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        state.leak_tracker.entries.clear();
        self.persist_leak_tracker(&state).await
    }

    pub async fn task_add(&self, entry: SwarmTaskEntry) {
        let mut state = self.state.write().await;
        state.tasks.push(entry);
    }

    pub async fn evidence_add(&self, entry: SwarmEvidenceEntry) {
        let mut state = self.state.write().await;
        state.evidence.push(entry);
    }

    pub async fn decision_add(&self, entry: SwarmDecisionEntry) {
        let mut state = self.state.write().await;
        state.decisions.push(entry);
    }

    pub async fn artifact_add(&self, entry: SwarmArtifactEntry) {
        let mut state = self.state.write().await;
        state.artifacts.push(entry);
    }

    async fn persist_leak_tracker(&self, state: &SwarmHubState) -> Result<(), String> {
        let Some(path) = state.leak_tracker_path.clone().or_else(|| {
            self.storage
                .codex_home
                .clone()
                .map(|home| home.join("swarm_leak_tracker.json"))
        }) else {
            return Ok(());
        };
        let payload = serde_json::to_string_pretty(&state.leak_tracker)
            .map_err(|err| format!("failed to serialize leak tracker: {err}"))?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| format!("failed to create leak tracker dir: {err}"))?;
        }
        tokio::fs::write(&path, payload)
            .await
            .map_err(|err| format!("failed to write leak tracker: {err}"))?;
        Ok(())
    }
}

pub fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

pub fn thread_id_string(thread_id: Option<ThreadId>) -> Option<String> {
    thread_id.map(|id| id.to_string())
}
