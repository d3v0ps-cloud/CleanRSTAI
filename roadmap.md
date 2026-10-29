# CleanRSTAI Roadmap

Ticked items have shipped: items 1–8 in **v0.3.0**, items 9–12 in **v0.4.0**, and rich
markdown and SVGs in chat, "Always allow" on the approval prompt and the web fetch tool in
**v0.5.0**, workspace folders with file, search and shell tools in **v0.6.0**, PDF and
Office document attachments in **v0.7.0**, the live stats readout while a response
streams in and llama.cpp slot usage in **v0.7.4**, and in-app updates from a configurable
source in **v0.7.5**, and llama.cpp context use and prompt progress, llama.cpp router mode
support and Windows builds again (on the self-hosted DARKBASE runner) in **v0.7.8**, and
llama.cpp server details and model loading, LiteLLM model, key and proxy details, Ollama,
LM Studio, vLLM and OpenRouter provider types, the Windows icon and in-app updates, and the
accessibility pass in **v0.7.9**, and one macOS Keychain item for all secrets (one password
prompt instead of one per key) in **v0.7.10**, and artifacts (web pages, drawings, documents and
longer code in a side panel, with a link in the chat) in **v0.7.11**, and remote access in the browser in **v0.8.0**, and the Cloudflare Tunnel (quick tunnels, or
named tunnels) in **v0.8.1**, with named tunnels needing only the tunnel token (the tunnel and
its route set up in the Cloudflare dashboard) in **v0.8.2**, and the Docker sandbox (custom
images and Dockerfiles, a custom network with host, protocol and port rules, and subagent
containers) in **v0.9.0**, and context compaction, project instructions, diffs and undo for
file edits, web search, the live web page preview (macOS and Windows), sharing a chat as HTML,
reasoning and sampling options per provider, model and chat, the Anthropic Messages and OpenAI
Responses APIs, and per-chat permission modes in **v0.9.1**, and forking a chat, slash
commands and reviewing changes in **v0.9.2-dev.1**, and compaction in the chat's stats, review
strictness, a context size per provider, a default permission mode, Kagi search, the changed
files list and read-only git tools in **v0.9.2-dev.2**, and skills and background replies (a
limit on replies at once with a queue, a time limit, desktop notifications, the Background
settings, and keeping running in the menu bar or system tray), scheduled tasks, hooks around
tool calls and when a reply is done, and the command-line mode (`cleanrstai exec`, with its
settings on the Background tab) in **v0.9.2-dev.4**, and OCR for scanned PDFs (a vision model or
Tesseract, switched on the Context & files tab) in **v0.9.2-dev.5**.
Unticked items are still open.

## 1. Display prefill and decode speed per request

- [x] **Prefill (prompt processing)**: tokens/s spent reading the prompt, plus time to first
  token
- [x] **Decode (generation)**: tokens/s while streaming the answer
- [x] Token counts (prompt, completion, total) and wall-clock duration per request
- [x] Running total for the whole chat
- [x] Client-side measurement when the server doesn't report timings (time to first token,
  decode speed, `stream_options.include_usage`)
- [x] Estimate completion tokens when the server doesn't send `usage`
- [x] In a turn with tool calls, show each model request separately, plus a combined total
- [x] Label each figure as *measured* or *reported by server*
- [x] Live readout above the message box while a response is streaming in, which disappears
  once the request finishes

## 2. UI redesign

- [x] Visual refresh: typography, spacing, colour palette, message layout, icons in place of
  emoji glyphs
- [x] Collapsible sidebar with search, pinned chats and recents
- [x] Quieter look in **v0.5.0**: warm charcoal and paper themes, a serif for replies, a
  greeting on new chats, user messages in bubbles and the model picker under the message box
- [x] Compact tool-call cards, with arguments and results expandable
- [x] Settings as a full page with a search box
- [x] Command palette (⌘K) and keyboard shortcuts
- [x] Screen-reader labels on icon-only buttons
- [x] Accessibility pass: focus order and a high-contrast check. A visible focus ring while
  moving with Tab, Tab leaves the message box (no keyboard trap), the message box has focus
  at launch, and every text colour meets WCAG AA (4.5:1) in both themes (a test checks it;
  muted text, links and status colours were darkened in the light theme)

## 3. Provider types that report stats based on what's provided

