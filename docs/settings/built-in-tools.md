---
title: Built-in Tools
parent: Settings
nav_order: 4
---

# Built-in Tools

Tools that ship with the app, so they work without an MCP server. These are the defaults for
new chats; switch them for one chat from the tools menu next to the message box. All are on by
default.

| Setting | Description |
|---|---|
| **Time & date** | The current date, time, weekday and time zone, and the time at other UTC offsets. |
| **Calculator** | Exact arithmetic and functions (powers, roots, logarithms, trigonometry, rounding, min/max/avg). |
| **Task list** | For multi-step requests the model breaks the work into tasks, shows them as a checklist in the chat and ticks them off as it goes. |
| **Web fetch** | Reads a web URL: pages as text with their links, plain text and JSON as they are, images as images. Asks for approval before each fetch. |
| **Web search** | Searches the web with the service below and lists the results (title, address, snippet), so the model can read the useful pages with web fetch. Asks for approval before each search by default. |
| **Workspace tools** | When a chat has a working folder: `read_file`, `file_glob_search`, `grep_search` and `get_info` run freely inside it; `write_file`, `edit_file` and `exec_shell_command` ask for approval. Paths can't leave the folder. |

## Let web fetch reach this computer and private networks

Off by default. While off, web fetch refuses localhost, private (10.x, 192.168.x, …) and
link-local addresses, such as cloud metadata endpoints, also after a redirect.

{: .warning }
> Subagents and auto-approved calls fetch without asking, so leave this off unless you need it.

## Web search

| Setting | Default | Description |
|---|---|---|
| **Service** | DuckDuckGo | **DuckDuckGo** (its HTML results page; no account or key, but it may ask for a human check after many searches), **SearXNG** (your own or a public instance), **Brave Search API**, **Tavily** or **Kagi Search API** (these three need an API key). |
| **Instance** | (empty) | SearXNG only: the instance's address, such as `https://searx.example.org` or `http://localhost:8888`. The instance must allow JSON results: add `json` to `search.formats` in its `settings.yml`. |
| **API key** | (empty) | Brave, Tavily and Kagi only. Kept in the system keychain when **Store API keys and secrets in the system keychain** is on ([General](general.html#secrets)). |
| **Results** | 5 per search | 1–20. The model may ask for more in a call, up to 20. |
| **Safe search** | on | Asks the service to filter adult results (Tavily and Kagi have no such option). |
| **Ask before each search** | on | The query leaves your computer and can contain parts of the chat. Subagents and auto-approved calls never ask. |
| **Subagents may search** | on | When subagents may use the built-in tools ([Subagents](subagents.html)). |

The service and its address come from these settings, never from the model, so a SearXNG
instance on your own computer or network works.

## Slash commands

Saved prompts, typed as `/name` in the message box (see
[Chats → Slash commands](../using/chats.html#slash-commands)). Two examples are included.

| Setting | Default | Description |
|---|---|---|
| **Slash commands (saved prompts)** | on | Expands `/name` when a message is sent, and lists the matching commands while you type. Off: `/name` is sent as typed. |
| **Commands** | `explain`, `summarize` | Each has a **name** (letters, digits, `-` and `_`; trimmed, and a name used twice keeps its first entry), a **description** shown in the list, and a **prompt** (several lines). `{{args}}` in the prompt is replaced by what you type after the name; without it, the text is added after a blank line. Delete with the bin button, add with **Add command**. |
| **Also load commands from the chat's working folder** | on | Each `.cleanrstai/commands/<name>.md` file in the chat's folder is a command whose prompt is the file's text. A first line `description: …` sets its description and is not sent. A file replaces a command here with the same name. |

The commands are kept in `settings.json` with the other settings; the workspace files are
read only when you type a command, so nothing is read from the folder otherwise.

The `delegate` tool has its own page: [Subagents](subagents.html). Context compaction, project
instructions and undo for file edits are on [Context & files](context.html). See
[Tools and approvals](../using/tools.html) for how the tools behave in a chat.
