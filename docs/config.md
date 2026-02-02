# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Connecting to MCP servers

Codex can connect to MCP servers configured in `~/.codex/config.toml`. See the configuration reference for the latest MCP server options:

- https://developers.openai.com/codex/config-reference

## Apps (Connectors)

Use `$` in the composer to insert a ChatGPT connector; the popover lists accessible
apps. The `/apps` command lists available and installed apps. Connected apps appear first
and are labeled as connected; others are marked as can be installed.

## Notify

Codex can run a notification hook when the agent finishes a turn. See the configuration reference for the latest notification settings:

- https://developers.openai.com/codex/config-reference

## Swarm (multi-agent)

Swarm settings live under the `[swarm]` table in `config.toml` and are optional.
Keys use `camelCase` to match the schema.

Example:

```toml
[swarm]
enabled = true
rootRole = "Scholar"
defaultSpawnRole = "Scribe"

[[swarm.roles]]
name = "Scout"
model = "gpt-5.1-codex-mini"
tier = 0
baseInstructions = "High-speed triage and acquisition."

[swarm.hierarchy]
allowUpwardCalls = false
allowSameTierCalls = true

[swarm.hub]
leakTrackerPath = "C:\\\\Users\\\\you\\\\.codex\\\\swarm\\\\leaks.json"
storageDir = "C:\\\\Users\\\\you\\\\.codex\\\\swarm"
```

Swarm Hub state is shared across all swarm agents in a session.

## JSON Schema

The generated JSON Schema for `config.toml` lives at `codex-rs/core/config.schema.json`.

## Notices

Codex stores "do not show again" flags for some UI prompts under the `[notice]` table.

Ctrl+C/Ctrl+D quitting uses a ~1 second double-press hint (`ctrl + c again to quit`).
