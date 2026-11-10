---
title: Permissions
parent: Settings
nav_order: 5
---

# Permissions
{: .no_toc }

What runs without asking, how far one reply may go, and plan mode. Each chat picks its own permission mode under the message box; this page sets where new chats start (see [Tools and approvals](../using/tools.html)).

1. TOC
{:toc}

## Tool calls

**Run MCP tools and subagents without asking for approval** (default off).

{: .warning }
> This skips every approval, including web fetch and the workspace tools that write files and
> run shell commands.

**New chats start in** (default **Default permissions**) picks the [permission mode](../using/tools.html#permissions)
a new chat starts in: Default permissions, Auto in workspace, Allow all or Read only. Each chat
can change its own mode in the tools menu or with ⌘K; chats that already exist keep theirs.

## Tool rounds per reply

How many times the model may call tools before it must answer: 1 to 200, default 20.

## Reply limits

Stop a reply once the requests it made used this many tokens in all (prompt and completion
of every round, 0 to 100,000,000), or cost this much (in US dollars, 0 to 10,000, where the
server reports or the app estimates a cost). Both default to 0, no limit. The check runs after
each request, before the tools it asked for, so a reply over a limit ends with a message
instead of calling more tools.

## Tool timeout

How long one tool call may take: 5 to 3600 seconds, default 120.

## Plan mode

**Offer Plan in the permission menus** (default on) lists Plan in the permission menus, for a chat or
a new one; off, Plan is hidden and a chat set to it is read only. A Plan chat uses the
[Plan permission](../using/tools.html#permissions): the model reads, searches and proposes a numbered
plan, and changes nothing until you press **Approve plan** under the reply.

**Mode after approval** (default **Default permissions**) is the mode the chat switches to when you
approve a plan. Plan and Read only are not offered, since the plan is meant to run.

**Approval message** (default "The plan is approved. Carry it out.") is the message sent to the model
on approval. An empty message restores the default.

**Plan instructions** is the system prompt section a Plan chat gets, in place of the default text;
an empty field restores it, and **Reset** does the same.
