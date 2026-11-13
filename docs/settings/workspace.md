---
title: Workspace
parent: Settings
nav_order: 7
---

# Workspace

What the model may do in a chat's working folder: the file and shell tools, commits, project instructions, diffs and undo for file edits, and code review.

## Workspace tools

**Workspace tools** switches the file, search and shell tools on for chats that have a working folder (see [Tools and approvals](../using/tools.html)). **Commit (git_commit)** lets the model make one local commit of the workspace's changes; every commit asks for approval, nothing is pushed and git hooks don't run. It is off by default. **Allow signed commits** lets a repository that signs its commits sign them.

## Project instructions

**Read project instructions from the workspace** (default on). When a chat has a working
folder, files such as `AGENTS.md` (read by Codex and other coding agents) or `CLAUDE.md` in it
are added to the system prompt, so the model follows the project's own conventions. The tools
menu shows which files the latest request read.

| Setting | Default | Description |
|---|---|---|
| **File names** | `AGENTS.md, CLAUDE.md` | Names to look for, in this order, separated by commas. |
| **Which files** | The first one found | Or **Every one found, in order**. |
| **Also read parent folders** | off | Read them from the folders above the workspace, outermost first, so the workspace's own file reads as the most specific. |
| **Size limit** | 64 KB | Text beyond this, in all the files together, is left out (1–1024 KB). |

## Workspace file edits

| Setting | Default | Description |
|---|---|---|
| **Show changes as a diff** | on | On the approval prompt and in the tool card. `write_file` and `edit_file` show what they will change before you approve them, and what they changed afterwards, as a diff with added lines in green and removed lines in red. |
| **Keep checkpoints for undo** | on | So a turn's `write_file` and `edit_file` changes can be undone. Each edit keeps the file's previous content in the chat. |
| **Largest file** | 1024 KB | Files bigger than this are changed without a checkpoint (16–65,536 KB). |
| **Keep for** | 20 turns | Checkpoints older than this many turns are dropped (0–1000); 0 keeps them all. |

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
| **Changes to review** | Uncommitted changes | **Uncommitted changes** or **This branch against its base**. Uncommitted: everything since the last commit (staged, unstaged, and new files, which are listed by name only). Branch: the commits on this branch since it forked from its base, plus nothing uncommitted. |
| **Base branch** | auto | The branch for the branch scope. Empty uses `origin/HEAD`, then `main`, then `master`. |
| **Review by** | The chat's provider | The provider profile that writes the review. |
| **Review model** | the provider's selected model | A model for the review, such as a stronger one. |
| **Largest diff** | 200 KB | A longer diff is cut to this size (16–4096 KB), and the model is told where it was cut. |
| **How strict** | Normal | **Lenient** reports only problems that would cause bugs, data loss or security issues, and skips style and minor suggestions. **Normal** sends the instructions as written. **Strict** also reports style, naming, readability, missing tests and documentation, and any risky assumption. The chosen level is added to the instructions. |
| **Review instructions** | (built in) | What the model is asked to look for. **Reset** restores the built-in text. |

The working folder must be a git repository. If there are no changes, or the base branch can't
be found, the app says so and sends nothing.
