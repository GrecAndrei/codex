Swarm multi-agent plan (draft)

Status: Core config/runtime + Swarm Hub tool implemented; TUI full-screen swarm dashboard overlay in progress; settings/CLI/exec wiring pending.

Goals
- Full multi-agent support across TUI, CLI, and exec.
- Three default roles with configurable models and prompts:
  - Scout (gpt-5.1-codex-mini): high throughput, acquisition, triage.
  - Scribe (gpt-5.1-codex / max): mapping, audit, management.
  - Scholar (gpt-5.2-codex): deep reasoning, synthesis.
- Hierarchy: higher tier can call any lower tier. Model mapping remains configurable.
- Agents can message each other directly (not just parent to child).
- All agents can use Codex tools and MCPs, subject to existing sandbox/approval policies.
- Swarm Hub is shared state across all agents (global for a session).
- Swarm Hub implemented as native tools, not MCP.
- Everything configurable in the existing Settings UI.
- Leak tracker path is configurable, and selection is managed by the LLM.

Core features to implement
- Agent registry: track role, model, status, parent/child, last update.
- Inter-agent routing: allow send_input between any agents with hierarchy rule.
- Collab tool behavior: keep spawn limits, but do not block messaging for sub-agents.
- Persistence: swarm metadata and hub state survive resume.

Swarm Hub tools (native)
- Timer: context-aware time allocation, start/stop, reminders.
- Voting: weighted votes (Scholar=2, Scout/Scribe=1) with outcome.
- Lounge: shared brainstorm buffer (append/read/clear).
- Leak tracker: structured store, add/list/export entries.
- Task queue: ownership, status, priority.
- Evidence ledger: findings with source and confidence.
- Decision log: rationale snapshots for key choices.
- Artifact index: shared list of extracted files and locations.
- Risk register and consensus rules (optional but recommended).
- Global budget/kill switch (optional safety).

UI scope (based on actual TUI architecture; full-screen view requested)
- TUI: full-screen Swarm Dashboard overlay (new `/swarm` command).
  - Tabs: All agents, per-agent, and Hub.
  - All-agents view shows messages + tool use using the same HistoryCell rendering as the main chat.
  - Per-agent view mirrors the single-agent transcript output, color-coded by agent.
  - Hub view renders lounge, timer, votes, leak tracker, tasks, evidence, decisions, artifacts.
- CLI: codex agent spawn/list/send/wait/close.
- Exec: agent and swarm flags for automation.
- Settings: Swarm section that configures roles, models, prompts, hub settings.

Notes
- Intended for authorized analysis only.
- Keep role/tool access configurable with sane defaults.
