# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

CleanRSTAI is a desktop AI chat client (Rust, egui/eframe) for OpenAI-compatible endpoints
with MCP support, built-in tools, subagents and per-chat workspace folders. It is a single
binary crate (no lib target); edition 2024, `rust-version = "1.95"`.

## Commands

```sh
cargo run --release                          # run the app
cargo fmt --check                            # CI checks formatting (on Linux)
cargo clippy --all-targets -- -D warnings    # CI fails on any warning
cargo test                                   # unit tests
cargo test <name>                            # one test or module, e.g. `cargo test tools::workspace`
cargo test -- --ignored                      # e2e: mock LLM + `npx -y @modelcontextprotocol/server-everything`
                                             # over stdio and Streamable HTTP (needs Node/npx)
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy, `cargo test` and the ignored e2e tests on
self-hosted runners (Gentoo Linux x64, Mac mini ARM64); Windows is intentionally disabled. A
step puts `$CARGO_HOME/bin` first on `PATH` because the Gentoo box has an older system Rust.

## Releases

- The release tag must equal `v` + the `Cargo.toml` version (the workflow checks it), so
  bump `version` in `Cargo.toml` (and `Cargo.lock`, via `cargo build`) in a PR first.
- Then run `.github/workflows/release.yml` by `workflow_dispatch` with `version: vX.Y.Z`
  (or push the tag). It builds a universal macOS `.app`/`.dmg`/`.zip` on the Mac and
  cross-builds both Linux tarballs with `cargo-zigbuild` against glibc 2.28, then creates the
  GitHub Release with the files and `SHA256SUMS.txt`.
- `scripts/release-local.sh [--publish] [--skip-tests]` does the same from a local machine.
- The repository name is hard-coded in `src/update.rs` (`LATEST_RELEASE_URL`) and
  `scripts/release-local.sh`.
- `roadmap.md` tracks which items shipped in which version.

## Architecture

**Threads and events.** egui runs on the main thread (`app::ChatApp`); network and MCP I/O
run on a tokio runtime created in `main.rs`. Background work sends `agent::AppEvent`s over an
unbounded channel that `ChatApp::handle_events` drains each frame, and calls
`ctx.request_repaint()`. UI code never awaits.

**One generation** (`agent::run`) streams a completion (`llm::chat_completion`, SSE via
`sse.rs`), emits `ChatEvent`s (deltas, final message, tool approval, tool result, done), runs
any tool calls and loops until the model answers without tools or hits `max_tool_rounds`.
The toolbox is rebuilt every round so MCP servers that finish connecting are picked up.

**Tools** (`tools/mod.rs`): a `Toolbox` for one request combines:
- built-ins named `builtin__*` (time, calculate, update_tasks, fetch, delegate);
- workspace tools with plain names (`read_file`, `file_glob_search`, `grep_search`,
  `get_info`, `write_file`, `edit_file`, `exec_shell_command`, in `tools/workspace.rs`),
  offered only when the chat has a `workspace` folder;
- MCP tools routed as `<server>__<tool>` by `mcp::manager::ToolRouter`.

`ToolSelection` resolves what is on: Settings defaults plus per-chat `ToolOverrides` keyed
`builtin:<key>`, `server:<id>`, `tool:<server id>:<tool>`. `Toolbox::needs_approval` decides
which calls ask first (MCP tools, `delegate`, `fetch`, and the workspace tools that write or
run commands); everything else runs directly. Workspace paths are confined to the chosen
folder (`..`, outside absolute paths and symlinks out are refused).

**Approvals** (`approval.rs`): the agent sends `ChatEvent::ToolApproval` with a oneshot
sender and the call's MCP origin; the app holds `PendingApproval`s on the `Run` and answers
them. "Always allow this tool/server" is an in-memory per-chat `AlwaysAllow` list keyed by
server id and MCP tool name (not the routed function name), so it survives renames.

**Subagents** (`subagent.rs`): the `delegate` tool runs tasks in parallel, each with its own
model/tool loop and a derived `ToolSelection` (never the task list, `delegate` or workspace
tools; time/calculator/fetch only when allowed for subagents). Progress streams back as
`SubagentProgress` events.

**MCP** (`mcp/`): stdio and Streamable HTTP transports, OAuth (PKCE, dynamic registration,
loopback redirect) in `mcp/oauth.rs`, and `McpManager`, which keeps connections in sync with
settings, refreshes tools/resources/prompts on `list_changed`, and builds the `ToolRouter`.

**Persistence**: settings are one JSON file (`config.rs`, paths via `directories` with
`ProjectDirs("net", "bukn", "CleanRSTAI")`); each conversation is its own JSON file
(`types::Conversation`). New fields must be `#[serde(default)]` (usually with
`skip_serializing_if`) so older files still load. API keys and MCP secrets go to the OS
keychain through the `secrets::SecretStore` trait (tests use the in-memory store), with
plain-text fallback when the keychain is off or unavailable.

**Providers**: `ProviderKind` (generic OpenAI-compatible, llama.cpp, LiteLLM) is detected by
`llm::detect_kind` and decides which stats are read (`stats.rs`: server timings vs. measured
prefill/decode, cost, served model).

## UI conventions

- The look follows the Claude desktop app: palette, fonts and widget styling live in
  `ui/theme.rs` (`theme::palette(dark)`). Fonts are bundled in `assets/fonts` (Instrument
  Sans for the interface, Newsreader serif for the greeting and replies, IBM Plex Mono for
  code, Phosphor for icons; codepoints in `theme::icon`).
- egui 0.37 takes button text from `TextStyle::Body`, so `Body` is the sans interface style;
  prose uses the custom `theme::reading()` style, applied around markdown in `ui/markdown.rs`.
- `ui/markdown.rs` splits replies into markdown segments (rendered by `egui_commonmark`) and
  drawings: closed top-level ```` ```mermaid ```` / ```` ```svg ```` blocks and inline
  `<svg>`, rendered to SVG and shown through egui's SVG loader. Web images become links
  unless "Load web images in replies" is on.
- Widget ids inside a reply are scoped by message index (`push_id`), and tool results are
  matched to the tool messages that directly follow their assistant message, because some
  servers reuse call ids like `call_0` across turns.
- Chat views return `Action`s that `ChatApp::apply_actions` applies after drawing, rather
  than mutating app state while borrowed.

## Testing helpers

- `testutil::serve` starts a mock HTTP server on a free port and records requests; agent,
  LLM, fetch and MCP-over-HTTP tests use it as a fake OpenAI endpoint.
- Workspace and fetch tests use real temp folders and real HTTP against that server.
