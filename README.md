<p align="center"><strong>Codex CLI (Custom Mod)</strong> is a local coding agent based on OpenAI’s Codex CLI.</p>
<p align="center">This fork is not published to the official npm or Homebrew registries.</p>
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>
</br>
If you want the official Codex in your IDE or on the web, refer to OpenAI’s documentation. This repository is a custom modification of the CLI.</p>

---

## Quickstart

### Installing and running Codex CLI (custom fork)

There is no official npm or Homebrew package for this fork. Use one of the options below.

#### Option A: Download a prebuilt binary

- Windows: download the `codex-windows-x64.zip` artifact from the **Windows Build** workflow.
- macOS/Linux: use a published GitHub Release if one exists for this fork.

Then run `codex` (or `codex.exe`) to get started.

#### Option B: Build from source

```shell
cd codex-rs
cargo build -p codex-cli --bin codex --release
```

Run the binary:

```shell
# macOS/Linux
./target/release/codex

# Windows
.\target\release\codex.exe
```

### Authentication

This fork still supports the same authentication flows as upstream Codex CLI. Follow the prompts in the CLI or refer to the docs in this repo if you need help.

## Docs

- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
