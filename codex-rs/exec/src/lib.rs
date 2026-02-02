// - In the default output mode, it is paramount that the only thing written to
//   stdout is the final message (if any).
// - In --json mode, stdout must be valid JSONL, one event per line.
// For both modes, any other output must be written to stderr.
#![deny(clippy::print_stdout)]

mod cli;
mod event_processor;
mod event_processor_with_human_output;
pub mod event_processor_with_jsonl_output;
pub mod exec_events;

pub use cli::Cli;
pub use cli::Command;
pub use cli::ReviewArgs;
pub use cli::SwarmCommand;
use codex_cloud_requirements::cloud_requirements_loader;
use codex_common::oss::ensure_oss_provider_ready;
use codex_common::oss::get_default_model_for_oss_provider;
use codex_common::oss::ollama_chat_deprecation_notice;
use codex_core::AgentRole;
use codex_core::AuthManager;
use codex_core::LMSTUDIO_OSS_PROVIDER_ID;
use codex_core::NewThread;
use codex_core::OLLAMA_CHAT_PROVIDER_ID;
use codex_core::OLLAMA_OSS_PROVIDER_ID;
use codex_core::ThreadManager;
use codex_core::auth::enforce_login_restrictions;
use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_core::config::ConfigOverrides;
use codex_core::config::find_codex_home;
use codex_core::config::load_config_as_toml_with_cli_overrides;
use codex_core::config::resolve_oss_provider;
use codex_core::config_loader::ConfigLoadError;
use codex_core::config_loader::format_config_error_with_source;
use codex_core::git_info::get_git_repo_root;
use codex_core::models_manager::manager::RefreshStrategy;
use codex_core::protocol::AskForApproval;
use codex_core::protocol::Event;
use codex_core::protocol::EventMsg;
use codex_core::protocol::Op;
use codex_core::protocol::ReviewRequest;
use codex_core::protocol::ReviewTarget;
use codex_core::protocol::SessionSource;
use codex_core::swarm::SwarmAgentInfo;
use codex_core::swarm::SwarmRole;
use codex_protocol::ThreadId;
use codex_protocol::approvals::ElicitationAction;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;
use event_processor_with_human_output::EventProcessorWithHumanOutput;
use event_processor_with_jsonl_output::EventProcessorWithJsonOutput;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::IsTerminal;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use supports_color::Stream;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::warn;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;
use uuid::Uuid;

use crate::cli::AgentTypeArg;
use crate::cli::Command as ExecCommand;
use crate::cli::SwarmAction;
use crate::event_processor::CodexStatus;
use crate::event_processor::EventProcessor;
use codex_core::default_client::set_default_client_residency_requirement;
use codex_core::default_client::set_default_originator;
use codex_core::find_thread_path_by_id_str;
use codex_core::find_thread_path_by_name_str;

enum InitialOperation {
    UserTurn {
        items: Vec<UserInput>,
        output_schema: Option<Value>,
    },
    Review {
        review_request: ReviewRequest,
    },
}

#[derive(Clone)]
struct ThreadEventEnvelope {
    thread_id: codex_protocol::ThreadId,
    thread: Arc<codex_core::CodexThread>,
    event: Event,
}

