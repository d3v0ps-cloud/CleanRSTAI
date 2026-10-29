---
title: Memory
parent: Using the App
nav_order: 10
---

# Memory

Memory lets the model keep facts about you across chats: your name, what you work on, how you
like answers written. A fact is saved when you ask the model to remember something, or when you
state it about yourself and the model decides it is worth keeping. The saved facts are added to
the system prompt of the chats that have memory on, so a new chat already knows them.

Memory is off by default. Turn it on under **Settings → Context & files → Memory**.

## What the model can do

Memory gives the model one tool, **Memory**, with three actions:

- **save** stores a fact, in one line of at most 500 characters. A fact the same as one already
  remembered (ignoring capitals) is not saved again, and the model is told so.
- **list** shows what this chat can see. It asks for nothing.
- **delete** forgets a memory by its id. A chat can delete only the memories it can see.

Each memory has an id, its text, when it was saved, and the chat it was saved from. The model
sees the ids in the system prompt, so it can delete a memory when you say it is wrong.

## Approval

With **Ask before saving or deleting a memory** on (the default), a save or delete shows an
approval card like other tools that change things, and nothing is saved until you approve it.
Listing never asks. Turn the question off to let the model save and delete without asking.

In a command-line run ([`cleanrstai exec`](command-line.html)) there is nobody to ask, so a save
or delete is denied unless the run's approval mode allows it, as for any other tool.

## What the chat sees

Under **Scope** in Settings:

- **Global** (the default): one set of memories for every chat. Every memory is global, and
  every chat sees all of them.
- **Per project**: a chat in a [project](projects.html) sees the global memories and that
  project's own, and saves new ones to the project. A chat outside any project sees only the
  global memories. Memories of other projects are never shown to a chat.

Switching the scope doesn't delete anything. Under Global, the project memories are hidden
(they stay in the list in Settings).

## Incognito chats

A chat can switch memory off for itself: the **Memory** switch under **Chat features** in the
tools menu. Such a chat is incognito:

- its system prompt has no memories, and the model is not told about memory at all;
- the model has no memory tool, so it can't save or delete anything;
- the top bar shows **Incognito** while the chat is open.

**New incognito chat** in the command palette (⌘K) starts a new chat with memory off, whatever
the Settings default is. Subagents never get memories or the memory tool.

## Managing memories in Settings

**Settings → Context & files → Memory** lists every saved memory, newest first, with its scope,
the date it was saved and its text. You can:

- edit a memory's text: the change is saved when you leave the field. An empty text deletes it;
  a text that another memory in the same scope already has is refused;
- delete one with the trash button;
- **Delete all memories** (asks first);
- **Add a memory** yourself, as global or, in per-project scope, for a project.

Changes made in Settings take effect at once, without **Apply**.

## What is sent to the model

With memory on, the memories the chat can see are added to the system prompt of each reply,
newest first, up to the **Size limit** (8 KB by default, 1 to 64 KB). Older memories beyond the
limit are left out and the model is told how many. The list is read when a reply starts, and the
same list is sent with every request of that reply, so a memory the model saves during a reply
is used from the next reply on.

Memories are sent to your provider with every request where memory is on, the same way as the
chat itself. See [Data and privacy](../reference/data-and-privacy.html).

## Settings

| Setting | Default | Description |
|---|---|---|
| **New chats use memory** | Off | Whether a new chat has memory on. Each chat can switch it off for itself. |
| **Ask before saving or deleting a memory** | On | Saves and deletes wait for your approval. |
| **Size limit** | 8 KB | The text of the memories sent with each request (1 to 64 KB). |
| **Scope** | Global | Global, or per project (see above). |

The same switch for one chat is **Memory** under **Chat features** in the tools menu.
`cleanrstai exec` follows the setting; `--no-memory` makes one run incognito.
