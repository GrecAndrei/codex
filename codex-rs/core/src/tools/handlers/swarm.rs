use crate::codex::Session;
use crate::function_tool::FunctionCallError;
use crate::swarm::SwarmArtifactEntry;
use crate::swarm::SwarmDecisionEntry;
use crate::swarm::SwarmEvidenceEntry;
use crate::swarm::SwarmLeakEntry;
use crate::swarm::SwarmLoungeEntry;
use crate::swarm::SwarmTaskEntry;
use crate::swarm::SwarmTimerState;
use crate::swarm::SwarmVote;
use crate::swarm::SwarmVoteCast;
use crate::swarm::now_unix_ms;
use crate::swarm::thread_id_string;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use uuid::Uuid;

pub struct SwarmHubHandler;

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum SwarmHubArgs {
    LoungeAppend {
        text: String,
    },
    LoungeRead {
        limit: Option<usize>,
    },
    LoungeClear,
    VoteCreate {
        topic: String,
        options: Vec<String>,
    },
    VoteCast {
        vote_id: String,
        option: String,
        weight: Option<i32>,
    },
    VoteStatus {
        vote_id: Option<String>,
    },
    TimerStart {
        label: Option<String>,
        duration_ms: Option<u64>,
    },
    TimerStop,
    TimerStatus,
    LeakTrackerSetPath {
        path: String,
        load_existing: Option<bool>,
    },
    LeakTrackerAdd {
        label: String,
        value: String,
        context: Option<String>,
        severity: Option<String>,
    },
    LeakTrackerList {
        limit: Option<usize>,
    },
    LeakTrackerClear,
    TaskAdd {
        title: String,
        status: Option<String>,
        notes: Option<String>,
    },
    TaskList {
        limit: Option<usize>,
    },
    EvidenceAdd {
        summary: String,
        severity: Option<String>,
        source: Option<String>,
    },
    EvidenceList {
        limit: Option<usize>,
    },
    DecisionAdd {
        summary: String,
        rationale: Option<String>,
    },
    DecisionList {
        limit: Option<usize>,
    },
    ArtifactAdd {
        label: String,
        path: Option<String>,
    },
    ArtifactList {
        limit: Option<usize>,
    },
}

