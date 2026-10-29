---
title: Settings
nav_order: 4
---

# Settings

Open **Settings** with the gear icon in the sidebar or top bar, or press **⌘/Ctrl + ,**. It
opens on **General** and takes over the main window.

| Page | What you can set |
|---|---|
| [General](general.html) | System prompt, theme, UI scale, send key, secrets, updates, reasoning display, stats, web images, artifacts and live web page preview, tool approval, tool rounds and timeout, remote access and Cloudflare Tunnel, export and import |
| [Providers](providers.html) | Endpoints: base URL, API (Chat Completions, Responses, Anthropic), API key, model, reasoning effort and sampling options per provider and model, server type, token usage, streaming, timeout, extra headers |
| [MCP Servers](mcp-servers.html) | Add, import, enable or disable servers; stdio or HTTP connection; sign-in; per-tool toggles |
| [Built-in Tools](built-in-tools.html) | Time & date, calculator, task list, web fetch, web search (service, key, results) and workspace tools on or off by default |
| [Skills & hooks](skills-and-hooks.html) | Slash commands (saved prompts), skills: folders of instructions the model loads when a task matches, from your skills folder or the workspace, and hooks: commands that run before or after a tool call, or when a reply is done |
| [Context & files](context.html) | Context compaction (threshold, kept messages, context size, summary model and instructions, shortening older tool results), project instructions (`AGENTS.md`, `CLAUDE.md`), diffs and undo for file edits |
| [Subagents](subagents.html) | Delegation on or off; provider and model; tools, including the workspace in sandbox containers of their own; parallelism and limits; system prompt |
| [Docker sandbox](docker.html) | Run shell commands in a Docker container: image or custom Dockerfile (small and full examples), network (none, normal, or allowed hosts, protocols and ports), CPU, memory and process limits, read-only folder |
| [Background](background.html) | Desktop notifications for finished, failed and approval-waiting replies; how many replies run at once (the rest queue); a time limit per reply; the command line (`cleanrstai exec`) |
| [Voice](voice.html) | Voice input (dictation into the message box) with its provider, model, language and recording limit; reading replies aloud with the system voice or a speech endpoint, with its provider, model, voice, speed and length limit |

## Finding a setting

Type in **search settings** at the top of the left column to search every page; click a result
to jump to its page.

## Saving changes

Edits are made on a draft and take effect only when you save:

- **Save & Close**: apply the changes and close settings.
- **Apply**: apply them and stay.
- **Discard changes** (or **Close** when nothing changed): leave without applying.

**Unsaved changes** shows while there are changes. **Esc** closes settings when there are no
unsaved changes. Only MCP servers whose connection settings changed are reconnected.

## Where settings are stored

| | Settings | Conversations |
|---|---|---|
| macOS | `~/Library/Application Support/net.bukn.CleanRSTAI/settings.json` | `~/Library/Application Support/net.bukn.CleanRSTAI/conversations/` |
| Linux | `~/.config/cleanrstai/settings.json` | `~/.local/share/cleanrstai/conversations/` |
| Windows | `%APPDATA%\bukn\CleanRSTAI\config\settings.json` | `%APPDATA%\bukn\CleanRSTAI\data\conversations\` |

The exact paths are shown at the bottom of the General page. API keys and other secrets are
not in `settings.json`; see [Secrets](general.html#secrets).
