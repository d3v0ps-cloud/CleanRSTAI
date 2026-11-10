---
title: Settings
nav_order: 4
---

# Settings

Open **Settings** from the sidebar, from the menu on your name at the bottom of it, or with
**⌘/Ctrl + ,**. It opens over the window (which dims behind it) on **General**.
The pages are grouped on the left: **App**, **Chats**, **Models & tools** and **This computer**.
Most settings are one row: its name and a short description on the left, a switch, list or
field on the right.

| Group | Page | What you can set |
|---|---|---|
| App | [General](general.html) | System prompt, your name, send key, updates |
| | [Appearance](appearance.html) | Theme, UI scale, reduce motion, reasoning display, speed stats |
| | [Voice](voice.html) | Voice input (dictation) and reading replies aloud: provider, model, language, voice, speed and limits |
| | [Data & privacy](privacy.html) | The keychain for secrets, web images in replies, export and import, where files are kept |
| Chats | [Permissions](permissions.html) | The mode new chats start in, auto-approve, tool rounds, reply limits, tool timeout, plan mode |
| | [Context & memory](context.html) | Context compaction, memory, projects, branches |
| | [Workspace](workspace.html) | Workspace tools and commits, project instructions (`AGENTS.md`, `CLAUDE.md`), diffs and undo for file edits, code review |
| | [Artifacts & files](artifacts.html) | The artifact panel, live web page preview, Save as formats, OCR for scanned PDFs |
| Models & tools | [Providers](providers.html) | Endpoints: base URL, API, API key, model, reasoning effort and sampling options, server type, streaming, timeout, extra headers |
| | [MCP servers](mcp-servers.html) | Add, import, enable or disable servers; stdio or HTTP connection; sign-in; per-tool toggles |
| | [Tools](built-in-tools.html) | Time & date, calculator, task list, web fetch and web search (service, key, results) on or off by default |
| | [Skills & hooks](skills-and-hooks.html) | Slash commands, skills, and hooks: commands that run before or after a tool call, or when a reply is done |
| | [Subagents](subagents.html) | Delegation on or off; provider and model; tools; parallelism and limits; system prompt |
| This computer | [Docker sandbox](docker.html) | Run shell commands in a Docker container: image or Dockerfile, network, CPU, memory and process limits |
| | [Background](background.html) | Keep running in the tray, notifications, how many replies run at once, a time limit per reply, the command line (`cleanrstai exec`) |
| | [Remote access](remote-access.html) | The browser client's server, password and HTTPS, Cloudflare Tunnel and Cloudflare Access |

Scheduled agents are not in Settings: they have the **Scheduled** page in the sidebar.

## Finding a setting

Type in the **Search** box at the top of the left column: the list keeps only the pages whose name
or settings match, and **Matching settings** under it lists each setting found; click one to jump
to its page.

## Saving changes

Edits are made on a draft and take effect only when you save:

- **Save**: apply the changes and close settings.
- **Apply**: apply them and stay.
- **Cancel** (or **Close** when nothing changed): leave without applying.

**Unsaved changes** shows while there are changes. **Esc**, the **×** at the top right, or a
click outside closes settings when there are no unsaved changes. Only MCP servers whose connection settings changed are reconnected.

## Where settings are stored

| | Settings | Conversations |
|---|---|---|
| macOS | `~/Library/Application Support/net.bukn.CleanRSTAI/settings.json` | `~/Library/Application Support/net.bukn.CleanRSTAI/conversations/` |
| Linux | `~/.config/cleanrstai/settings.json` | `~/.local/share/cleanrstai/conversations/` |
| Windows | `%APPDATA%\bukn\CleanRSTAI\config\settings.json` | `%APPDATA%\bukn\CleanRSTAI\data\conversations\` |

The exact paths are shown at the bottom of the Data & privacy page. API keys and other secrets are
not in `settings.json`; see [Secrets](privacy.html#secrets).