#[async_trait]
impl ToolHandler for SwarmHubHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn: _,
            payload,
            ..
        } = invocation;
        let ToolPayload::Function { arguments } = payload else {
            return Err(FunctionCallError::RespondToModel(
                "swarm_hub handler received unsupported payload".to_string(),
            ));
        };
        let args: SwarmHubArgs = parse_arguments(&arguments)?;
        match args {
            SwarmHubArgs::LoungeAppend { text } => {
                if text.trim().is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "lounge text must be non-empty".to_string(),
                    ));
                }
                session
                    .services
                    .swarm_hub
                    .lounge_append(SwarmLoungeEntry {
                        text,
                        author_thread_id: thread_id_string(Some(session.conversation_id)),
                        created_at_unix_ms: now_unix_ms(),
                    })
                    .await;
                Ok(tool_ok(json!({ "ok": true })))
            }
            SwarmHubArgs::LoungeRead { limit } => {
                let state = session.services.swarm_hub.snapshot().await;
                let limit = limit.unwrap_or(state.lounge.len());
                let items: Vec<_> = state.lounge.iter().rev().take(limit).cloned().collect();
                Ok(tool_ok(json!({ "entries": items })))
            }
            SwarmHubArgs::LoungeClear => {
                session.services.swarm_hub.lounge_clear().await;
                Ok(tool_ok(json!({ "ok": true })))
            }
            SwarmHubArgs::VoteCreate { topic, options } => {
                if topic.trim().is_empty() || options.is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "vote topic and options are required".to_string(),
                    ));
                }
                let vote = SwarmVote {
                    id: Uuid::new_v4().to_string(),
                    topic,
                    options,
                    created_at_unix_ms: now_unix_ms(),
                    votes: Vec::new(),
                };
                session.services.swarm_hub.upsert_vote(vote.clone()).await;
                Ok(tool_ok(json!({ "vote": vote })))
            }
            SwarmHubArgs::VoteCast {
                vote_id,
                option,
                weight,
            } => {
                let mut state = session.services.swarm_hub.snapshot().await;
                let vote = state
                    .votes
                    .iter_mut()
                    .find(|vote| vote.id == vote_id)
                    .ok_or_else(|| {
                        FunctionCallError::RespondToModel("vote_id not found".to_string())
                    })?;
                let weight = match weight {
                    Some(weight) => {
                        if weight <= 0 {
                            return Err(FunctionCallError::RespondToModel(
                                "vote weight must be positive".to_string(),
                            ));
                        }
                        weight
                    }
                    None => default_vote_weight(&session).await,
                };
                vote.votes.push(SwarmVoteCast {
                    option,
                    weight,
                    voter_thread_id: thread_id_string(Some(session.conversation_id)),
                });
                session.services.swarm_hub.upsert_vote(vote.clone()).await;
                Ok(tool_ok(json!({ "vote": vote })))
            }
            SwarmHubArgs::VoteStatus { vote_id } => {
                let state = session.services.swarm_hub.snapshot().await;
                let votes = if let Some(vote_id) = vote_id {
                    state
                        .votes
                        .into_iter()
                        .filter(|vote| vote.id == vote_id)
                        .collect::<Vec<_>>()
                } else {
                    state.votes
                };
                Ok(tool_ok(json!({ "votes": votes })))
            }
            SwarmHubArgs::TimerStart { label, duration_ms } => {
                session
                    .services
                    .swarm_hub
                    .set_timer(SwarmTimerState {
                        label,
                        duration_ms,
                        started_at_unix_ms: Some(now_unix_ms()),
                        running: true,
                    })
                    .await;
                Ok(tool_ok(json!({ "ok": true })))
            }
            SwarmHubArgs::TimerStop => {
                let mut state = session.services.swarm_hub.snapshot().await;
                state.timer.running = false;
                state.timer.started_at_unix_ms = None;
                session
                    .services
                    .swarm_hub
                    .set_timer(state.timer.clone())
                    .await;
                Ok(tool_ok(json!({ "timer": state.timer })))
            }
            SwarmHubArgs::TimerStatus => {
                let state = session.services.swarm_hub.snapshot().await;
                Ok(tool_ok(json!({ "timer": state.timer })))
            }
            SwarmHubArgs::LeakTrackerSetPath {
                path,
                load_existing,
            } => {
                let path = PathBuf::from(path);
                session
                    .services
                    .swarm_hub
                    .leak_tracker_set_path(path, load_existing.unwrap_or(true))
                    .await;
                Ok(tool_ok(json!({ "ok": true })))
            }
            SwarmHubArgs::LeakTrackerAdd {
                label,
                value,
                context,
                severity,
            } => {
                if label.trim().is_empty() || value.trim().is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "label and value are required".to_string(),
                    ));
                }
                session
                    .services
                    .swarm_hub
                    .leak_tracker_add(SwarmLeakEntry {
                        id: Uuid::new_v4().to_string(),
                        label,
                        value,
                        context,
                        severity,
                        created_at_unix_ms: now_unix_ms(),
                        source_thread_id: thread_id_string(Some(session.conversation_id)),
                    })
                    .await
                    .map_err(FunctionCallError::RespondToModel)?;
                Ok(tool_ok(json!({ "ok": true })))
            }
            SwarmHubArgs::LeakTrackerList { limit } => {
                let state = session.services.swarm_hub.snapshot().await;
                let limit = limit.unwrap_or(state.leak_tracker.entries.len());
                let entries: Vec<_> = state
                    .leak_tracker
                    .entries
                    .iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect();
                Ok(tool_ok(json!({ "entries": entries })))
            }
            SwarmHubArgs::LeakTrackerClear => {
                session
                    .services
                    .swarm_hub
                    .leak_tracker_clear()
                    .await
                    .map_err(FunctionCallError::RespondToModel)?;
                Ok(tool_ok(json!({ "ok": true })))
            }
            SwarmHubArgs::TaskAdd {
                title,
                status,
                notes,
            } => {
                if title.trim().is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "task title is required".to_string(),
                    ));
                }
                let entry = SwarmTaskEntry {
                    id: Uuid::new_v4().to_string(),
                    title,
                    status: status.unwrap_or_else(|| "pending".to_string()),
                    owner_thread_id: thread_id_string(Some(session.conversation_id)),
                    notes,
                    created_at_unix_ms: now_unix_ms(),
                };
                session.services.swarm_hub.task_add(entry.clone()).await;
                Ok(tool_ok(json!({ "task": entry })))
            }
            SwarmHubArgs::TaskList { limit } => {
                let state = session.services.swarm_hub.snapshot().await;
                let limit = limit.unwrap_or(state.tasks.len());
                let tasks: Vec<_> = state.tasks.iter().rev().take(limit).cloned().collect();
                Ok(tool_ok(json!({ "tasks": tasks })))
            }
            SwarmHubArgs::EvidenceAdd {
                summary,
                severity,
                source,
            } => {
                if summary.trim().is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "evidence summary is required".to_string(),
                    ));
                }
                let entry = SwarmEvidenceEntry {
                    id: Uuid::new_v4().to_string(),
                    summary,
                    severity,
                    source,
                    created_at_unix_ms: now_unix_ms(),
                };
                session.services.swarm_hub.evidence_add(entry.clone()).await;
                Ok(tool_ok(json!({ "evidence": entry })))
            }
            SwarmHubArgs::EvidenceList { limit } => {
                let state = session.services.swarm_hub.snapshot().await;
                let limit = limit.unwrap_or(state.evidence.len());
                let evidence: Vec<_> = state.evidence.iter().rev().take(limit).cloned().collect();
                Ok(tool_ok(json!({ "evidence": evidence })))
            }
            SwarmHubArgs::DecisionAdd { summary, rationale } => {
                if summary.trim().is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "decision summary is required".to_string(),
                    ));
                }
                let entry = SwarmDecisionEntry {
                    id: Uuid::new_v4().to_string(),
                    summary,
                    rationale,
                    created_at_unix_ms: now_unix_ms(),
                };
                session.services.swarm_hub.decision_add(entry.clone()).await;
                Ok(tool_ok(json!({ "decision": entry })))
            }
            SwarmHubArgs::DecisionList { limit } => {
                let state = session.services.swarm_hub.snapshot().await;
                let limit = limit.unwrap_or(state.decisions.len());
                let decisions: Vec<_> = state.decisions.iter().rev().take(limit).cloned().collect();
                Ok(tool_ok(json!({ "decisions": decisions })))
            }
            SwarmHubArgs::ArtifactAdd { label, path } => {
                if label.trim().is_empty() {
                    return Err(FunctionCallError::RespondToModel(
                        "artifact label is required".to_string(),
                    ));
                }
                let entry = SwarmArtifactEntry {
                    id: Uuid::new_v4().to_string(),
                    label,
                    path,
                    created_at_unix_ms: now_unix_ms(),
                };
                session.services.swarm_hub.artifact_add(entry.clone()).await;
                Ok(tool_ok(json!({ "artifact": entry })))
            }
            SwarmHubArgs::ArtifactList { limit } => {
                let state = session.services.swarm_hub.snapshot().await;
                let limit = limit.unwrap_or(state.artifacts.len());
                let artifacts: Vec<_> = state.artifacts.iter().rev().take(limit).cloned().collect();
                Ok(tool_ok(json!({ "artifacts": artifacts })))
            }
        }
    }
}

async fn default_vote_weight(session: &Session) -> i32 {
    let Some(info) = session
        .services
        .swarm_registry
        .get(session.conversation_id)
        .await
    else {
        return 1;
    };
    if info.role.eq_ignore_ascii_case("scholar") || info.tier >= 2 {
        2
    } else {
        1
    }
}

fn tool_ok(payload: serde_json::Value) -> ToolOutput {
    let content = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    ToolOutput::Function {
        content,
        success: Some(true),
        content_items: None,
    }
}