- [x] **llama.cpp server**: native `timings` for accurate prefill and decode speeds
- [x] **llama.cpp server**: context size
- [x] **llama.cpp server**: slot usage
- [x] **LiteLLM proxy**: the model or deployment that served the request, and cost
- [x] **Generic OpenAI-compatible**: standard `usage` plus client-side timing
- [x] Auto-detect the type from a probe request, with a manual override
- [x] Only show stats that the server actually provides
- [x] Dedicated types for Ollama, LM Studio, vLLM and OpenRouter, each recognised by
  **Detect**, with context use against the size the model runs with and the server's details
  beside the model picker (Ollama `/api/ps` and `/api/show`, LM Studio `/api/v0/models`, vLLM
  `/v1/models`, `/version` and `/metrics`, OpenRouter `/models` and `/key`), and the provider
  that served each OpenRouter request

More of what the servers report (from the llama.cpp server README and the LiteLLM proxy
source):

- [x] **llama.cpp server**: context use (`prompt_n + cache_n + predicted_n` against `n_ctx`)
  under each reply and in the chat's totals, in the warning colour from 80%
- [x] **llama.cpp server**: model details from `/props` and `/v1/models`: the loaded file
  (`model_path`), size, parameter count, trained context (`n_ctx_train`), vision support
  (`modalities`) and server build (`build_info`), in the hover of an info icon beside the
  model picker
- [x] **llama.cpp server**: prompt processing progress while a long prompt is read
  (`return_progress` → `prompt_progress`), as a bar above the message box with tokens read,
  cached tokens, time so far and time left
- [x] **llama.cpp server**: "Model loading" from `/health` (503) and "Model asleep" from
  `/props` `is_sleeping`
- [x] **llama.cpp server**: live speeds while streaming (`timings_per_token`)
- [x] **llama.cpp server**: router mode: load state of each model in the model picker
  (`/models` `status`), with load and unload (`/models/load`, `/models/unload`)
- [x] **llama.cpp server**: optional `/metrics` readout (queued requests, speculative
  decoding acceptance) when the server runs with `--metrics`
- [x] **LiteLLM proxy**: the deployment that served the request (`x-litellm-model-name`,
  `-model-id`, `-model-api-base`), and retries or fallbacks
  (`x-litellm-attempted-retries`, `-attempted-fallbacks`)
- [x] **LiteLLM proxy**: key spend against its budget (`x-litellm-key-spend`,
  `-key-max-budget`, or `/key/info` before the first request) and remaining rate limits
  (`x-litellm-key-remaining-requests-*`, `x-ratelimit-*`)
- [x] **LiteLLM proxy**: cost breakdown (input, output, cache read and write, reasoning) from
  the `x-litellm-response-cost-*` headers, on non-streamed responses
- [x] **LiteLLM proxy**: cost of streamed replies: LiteLLM sends no cost header for them, so
  the cost is estimated from the prices it lists in `/model/info` (marked "~"), after
  `usage.cost` and the `x-litellm-response-cost-original` header
- [x] **LiteLLM proxy**: context size and capabilities from `/model/info`
  (`max_input_tokens`, `supports_vision`, `supports_function_calling`, …), for context use and
  the model picker
- [x] **LiteLLM proxy**: response cache use (`x-litellm-cache-key`) and LiteLLM's own overhead
  (`x-litellm-overhead-duration-ms`)

## 4. Import settings and conversations

- [x] Import a settings export, choosing to merge with or replace the current setup
- [x] Re-prompt for secrets that were left blank in the export
- [x] Import a conversations export, skipping chats that already exist (matched by ID)
- [x] Validate the file's `format` and `format_version` before applying anything

## 5. Secure credential storage

- [x] Keep API keys and MCP secrets in the OS keychain (macOS Keychain, Windows Credential
  Manager, Secret Service on Linux) instead of plain text in `settings.json`
- [x] Migrate existing plain-text keys on first launch

## 6. Signed builds, installers and updates

- [x] macOS signing and notarization in the release workflow (Developer ID, hardened runtime,
  stapled app and DMG)
- [x] Windows Authenticode signing of the `.exe` and `.msi` in the release workflow
- [ ] Add the Apple and Windows certificates as repository secrets and ship a signed release
  (see the README for the secret names)
- [x] macOS `.dmg`
- [x] Windows `.msi` installer
- [x] In-app update check against GitHub Releases
- [x] Install updates in the app on macOS and Linux (download, verify against
  `SHA256SUMS.txt`, replace the app, restart)
