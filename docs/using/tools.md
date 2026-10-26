---
title: Tools and approvals
parent: Using the App
nav_order: 2
---

# Tools and approvals
{: .no_toc }

1. TOC
{:toc}

The model can call tools across several rounds before it answers. Each call shows as a card
with its arguments and result. Tools come from three places: the built-in tools, the
workspace tools (when a chat has a folder) and [MCP servers](mcp.html).

## Built-in tools

| Tool | What it does | Asks first? |
|---|---|---|
| **Time & date** | The current date, time, weekday and time zone, and the time at other UTC offsets. | No |
| **Calculator** | Exact arithmetic and functions: powers, roots, logarithms, trigonometry, rounding, min/max/avg. | No |
| **Task list** | For multi-step requests the model breaks the work into tasks, shows them as a checklist in the chat and ticks them off. | No |
| **Web fetch** | Reads an http(s) URL. Pages come back as readable text with their links, plain text and JSON as they are, images as images. Long pages are read in parts; downloads stop at 5 MB. | Yes |

Web fetch only reaches public internet addresses: not localhost, private networks or cloud
metadata endpoints, also after a redirect. To reach those, turn on *Let web fetch reach this
computer and private networks* in [Built-in Tools](../settings/built-in-tools.html).

## Workspace

Click **Set working directory** under the message box and choose a folder. The model then gets
these tools for that chat:

| Tool | What it does | Asks first? |
|---|---|---|
| `read_file` | Read a file, or part of one by line range | No |
| `file_glob_search` | Find files by glob pattern | No |
| `grep_search` | Search file contents with a regular expression | No |
| `get_info` | The workspace's path, OS, shell, git branch and top-level contents, or a file's or folder's details | No |
| `write_file` | Create or overwrite a file | Yes |
| `edit_file` | Replace exact text in a file | Yes |
| `exec_shell_command` | Run a command in the folder (`sh`, or `cmd` on Windows); 120 s by default, at most 600 s | Yes |

Every path stays inside the folder: `..`, absolute paths outside it and symlinks that lead out
are refused. Searches skip files ignored by `.gitignore`.

Hover over the folder name to see the full path; click it to choose another folder, or click
**×** to stop using it.

## Docker sandbox

With the [Docker sandbox](../settings/docker.html) on, **Run command** (`exec_shell_command`)
runs in a Docker container with the chat's folder mounted at `/workspace`, not on your computer.
The model is told the image and what network the container has (with a custom network, the
hosts and ports it may reach). The time limit and
the result (exit code, stdout and stderr) are the same as on your computer. A command that ran
in the sandbox shows **in <image>** after its status in the tool card; hover over it to see the
container's name. Approval is still asked for each command, as described below. If Docker isn't
running or the image is missing, the command is refused with a message; it never falls back to
your computer.

## Per-chat tool switches

The tools menu (sliders icon) next to the message box lists the built-in tools, the workspace
tools when the chat has a folder, and every MCP server and its tools, each with a switch. The
switches apply to the current chat from the next message. Defaults for new chats are in
[Settings → Built-in Tools](../settings/built-in-tools.html) and
[MCP Servers](../settings/mcp-servers.html).

## Approving tool calls

Tools that reach outside the app or change things ask first: MCP tools, web fetch,
`delegate` (subagents) and the workspace tools that write files or run commands. The card
shows the tool and its arguments and asks **Allow this tool to run?**:

- **Run**: run this call.
- **Always allow this tool**: run this tool without asking for the rest of the chat.
- **Always allow this server** (MCP tools only): run every tool from that MCP server without
  asking for the rest of the chat.
- **Deny**: refuse it; the model is told you declined and carries on.

"Always allow" lasts until the app quits.

{: .warning }
> **Run MCP tools and subagents without asking for approval** in
> [General](../settings/general.html#tool-calls) skips every approval, including web fetch
> and the workspace tools that write files and run shell commands. Only turn it on if you
> trust the model and the folder it works in.

## Limits

- **Max tool rounds** (default 20): how many times the model may call tools before it must
  answer.
- **Tool timeout** (default 120 s): how long one tool call may take.

Both are in [General](../settings/general.html#max-tool-rounds).
