# CleanRSTAI Roadmap

Ticked items have shipped: items 1–8 in **v0.3.0**, items 9–12 in **v0.4.0**, and rich
markdown and SVGs in chat, "Always allow" on the approval prompt and the web fetch tool in
**v0.5.0**. Unticked items are still open.

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

## 2. UI redesign

- [x] Visual refresh: typography, spacing, colour palette, message layout, icons in place of
  emoji glyphs
- [x] Collapsible sidebar, conversation grouping (Today / Yesterday / Last 7 days) and pinned
  chats
- [x] Compact tool-call cards, with arguments and results expandable
- [x] Settings as a full page with a search box
- [x] Command palette (⌘K) and keyboard shortcuts
- [x] Screen-reader labels on icon-only buttons
- [ ] Accessibility pass: focus order and a high-contrast check

## 3. Provider types that report stats based on what's provided

- [x] **llama.cpp server**: native `timings` for accurate prefill and decode speeds
- [x] **llama.cpp server**: context size
- [ ] **llama.cpp server**: slot usage
- [x] **LiteLLM proxy**: the model or deployment that served the request, and cost
- [x] **Generic OpenAI-compatible**: standard `usage` plus client-side timing
- [x] Auto-detect the type from a probe request, with a manual override
- [x] Only show stats that the server actually provides
- [ ] Dedicated types for Ollama, LM Studio, vLLM and OpenRouter (OpenRouter's `usage.cost`
  is already read)

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
- [ ] A proper app icon (replacing the placeholder), used for the window, taskbar/dock, `.dmg`
  and `.msi`

## 7. Richer chat input and output

- [x] Attach images and files, for vision-capable models
- [x] Render images returned by MCP tools
- [x] Export a single chat as Markdown
- [x] Rich markdown rendering in chat, including code blocks, tables, images and mermaid diagrams
- [x] Support for rendering SVGs in chat

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

## Maintenance

- [x] Move GitHub Actions off the deprecated Node.js 20 runtime
- [x] Turn on "Automatically delete head branches" in the repository settings, and delete the
  merged `claude/*` branches
