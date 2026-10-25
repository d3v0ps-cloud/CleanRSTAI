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
| **Workspace tools** | When a chat has a working folder: `read_file`, `file_glob_search`, `grep_search` and `get_info` run freely inside it; `write_file`, `edit_file` and `exec_shell_command` ask for approval. Paths can't leave the folder. |

## Let web fetch reach this computer and private networks

Off by default. While off, web fetch refuses localhost, private (10.x, 192.168.x, …) and
link-local addresses, such as cloud metadata endpoints, also after a redirect.

{: .warning }
> Subagents and auto-approved calls fetch without asking, so leave this off unless you need it.

The `delegate` tool has its own page: [Subagents](subagents.html). See
[Tools and approvals](../using/tools.html) for how the tools behave in a chat.