pub async fn run_main(cli: Cli, codex_linux_sandbox_exe: Option<PathBuf>) -> anyhow::Result<()> {
    if let Err(err) = set_default_originator("codex_exec".to_string()) {
        tracing::warn!(?err, "Failed to set codex exec originator override {err:?}");
    }

    let Cli {
        command,
        images,
        model: model_cli_arg,
        oss,
        oss_provider,
        config_profile,
        full_auto,
        dangerously_bypass_approvals_and_sandbox,
        cwd,
        skip_git_repo_check,
        add_dir,
        color,
        last_message_file,
        json: json_mode,
        sandbox_mode: sandbox_mode_cli_arg,
        prompt,
        output_schema: output_schema_path,
        config_overrides,
    } = cli;

    let (stdout_with_ansi, stderr_with_ansi) = match color {
        cli::Color::Always => (true, true),
        cli::Color::Never => (false, false),
        cli::Color::Auto => (
            supports_color::on_cached(Stream::Stdout).is_some(),
            supports_color::on_cached(Stream::Stderr).is_some(),
        ),
    };

    // Build fmt layer (existing logging) to compose with OTEL layer.
    let default_level = "error";

    // Build env_filter separately and attach via with_filter.
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default_level))
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_ansi(stderr_with_ansi)
        .with_writer(std::io::stderr)
        .with_filter(env_filter);

    let sandbox_mode = if full_auto {
        Some(SandboxMode::WorkspaceWrite)
    } else if dangerously_bypass_approvals_and_sandbox {
        Some(SandboxMode::DangerFullAccess)
    } else {
        sandbox_mode_cli_arg.map(Into::<SandboxMode>::into)
    };

    // Parse `-c` overrides from the CLI.
    let cli_kv_overrides = match config_overrides.parse_overrides() {
        Ok(v) => v,
        #[allow(clippy::print_stderr)]
        Err(e) => {
            eprintln!("Error parsing -c overrides: {e}");
            std::process::exit(1);
        }
    };

    let resolved_cwd = cwd.clone();
    let config_cwd = match resolved_cwd.as_deref() {
        Some(path) => AbsolutePathBuf::from_absolute_path(path.canonicalize()?)?,
        None => AbsolutePathBuf::current_dir()?,
    };

    // we load config.toml here to determine project state.
    #[allow(clippy::print_stderr)]
    let codex_home = match find_codex_home() {
        Ok(codex_home) => codex_home,
        Err(err) => {
            eprintln!("Error finding codex home: {err}");
            std::process::exit(1);
        }
    };

    #[allow(clippy::print_stderr)]
    let config_toml = match load_config_as_toml_with_cli_overrides(
        &codex_home,
        &config_cwd,
        cli_kv_overrides.clone(),
    )
    .await
    {
        Ok(config_toml) => config_toml,
        Err(err) => {
            let config_error = err
                .get_ref()
                .and_then(|err| err.downcast_ref::<ConfigLoadError>())
                .map(ConfigLoadError::config_error);
            if let Some(config_error) = config_error {
                eprintln!(
                    "Error loading config.toml:\n{}",
                    format_config_error_with_source(config_error)
                );
            } else {
                eprintln!("Error loading config.toml: {err}");
            }
            std::process::exit(1);
        }
    };

    let cloud_auth_manager = AuthManager::shared(
        codex_home.clone(),
        false,
        config_toml.cli_auth_credentials_store.unwrap_or_default(),
    );
    let chatgpt_base_url = config_toml
        .chatgpt_base_url
        .clone()
        .unwrap_or_else(|| "https://chatgpt.com/backend-api/".to_string());
    // TODO(gt): Make cloud requirements failures blocking once we can fail-closed.
    let cloud_requirements = cloud_requirements_loader(cloud_auth_manager, chatgpt_base_url);

    let model_provider = if oss {
        let resolved = resolve_oss_provider(
            oss_provider.as_deref(),
            &config_toml,
            config_profile.clone(),
        );

        if let Some(provider) = resolved {
            Some(provider)
        } else {
            return Err(anyhow::anyhow!(
                "No default OSS provider configured. Use --local-provider=provider or set oss_provider to one of: {LMSTUDIO_OSS_PROVIDER_ID}, {OLLAMA_OSS_PROVIDER_ID}, {OLLAMA_CHAT_PROVIDER_ID} in config.toml"
            ));
        }
    } else {
        None // No OSS mode enabled
    };

    // When using `--oss`, let the bootstrapper pick the model based on selected provider
    let model = if let Some(model) = model_cli_arg {
        Some(model)
    } else if oss {
        model_provider
            .as_ref()
            .and_then(|provider_id| get_default_model_for_oss_provider(provider_id))
            .map(std::borrow::ToOwned::to_owned)
    } else {
        None // No model specified, will use the default.
    };

    // Load configuration and determine approval policy
    let overrides = ConfigOverrides {
        model,
        review_model: None,
        config_profile,
        // Default to never ask for approvals in headless mode. Feature flags can override.
        approval_policy: Some(AskForApproval::Never),
        sandbox_mode,
        cwd: resolved_cwd,
        model_provider: model_provider.clone(),
        codex_linux_sandbox_exe,
        base_instructions: None,
        developer_instructions: None,
        personality: None,
        compact_prompt: None,
        include_apply_patch_tool: None,
        show_raw_agent_reasoning: oss.then_some(true),
        tools_web_search_request: None,
        ephemeral: None,
        additional_writable_roots: add_dir,
    };

    let config = ConfigBuilder::default()
        .cli_overrides(cli_kv_overrides)
        .harness_overrides(overrides)
        .cloud_requirements(cloud_requirements)
        .build()
        .await?;
    set_default_client_residency_requirement(config.enforce_residency.value());

    if let Err(err) = enforce_login_restrictions(&config) {
        eprintln!("{err}");
        std::process::exit(1);
    }

    let ollama_chat_support_notice = match ollama_chat_deprecation_notice(&config).await {
        Ok(notice) => notice,
        Err(err) => {
            tracing::warn!(?err, "Failed to detect Ollama wire API");
            None
        }
    };

    let otel = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        codex_core::otel_init::build_provider(&config, env!("CARGO_PKG_VERSION"), None, false)
    })) {
        Ok(Ok(otel)) => otel,
        Ok(Err(e)) => {
            eprintln!("Could not create otel exporter: {e}");
            None
        }
        Err(_) => {
            eprintln!("Could not create otel exporter: panicked during initialization");
            None
        }
    };

    let otel_logger_layer = otel.as_ref().and_then(|o| o.logger_layer());

    let otel_tracing_layer = otel.as_ref().and_then(|o| o.tracing_layer());

    let _ = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(otel_tracing_layer)
        .with(otel_logger_layer)
        .try_init();

    let mut event_processor: Box<dyn EventProcessor> = match json_mode {
        true => Box::new(EventProcessorWithJsonOutput::new(last_message_file.clone())),
        _ => Box::new(EventProcessorWithHumanOutput::create_with_ansi(
            stdout_with_ansi,
            &config,
            last_message_file.clone(),
        )),
    };
    if let Some(notice) = ollama_chat_support_notice {
        event_processor.process_event(Event {
            id: String::new(),
            msg: EventMsg::DeprecationNotice(notice),
        });
    }

    if oss {
        // We're in the oss section, so provider_id should be Some
        // Let's handle None case gracefully though just in case
        let provider_id = match model_provider.as_ref() {
            Some(id) => id,
            None => {
                error!("OSS provider unexpectedly not set when oss flag is used");
                return Err(anyhow::anyhow!(
                    "OSS provider not set but oss flag was used"
                ));
            }
        };
        ensure_oss_provider_ready(provider_id, &config)
            .await
            .map_err(|e| anyhow::anyhow!("OSS setup failed: {e}"))?;
    }

    let default_cwd = config.cwd.to_path_buf();
    let default_approval_policy = config.approval_policy.value();
    let default_sandbox_policy = config.sandbox_policy.get();
    let default_effort = config.model_reasoning_effort;
    let default_summary = config.model_reasoning_summary;

    // When --yolo (dangerously_bypass_approvals_and_sandbox) is set, also skip the git repo check
    // since the user is explicitly running in an externally sandboxed environment.
    if !skip_git_repo_check
        && !dangerously_bypass_approvals_and_sandbox
        && get_git_repo_root(&default_cwd).is_none()
    {
        eprintln!("Not inside a trusted directory and --skip-git-repo-check was not specified.");
        std::process::exit(1);
    }

    let auth_manager = AuthManager::shared(
        config.codex_home.clone(),
        true,
        config.cli_auth_credentials_store_mode,
    );
    let thread_manager = Arc::new(ThreadManager::new(
        config.codex_home.clone(),
        auth_manager.clone(),
        SessionSource::Exec,
    ));
    let default_model = thread_manager
        .get_models_manager()
        .get_default_model(&config.model, &config, RefreshStrategy::OnlineIfUncached)
        .await;

    let command = match command {
        Some(ExecCommand::Swarm(swarm_cmd)) => {
            run_swarm_command(
                swarm_cmd,
                Arc::clone(&thread_manager),
                Arc::clone(&auth_manager),
                config.clone(),
                default_model.clone(),
                json_mode,
            )
            .await?;
            return Ok(());
        }
        other => other,
    };

    // Handle resume subcommand by resolving a rollout path and using explicit resume API.
    let NewThread {
        thread_id: primary_thread_id,
        thread,
        session_configured,
    } = if let Some(ExecCommand::Resume(args)) = command.as_ref() {
        let resume_path = resolve_resume_path(&config, args).await?;

        if let Some(path) = resume_path {
            thread_manager
                .resume_thread_from_rollout(config.clone(), path, auth_manager.clone())
                .await?
        } else {
            thread_manager.start_thread(config.clone()).await?
        }
    } else {
        thread_manager.start_thread(config.clone()).await?
    };
    let (initial_operation, prompt_summary) = match (command, prompt, images) {
        (Some(ExecCommand::Review(review_cli)), _, _) => {
            let review_request = build_review_request(review_cli)?;
            let summary = codex_core::review_prompts::user_facing_hint(&review_request.target);
            (InitialOperation::Review { review_request }, summary)
        }
        (Some(ExecCommand::Swarm(_)), _, _) => {
            unreachable!("swarm command handled before initialization");
        }
        (Some(ExecCommand::Resume(args)), root_prompt, imgs) => {
            let prompt_arg = args
                .prompt
                .clone()
                .or_else(|| {
                    if args.last {
                        args.session_id.clone()
                    } else {
                        None
                    }
                })
                .or(root_prompt);
            let prompt_text = resolve_prompt(prompt_arg);
            let mut items: Vec<UserInput> = imgs
                .into_iter()
                .chain(args.images.into_iter())
                .map(|path| UserInput::LocalImage { path })
                .collect();
            items.push(UserInput::Text {
                text: prompt_text.clone(),
                // CLI input doesn't track UI element ranges, so none are available here.
                text_elements: Vec::new(),
            });
            let output_schema = load_output_schema(output_schema_path.clone());
            (
                InitialOperation::UserTurn {
                    items,
                    output_schema,
                },
                prompt_text,
            )
        }
        (None, root_prompt, imgs) => {
            let prompt_text = resolve_prompt(root_prompt);
            let mut items: Vec<UserInput> = imgs
                .into_iter()
                .map(|path| UserInput::LocalImage { path })
                .collect();
            items.push(UserInput::Text {
                text: prompt_text.clone(),
                // CLI input doesn't track UI element ranges, so none are available here.
                text_elements: Vec::new(),
            });
            let output_schema = load_output_schema(output_schema_path);
            (
                InitialOperation::UserTurn {
                    items,
                    output_schema,
                },
                prompt_text,
            )
        }
    };

    // Print the effective configuration and initial request so users can see what Codex
    // is using.
    event_processor.print_config_summary(&config, &prompt_summary, &session_configured);

    info!("Codex initialized with event: {session_configured:?}");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ThreadEventEnvelope>();
    let attached_threads = Arc::new(Mutex::new(HashSet::from([primary_thread_id])));
    spawn_thread_listener(primary_thread_id, thread.clone(), tx.clone());

    {
        let thread = thread.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::debug!("Keyboard interrupt");
                // Immediately notify Codex to abort any in-flight task.
                thread.submit(Op::Interrupt).await.ok();
            }
        });
    }

    {
        let thread_manager = Arc::clone(&thread_manager);
        let attached_threads = Arc::clone(&attached_threads);
        let tx = tx.clone();
        let mut thread_created_rx = thread_manager.subscribe_thread_created();
        tokio::spawn(async move {
            loop {
                match thread_created_rx.recv().await {
                    Ok(thread_id) => {
                        if attached_threads.lock().await.contains(&thread_id) {
                            continue;
                        }
                        match thread_manager.get_thread(thread_id).await {
                            Ok(thread) => {
                                attached_threads.lock().await.insert(thread_id);
                                spawn_thread_listener(thread_id, thread, tx.clone());
                            }
                            Err(err) => {
                                warn!("failed to attach listener for thread {thread_id}: {err}")
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        warn!("thread_created receiver lagged; skipping resync");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    match initial_operation {
        InitialOperation::UserTurn {
            items,
            output_schema,
        } => {
            let task_id = thread
                .submit(Op::UserTurn {
                    items,
                    cwd: default_cwd,
                    approval_policy: default_approval_policy,
                    sandbox_policy: default_sandbox_policy.clone(),
                    model: default_model,
                    effort: default_effort,
                    summary: default_summary,
                    final_output_json_schema: output_schema,
                    collaboration_mode: None,
                    personality: None,
                })
                .await?;
            info!("Sent prompt with event ID: {task_id}");
            task_id
        }
        InitialOperation::Review { review_request } => {
            let task_id = thread.submit(Op::Review { review_request }).await?;
            info!("Sent review request with event ID: {task_id}");
            task_id
        }
    };

    // Run the loop until the task is complete.
    // Track whether a fatal error was reported by the server so we can
    // exit with a non-zero status for automation-friendly signaling.
    let mut error_seen = false;
    while let Some(envelope) = rx.recv().await {
        let ThreadEventEnvelope {
            thread_id,
            thread,
            event,
        } = envelope;
        if let EventMsg::ElicitationRequest(ev) = &event.msg {
            // Automatically cancel elicitation requests in exec mode.
            thread
                .submit(Op::ResolveElicitation {
                    server_name: ev.server_name.clone(),
                    request_id: ev.id.clone(),
                    decision: ElicitationAction::Cancel,
                })
                .await?;
        }
        if matches!(event.msg, EventMsg::Error(_)) {
            error_seen = true;
        }
        if thread_id != primary_thread_id && matches!(&event.msg, EventMsg::TurnComplete(_)) {
            continue;
        }
        let shutdown = event_processor.process_event(event);
        if thread_id != primary_thread_id && matches!(shutdown, CodexStatus::InitiateShutdown) {
            continue;
        }
        match shutdown {
            CodexStatus::Running => continue,
            CodexStatus::InitiateShutdown => {
                thread.submit(Op::Shutdown).await?;
            }
            CodexStatus::Shutdown if thread_id == primary_thread_id => break,
            CodexStatus::Shutdown => continue,
        }
    }
    event_processor.print_final_output();
    if error_seen {
        std::process::exit(1);
    }

    Ok(())
}

fn spawn_thread_listener(
    thread_id: codex_protocol::ThreadId,
    thread: Arc<codex_core::CodexThread>,
    tx: tokio::sync::mpsc::UnboundedSender<ThreadEventEnvelope>,
) {
    tokio::spawn(async move {
        loop {
            match thread.next_event().await {
                Ok(event) => {
                    debug!("Received event: {event:?}");

                    let is_shutdown_complete = matches!(event.msg, EventMsg::ShutdownComplete);
                    if let Err(err) = tx.send(ThreadEventEnvelope {
                        thread_id,
                        thread: Arc::clone(&thread),
                        event,
                    }) {
                        error!("Error sending event: {err:?}");
                        break;
                    }
                    if is_shutdown_complete {
                        info!(
                            "Received shutdown event for thread {thread_id}, exiting event loop."
                        );
                        break;
                    }
                }
                Err(err) => {
                    error!("Error receiving event: {err:?}");
                    break;
                }
            }
        }
    });
}

async fn resolve_resume_path(
    config: &Config,
    args: &crate::cli::ResumeArgs,
) -> anyhow::Result<Option<PathBuf>> {
    if args.last {
        let default_provider_filter = vec![config.model_provider_id.clone()];
        let filter_cwd = if args.all {
            None
        } else {
            Some(config.cwd.as_path())
        };
        match codex_core::RolloutRecorder::find_latest_thread_path(
            &config.codex_home,
            1,
            None,
            codex_core::ThreadSortKey::UpdatedAt,
            &[],
            Some(default_provider_filter.as_slice()),
            &config.model_provider_id,
            filter_cwd,
        )
        .await
        {
            Ok(path) => Ok(path),
            Err(e) => {
                error!("Error listing threads: {e}");
                Ok(None)
            }
        }
    } else if let Some(id_str) = args.session_id.as_deref() {
        if Uuid::parse_str(id_str).is_ok() {
            let path = find_thread_path_by_id_str(&config.codex_home, id_str).await?;
            Ok(path)
        } else {
            let path = find_thread_path_by_name_str(&config.codex_home, id_str).await?;
            Ok(path)
        }
    } else {
        Ok(None)
    }
}

/// Minimum wait timeout to prevent tight polling loops from burning CPU.
const MIN_WAIT_TIMEOUT_MS: i64 = 10_000;
const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000;
const MAX_WAIT_TIMEOUT_MS: i64 = 300_000;

#[derive(Debug, Serialize)]
struct SwarmSpawnOutput {
    agent_id: String,
    status: AgentStatus,
}

#[derive(Debug, Serialize)]
struct SwarmSendOutput {
    submission_id: String,
}

#[derive(Debug, Serialize)]
struct SwarmWaitOutput {
    status: HashMap<String, AgentStatus>,
    timed_out: bool,
}

#[derive(Debug, Serialize)]
struct SwarmCloseOutput {
    status: AgentStatus,
}

#[derive(Debug, Serialize)]
struct SwarmStatusOutput {
    status: AgentStatus,
}

#[derive(Debug, Serialize)]
struct SwarmListOutput {
    agents: Vec<SwarmAgentInfo>,
}

#[derive(Debug, Serialize)]
struct SwarmInterruptOutput {
    submission_id: String,
}

async fn run_swarm_command(
    command: SwarmCommand,
    thread_manager: Arc<ThreadManager>,
    auth_manager: Arc<AuthManager>,
    config: Config,
    default_model: String,
    json_mode: bool,
) -> anyhow::Result<()> {
    let SwarmCommand {
        session_id,
        last,
        all,
        action,
    } = command;

    let registry = thread_manager.swarm_registry();
    registry
        .apply_storage_dir(config.swarm.hub.storage_dir.clone())
        .await;
    if let Err(err) = registry.load_from_storage().await {
        warn!("Failed to load swarm registry state: {err}");
    }

    let sender_thread_id = match action {
        SwarmAction::List(_) => None,
        _ => Some(
            resolve_swarm_sender_thread(
                &config,
                session_id.as_deref(),
                last,
                all,
                &thread_manager,
                &auth_manager,
            )
            .await?,
        ),
    };

    match action {
        SwarmAction::List(_) => {
            let agents = registry.snapshot().await;
            emit_swarm_output(json_mode, SwarmListOutput { agents }, |output| {
                if output.agents.is_empty() {
                    return "No agents in registry.".to_string();
                }
                let mut lines = Vec::new();
                for agent in &output.agents {
                    let parent = agent
                        .parent_thread_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let model = agent.model.as_deref().unwrap_or("-");
                    lines.push(format!(
                        "{}  role={}  model={}  tier={}  parent={parent}",
                        agent.thread_id, agent.role, model, agent.tier
                    ));
                }
                lines.join("\n")
            })?;
        }
        SwarmAction::Spawn(args) => {
            if args.agent_type.is_some() && args.swarm_role.is_some() {
                anyhow::bail!("Specify only one of --agent-type or --swarm-role.");
            }
            let sender_thread_id = sender_thread_id.expect("sender thread id required");
            let prompt = resolve_prompt(Some(args.message));
            if prompt.trim().is_empty() {
                anyhow::bail!("Empty message can't be sent to an agent.");
            }

            let swarm_role_name = args.swarm_role.as_deref().or_else(|| {
                if config.swarm.enabled {
                    config.swarm.default_spawn_role_name()
                } else {
                    None
                }
            });
            let swarm_role = swarm_role_name.and_then(|name| config.swarm.role(name));
            if let Some(name) = swarm_role_name
                && swarm_role.is_none()
            {
                anyhow::bail!("Unknown swarm role '{name}'.");
            }
            if config.swarm.enabled
                && let Some(role) = swarm_role
                && let Some(sender) = registry.get(sender_thread_id).await
                && !config.swarm.can_call(sender.tier, role.tier)
            {
                anyhow::bail!("Swarm hierarchy prevents spawning a higher-tier agent.");
            }

            let agent_role = args.agent_type.map(agent_role_from_arg);
            let spawn_config =
                build_spawn_config(config.clone(), &default_model, swarm_role, agent_role)?;
            let agent_model = spawn_config.model.clone();
            let new_thread_id = thread_manager
                .spawn_agent_from_thread(sender_thread_id, spawn_config, prompt)
                .await?;
            if config.swarm.enabled
                && let Some(role) = swarm_role
            {
                registry
                    .register_child(new_thread_id, sender_thread_id, role, agent_model)
                    .await;
            }
            let status = thread_manager.agent_status(new_thread_id).await;
            emit_swarm_output(
                json_mode,
                SwarmSpawnOutput {
                    agent_id: new_thread_id.to_string(),
                    status: status.clone(),
                },
                |_| format!("spawned {new_thread_id} ({})", format_agent_status(&status)),
            )?;
        }
        SwarmAction::Send(args) => {
            let sender_thread_id = sender_thread_id.expect("sender thread id required");
            let receiver_thread_id = parse_thread_id(&args.id)?;
            if args.message.trim().is_empty() {
                anyhow::bail!("Empty message can't be sent to an agent.");
            }
            if config.swarm.enabled {
                if let (Some(sender), Some(receiver)) = (
                    registry.get(sender_thread_id).await,
                    registry.get(receiver_thread_id).await,
                ) && !config.swarm.can_call(sender.tier, receiver.tier)
                {
                    anyhow::bail!("Swarm hierarchy prevents sending input to a higher-tier agent.");
                }
            }
            if args.interrupt {
                let _ = thread_manager.interrupt_agent(receiver_thread_id).await?;
            }
            let prompt = resolve_prompt(Some(args.message));
            let submission_id = thread_manager
                .send_agent_prompt(receiver_thread_id, prompt)
                .await?;
            emit_swarm_output(json_mode, SwarmSendOutput { submission_id }, |output| {
                format!("submission_id={}", output.submission_id)
            })?;
        }
        SwarmAction::Wait(args) => {
            let timeout_ms = resolve_wait_timeout(args.timeout_ms)?;
            let ids = args
                .ids
                .iter()
                .map(|id| parse_thread_id(id))
                .collect::<anyhow::Result<Vec<_>>>()?;
            if ids.is_empty() {
                anyhow::bail!("Must provide at least one agent id.");
            }

            let mut status_rxs = Vec::with_capacity(ids.len());
            let mut initial_final_statuses = Vec::new();
            for id in &ids {
                match thread_manager.subscribe_agent_status(*id).await {
                    Ok(rx) => {
                        let status = rx.borrow().clone();
                        if is_final_status(&status) {
                            initial_final_statuses.push((*id, status));
                        }
                        status_rxs.push((*id, rx));
                    }
                    Err(codex_core::error::CodexErr::ThreadNotFound(_)) => {
                        initial_final_statuses.push((*id, AgentStatus::NotFound));
                    }
                    Err(err) => {
                        return Err(err.into());
                    }
                }
            }

            let statuses = if !initial_final_statuses.is_empty() {
                initial_final_statuses
            } else {
                let mut join_set = JoinSet::new();
                for (id, rx) in status_rxs.into_iter() {
                    let manager = Arc::clone(&thread_manager);
                    join_set.spawn(wait_for_final_status(manager, id, rx));
                }
                let deadline = Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
                match tokio::time::timeout_at(deadline, join_set.join_next()).await {
                    Ok(Some(Ok(Some(result)))) => vec![result],
                    _ => Vec::new(),
                }
            };

            let mut status_map = HashMap::new();
            for (thread_id, status) in statuses {
                status_map.insert(thread_id.to_string(), status);
            }
            let timed_out = status_map.is_empty();
            emit_swarm_output(
                json_mode,
                SwarmWaitOutput {
                    status: status_map.clone(),
                    timed_out,
                },
                |_| format_wait_output(&status_map, timed_out),
            )?;
        }
        SwarmAction::Close(args) => {
            let sender_thread_id = sender_thread_id.expect("sender thread id required");
            let agent_id = parse_thread_id(&args.id)?;
            if config.swarm.enabled {
                if let (Some(sender), Some(receiver)) = (
                    registry.get(sender_thread_id).await,
                    registry.get(agent_id).await,
                ) && !config.swarm.can_call(sender.tier, receiver.tier)
                {
                    anyhow::bail!("Swarm hierarchy prevents closing a higher-tier agent.");
                }
            }
            let status = match thread_manager.subscribe_agent_status(agent_id).await {
                Ok(mut status_rx) => status_rx.borrow_and_update().clone(),
                Err(err) => {
                    thread_manager.agent_status(agent_id).await;
                    return Err(err.into());
                }
            };
            if !matches!(status, AgentStatus::Shutdown) {
                let _ = thread_manager.shutdown_agent(agent_id).await?;
            }
            emit_swarm_output(
                json_mode,
                SwarmCloseOutput {
                    status: status.clone(),
                },
                |_| format!("closed {agent_id} ({})", format_agent_status(&status)),
            )?;
        }
        SwarmAction::Interrupt(args) => {
            let agent_id = parse_thread_id(&args.id)?;
            let submission_id = thread_manager.interrupt_agent(agent_id).await?;
            emit_swarm_output(
                json_mode,
                SwarmInterruptOutput { submission_id },
                |output| format!("submission_id={}", output.submission_id),
            )?;
        }
        SwarmAction::Status(args) => {
            let agent_id = parse_thread_id(&args.id)?;
            let status = thread_manager.agent_status(agent_id).await;
            emit_swarm_output(
                json_mode,
                SwarmStatusOutput {
                    status: status.clone(),
                },
                |_| format!("status={} ({})", agent_id, format_agent_status(&status)),
            )?;
        }
    }

    Ok(())
}

async fn resolve_swarm_sender_thread(
    config: &Config,
    session_id: Option<&str>,
    last: bool,
    all: bool,
    thread_manager: &ThreadManager,
    auth_manager: &Arc<AuthManager>,
) -> anyhow::Result<ThreadId> {
    let resume_path = resolve_swarm_resume_path(config, session_id, last, all).await?;
    let NewThread { thread_id, .. } = if let Some(path) = resume_path {
        thread_manager
            .resume_thread_from_rollout(config.clone(), path, Arc::clone(auth_manager))
            .await?
    } else {
        thread_manager.start_thread(config.clone()).await?
    };
    Ok(thread_id)
}

async fn resolve_swarm_resume_path(
    config: &Config,
    session_id: Option<&str>,
    last: bool,
    all: bool,
) -> anyhow::Result<Option<PathBuf>> {
    if last {
        let default_provider_filter = vec![config.model_provider_id.clone()];
        let filter_cwd = if all {
            None
        } else {
            Some(config.cwd.as_path())
        };
        match codex_core::RolloutRecorder::find_latest_thread_path(
            &config.codex_home,
            1,
            None,
            codex_core::ThreadSortKey::UpdatedAt,
            &[],
            Some(default_provider_filter.as_slice()),
            &config.model_provider_id,
            filter_cwd,
        )
        .await
        {
            Ok(path) => Ok(path),
            Err(e) => {
                error!("Error listing threads: {e}");
                Ok(None)
            }
        }
    } else if let Some(id_str) = session_id {
        if Uuid::parse_str(id_str).is_ok() {
            let path = find_thread_path_by_id_str(&config.codex_home, id_str).await?;
            Ok(path)
        } else {
            let path = find_thread_path_by_name_str(&config.codex_home, id_str).await?;
            Ok(path)
        }
    } else {
        Ok(None)
    }
}

fn agent_role_from_arg(arg: AgentTypeArg) -> AgentRole {
    match arg {
        AgentTypeArg::Default => AgentRole::Default,
        AgentTypeArg::Explorer => AgentRole::Explorer,
        AgentTypeArg::Worker => AgentRole::Worker,
        AgentTypeArg::Orchestrator => AgentRole::Orchestrator,
    }
}

fn build_spawn_config(
    mut config: Config,
    default_model: &str,
    swarm_role: Option<&SwarmRole>,
    agent_role: Option<AgentRole>,
) -> anyhow::Result<Config> {
    if let Some(role) = swarm_role {
        if let Some(model) = role.model.as_ref() {
            config.model = Some(model.clone());
        }
        if let Some(role_instructions) = role.base_instructions.as_ref()
            && !role_instructions.trim().is_empty()
        {
            config.base_instructions = Some(match config.base_instructions.as_ref() {
                Some(current) if !current.trim().is_empty() => {
                    format!("{current}\n\n{role_instructions}")
                }
                _ => role_instructions.clone(),
            });
        }
    }
    if config.model.is_none() {
        config.model = Some(default_model.to_string());
    }
    if let Some(agent_role) = agent_role {
        agent_role
            .apply_to_config(&mut config)
            .map_err(anyhow::Error::msg)?;
    }
    Ok(config)
}

fn parse_thread_id(id: &str) -> anyhow::Result<codex_protocol::ThreadId> {
    codex_protocol::ThreadId::from_string(id)
        .map_err(|err| anyhow::anyhow!("invalid agent id {id}: {err:?}"))
}

fn resolve_wait_timeout(timeout_ms: Option<i64>) -> anyhow::Result<i64> {
    let Some(timeout_ms) = timeout_ms else {
        return Ok(DEFAULT_WAIT_TIMEOUT_MS);
    };
    if timeout_ms <= 0 {
        anyhow::bail!("timeout_ms must be positive");
    }
    Ok(timeout_ms.clamp(MIN_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS))
}

async fn wait_for_final_status(
    manager: Arc<ThreadManager>,
    thread_id: codex_protocol::ThreadId,
    mut status_rx: tokio::sync::watch::Receiver<AgentStatus>,
) -> Option<(codex_protocol::ThreadId, AgentStatus)> {
    let mut status = status_rx.borrow().clone();
    if is_final_status(&status) {
        return Some((thread_id, status));
    }

    loop {
        if status_rx.changed().await.is_err() {
            let latest = manager.agent_status(thread_id).await;
            return is_final_status(&latest).then_some((thread_id, latest));
        }
        status = status_rx.borrow().clone();
        if is_final_status(&status) {
            return Some((thread_id, status));
        }
    }
}

fn is_final_status(status: &AgentStatus) -> bool {
    !matches!(status, AgentStatus::PendingInit | AgentStatus::Running)
}

fn format_wait_output(statuses: &HashMap<String, AgentStatus>, timed_out: bool) -> String {
    if timed_out {
        return "timed out".to_string();
    }
    let mut lines = Vec::new();
    for (id, status) in statuses {
        lines.push(format!("{id}: {}", format_agent_status(status)));
    }
    lines.join("\n")
}

fn format_agent_status(status: &AgentStatus) -> String {
    match status {
        AgentStatus::PendingInit => "pending init".to_string(),
        AgentStatus::Running => "running".to_string(),
        AgentStatus::Completed(Some(message)) => {
            let preview = truncate_preview(message.trim(), 120);
            if preview.is_empty() {
                "completed".to_string()
            } else {
                format!("completed: \"{preview}\"")
            }
        }
        AgentStatus::Completed(None) => "completed".to_string(),
        AgentStatus::Errored(message) => {
            let preview = truncate_preview(message.trim(), 120);
            if preview.is_empty() {
                "errored".to_string()
            } else {
                format!("errored: \"{preview}\"")
            }
        }
        AgentStatus::Shutdown => "shutdown".to_string(),
        AgentStatus::NotFound => "not found".to_string(),
    }
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let preview = text.chars().take(max_chars).collect::<String>();
    format!("{preview}…")
}

fn emit_swarm_output<T, F>(json_mode: bool, output: T, human: F) -> anyhow::Result<()>
where
    T: Serialize,
    F: FnOnce(&T) -> String,
{
    if json_mode {
        let payload = serde_json::to_string(&output)?;
        #[allow(clippy::print_stdout)]
        {
            println!("{payload}");
        }
        return Ok(());
    }

    let text = human(&output);
    #[allow(clippy::print_stdout)]
    {
        println!("{text}");
    }
    Ok(())
}

fn load_output_schema(path: Option<PathBuf>) -> Option<Value> {
    let path = path?;

    let schema_str = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) => {
            eprintln!(
                "Failed to read output schema file {}: {err}",
                path.display()
            );
            std::process::exit(1);
        }
    };

    match serde_json::from_str::<Value>(&schema_str) {
        Ok(value) => Some(value),
        Err(err) => {
            eprintln!(
                "Output schema file {} is not valid JSON: {err}",
                path.display()
            );
            std::process::exit(1);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptDecodeError {
    InvalidUtf8 { valid_up_to: usize },
    InvalidUtf16 { encoding: &'static str },
    UnsupportedBom { encoding: &'static str },
}

impl std::fmt::Display for PromptDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PromptDecodeError::InvalidUtf8 { valid_up_to } => write!(
                f,
                "input is not valid UTF-8 (invalid byte at offset {valid_up_to}). Convert it to UTF-8 and retry (e.g., `iconv -f <ENC> -t UTF-8 prompt.txt`)."
            ),
            PromptDecodeError::InvalidUtf16 { encoding } => write!(
                f,
                "input looked like {encoding} but could not be decoded. Convert it to UTF-8 and retry."
            ),
            PromptDecodeError::UnsupportedBom { encoding } => write!(
                f,
                "input appears to be {encoding}. Convert it to UTF-8 and retry."
            ),
        }
    }
}

fn decode_prompt_bytes(input: &[u8]) -> Result<String, PromptDecodeError> {
    let input = input.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(input);

    if input.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        return Err(PromptDecodeError::UnsupportedBom {
            encoding: "UTF-32LE",
        });
    }

    if input.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        return Err(PromptDecodeError::UnsupportedBom {
            encoding: "UTF-32BE",
        });
    }

    if let Some(rest) = input.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, "UTF-16LE", u16::from_le_bytes);
    }

    if let Some(rest) = input.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, "UTF-16BE", u16::from_be_bytes);
    }

    std::str::from_utf8(input)
        .map(str::to_string)
        .map_err(|e| PromptDecodeError::InvalidUtf8 {
            valid_up_to: e.valid_up_to(),
        })
}

fn decode_utf16(
    input: &[u8],
    encoding: &'static str,
    decode_unit: fn([u8; 2]) -> u16,
) -> Result<String, PromptDecodeError> {
    if !input.len().is_multiple_of(2) {
        return Err(PromptDecodeError::InvalidUtf16 { encoding });
    }

    let units: Vec<u16> = input
        .chunks_exact(2)
        .map(|chunk| decode_unit([chunk[0], chunk[1]]))
        .collect();

    String::from_utf16(&units).map_err(|_| PromptDecodeError::InvalidUtf16 { encoding })
}

fn resolve_prompt(prompt_arg: Option<String>) -> String {
    match prompt_arg {
        Some(p) if p != "-" => p,
        maybe_dash => {
            let force_stdin = matches!(maybe_dash.as_deref(), Some("-"));

            if std::io::stdin().is_terminal() && !force_stdin {
                eprintln!(
                    "No prompt provided. Either specify one as an argument or pipe the prompt into stdin."
                );
                std::process::exit(1);
            }

            if !force_stdin {
                eprintln!("Reading prompt from stdin...");
            }

            let mut bytes = Vec::new();
            if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
                eprintln!("Failed to read prompt from stdin: {e}");
                std::process::exit(1);
            }

            let buffer = match decode_prompt_bytes(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to read prompt from stdin: {e}");
                    std::process::exit(1);
                }
            };

            if buffer.trim().is_empty() {
                eprintln!("No prompt provided via stdin.");
                std::process::exit(1);
            }
            buffer
        }
    }
}

