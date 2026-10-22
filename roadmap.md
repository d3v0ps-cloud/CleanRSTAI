# CleanRSTAI Roadmap

Planned work, roughly in priority order. Items further down are less defined and may change.

## 1. Display prefill and decode speed per request

Show how fast each response was produced, under each assistant message and as a running total
for the chat.

- **Prefill (prompt processing)**: tokens/s spent reading the prompt, plus time to first token.
- **Decode (generation)**: tokens/s while streaming the answer.
- Token counts (prompt, completion, total) and wall-clock duration per request.
- When the server doesn't report timings, measure them client-side:
  - time to first token comes from the first streamed chunk;
  - decode speed is completion tokens divided by the time from first to last token;
  - completion tokens come from `usage`, requested with `stream_options.include_usage`, or are
    estimated when the server doesn't send them.
- In a turn with tool calls, show each model request separately, plus a combined total.
- Label each figure as *measured* or *reported by server*, so they aren't mixed up.

## 2. UI Redesign

A more polished, native-feeling interface, with macOS as the reference platform.

- Visual refresh: typography, spacing, colour palette, message layout, icons in place of emoji
  glyphs.
- Collapsible sidebar, conversation grouping (Today / Yesterday / Last 7 days) and pinned chats.
- Clearer tool-call cards: compact by default, with arguments and results expandable.
- Settings rebuilt as a full page or sheet instead of a floating window, with a search box.
- Keyboard-first navigation and a command palette (⌘K).
- Accessibility: screen-reader labels, focus order, high-contrast check.

## 3. Provider type option that reports stats based on what's provided

Let each provider profile declare which kind of server it is, so the app can use that server's
extra features and report richer stats (feeds into item 1).

- **llama.cpp server**: read its native `timings` (prompt and predicted token counts, ms and
  tokens/s) for accurate prefill and decode speeds; optionally show slot and context usage.
- **LiteLLM proxy**: show the underlying model or deployment that served the request, and cost
  where the proxy reports it.
- **Generic OpenAI-compatible**: the current behaviour, using standard `usage` plus client-side
  timing.
- Possibly later: Ollama, LM Studio, vLLM, OpenRouter (cost and provider routing).
- Auto-detect the type from the base URL or a probe request, with a manual override.
- Each type declares which stats it supports, so the UI only shows fields that will be filled.

## 4. Import settings and conversations

The counterpart to the export added in Settings → General.

- Import a settings export, choosing to merge with or replace the current setup. Re-prompt for
  any secrets that were left blank in the export.
- Import a conversations export, skipping chats that already exist (matched by ID).
- Validate the file's `format` and `format_version` before applying anything.

## 5. Secure credential storage

- Keep API keys and MCP secrets in the OS keychain (macOS Keychain, Windows Credential
  Manager, Secret Service on Linux) instead of plain text in `settings.json`.
- Migrate existing plain-text keys on first launch.

## 6. Signed and notarized builds

- Sign and notarize the macOS app with an Apple Developer ID so Gatekeeper doesn't block it.
- Sign the Windows executable; consider an installer (MSI) and a DMG for macOS.
- In-app update check against GitHub Releases.

## 7. Richer chat input and output

- Attach images and files, for vision-capable models.
- Render images returned by MCP tools.
- Export a single chat as Markdown.

## 8. Broader MCP support

- MCP resources and prompts, alongside the tools already supported.
- OAuth sign-in for remote (Streamable HTTP) MCP servers.
- Re-fetch tools automatically on `notifications/tools/list_changed`.

## Maintenance

- Update GitHub Actions that still target the deprecated Node.js 20 runtime.
