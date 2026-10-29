---
title: Skills & hooks
parent: Settings
nav_order: 5
---

# Skills & hooks

Saved prompts you type as slash commands, skills: folders of instructions the model loads
when a task matches one of them, and hooks: commands that run around the model's tool calls.

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

## Skills

A skill is a folder with a `SKILL.md` file: instructions for one kind of task, such as writing
release notes or following your team's review checklist. The app lists each skill's name and
description in the system prompt. When a task matches a skill, the model calls the
`load_skill` tool to read its text and then follows it. The text of a skill is sent to the
provider only when the model loads it (see [Data and privacy](../reference/data-and-privacy.html)).

| Setting | Default | Description |
|---|---|---|
| **Offer skills to the model** | on | Lists the skills in the system prompt and offers the `load_skill` tool. Off: no skill is listed and the tool isn't offered. |
| **Load skills from the workspace's .cleanrstai/skills folder** | on | Each `.cleanrstai/skills/<name>/SKILL.md` in a chat's working folder is a skill for that chat. A workspace skill replaces a skill from your skills folder with the same name. |
| **Size limit per load** | 64 KB (1–1024) | Text beyond this is left out when the model loads a skill, with a note saying so. |
| **Open skills folder** | | Creates the skills folder if it's missing and opens it in the file manager. Its path is shown under the button. |
| **Skill checkboxes** | on | Lists the skills in your skills folder. Switch one off and it is neither listed nor loadable, in any chat. **Refresh** reads the folder again. |

Per chat, the tools menu next to the message box has a **Skills** switch. It appears when the
chat can see any skills (in your skills folder or in its working folder), and it is on by
default.

### Where skills come from

- **Your skills folder**: the `skills` folder in the app's settings folder. **Open skills
  folder** shows the exact path.
- **The workspace**: `.cleanrstai/skills/` in the chat's working folder, when *Load skills from
  the workspace's .cleanrstai/skills folder* is on.

Skills are read when a message is sent, so a new skill is available to the next message.
Subagents don't get skills.

### Writing a skill

A skill's folder holds `SKILL.md` and, optionally, other files the instructions refer to:

```text
release-notes/
  SKILL.md
  template.md
  scripts/check-tags.sh
```

`SKILL.md` can start with front matter between two `---` lines, followed by the instructions:

```markdown
---
name: release-notes
description: Write release notes from the merged pull requests since the last tag
---
# Release notes

1. Run `git log --oneline <last-tag>..HEAD` in the working folder.
2. Group the changes under Added, Changed and Fixed.
3. Lay the notes out as in `template.md`.
```

- **name** is what `load_skill` is called with. It defaults to the folder's name. Names must be
  unique within a folder; a workspace skill with the same name as one of yours replaces it.
- **description** is one short sentence. It is the only part the model sees before it decides
  to load the skill, so say what the skill is for. It defaults to the first line of the body
  that isn't a heading, and is cut to 200 characters.
- **Other files** in the folder are named at the end of the text the model gets from
  `SKILL.md`, up to 50 of them, and the model can read one by its relative path. Folders whose
  names start with a dot are skipped.
- **Files stay in the folder.** `..`, absolute paths and symlinks that point outside the skill's
  folder are refused.

A skill is only text. It can tell the model to run commands, but running them still goes through
the workspace tools, which ask for approval as usual. The `load_skill` tool itself never asks.

## Hooks

A hook is a command that runs around the model's tool calls, or when a reply finishes. Use
hooks to run a formatter after an edit, or to refuse a kind of command before it runs. Hooks
come only from this page: nothing is read from a workspace, and the model can't add one.

Hooks run commands with your user's rights on this computer. With the Docker sandbox on, they
run in the chat's container instead (see [Running](#where-hooks-run)).

