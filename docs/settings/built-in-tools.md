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
| **Web search** | Searches the web with the service below (or the provider's own search, when **Search with** says so) and lists the results (title, address, snippet), so the model can read the useful pages with web fetch. Asks for approval before each search by default. |
| **Workspace tools** | When a chat has a working folder: `read_file`, `file_glob_search`, `grep_search`, `get_info`, `git_status` and `git_diff` run freely inside it; `write_file`, `edit_file` and `exec_shell_command` ask for approval. Paths can't leave the folder. |

## Let web fetch reach this computer and private networks

Off by default. While off, web fetch refuses localhost, private (10.x, 192.168.x, …) and
link-local addresses, such as cloud metadata endpoints, also after a redirect.

{: .warning }
> Subagents and auto-approved calls fetch without asking, so leave this off unless you need it.

## Web search

| Setting | Default | Description |
|---|---|---|
| **Search with** | CleanRSTAI's search service | **CleanRSTAI's search service** (the service below), or **the provider's own search when it has one**: Anthropic (Messages API), OpenAI (Responses API) and OpenRouter. Other providers keep the service below. |
| **Service** | DuckDuckGo | **DuckDuckGo** (its HTML results page; no account or key, but it may ask for a human check after many searches), **SearXNG** (your own or a public instance), **Brave Search API**, **Tavily** or **Kagi Search API** (these three need an API key). Not used by the provider's own search. |
| **Instance** | (empty) | SearXNG only: the instance's address, such as `https://searx.example.org` or `http://localhost:8888`. The instance must allow JSON results: add `json` to `search.formats` in its `settings.yml`. |
| **API key** | (empty) | Brave, Tavily and Kagi only. Kept in the system keychain when **Store API keys and secrets in the system keychain** is on ([General](general.html#secrets)). |
| **Results** | 5 per search | 1–20. The model may ask for more in a call, up to 20. Not used by the provider's own search. |
| **Searches per reply** | 5 | Only with the provider's own search: 1–20 searches the provider may run for one reply. |
| **Safe search** | on | Asks the service to filter adult results (Tavily and Kagi have no such option). |
| **Ask before each search** | on | The query leaves your computer and can contain parts of the chat. Subagents and auto-approved calls never ask. Doesn't apply to the provider's own search, which runs on the provider's side and can't ask. |
| **Subagents may search** | on | When subagents may use the built-in tools ([Subagents](subagents.html)). |

The service and its address come from these settings, never from the model, so a SearXNG
instance on your own computer or network works.

With **Search with** set to the provider's own search, the provider runs the searches on its
side, for the model, and cites the pages it used. The reply then shows "Searched the web ·
N sources" with the links under it, which open in your browser. The query goes to that
provider, in the request the chat already sends, and CleanRSTAI doesn't contact the search
service. The model must support the provider's search, and searches may be billed by the
provider. Nothing is asked before each search. A chat with web search switched off in the tools
menu searches nowhere.

## Slash commands and skills

Saved prompts (slash commands) and skills, the folders of instructions the model loads, have
their own page: [Skills & hooks](skills-and-hooks.html).

The `delegate` tool has its own page: [Subagents](subagents.html). Context compaction, project
instructions and undo for file edits are on [Context & files](context.html). See
[Tools and approvals](../using/tools.html) for how the tools behave in a chat.
