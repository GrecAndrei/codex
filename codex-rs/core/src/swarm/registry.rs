use crate::swarm::config::SwarmRole;
use codex_protocol::ThreadId;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmAgentInfo {
    pub thread_id: ThreadId,
    pub role: String,
    pub model: Option<String>,
    pub tier: i32,
    pub parent_thread_id: Option<ThreadId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct SwarmRegistrySnapshot {
    agents: Vec<SwarmAgentInfo>,
}

#[derive(Clone)]
pub struct SwarmRegistry {
    state: Arc<RwLock<HashMap<ThreadId, SwarmAgentInfo>>>,
    storage: Arc<SwarmRegistryStorage>,
}

#[derive(Default)]
struct SwarmRegistryStorage {
    codex_home: Option<PathBuf>,
    storage_dir: RwLock<Option<PathBuf>>,
}

impl SwarmRegistry {
    pub fn new(codex_home: PathBuf) -> Self {
        let codex_home = if codex_home.as_os_str().is_empty() {
            None
        } else {
            Some(codex_home)
        };
        Self {
            state: Arc::new(RwLock::new(HashMap::new())),
            storage: Arc::new(SwarmRegistryStorage {
                codex_home,
                storage_dir: RwLock::new(None),
            }),
        }
    }

    pub async fn apply_storage_dir(&self, storage_dir: Option<PathBuf>) {
        if let Some(storage_dir) = storage_dir {
            let mut guard = self.storage.storage_dir.write().await;
            *guard = Some(storage_dir);
        }
    }

    pub async fn load_from_storage(&self) -> Result<(), String> {
        let path = self.registry_state_path().await;
        let Some(path) = path else {
            return Ok(());
        };
        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to read swarm registry: {err}")),
        };
        let snapshot: SwarmRegistrySnapshot = serde_json::from_str(&contents)
            .map_err(|err| format!("failed to parse swarm registry: {err}"))?;
        let mut guard = self.state.write().await;
        *guard = snapshot
            .agents
            .into_iter()
            .map(|agent| (agent.thread_id, agent))
            .collect();
        Ok(())
    }

    pub async fn register_root(
        &self,
        thread_id: ThreadId,
        role: &SwarmRole,
        model: Option<String>,
    ) {
        self.insert(SwarmAgentInfo {
            thread_id,
            role: role.name.clone(),
            model,
            tier: role.tier,
            parent_thread_id: None,
        })
        .await;
    }

    pub async fn register_child(
        &self,
        thread_id: ThreadId,
        parent_thread_id: ThreadId,
        role: &SwarmRole,
        model: Option<String>,
    ) {
        self.insert(SwarmAgentInfo {
            thread_id,
            role: role.name.clone(),
            model,
            tier: role.tier,
            parent_thread_id: Some(parent_thread_id),
        })
        .await;
    }

    pub async fn get(&self, thread_id: ThreadId) -> Option<SwarmAgentInfo> {
        let guard = self.state.read().await;
        guard.get(&thread_id).cloned()
    }

    pub async fn snapshot(&self) -> Vec<SwarmAgentInfo> {
        let guard = self.state.read().await;
        guard.values().cloned().collect()
    }

    pub async fn persist_now(&self) -> Result<(), String> {
        let snapshot = {
            let guard = self.state.read().await;
            SwarmRegistrySnapshot {
                agents: guard.values().cloned().collect(),
            }
        };
        self.persist_state(&snapshot).await
    }

    async fn insert(&self, info: SwarmAgentInfo) {
        let snapshot = {
            let mut guard = self.state.write().await;
            guard.insert(info.thread_id, info);
            SwarmRegistrySnapshot {
                agents: guard.values().cloned().collect(),
            }
        };
        let _ = self.persist_state(&snapshot).await;
    }

    async fn persist_state(&self, snapshot: &SwarmRegistrySnapshot) -> Result<(), String> {
        let Some(path) = self.registry_state_path().await else {
            return Ok(());
        };
        let payload = serde_json::to_string_pretty(snapshot)
            .map_err(|err| format!("failed to serialize swarm registry: {err}"))?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| format!("failed to create swarm registry dir: {err}"))?;
        }
        tokio::fs::write(&path, payload)
            .await
            .map_err(|err| format!("failed to write swarm registry: {err}"))?;
        Ok(())
    }

    async fn registry_state_path(&self) -> Option<PathBuf> {
        let storage_dir = self.storage.storage_dir.read().await.clone();
        let base = storage_dir.or_else(|| {
            self.storage
                .codex_home
                .clone()
                .map(|home| home.join("swarm"))
        });
        base.map(|dir| dir.join("swarm_registry.json"))
    }
}

impl Default for SwarmRegistry {
    fn default() -> Self {
        Self::new(PathBuf::new())
    }
}
