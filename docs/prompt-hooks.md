# Prompt hooks

Codex now supports a sidecar prompt hook file at `~/.codex/prompt-hooks.toml`.

This is the low-merge-pain way to customize prompt behavior without rewriting
the whole repo every time upstream changes something.

## What it can hook

These top-level sections are supported:

- `base_instructions`
- `developer_message`
- `user_context`
- `compact_prompt`
- `review_prompt`

Each section supports:

- `enabled = true|false`
- `mode = "append" | "prepend" | "replace"`
- `text = """..."""`
- `file = "relative/or/absolute/path.txt"`

Relative `file` paths are resolved against `~/.codex/`.

If both `file` and `text` are present, the file contents are loaded first and
`text` is appended after them.

## Merge behavior

- `append`: add your text after the built-in text
- `prepend`: add your text before the built-in text
- `replace`: replace the built-in text entirely

For `developer_message` and `user_context`, the hook applies to the whole
assembled message payload, not each tiny subsection individually.

## Why this exists

The built-in prompt stack in Codex is split across:

- API `instructions`
- developer-role messages
- contextual user messages

That makes one-off prompt hacks annoying and fragile. Prompt hooks give you one
place to bend those layers without turning upstream rebases into a nightmare.

## Example

See `~/.codex/docs/prompt-hooks.example.toml` after Codex starts, or check
`docs/prompt-hooks.example.toml` in the repo.

## Notes

- `base_instructions` is the strongest hook here because it lands in the API
  `instructions` field.
- `developer_message` is good when you want strong turn guidance without
  fully replacing the base prompt.
- `user_context` lets you reshape the AGENTS/environment-style contextual user
  message.
- `compact_prompt` changes the auto-compaction summarization prompt.
- `review_prompt` changes the review sub-agent rubric.
