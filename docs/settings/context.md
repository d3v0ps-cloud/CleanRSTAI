---
title: Context & memory
parent: Settings
nav_order: 6
---

# Context & memory
{: .no_toc }

What a chat carries along: compaction of long chats, memory across chats, projects, and the versions a regenerated reply keeps. These are the defaults for new chats; switch each one for one chat under **Chat options** in the Tools menu next to the message box (see [Tools and approvals](../using/tools.html#per-chat-tool-switches)).

1. TOC
{:toc}

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
| **Shorten older tool results** | off | A cheaper step that sends less with every request: results of older tool calls, older than the kept messages, are cut in what is sent. The chat keeps the results whole. |
| **Shorten to** | 2000 characters | With **Shorten older tool results** on: the length older results are cut to (200–100,000). |
| **Summary instructions** | (built in) | What the summarising model is asked to keep. **Reset** restores the built-in text. |

**Compact now** in the tools menu, and **Compact this chat** in the command palette (⌘/Ctrl +
K), summarise the older messages straight away, even with automatic compaction off.

How the size is worked out: after a reply the server reports how many tokens it read; the app
adds about one token per four characters for what came after. Once a chat has a summary, the
whole estimate is by characters, since the server's counts covered messages that are no longer
sent.

The request that writes a summary counts in the chat's totals and cost, and its line shows under
the summary marker. Its size doesn't count as the chat's context use.

## Memory

**Memory** (default off) lets the model keep facts about you across chats. The saved memories are
listed on this tab, where you can edit, delete and add them. See [Memory](../using/memory.html).

| Setting | Default | Description |
|---|---|---|
| **New chats use memory** | Off | Chats have memory on unless switched off for one chat (incognito). |
| **Ask before saving or deleting a memory** | On | A save or delete waits for your approval. Listing never asks. |
| **Size limit** | 8 KB | The text of the memories sent with each request, newest first (1–64 KB). |
| **Scope** | Global | Global: one set of memories for every chat. Per project: a project's chats also see and save that project's memories. |

A memory is one line of at most 500 characters. The list shows each memory's scope and the date it
was saved, and **Delete all memories** asks first.

## Projects

**Projects** (default on) groups chats on the **Projects** page, with the instructions and files
each project adds to its chats. Off hides the projects (the page offers to turn them on) and stops
adding their
context to requests; a chat can also leave the project context out from the tools menu. See
[Projects](../using/projects.html).

| Setting | Default | Description |
|---|---|---|
| **Files size limit** | 256 KB | The text of a project's files sent with each request, in all (16–4096 KB). Longer files are cut. |
| **New projects: folder** | None | The working folder a new project starts with. |
| **New projects: provider** | Settings' active provider | The provider a new project's chats start with. |
| **New projects: instructions** | Empty | The instructions a new project starts with. |

## Branches

**Keep the version a reply replaces** (default on): retrying a reply or editing a message keeps
the version it replaces, and arrows switch between the versions. See
[Editing and retrying](../using/chats.html#branches). Off, the replaced version is dropped as
before; versions already kept stay switchable. A chat's **Branches** switch overrides this.

**Versions kept per message** is how many older versions of one message are kept: 1 to 50,
default 10. The oldest is dropped beyond that.