- [x] Configurable update source ("Update from"), kept in the settings once changed
- [x] A proper app icon, with light and dark variants, for the window, taskbar/dock and the
  macOS `.app`
- [x] Use the app icon (`assets/icon/icon.ico`) for the Windows `.exe` and `.msi`
- [x] Install updates in the app on Windows: the portable `.exe` in place, an installed copy
  by running the new `.msi`

## 7. Richer chat input and output

- [x] Attach images and files, for vision-capable models
- [x] Attach PDF, Word, Excel, PowerPoint (current and 97–2003 formats), OpenDocument and RTF
  documents as their text
- [x] Render images returned by MCP tools
- [x] Export a single chat as Markdown
- [x] Rich markdown rendering in chat, including code blocks, tables, images and mermaid diagrams
- [x] Support for rendering SVGs in chat
- [x] Artifacts: web pages, drawings, documents and longer code open in a side panel beside
  the chat, with preview and code tabs, versions, copy, save and "Open in browser"
- [x] Live HTML preview inside the panel, in an embedded web view (macOS and Windows; see
  section 24)

## 8. Broader MCP support

- [x] MCP resources and prompts, alongside tools
- [x] OAuth sign-in for remote (Streamable HTTP) MCP servers
- [x] Re-fetch tools, resources and prompts automatically on `list_changed` notifications

## 9. Built-in tools

Tools that ship with the app, so they work without configuring an MCP server.

- [x] **Time & calculation**: current date, time and time zone, and a calculator for exact
  arithmetic
- [x] **Task management**: the agent breaks a request down into actionable tasks, shows them as
  a checklist in the chat, and updates each one as it works through them
- [x] **Web fetch**: read a web URL as text (pages, plain text, JSON) or as an image, with
  approval before each fetch
- [x] **Workspace**: give a chat a local working folder; the model reads, searches, edits and
  runs commands in it (changes and commands need approval)
- [x] Enable or disable each built-in tool in Settings

## 10. Toggle tools and MCP servers during a session

- [x] A dropdown next to the input that lists every MCP server and its tools (and the built-in
  tools), with a switch for each
- [x] Turn whole servers or single tools on and off for the current chat without opening
  Settings
- [x] Changes apply from the next request; Settings keeps the default for new chats
- [x] On the tool call approval prompt, offer "Always allow this tool" and "Always allow this
  MCP server" buttons that skip approval for the rest of the session

## 11. Reasoning display

- [x] Collapse reasoning by default
- [x] A setting in **Settings → General** to choose the default: collapsed, expanded, or
  expanded only while streaming

## 12. Subagents

A tool the model can call to delegate tasks to other agents, which run on their own and report
back.

- [x] A `delegate` tool that hands one or more tasks to subagents, runs them in parallel, and
  returns each result to the main agent
- [x] Show each subagent's progress and result in the chat, collapsible like tool calls
- [x] **Settings → Subagents**: system prompt, which tools and MCP servers subagents may use,
  maximum parallel subagents, round limit and timeout
- [x] Delegate to a different provider and model than the main chat, chosen in Settings or per
  task, e.g. a fast local model for simple tasks
- [x] Count subagent usage in the chat's stats and cost totals

## 13. Remote access in the browser

Run CleanRSTAI as a server so the same interface can be used from another device on the network.

- [x] A built-in web server, switched on in **Settings → General**, that serves the chat
  interface to a browser on another device
- [x] Log in before the interface is shown, with a password or token set in Settings; off by
  default, and refused until one is set
- [x] Serve over HTTPS (own certificate or one from the OS), and warn when it is plain HTTP
- [x] Choose the address to listen on (this computer only, or the local network), and the port
- [x] Keep API keys and MCP secrets on the host: the browser only ever sees chats and replies
- [x] Run tools and approvals on the host, with approval prompts shown in the browser
- [x] Work at phone width, and keep the look and keyboard shortcuts of the desktop app
- [x] Document setup and security in the docs (settings, data and privacy, and a remote-access page)

Later, beyond the first version:

- [x] Cloudflare Tunnel integration: start and stop a tunnel from Settings, using `cloudflared`
  with a token for a named tunnel, so the server is reachable from outside the network without
  opening a port on the router; show the public address, and keep the login required. Also
  quick tunnels (`trycloudflare.com`, no account, optional email check)