| Setting | Default | Description |
|---|---|---|
| **Run hooks** | off | The default for each chat. A chat's own **Hooks** switch in the tools menu overrides it. |
| **Hook checkbox** | on | Turns one hook on or off without deleting it. |
| **Name** | | Shown in a blocked result (`Blocked by hook "name"`), in the notice a reply-done hook gives and in the `[hook name]` line of an after-tool result. Blank becomes `hook N`. |
| **Event** | Before a tool runs | **Before a tool runs**, **After a tool runs** or **When a reply is done** (see below). |
| **Tools** | empty | Comma-separated patterns matched against the tool's name as the model calls it, with `*` for any text: `write_file, edit_file, github__*`. Empty matches every tool. Not shown for **When a reply is done**. |
| **Command** | | The command, several lines allowed. Run by the platform shell: `sh -c` on macOS and Linux, `cmd /C` on Windows. |
| **Timeout** | 30 s (1–600) | The command is stopped when it runs longer. |

Delete a hook with the bin button and add one with **Add hook**. A hook with a blank command
is removed when the settings are saved.

### When each event runs

- **Before a tool runs**: after the call is approved (if it asks), before it runs. If the
  command exits with a non-zero status, or it times out, the call does not run. The model gets
  an error: `Blocked by hook "name": <the command's output>`, or the exit code when the command
  printed nothing. Hooks are tried in the order listed; the first that fails blocks the call.
- **After a tool runs**: after the call finishes, whether it succeeded or not. Non-empty output
  is appended to the tool's result, under `[hook name]`, and the model sees it. A failing
  command is added the same way, and never stops the reply.
- **When a reply is done**: once, when the reply finishes without an error (not when it is
  stopped or fails). Non-empty output, or a failure, shows as a notice at the top of the window.

A hook sees the call in these environment variables:

| Variable | Value |
|---|---|
| `CLEANRSTAI_EVENT` | `before_tool`, `after_tool` or `reply_done` |
| `CLEANRSTAI_TOOL` | The tool's name as the model calls it, such as `write_file` or `github__create_issue` (empty for a reply) |
| `CLEANRSTAI_TOOL_ARGS` | The call's arguments as JSON, cut to 32 KB |
| `CLEANRSTAI_TOOL_RESULT` | The tool's result text, for after-tool hooks only, cut to 64 KB |
| `CLEANRSTAI_CHAT_ID` | The chat's identifier |
| `CLEANRSTAI_WORKSPACE` | The chat's working folder, or `/workspace` in the sandbox; empty without a folder |

The output of a command is its stdout and stderr together, and the first 16 KB of it is kept.

### Where hooks run

- In the chat's **working folder**, which is the current directory. A chat without a folder runs
  its hooks in your home folder.
- In the chat's **Docker sandbox container**, with the folder at `/workspace`, when the chat's
  commands run there: the sandbox is on and the chat's permission doesn't turn it off. A
  container is started for the first hook of a chat, the same as for a command. A hook that
  times out has its container removed, as a command does. If Docker can't run the hook, the
  call is blocked (for a before-tool hook) and the error is shown; nothing runs on your
  computer instead.

Hooks never run for subagents' tool calls, for **Compact now**, or for the commands a subagent
runs.

### Examples

Run `cargo fmt` after the model edits a file, and give the model the result of a failed
format as the tool's output:

| Setting | Value |
|---|---|
| Name | `rustfmt` |
| Event | After a tool runs |
| Tools | `write_file, edit_file` |
| Command | `cargo fmt` |
| Timeout | 120 s |

Refuse shell commands that delete with `rm -rf`, before they are approved and run:

| Setting | Value |
|---|---|
| Name | `no-rm-rf` |
| Event | Before a tool runs |
| Tools | `exec_shell_command` |
| Command | `case "$CLEANRSTAI_TOOL_ARGS" in *"rm -rf"*) echo "rm -rf is not allowed here"; exit 1;; esac` |

The second command uses `sh` syntax, so it works on macOS and Linux; on Windows, write the check
for `cmd`. Hooks run whatever the command does, so keep them to checks you have read.
