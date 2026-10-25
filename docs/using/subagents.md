---
title: Subagents
parent: Using the App
nav_order: 4
---

# Subagents

With subagents on, the model gets a `delegate` tool. It can hand independent tasks to
subagents that each work on one task on their own, in parallel, and report back. The main chat
then uses their results.

![Subagents, the calculator and the task list in a chat]({{ '/tools-subagents.png' | relative_url }})

## How it works

1. The model calls `delegate` with a list of tasks. Like MCP tools, this asks for your
   approval unless tool calls are auto-approved.
2. Each subagent runs its own model and tool loop. Up to **Run at once** (default 3) work at
   the same time; the rest wait for a free slot.
3. Each subagent's progress, tool calls and result show in the chat, with a status icon for
   waiting, running, done or failed.
4. Their token usage counts towards the chat's totals.

## What subagents can use

- **Provider and model**: the chat's, unless you choose another in
  [Settings → Subagents](../settings/subagents.html). With **Per-task choice** on, the model can
  pick one of your provider profiles per task, for example a fast local model for simple
  tasks.
- **Tools**: time, calculator and web fetch when they are on in the chat, and optionally the
  chat's MCP tools. Subagents never get the task list, `delegate` or the workspace tools.

{: .warning }
> Subagents call MCP tools and fetch URLs **without asking for approval**. Only give them MCP
> tools from servers you trust to run unattended.

## Turning it off

Untick **Let the model delegate tasks to subagents** in
[Settings → Subagents](../settings/subagents.html), or switch `delegate` off for one chat in the
tools menu.