- [ ] Cloudflare Access in front of the tunnel (optional), for single sign-on before the login

## 14. Docker sandbox for tools

Run the commands and code the model triggers inside a local Docker container, not on the host.

- [x] **Settings → Docker**: turn the sandbox on or off, and choose the container image to run in
  (for example `python:3.13-slim` or a custom image name and tag, or a custom Dockerfile the app builds)
- [x] Choose how the image is obtained: pull it when missing, pull on every start, or use only
  an image already on the machine
- [x] Run the workspace shell tool (`exec_shell_command`) in a container with the chat's working
  folder mounted, and nothing else from the host
- [x] Choose network access for the container (none by default), and resource limits (CPU,
  memory, time)
- [x] Subagents can use the workspace, each running commands in a sandbox container of its own
  on the chat's network
- [x] A custom network that allows only listed outbound hosts (domains, IPv4 addresses and
  ranges), protocols (TCP, UDP, ICMP) and ports, through a firewall and DNS companion container
- [x] Refuse to run tools in the sandbox, with a clear message, when Docker isn't running or the
  image is missing, rather than falling back to the host
- [x] Show in the chat which container ran a command, with its exit code and output
- [x] Document the Docker settings and what the sandbox does and doesn't protect against

## Features from Claude Desktop and Codex

Sections 15–27 come from comparing CleanRSTAI with Claude Desktop and Codex. Every feature
in them follows the same rule:

- Each one has a switch in Settings and its own options there (limits, prompts, models, paths),
  with a sensible default.
- Each one that acts on a chat can also be turned on or off for that chat. Use the tools menu
  next to the message box, or the chat's menu. As with tools, Settings keeps the default for
  new chats.
- Turning a feature off removes its UI and its tools and changes nothing else. Data it saved
  (memories, checkpoints, branches) stays until the user deletes it.
- Each one is documented on the matching settings page, with its default and ranges.

## 15. Context management

- [x] Compact a chat automatically when it nears the model's context size: summarise the older
  turns and keep the recent ones and the task list
