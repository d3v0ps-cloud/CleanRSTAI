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
| **Skills** | Loads a skill (a folder of instructions, see [Skills & hooks](../settings/skills-and-hooks.html#skills)) by name, or one of its other files. Offered only when skills exist. | No |
| **Task list** | For multi-step requests the model breaks the work into tasks, shows them as a checklist in the chat and ticks them off. | No |
| **Web fetch** | Reads an http(s) URL. Pages come back as readable text with their links, plain text and JSON as they are, images as images. Long pages are read in parts; downloads stop at 5 MB. | Yes |
| **Web search** | Searches the web (DuckDuckGo by default, or SearXNG, Brave, Tavily or Kagi) and lists the results with their addresses and snippets; the model then reads the useful pages with web fetch. With **Search with** set to the provider's own search, Anthropic, OpenAI (Responses) or OpenRouter searches instead, and the reply lists the pages it cited under "Searched the web". | Yes, unless turned off in [Built-in Tools](../settings/built-in-tools.html#web-search). The provider's own search doesn't ask |

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
| `git_status` | The repository's status: the branch, and the changed, staged and untracked files | No |
| `git_diff` | The repository's diff: unstaged changes, or staged ones with `staged`; optionally one `path` or against a `base` branch, tag or commit | No |
| `write_file` | Create or overwrite a file | Yes |
| `edit_file` | Replace exact text in a file | Yes |
| `exec_shell_command` | Run a command in the folder (`sh`, or `cmd` on Windows); 120 s by default, at most 600 s | Yes |

Every path stays inside the folder: `..`, absolute paths outside it and symlinks that lead out
are refused. Searches skip files ignored by `.gitignore`.

`git_status` and `git_diff` only read, so they run on this computer even when the Docker sandbox
is on. A repository's own config can't make them run programs: its fsmonitor hook, external diff
drivers, textconv filters and clean, smudge and process filters are switched off, submodules are
left out, and repository hooks never run.

Hover over the folder name to see the full path; click it to choose another folder, or click
**×** to stop using it.

### Project instructions

When the folder has an `AGENTS.md` or `CLAUDE.md` (the names and order are set in
[Context & files](../settings/context.html#project-instructions)), it is added to the system
prompt of every request, so the model follows the project's conventions. Under **Chat features**
in the tools menu, **Project instructions** shows which files the latest request read and
switches them off for the chat.

### Diffs and undo

`write_file` and `edit_file` show their change as a diff on the approval prompt, before they
run, and in the tool card afterwards; the card's header shows how many lines were added and
removed. Each edit also keeps the file's previous content, so **Undo file edits** under the
last reply of a turn puts back every file that turn changed, newest first. A file that changed
again since is left alone, and commands run with `exec_shell_command` aren't undone. The card of
an undone edit says **Undone**. **Undo the last turn's file edits** in the command palette does
the same for the latest turn. Both can be turned off in
[Context & files](../settings/context.html#workspace-file-edits).

**Changed files** in the tools menu (under **Chat features**), or **Show files changed in this
chat** in the command palette, opens a read-only window listing every file the current chat's
edits changed. Each file has a collapsible section with the number of lines added and removed,
and each edit to it shows its diff, marked **Undone** where it was undone. It follows the chat
you have open, and it has no undo buttons: use **Undo file edits** for that.

### Reviewing changes

**Review changes** (the sliders button next to the message box, or the command palette) sends
the working folder's git diff to a model with a review prompt. The diff is attached as
`changes.diff`, and the model's findings come back as a normal reply in the chat, so you can
follow up on them there. It reads the uncommitted changes by default, or the branch against its
base. The review model, the prompt and the diff size limit are set under
[Context & files](../settings/context.html#code-review).

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

Under **Chat features**, the same menu switches **Context compaction**, **Project
instructions**, **Undo for file edits** and **Hooks** for the chat (defaults in
[Context & files](../settings/context.html) and [Skills & hooks](../settings/skills-and-hooks.html)),
and **Compact now** summarises the chat's older messages straight away.

## Hooks

Hooks are commands you set in [Skills & hooks](../settings/skills-and-hooks.html#hooks). They
run on your computer (or in the Docker sandbox, when it is on) before a tool call, after one, or
when a reply is done. A hook can block a call, such as a `write_file` you don't want, and an
after-tool hook's output is added to the tool's result, where the model reads it. The **Hooks**
switch under **Chat features** turns them off for the current chat; a chat that has them off
runs none.

## Approving tool calls

Tools that reach outside the app or change things ask first: MCP tools, web fetch, web
search (unless turned off; not the provider's own search, which runs on the provider's side),
`delegate` (subagents) and the workspace tools that write files or run commands. The card shows the tool and its arguments (or, for `write_file` and `edit_file`,
the change as a diff) and asks **Allow this tool to run?**:

- **Run**: run this call.
- **Always allow this tool**: run this tool without asking for the rest of the chat.
- **Always allow this server** (MCP tools only): run every tool from that MCP server without
  asking for the rest of the chat.
- **Deny**: refuse it; the model is told you declined and carries on.

"Always allow" lasts until the app quits.

### Permissions

**Permissions** at the top of the tools menu picks how this chat's tool calls are approved,
from the next call on. The tools menu icon turns the accent colour when a chat changes it.
New chats start in the mode set under [Settings → General](../settings/general.html#tool-calls)
(Default permissions unless changed). The ⌘K command palette has a "Permissions: …" entry for
each mode, which sets the current chat's mode (or the next chat's, before a chat is open).

| Mode | What happens |
|---|---|
| **Default permissions** (default) | Calls ask as described above, unless the setting below or "Always allow" says otherwise. |
| **Auto in workspace** | `write_file`, `edit_file` and commands in the Docker sandbox run without asking; commands on this computer, MCP tools, web fetch and `delegate` still ask. |
| **Allow all** | Every tool call runs without asking, as with the setting below but for this chat only. |
| **Read only** | The workspace tools that write files or run commands are off (also for subagents); other tools ask as by default. |

**Sandboxing for terminal** (on when the [Docker sandbox](#docker-sandbox) is on) runs this
chat's commands in the sandbox. Turn it off to run them on this computer; subagents then get no
workspace, since they only run commands in containers. It can't be turned on while the sandbox
is off in [Settings → Docker](../settings/docker.html).

Approving a `delegate` call also approves what its subagents do: they call their tools without
asking, including, when they may use the workspace, writing files and running commands in their
own sandbox containers (see [Subagents](subagents.html)).

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