fn build_review_request(args: ReviewArgs) -> anyhow::Result<ReviewRequest> {
    let target = if args.uncommitted {
        ReviewTarget::UncommittedChanges
    } else if let Some(branch) = args.base {
        ReviewTarget::BaseBranch { branch }
    } else if let Some(sha) = args.commit {
        ReviewTarget::Commit {
            sha,
            title: args.commit_title,
        }
    } else if let Some(prompt_arg) = args.prompt {
        let prompt = resolve_prompt(Some(prompt_arg)).trim().to_string();
        if prompt.is_empty() {
            anyhow::bail!("Review prompt cannot be empty");
        }
        ReviewTarget::Custom {
            instructions: prompt,
        }
    } else {
        anyhow::bail!(
            "Specify --uncommitted, --base, --commit, or provide custom review instructions"
        );
    };

    Ok(ReviewRequest {
        target,
        user_facing_hint: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn builds_uncommitted_review_request() {
        let request = build_review_request(ReviewArgs {
            uncommitted: true,
            base: None,
            commit: None,
            commit_title: None,
            prompt: None,
        })
        .expect("builds uncommitted review request");

        let expected = ReviewRequest {
            target: ReviewTarget::UncommittedChanges,
            user_facing_hint: None,
        };

        assert_eq!(request, expected);
    }

    #[test]
    fn builds_commit_review_request_with_title() {
        let request = build_review_request(ReviewArgs {
            uncommitted: false,
            base: None,
            commit: Some("123456789".to_string()),
            commit_title: Some("Add review command".to_string()),
            prompt: None,
        })
        .expect("builds commit review request");

        let expected = ReviewRequest {
            target: ReviewTarget::Commit {
                sha: "123456789".to_string(),
                title: Some("Add review command".to_string()),
            },
            user_facing_hint: None,
        };

        assert_eq!(request, expected);
    }

    #[test]
    fn builds_custom_review_request_trims_prompt() {
        let request = build_review_request(ReviewArgs {
            uncommitted: false,
            base: None,
            commit: None,
            commit_title: None,
            prompt: Some("  custom review instructions  ".to_string()),
        })
        .expect("builds custom review request");

        let expected = ReviewRequest {
            target: ReviewTarget::Custom {
                instructions: "custom review instructions".to_string(),
            },
            user_facing_hint: None,
        };

        assert_eq!(request, expected);
    }

    #[test]
    fn decode_prompt_bytes_strips_utf8_bom() {
        let input = [0xEF, 0xBB, 0xBF, b'h', b'i', b'\n'];

        let out = decode_prompt_bytes(&input).expect("decode utf-8 with BOM");

        assert_eq!(out, "hi\n");
    }

    #[test]
    fn decode_prompt_bytes_decodes_utf16le_bom() {
        // UTF-16LE BOM + "hi\n"
        let input = [0xFF, 0xFE, b'h', 0x00, b'i', 0x00, b'\n', 0x00];

        let out = decode_prompt_bytes(&input).expect("decode utf-16le with BOM");

        assert_eq!(out, "hi\n");
    }

    #[test]
    fn decode_prompt_bytes_decodes_utf16be_bom() {
        // UTF-16BE BOM + "hi\n"
        let input = [0xFE, 0xFF, 0x00, b'h', 0x00, b'i', 0x00, b'\n'];

        let out = decode_prompt_bytes(&input).expect("decode utf-16be with BOM");

        assert_eq!(out, "hi\n");
    }

    #[test]
    fn decode_prompt_bytes_rejects_utf32le_bom() {
        // UTF-32LE BOM + "hi\n"
        let input = [
            0xFF, 0xFE, 0x00, 0x00, b'h', 0x00, 0x00, 0x00, b'i', 0x00, 0x00, 0x00, b'\n', 0x00,
            0x00, 0x00,
        ];

        let err = decode_prompt_bytes(&input).expect_err("utf-32le should be rejected");

        assert_eq!(
            err,
            PromptDecodeError::UnsupportedBom {
                encoding: "UTF-32LE"
            }
        );
    }

    #[test]
    fn decode_prompt_bytes_rejects_utf32be_bom() {
        // UTF-32BE BOM + "hi\n"
        let input = [
            0x00, 0x00, 0xFE, 0xFF, 0x00, 0x00, 0x00, b'h', 0x00, 0x00, 0x00, b'i', 0x00, 0x00,
            0x00, b'\n',
        ];

        let err = decode_prompt_bytes(&input).expect_err("utf-32be should be rejected");

        assert_eq!(
            err,
            PromptDecodeError::UnsupportedBom {
                encoding: "UTF-32BE"
            }
        );
    }

    #[test]
    fn decode_prompt_bytes_rejects_invalid_utf8() {
        // Invalid UTF-8 sequence: 0xC3 0x28
        let input = [0xC3, 0x28];

        let err = decode_prompt_bytes(&input).expect_err("invalid utf-8 should fail");

        assert_eq!(err, PromptDecodeError::InvalidUtf8 { valid_up_to: 0 });
    }
}
