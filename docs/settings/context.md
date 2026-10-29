---
title: Context & files
parent: Settings
nav_order: 5
---

# Context & files

Context compaction, project instructions from the workspace, and diffs and undo for workspace
file edits. These are the defaults for new chats; switch each one for one chat under **Chat
features** in the tools menu next to the message box (see
[Tools and approvals](../using/tools.html#per-chat-tool-switches)).

## Context compaction

**Compact long chats automatically** (default on). When the next request would fill most of
the model's context, the older messages are summarised by a model and only the summary and the
latest messages are sent from then on. The chat keeps every message, with a marker where the
summary starts (see [Chats → Long chats](../using/chats.html#long-chats)).

| Setting | Default | Description |
|---|---|---|
| **Compact at** | 80% | Compact once a request would fill this share of the context (50–95%). |
| **Keep word for word** | 6 latest messages | Messages at the end of the chat that are always sent in full (2–50). A tool result stays with the call that asked for it, so a few more may be kept. |
| **Context size** | 0 | The context size, in tokens, to assume when the server doesn't report one. With 0, automatic compaction only happens when it does: llama.cpp, LiteLLM, Ollama, LM Studio, vLLM and OpenRouter report it (set **Server type** on the provider). For other servers, enter the model's context size. |
| **Summary by** | The chat's provider | The provider profile that writes the summary. |
| **Summary model** | the provider's selected model | A model for the summary, such as a cheaper or faster one. |
| **Older tool results** | off, 2000 characters | **Shorten long results of older tool calls in what is sent**: results older than the kept messages are cut to this many characters (200–100,000). This is a cheaper step that sends less with every request; the chat keeps the results whole. |
| **Summary instructions** | (built in) | What the summarising model is asked to keep. **Reset** restores the built-in text. |

**Compact now** in the tools menu, and **Compact this chat** in the command palette (⌘/Ctrl +
K), summarise the older messages straight away, even with automatic compaction off.

How the size is worked out: after a reply the server reports how many tokens it read; the app
adds about one token per four characters for what came after. Once a chat has a summary, the
whole estimate is by characters, since the server's counts covered messages that are no longer
sent.

## Project instructions

**Read project instructions from the workspace** (default on). When a chat has a working
folder, files such as `AGENTS.md` (read by Codex and other coding agents) or `CLAUDE.md` in it
are added to the system prompt, so the model follows the project's own conventions. The tools
menu shows which files the latest request read.

| Setting | Default | Description |
|---|---|---|
| **File names** | `AGENTS.md, CLAUDE.md` | Names to look for, in this order, separated by commas. |
| **Which files** | The first one found | Or **Every one found, in order**. |
| **Parent folders** | off | Also read them from the folders above the workspace, outermost first, so the workspace's own file reads as the most specific. |
| **Size limit** | 64 KB | Text beyond this, in all the files together, is left out (1–1024 KB). |

## Workspace file edits

| Setting | Default | Description |
|---|---|---|
| **Show the change as a diff on the approval prompt and in the tool card** | on | `write_file` and `edit_file` show what they will change before you approve them, and what they changed afterwards, as a diff with added lines in green and removed lines in red. |
| **Keep checkpoints so a turn's `write_file` and `edit_file` changes can be undone** | on | Each edit keeps the file's previous content in the chat. |
| **Largest file** | 1024 KB | Files bigger than this are changed without a checkpoint (16–65,536 KB). |
| **Keep for** | 20 turns | Checkpoints older than this many turns are dropped; 0 keeps them all. |

With checkpoints on, **Undo file edits** under the last reply of a turn puts back the files that
turn changed, newest first. A file that changed again after the edit (by you, a later turn or
a shell command) is left alone, and the notice says so. Commands run with
`exec_shell_command` aren't undone. Checkpoints are saved in the chat's file, so they survive a
restart and are deleted with the chat.

## Code review

**Review changes** in the tools menu, and in the command palette (⌘/Ctrl + K), reads the
working folder's git diff and sends it to a model with the instructions below, as a new message
in the chat. The diff goes as the attached file `changes.diff`. The review runs with the provider
chosen here, for that one reply; the chat's own provider is unchanged. The button is enabled
when the chat has a working folder and no reply is running. Turn **Code review** off to hide
both entries.

| Setting | Default | Description |
|---|---|---|
| **Code review** | on | Shows **Review changes** in the tools menu and the command palette. |
| **Uncommitted changes** or **This branch against its base** | Uncommitted changes | Uncommitted: everything since the last commit (staged, unstaged, and new files, which are listed by name only). Branch: the commits on this branch since it forked from its base, plus nothing uncommitted. |
| **Base branch** | auto | The branch for the branch scope. Empty uses `origin/HEAD`, then `main`, then `master`. |
| **Review by** | The chat's provider | The provider profile that writes the review. |
| **Review model** | the provider's selected model | A model for the review, such as a stronger one. |
| **Largest diff** | 200 KB | A longer diff is cut to this size (16–4096 KB), and the model is told where it was cut. |
| **Review instructions** | (built in) | What the model is asked to look for. **Reset** restores the built-in text. |

The working folder must be a git repository. If there are no changes, or the base branch can't
be found, the app says so and sends nothing.