- [x] Settings (**Context & files**): on/off, the threshold (percentage of the context), the
  model that summarises (the chat's model, or a provider profile), the summary prompt, and how
  many recent messages to keep; per chat in the tools menu
- [x] A **Compact now** command (and ⌘K entry), and a marker in the chat where compaction
  happened, with the summary expandable
- [x] Optional trimming of old tool results (keep the call, drop the long output) as a cheaper
  alternative, switched separately
- [x] Use the context size the provider reports (llama.cpp, LiteLLM, Ollama, LM Studio, vLLM,
  OpenRouter), or one set in Settings when the server doesn't report one
- [x] A context size per provider, instead of the one setting
- [x] Count compaction requests in the chat's stats and cost

## 16. Project instructions

- [x] Read `AGENTS.md` (and optionally `CLAUDE.md` and other names) from the chat's workspace
  folder and add it to the system prompt
- [x] Settings: on/off, the file names to look for and their order, the first or every file
  found, whether to include files in parent folders, and a size limit
- [x] Per chat: turn instruction files off, and see which files the latest request read

## 17. Projects

- [ ] Group chats into projects with shared instructions, attached files and a default
  workspace folder, provider, model and tool switches
- [ ] Settings: on/off (off hides projects from the sidebar), and the default for new projects
- [ ] Move chats in and out of a project; export and import projects with the chats

## 18. Memory

- [ ] Facts the model saves across chats (a `memory` built-in tool and a list in Settings),
  added to the system prompt of new chats
- [ ] Settings: on/off, saving with or without approval, a size limit, and global or per-project
  scope
- [ ] View, edit and delete memories in Settings; per chat: turn memory off (an "incognito" chat)

## 19. Safe file edits

- [x] Show a diff on the approval card for `write_file` and `edit_file`, and after the edit in the
  tool card
- [x] Checkpoints: keep the files' previous content for each turn, with **Undo file edits** under
  the turn's reply (and in ⌘K); files changed again since are left alone
- [x] A list of the files a chat changed, with their diffs together
- [x] Settings: diff preview on/off, checkpoints on/off, how long and how much to keep; per chat
  in the tools menu
- [x] Read-only `git_status` and `git_diff` workspace tools
- [ ] Commit from the chat (with approval)

## 20. Approval modes

- [x] Named modes: **Read only** (no writes or commands), **Ask** (today's behaviour), **Auto in
  workspace** (writes and sandboxed commands run freely, the rest asks) and **Full access**
- [ ] A plan mode: the model only reads and proposes a plan, then waits for **Approve plan**
  before it changes anything
- [x] Settings: the default mode for new chats; per chat: pick the mode in the tools menu and
  with ⌘K

## 21. Web search

- [x] A built-in `web_search` tool with a choice of backend (DuckDuckGo without a key, SearXNG,
  Brave, Tavily), with the API key in the keychain
- [x] Kagi Search API backend
- [ ] The provider's own search when it has one
- [x] Settings: on/off, backend, result count, safe search, approval (like `fetch`), and whether
  subagents may use it
- [x] Per chat: on/off in the tools menu, like the other built-in tools

## 22. Conversation branching

- [ ] Keep the old reply when regenerating and the old turns when editing a message, as
  branches, with ‹ 1/3 › arrows to switch between them
- [x] Fork a chat from any message into a new chat
- [ ] Settings: on/off (off restores today's behaviour of replacing the reply), and a limit on
  branches kept per message

## 23. Model options

- [x] Reasoning effort and thinking budget per provider and model (`reasoning_effort`,
  `thinking`, llama.cpp and vLLM options), with a switch under the message box per chat
- [x] Other sampling settings (temperature, top-p, max tokens, stop sequences) per provider,
  model and chat, each left unset unless the user changes it
- [x] Optional native APIs alongside OpenAI-compatible chat completions: the Anthropic Messages
  API (prompt caching, thinking) and the OpenAI Responses API, chosen per provider

## 24. Live HTML preview, voice and generated files

- [x] Live HTML preview in the artifact panel on macOS (WebKit) and Windows (WebView2): a switch
  to keep "Open in browser" only, and settings for whether scripts may run and whether the page
  may load from the network (off by default); links open in the browser
- [ ] Live HTML preview on Linux (WebKitGTK, which the release builds don't link yet)
- [ ] Voice input (a local Whisper model or an OpenAI-compatible transcription endpoint) and
  reading replies aloud (a TTS endpoint or the OS voice), each with its own switch, provider
  and shortcut
- [ ] Save replies or artifacts as `.docx`, `.pdf` or `.xlsx`, switched in Settings

## 25. Commands, skills and hooks

- [x] Slash commands: saved prompts with arguments, typed as `/name` in the message box, kept in
  Settings and optionally in the workspace (`.cleanrstai/commands/`)
- [x] Skills: folders of instructions and files the model loads when a task matches, each
  with its own on/off switch
- [x] Hooks: run a command before or after a tool call, or when a reply finishes (for example
  a formatter after `edit_file`), each with its own switch, in the sandbox when it is on
- [x] Settings: each of the three on/off, and the list of commands, skills and hooks

## 26. Headless and background work

- [x] A command-line mode (`cleanrstai exec "…"`) that runs one request with the saved
  settings and prints the reply, for scripts and CI; turned on in Settings, with its own
  approval mode (read only by default)
- [x] Keep CleanRSTAI running when its window is closed, with a macOS menu-bar icon and a
  Windows system-tray icon to reopen the window or quit (a Linux tray is still to come)
- [x] Background tasks: a request keeps running when another chat is open, and notifies when
  it finishes, fails or needs approval
- [x] Scheduled tasks: run a saved prompt on a schedule into a chat, with on/off per task
- [x] Settings and limits for background replies: notifications on or off, how many run at once
  (the rest queue) and a time limit.

## 27. Code review

- [x] **Review changes**: send the workspace's uncommitted diff, or a branch against its base,
  to the model with a review prompt and show findings as a list
- [x] Settings: on/off, review prompt, model (the chat's or a provider profile), how strict to
  be, base branch and diff size

Smaller items from the same comparison:

- [x] OCR for scanned PDFs without a text layer (a local engine or a vision model), switched in
  Settings
- [x] Share a chat as a single HTML file alongside the Markdown export

## Maintenance

- [x] Move GitHub Actions off the deprecated Node.js 20 runtime
- [x] Run CI and releases on self-hosted runners (Linux x64 and macOS ARM64)
- [x] A Windows runner (DARKBASE, self-hosted), so CI and releases build Windows again
- [x] Turn on "Automatically delete head branches" in the repository settings, and delete the
  merged `claude/*` branches
