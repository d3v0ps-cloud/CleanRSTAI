---
title: Context & files
parent: Settings
nav_order: 6
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
| **Context size** | 0 | The context size, in tokens, to assume when neither the server nor the provider's own **Context size** ([Providers](providers.html#context-size)) gives one. With 0, automatic compaction only happens when one of them does: llama.cpp, LiteLLM, Ollama, LM Studio, vLLM and OpenRouter report it (set **Server type** on the provider). For other servers, enter the model's context size on the provider. |
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

The request that writes a summary counts in the chat's totals and cost, and its line shows under
the summary marker. Its size doesn't count as the chat's context use.

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

## Projects

**Projects** (default on) groups chats in the sidebar, with the instructions and files each
project adds to its chats. Off hides the projects from the sidebar and stops adding their
context to requests; a chat can also leave the project context out from the tools menu. See
[Projects](../using/projects.html).

| Setting | Default | Description |
|---|---|---|
| **Files size limit** | 256 KB | The text of a project's files sent with each request, in all (16–4096 KB). Longer files are cut. |
| **New projects: folder** | None | The working folder a new project starts with. |
| **New projects: provider** | Settings' active provider | The provider a new project's chats start with. |
| **New projects: instructions** | Empty | The instructions a new project starts with. |

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
| **How strict** | Normal | **Lenient** reports only problems that would cause bugs, data loss or security issues, and skips style and minor suggestions. **Normal** sends the instructions as written. **Strict** also reports style, naming, readability, missing tests and documentation, and any risky assumption. The chosen level is added to the instructions. |
| **Review instructions** | (built in) | What the model is asked to look for. **Reset** restores the built-in text. |

The working folder must be a git repository. If there are no changes, or the base branch can't
be found, the app says so and sends nothing.

## OCR for scanned PDFs

A PDF with no text layer (a scan, or a photo of pages) has almost no text when the app reads it.
With **Read scanned PDFs (OCR)** on, its page images are read when the file is attached, and the
text comes back page by page under `--- Page N ---` headings, with a note that it came from OCR.
The reading shows "Reading scanned PDF… (OCR)" until it is done. With it off, such a PDF is
attached with the note "No text layer; turn on OCR in Settings → Context & files to read the
scanned pages."

| Setting | Default | Description |
|---|---|---|
| **Read scanned PDFs (OCR)** | off | Reads the page images of PDFs that have no text layer. |
| **Read with** | A vision model | **A vision model** sends each page image to a provider, one request per page. **Tesseract** runs the `tesseract` program on this computer. |
| **Vision model by** | The chat's provider | The provider profile for the vision model. The model must accept images. |
| **Tesseract program** | `tesseract` | A name on PATH, or the full path of the program. |
| **Languages** | `eng` | Tesseract language codes joined with `+`, such as `eng+deu`. Each must be installed with Tesseract. |
| **Pages per PDF** | 20 (1–200) | Pages past this are not read, and the text says how many were left out. |

Images in encodings this reader doesn't handle (JPEG 2000, CCITT fax, JBIG2, CMYK) are skipped,
with a note in the text. Images that are inside form XObjects are not read.
