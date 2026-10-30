---
title: Settings
nav_order: 4
---

# Settings

Open **Settings** from the menu on your name at the bottom of the sidebar, or press
**⌘/Ctrl + ,**. It opens over the window (which dims behind it) on **General**; **Customize** in
the sidebar opens it on Skills & hooks.
The pages are grouped on the left: **Preferences** (General, Appearance, Voice), **This
computer** (Docker sandbox, Background) and **Customize** (Providers, MCP servers, Built-in tools,
Skills & hooks, Context & files, Subagents).

| Page | What you can set |
|---|---|
| [General](general.html) | System prompt, send key, secrets, updates, live web page preview, tool approval, tool rounds and timeout, remote access and Cloudflare Tunnel, export and import |
| [Appearance](appearance.html) | Theme, UI scale, your name, reduce motion, reasoning display, speed stats, web images, the artifact panel |
| [Providers](providers.html) | Endpoints: base URL, API (Chat Completions, Responses, Anthropic), API key, model, reasoning effort and sampling options per provider and model, server type, token usage, streaming, timeout, extra headers |
| [MCP Servers](mcp-servers.html) | Add, import, enable or disable servers; stdio or HTTP connection; sign-in; per-tool toggles |
| [Built-in Tools](built-in-tools.html) | Time & date, calculator, task list, web fetch, web search (service, key, results) and workspace tools on or off by default |
| [Skills & hooks](skills-and-hooks.html) | Slash commands (saved prompts), skills: folders of instructions the model loads when a task matches, from your skills folder or the workspace, and hooks: commands that run before or after a tool call, or when a reply is done |
| [Context & files](context.html) | Context compaction (threshold, kept messages, context size, summary model and instructions, shortening older tool results), project instructions (`AGENTS.md`, `CLAUDE.md`), diffs and undo for file edits |
| [Subagents](subagents.html) | Delegation on or off; provider and model; tools, including the workspace in sandbox containers of their own; parallelism and limits; system prompt |
| [Docker sandbox](docker.html) | Run shell commands in a Docker container: image or custom Dockerfile (small and full examples), network (none, normal, or allowed hosts, protocols and ports), CPU, memory and process limits, read-only folder |
| [Background](background.html) | Desktop notifications for finished, failed and approval-waiting replies; how many replies run at once (the rest queue); a time limit per reply; the command line (`cleanrstai exec`) |
| [Voice](voice.html) | Voice input (dictation into the message box) with its provider, model, language, recording limit and microphone; reading replies aloud with the system voice or a speech endpoint, with its provider, model, voice, speed and length limit |

## Finding a setting

Type in the **Search** box at the top of the left column: the list keeps only the pages whose name
or settings match, and **Matching settings** under it lists each setting found; click one to jump
to its page.

## Saving changes

Edits are made on a draft and take effect only when you save:

- **Save & Close**: apply the changes and close settings.
- **Apply**: apply them and stay.
- **Discard changes** (or **Close** when nothing changed): leave without applying.

**Unsaved changes** shows while there are changes. **Esc**, the **×** at the top right, or a
click outside closes settings when there are no unsaved changes. Only MCP servers whose connection settings changed are reconnected.

## Where settings are stored

| | Settings | Conversations |
|---|---|---|
| macOS | `~/Library/Application Support/net.bukn.CleanRSTAI/settings.json` | `~/Library/Application Support/net.bukn.CleanRSTAI/conversations/` |
| Linux | `~/.config/cleanrstai/settings.json` | `~/.local/share/cleanrstai/conversations/` |
| Windows | `%APPDATA%\bukn\CleanRSTAI\config\settings.json` | `%APPDATA%\bukn\CleanRSTAI\data\conversations\` |

The exact paths are shown at the bottom of the General page. API keys and other secrets are
not in `settings.json`; see [Secrets](general.html#secrets).
