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
self-hosted runners (Gentoo Linux x64, Mac mini ARM64, DARKBASE Windows x64). A step puts
`$CARGO_HOME/bin` first on `PATH` because the Gentoo box has an older system Rust (skipped on
Windows). The runners are persistent machines, so the release workflow installs tools once
(WiX into the runner's tool cache) and removes signing certificates in always-run steps.
For the same reason both workflows build into a persistent `CARGO_TARGET_DIR` in the runner's
tool cache (`.github/actions/cargo-target`, cleared when `rustc -vV` changes or past 30 GiB)
instead of using the GitHub Actions cache, so paths in the release steps use
`$CARGO_TARGET_DIR`, not `target/`.
`.github/workflows/sandbox-image.yml` builds `assets/sandbox/full.Dockerfile` (amd64 + arm64 via
QEMU, on any self-hosted Linux x64 runner with Docker) and pushes
`ghcr.io/<owner>/cleanrstai-sandbox` (`full`/`latest` from main, `full-dev` from dev, plus
`full-<sha256-12>` and `sha-<commit>`); it runs only when that file changes, or by hand.
The repository is public, so CI skips pull requests from forks (they would run on those
machines); keep "Require approval for all external contributors" on in Settings → Actions, since
a fork's PR could edit that check.

## Releases

- The release tag must equal `v` + the `Cargo.toml` version (the workflow checks it), so
  bump `version` in `Cargo.toml` (and `Cargo.lock`, via `cargo build`) in a PR first.
- A tag with a `-` (e.g. `v0.8.0-dev.1`) makes a pre-release, which the in-app update check
  skips; the workflow strips the suffix for the `.msi` version, which must be numeric.
- Then run `.github/workflows/release.yml` by `workflow_dispatch` with `version: vX.Y.Z`
  (or push the tag). It builds a universal macOS `.app`/`.dmg`/`.zip` on the Mac, the
  Windows `.zip` and `.msi` on DARKBASE, and cross-builds both Linux tarballs with `cargo-zigbuild` against glibc 2.28, then creates the
  GitHub Release with the files and `SHA256SUMS.txt`.
- `scripts/release-local.sh [--publish] [--skip-tests]` does the same from a local machine.
- The repository name is hard-coded in `src/update.rs` (the `repo!` macro, used for
  `LATEST_RELEASE_URL`, the default of the "Update from" setting `general.update_url`, and the
  fetch tool's user agent) and `scripts/release-local.sh`.
- The in-app updater (`update::platform_package`) downloads release files by name:
  `CleanRSTAI-macos-universal.zip` (a `ditto` zip of the `.app`),
  `CleanRSTAI-linux-<arch>.tar.gz` (the bare `cleanrstai` binary),
  `CleanRSTAI-windows-<arch>.zip` (the bare `cleanrstai.exe`, for a portable copy in a
  writable folder; the running exe is renamed to `.old` and deleted at the next start),
  `CleanRSTAI-windows-<arch>.msi` (run with `msiexec` for a copy installed in Program Files)
  and `SHA256SUMS.txt`. Keep those names and layouts in the release workflow.
- `build.rs` embeds `assets/icon/icon.ico` in the Windows `.exe` (`winresource`); both WiX
  files take the icon as the `Icon` define.
- `roadmap.md` tracks which items shipped in which version.

## Documentation

The user docs live in `docs/` (Just the Docs remote theme, built by GitHub Pages from `main`
`/docs`, no workflow). When a change adds, removes or renames a feature, setting, default,
range or shortcut, update the matching page in the same PR:

- settings (labels, defaults and ranges in `ui/settings.rs` / `config.rs`) →
  `docs/settings/<tab>.md` (the Docker tab is `docs/settings/docker.md`);
- chats, attachments, tools and approvals, MCP, subagents, updates → `docs/using/*.md`;
- shortcuts → `docs/reference/keyboard-shortcuts.md`; storage, secrets or anything that
  leaves the computer → `docs/reference/data-and-privacy.md`;
- install steps or release file names → `docs/getting-started/installation.md`.

Keep the README's feature list in step with the docs. A new page needs `title` and
`nav_order` front matter, and `parent` matching the parent page's `title` exactly when nested.
Link pages with relative `.html` paths and images with `{{ '/name.png' | relative_url }}`;
the README's screenshots stay in `docs/`. The light/dark toggle is in `docs/_includes/`
(`head_custom.html` applies the saved or system theme, `header_custom.html` is the button);
the theme minifies HTML onto one line, so inline scripts there must use `/* */` comments,
never `//`. Preview with `cd docs && bundle install && bundle exec jekyll serve`.

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
- built-ins named `builtin__*` (time, calculate, update_tasks, fetch, web_search, delegate;
  `tools/search.rs` asks DuckDuckGo's HTML page, SearXNG, Brave or Tavily with the app's own
  client, since the service comes from Settings, never the model);
- workspace tools with plain names (`read_file`, `file_glob_search`, `grep_search`,
  `get_info`, `git_status`, `git_diff`, `write_file`, `edit_file`, `exec_shell_command`, in
  `tools/workspace.rs`), offered only when the chat has a `workspace` folder. Every git call the
  app makes goes through `git::safe_command` (`git.rs`), which switches off the repository's own
  fsmonitor, external diff, textconv and pager programs, blanks its clean/smudge/process filter
  drivers (git runs them on `status` and `diff`), and leaves submodules out
  (`--ignore-submodules=all`); `review.rs` uses it too. `git_commit` (`tools/commit.rs`, switch `builtin_tools.git_commit`, off by
  default) stages the chosen paths and runs `git commit -- <paths>` (hooks off, signing off unless
  `allow_signing`), never pushes; `approval::always_asks` makes it ask in every mode but AllowAll
  and keeps it out of "Always allow". Read only drops it, subagents never get it, and the Commit
  dialog (`ui/commit_dialog.rs`) runs the same functions on the user's click;
- MCP tools routed as `<server>__<tool>` by `mcp::manager::ToolRouter`.

`ToolSelection` resolves what is on: Settings defaults plus per-chat `ToolOverrides` keyed
`builtin:<key>`, `server:<id>`, `tool:<server id>:<tool>` (and `feature:<key>`, below).
`Toolbox::needs_approval` decides which calls ask first (MCP tools, `delegate`, `fetch`,
`web_search`, and the workspace tools that write or run commands; `Toolbox::asks` lets web
search follow its own setting); everything else runs directly. Workspace paths are confined to the chosen
folder (`..`, outside absolute paths and symlinks out are refused). `fetch` reaches only
public addresses (its own client with a filtering DNS resolver and redirect policy) unless
`fetch_private_networks` is on; tests against `testutil::serve` turn that on.

**Chat features** (per-chat switches keyed `feature:<key>` in `ToolOverrides`, defaults in
`GeneralSettings`, listed in `tools::FEATURES`):
- `compaction.rs`: before each round `agent::run` estimates the request's tokens (the last
  reply's server count plus chars/4, or chars/4 alone once a summary exists) and, past the
  threshold of the context size (reported, or `compaction.context_tokens`), summarises
  everything before `split_point` (which keeps tool results with their call). The app stores a
  `Role::System` marker message with `compaction: Some(..)` whose content is the summary;
  `start_run` sends only the messages after the latest marker and the summary goes into the
  system prompt (`system_with_summary`, plus `CONTINUE_NOTE` when the kept part doesn't start
  with a user message). "Compact now" is a run with `compact_only`.
- `instructions.rs`: `AGENTS.md`/`CLAUDE.md` (configurable) from the workspace, appended to the
  system prompt; `ChatEvent::Instructions` tells the tools menu which files were read.
- `checkpoints.rs` with `tools::workspace`: `write_file`/`edit_file` go through
  `Workspace::plan`, so the approval prompt's diff (`Workspace::preview`, carried on
  `PendingApproval::preview`) and the edit match; the result's `ChatMessage::edit` (`FileEdit`)
  holds the diff, the previous content and the SHA-256 of what was written. Undo restores a
  turn newest first and skips files whose hash changed; `prune` drops old checkpoints.

**Skills** (`skills.rs`): a skill is a folder with `SKILL.md` (optional `name:`/`description:`
front matter, parsed by hand) from `<config dir>/skills/` and, with `skills.workspace_skills`,
`<workspace>/.cleanrstai/skills/` (a workspace skill replaces a user one of the same name).
`for_chat` discovers them per run (`agent::run`), the system prompt lists names and
descriptions, and the built-in `builtin__load_skill` tool (key `builtin:skills`, read only, never
asks) returns a skill's text or one file of its folder, confined to that folder. `ToolSelection::with_skills`
offers the tool only when some exist; subagents are forced off. The tools menu caches the list
per folder in `SkillCache`, cleared when settings are applied.

**Hooks** (`hooks.rs`): commands from `general.hooks` (Settings only, never the workspace or the model) run around tool calls in `agent::run`: `Runner::before_tool` after approval (a failure becomes the call's error, `Blocked by hook …`), `after_tool` appends `[hook name]` output to the result, and `reply_done` gives `ChatEvent::HookNotice` once a reply ends without `ChatEvent::Error`. `Runner::run` uses `workspace::shell_command` in the workspace folder (home without one), or `Place::exec_env` in the chat's sandbox container when the workspace's commands run there (`docker exec -e` passes the `CLEANRSTAI_*` variables). The `feature:hooks` chat switch gates them, and subagents and compact-only runs never reach them. `hooks::matches` takes the comma-separated glob list.

**Projects** (`projects.rs`; sidebar in `app.rs`, editor in `ui/project_editor.rs`): a project
is one JSON file in `projects/`, and its chats link to it by `Conversation.project`. `apply_to`
and `new_chat` copy its folder, provider, model, tool switches and permission mode into a chat
(`Conversation.provider` and `model` override the active provider and its model in
`AgentRequest::for_chat`). The caller loads the text of the instructions and files as a
`ProjectContext` (`load_context`, on the runtime in `ChatApp::launch_run`, or in `exec`), and
`agent::run` adds it to the system prompt while `general.projects.enabled` and the chat's
`feature:project` switch are on. `bundle` and `unbundle` export a project with its chats, with
new ids on import.

**Approvals** (`approval.rs`): the agent sends `ChatEvent::ToolApproval` with a oneshot
sender and the call's MCP origin; the app holds `PendingApproval`s on the `Run` and answers
them. "Always allow this tool/server" is an in-memory per-chat `AlwaysAllow` list keyed by
server id and MCP tool name (not the routed function name), so it survives renames.
Per-chat permission modes live in `approval.rs` (`ApprovalMode`: Default, AutoWorkspace,
AllowAll, ReadOnly, Plan; `ChatPermissions` with `no_sandbox`, stored on `Conversation.permissions`).
`ApprovalMode::runs_unasked` lets `agent::run` skip asking, and ReadOnly and Plan switch the
write/shell workspace tools off via `ToolSelection::with_mode` (`is_read_only`). Plan mode (`plan.rs`,
Settings → General → Plan mode) appends its prompt to the system prompt; `plan::approve` switches the
chat to its after-approval mode and returns the approval message, and `plan::awaiting_approval` decides
whether the buttons under a finished reply are shown. `offered` and `effective` hide Plan and map it to
ReadOnly while plan mode is off.

**Subagents** (`subagent.rs`): the `delegate` tool runs tasks in parallel, each with its own
model/tool loop and a derived `ToolSelection` (never the task list or `delegate`;
time/calculator/fetch only when allowed for subagents). Workspace tools only with
`subagents.workspace` and the sandbox on (`Delegation::with_workspace`): each subagent gets
`Workspace::for_subagent`, whose shell runs in its own container (`sandbox::Subagent`, removed on
drop). Subagent tool calls never ask for approval. Progress streams back as
`SubagentProgress` events.

**Background replies** (`background.rs`): `ChatApp` starts a reply through `start_run_with`, which queues it (`RunQueue`, FIFO) when `background.max_parallel` replies already run or others wait; `logic()` starts queued ones, enforces `background.time_limit_mins` from each `Run::started`, and frees slots of tasks that ended without `Done`. A queued chat counts as busy (`ChatApp::busy`) and stopping it removes it from the queue. `should_notify` decides desktop notifications (`notify-rust`, sent on `spawn_blocking`, never in tests), and the `feature:notify` chat switch can override the setting per chat.
**Scheduled tasks** (`schedule.rs`, UI in `ui/schedule_settings.rs`): `GeneralSettings.schedules` holds the tasks, with `Schedule` (interval, daily, weekly, local time), `Target` (new chat or a chat) and a last run. `logic()` calls `ChatApp::run_schedules` at most every `schedule::CHECK_EVERY`, which runs `schedule::check` (pure: `step` decides wait, run or skip for each task) and starts each due task through `run_task` and the reply queue (`enqueue_run`, which takes a `Queued.provider`). Run times are saved with the settings; a Settings draft gets them copied by `copy_run_state`.

**Command line** (`exec.rs`): `main` hands `exec …` to `exec::run` and answers `--version`, before the window opens. It parses the arguments by hand (`parse_args`), loads `Settings` as the app does, builds the request with `AgentRequest::for_chat` (the same builder `launch_run` uses), waits for MCP servers, and runs `drive`, which consumes the `agent::run` events on a task. `headless_allows` answers approvals: only what `ApprovalMode::runs_unasked` allows, everything else is denied. `ExecSettings` (`general.exec`) is on the Background tab. Windows attaches to the parent console with `AttachConsole`.

**MCP** (`mcp/`): stdio and Streamable HTTP transports, OAuth (PKCE, dynamic registration,
loopback redirect) in `mcp/oauth.rs`, and `McpManager`, which keeps connections in sync with
settings, refreshes tools/resources/prompts on `list_changed`, and builds the `ToolRouter`.

**Attachments** (`attach.rs`): images stay base64 for vision models; everything else becomes
text inlined into the message as `<file name="…">`. `documents.rs` recognises documents by
content (not extension) and extracts their text: `pdf-extract` for PDF, `zip` + `quick-xml`
for docx/pptx/odt/odp, `calamine` for all spreadsheets, `cfb` plus small parsers for legacy
`.doc`/`.ppt`, and its own RTF reader. Its tests use the LibreOffice-made samples in
`tests/fixtures/documents/`. The app reads files on a blocking task and gets them back as
`AppEvent::FileAttached`. `attach::read_bytes` returns `Reading::Scanned` for a PDF whose text
is (nearly) empty (`documents::extract_or_scanned`, `ocr::is_scanned`); `ocr.rs` then reads it
when `general.ocr` is on: `lopdf` gives each page's image XObjects (JPEG as is, 8-bit Flate
RGB/grey as PNG), sent to a vision model through `llm::chat_completion` or run through
`tesseract`, and the text comes back as `Attachment::ocr_text`. `read_files` runs it on the
tokio runtime and sends `AppEvent::OcrStarted` first; with OCR off the note
`attach::NO_TEXT_LAYER_NOTE` is attached instead.

**Docker sandbox** (`sandbox/`): only `exec_shell_command` is sandboxed. `docker.rs` talks to
Docker only through the `docker` CLI; its argument builders (`run_args`, `exec_args`) are pure, and
spawns retry `ExecutableFileBusy`. `Sandbox` keeps one container per chat, named `cleanrstai-<chat>` (subagents get
`cleanrstai-<chat>-s<id>`; containers are keyed by name)
and labelled `net.bukn.cleanrstai.sandbox=1`, reused via `docker exec`. It removes them with
`rm -f` on timeout, cancel (a drop guard), 15 idle minutes, chat delete, settings change, last-clone
drop, and at startup for leftovers. Images are a named image with a pull policy, or a Dockerfile
built as `cleanrstai-sandbox:<sha256-12>` with no build context. A custom network (`network.rs`)
runs one gateway container per chat, `cleanrstai-<chat>-net`, shared by its containers (image `cleanrstai-sandbox-net:<hash>` built from
`GATEWAY_DOCKERFILE`; iptables, ipset and dnsmasq set up by `gateway_script`) whose network the
sandbox joins with `--network container:`; only the allowed domains resolve, into per-rule ipsets. Errors are returned as the
command's error; nothing falls back to the host. `ChatMessage.sandbox` records the image for the badge.

**Remote access** (`remote/`): an axum server, started by `ChatApp::sync_remote` when
`general.remote` is on, serves the browser client in `remote/web/` (plain JS embedded with
`include_str!`; strict CSP, so no inline scripts or styles, and reply text only reaches the DOM
through `markdown.js`, which escapes everything). Browser requests become
`AppEvent::Remote(RemoteCommand)`s that `ChatApp::handle_remote` answers on the UI thread with
the same functions the desktop uses (`send_message`, `approve`, `stop`, …); `publish_remote`
fingerprints chats each frame and pushes `RemoteEvent`s over SSE so browsers refetch. Views
(`ChatView`, `MessageView`, …) carry no keys, settings, workspace paths or attachment data.
`auth.rs` holds password checks, sessions and the login throttle; `tls.rs` the self-signed
certificate (reissued when the hosts change) or the user's PEM files, with the ring provider
passed explicitly. Every non-GET needs the `X-CleanRSTAI: 1` header, and the Host must be an
IP, `localhost`, a `.local` name or the machine's name. `tunnel.rs` runs `cloudflared` for a
Cloudflare Tunnel: a quick tunnel (`tunnel --url https://127.0.0.1:<port>`, its
`trycloudflare.com` address read from the banner; quick tunnels buffer SSE, so the server sends
a `hello` event first and `app.js` polls when it doesn't arrive) or a named one (`tunnel run`,
token in `TUNNEL_TOKEN`, never the arguments; the user creates the tunnel and its route in the
dashboard). It follows the log for connections and hostnames, and restarts
cloudflared with backoff; `sync_remote`
restarts the server only when `server_settings` (everything but the tunnel) changes. The
tunnel's hostnames (`TunnelHosts`, shared with the server) are accepted as Host while it runs,
and only on a loopback connection to such a host is `Cf-Connecting-IP` trusted for the login
throttle, plus a tunnel-wide limit. `remote/access.rs` checks Cloudflare Access JWTs (RS256 against
the team's cached JWKS, from the `Cf-Access-Jwt-Assertion` header or the `CF_Authorization` cookie)
for a named tunnel's hostname; `server.rs`'s `access_guard` runs that before the session check.

**Tray** (`tray.rs`, Settings → Background "keep running"): on macOS and Windows a `tray-icon`
icon (menu bar / system tray) is created from `logic` once the setting is on, and dropped when it
goes off. `ChatApp::handle_tray` turns a close request into `CancelClose` + `Visible(false)` while
`tray::on_close` says hide (`keep_running`, a tray exists, and not quitting); Quit from the menu, and
the update restart, set `quitting` first because egui sends its own `Close` as a close request too.
Hidden, the app asks for a repaint every 250 ms to poll the tray. Linux has a stub (no tray; tray-icon
needs GTK there), and `tray-icon` stays under the macOS/Windows target section of `Cargo.toml`.

**Voice** (`voice/`): `VoiceSettings` (`GeneralSettings.voice`, a Voice tab) holds voice
input (`input`: a provider profile, model, language, `max_secs`) and reading aloud (`output`: an
`OsVoice` or `Endpoint` engine). `record::start` captures 16 kHz mono WAV on a thread: cpal on
macOS and Windows, `pw-record`/`parecord`/`arecord` (stopped with SIGINT) elsewhere, since cpal
needs ALSA headers the Linux release can't link. `AppEvent::VoiceCaptured` starts
`voice::spawn_transcription` (multipart `POST {base}/audio/transcriptions`), and the text goes into
the message box at the cursor. `speak::prose` turns Markdown into speech text; `VoiceState::start_speaking`
runs `speak::os_voice_command` (text on stdin, never in argv on Windows) or fetches
`POST {base}/audio/speech` WAV and plays it with `speak::play_wav`. Voice threads report through
`AppEvent::SpeechEnded`/`VoiceTranscribed`; the speaker button is an `Action::Speak` in the reply action row.
**Persistence**: settings are one JSON file (`config.rs`, paths via `directories` with
`ProjectDirs("net", "bukn", "CleanRSTAI")`); each conversation is its own JSON file
(`types::Conversation`). New fields must be `#[serde(default)]` (usually with
`skip_serializing_if`) so older files still load. API keys and MCP secrets go to the OS
keychain through the `secrets::SecretStore` trait (tests use the in-memory store), with
plain-text fallback when the keychain is off or unavailable.

**Share as HTML** (`export.rs`, `render_body`) renders the chat's Markdown with pulldown-cmark,
drops all raw HTML except the app's own `<details>` wrapper, and allow-lists link (http/https/mailto)
and image (raster `data:`) destinations, so the shared file loads nothing.

**Save as files** (`save_as.rs`): "Save as…" on a reply or a Markdown, code or CSV artifact
(`ui/chat_view.rs`, `save_as_menu`) reads the Markdown into a block model (`blocks`) that the
Word, PDF and Excel writers share. Word and Excel are hand-written OOXML over the `zip` crate;
PDF is `genpdf` with the static Instrument Sans and IBM Plex Mono in `assets/fonts`. The dialog
runs on the UI thread, the rendering on a blocking task (`AppEvent::SavedAs`). Formats come from
`GeneralSettings.save_as`; Excel only where there is a table or CSV.

**Providers**: `ProviderKind` (generic OpenAI-compatible, llama.cpp, LiteLLM, Ollama,
LM Studio, vLLM, OpenRouter) is detected by `llm::detect_kind` and decides which stats are
read (`stats.rs`: server timings vs. measured prefill/decode, cost, served model, context
size). llama.cpp and LiteLLM extras live in `llm.rs`; the other four in `servers.rs`
(detection, context size, and label/value rows for the info icon beside the model picker,
which `ChatApp::server_status` refreshes in the background as an `AppEvent::ServerInfo`).
`ProviderApi` picks the wire API: Chat Completions (`llm.rs`), or the Anthropic Messages and
OpenAI Responses APIs in `native.rs`, which convert the Chat Completions messages from
`llm::to_api_messages` and report the same `AssistantTurn`. Items those APIs need back
(Anthropic's signed thinking, Responses' encrypted reasoning) are kept on the message as
`ChatMessage.native` and sent only to the API that made them. `model_options.rs` holds
reasoning effort and sampling options, layered provider → model (`ProviderConfig::resolved_options`)
→ chat (`Conversation.model_options`, applied by `with_chat_options` in `start_run_with`).

Provider web search (`native.rs`): with `builtin_tools.search.native` = `PreferProvider`,
`native_search_request` sets `ProviderConfig.native_search` (per request, never saved) for a chat
whose `web_search` is on and whose provider has a search (`native_search_support`: Anthropic's
`web_search_20250305` tool, Responses' `web_search` tool, OpenRouter's `web` plugin).
`ToolSelection::with_native_search` then leaves the built-in `web_search` out of the toolbox, so
subagents keep it. The Anthropic reader keeps `server_tool_use`/`web_search_tool_result` blocks in
`ChatMessage.native` (sent back only to that API; Responses' `web_search_call` isn't kept, since
`store: false` means the server can't look it up later), collect
citations into `ChatMessage.sources` and set `searched`; `chat_view` shows them under the reply.
An Anthropic `pause_turn` continues the reply in the agent loop.

## UI conventions

- The look follows the Claude desktop app: palette, fonts and widget styling live in
  `ui/theme.rs` (`theme::palette(dark)`). Fonts are bundled in `assets/fonts` (Instrument
  Sans for the interface and replies, IBM Plex Mono for code, Phosphor for icons; codepoints
  in `theme::icon`).
- The app icon lives in `assets/icon/`: `glyph.png` is the source art and
  `scripts/make-icons.py` (Pillow) makes the rest from it: light/dark 1024 px masters, the
  `macos/` sizes listed under `[package.metadata.bundle] icon` in `Cargo.toml`, `icon.ico`,
  and the 256 px `window-{light,dark}.png` that `theme::window_icon` embeds. The app swaps the
  window icon when the theme changes.
- egui 0.37 takes button text from `TextStyle::Body`, so `Body` is the sans interface style;
  prose uses `TextStyle::Body` (Instrument Sans), applied around markdown in `ui/markdown.rs`.
- `ui/markdown.rs` splits replies into markdown segments (rendered by `egui_commonmark`) and
  drawings: closed top-level ```` ```mermaid ```` / ```` ```svg ```` blocks and inline
  `<svg>`, rendered to SVG and shown through egui's SVG loader. Web images become links
  unless "Load web images in replies" is on.
- `egui_commonmark_backend` is vendored in `vendor/` (via `[patch.crates-io]`) for real bold
  text (the `wght` font axis) and the code-block copy button; changes are marked
  `CleanRSTAI:` and listed in `vendor/egui_commonmark_backend/VENDORED.md`. Re-apply them when
  upgrading `egui_commonmark`.
- Artifacts (`artifacts.rs`) are closed top-level fenced blocks in replies: html, svg, mermaid,
  markdown at any length, other code from 10 lines. `markdown::segments` swaps them by byte
  range for one-line links (`artifact_panel::card`, expandable inline), so both parse with
  `artifacts::markdown_options`. `ui/artifact_panel.rs` is the right-hand panel; `ChatApp`
  keeps the open `ArtifactView` and the chat's artifact list, re-found when messages change.
  `ui/webview.rs` previews HTML live on macOS and Windows: a `wry` child web view (a
  target-specific dependency) placed over the area `artifact_panel::show` returns, with a CSP
  `<meta>` from `HtmlPreviewSettings`. On Linux `ui/linux_preview.rs` opens the page in its own
  WebKitGTK window instead: GTK and WebKitGTK are `dlopen`ed at run time (`libloading`; the
  release builds link neither), GTK runs on one preview thread, and the CSP (`webview::page`) is
  shared. Without them, "Open in browser" writes a temp file. It is a native window above egui, so `ChatApp::ui` hides it while
  settings, the palette, a popup, a dialog or a file drop could cover it.
- Widget ids inside a reply are scoped by message index (`push_id`), and tool results are
  matched to the tool messages that directly follow their assistant message, because some
  servers reuse call ids like `call_0` across turns.
- Chat views return `Action`s that `ChatApp::apply_actions` applies after drawing, rather
  than mutating app state while borrowed.

## Testing helpers

- `testutil::serve` starts a mock HTTP server on a free port and records requests; agent,
  LLM, fetch and MCP-over-HTTP tests use it as a fake OpenAI endpoint.
- Workspace and fetch tests use real temp folders and real HTTP against that server.
