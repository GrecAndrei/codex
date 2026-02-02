use crate::swarm::config::SwarmRole;
use codex_protocol::ThreadId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmAgentInfo {
    pub thread_id: ThreadId,
    pub role: String,
    pub model: Option<String>,
    pub tier: i32,
    pub parent_thread_id: Option<ThreadId>,
}

#[derive(Clone, Default)]
pub struct SwarmRegistry {
    state: Arc<RwLock<HashMap<ThreadId, SwarmAgentInfo>>>,
}

impl SwarmRegistry {
    pub fn new() -> Self {
        Self::default()
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

    async fn insert(&self, info: SwarmAgentInfo) {
        let mut guard = self.state.write().await;
        guard.insert(info.thread_id, info);
    }
}
