---
title: Subagents
parent: Settings
nav_order: 5
---

# Subagents

Let the model delegate independent tasks to agents that work in parallel and report back. See
[Using the App → Subagents](../using/subagents.html) for how it looks in a chat.

| Setting | Default | Description |
|---|---|---|
| **Let the model delegate tasks to subagents** | on | Adds the `delegate` tool. Delegating asks for approval unless tool calls are auto-approved. It can also be switched per chat from the tools menu. |
| **Provider** | Same as the chat | The provider profile subagents use. |
| **Model** | the provider's selected model | Type a model name or pick one from the provider's fetched list. |
| **Per-task choice** | on | The `delegate` tool lists your provider profiles, so the model can pick another profile or model per task, for example a fast local model for simple tasks. |
| **Tools: Time, calculator and web fetch** | on | Subagents get these when they are on in the chat. |
| **Tools: The chat's MCP tools** | off | Subagents get the chat's MCP tools; untick servers to leave them out. Subagents call MCP tools without asking for approval. |
| **Tools: The workspace tools, with commands in their own sandbox container** | off | Only while the [Docker sandbox](docker.html) is on and the chat has a working folder. Each subagent gets the chat's workspace tools, and runs shell commands in a container of its own (see [Subagent containers](docker.html#subagent-containers)). Subagents read, write and run commands without asking for approval: approving the `delegate` call allows it. |
| **Run at once** | 3 | How many subagents work at the same time (1–16). |
| **Tasks per call** | 8 | Most tasks one `delegate` call may hand out (1–32). |
| **Max tool rounds** | 10 | Tool-call rounds each subagent may make (1–100). |
| **Time limit** | 300 s | Per subagent (10–3600 s). |
| **Subagent system prompt** | see below | Instructions every subagent starts with. |

The default subagent system prompt:

> You are a subagent working on one task delegated by another assistant. Complete the task on
> your own and reply with a concise, complete result. You cannot ask follow-up questions.

Subagents never get the task list or `delegate`. They get the workspace tools only with the
setting above and the Docker sandbox on, so their commands never run on your computer itself.
With **Mount read-only** on, they don't get `write_file` or `edit_file` (those work on your
computer, not in the container).
