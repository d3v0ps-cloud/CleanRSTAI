---
title: Command line
parent: Using the App
nav_order: 8
---

# Command line
{: .no_toc }

`cleanrstai exec` runs one request from a terminal or a script, with the settings you saved in the
app: the same provider, tools, MCP servers, instructions and skills as a new chat. The reply goes to
stdout, and the tool calls to stderr. Nothing is ever asked interactively.

Turn it on in [Settings → Background](../settings/background.html#command-line) first; it is on by
default, and it only runs when you type the command.

1. TOC
{:toc}

## Usage

```sh
cleanrstai exec [OPTIONS] PROMPT
cleanrstai exec [OPTIONS] -        # read the prompt from stdin
cleanrstai --version
```

The prompt is the rest of the arguments joined by spaces. Flags can come before or after it; `--`
ends the flags, so a prompt can start with a dash. A `-` among the arguments adds stdin to the
prompt, after the words.

| Flag | What it does |
|---|---|
| `--provider NAME` | The provider profile to use, by name (without regard to case) or by id. Default: the active provider. |
| `--model MODEL` | The model to use instead of the provider's. |
| `--workspace DIR` | The working folder for the file, search, git and shell tools, as a chat's folder. Without it, no workspace tools are offered. |
| `--mode MODE` | The approval mode: `read-only`, `default`, `auto` or `allow-all`. Default: the mode set in Settings. |
| `--json` | Print nothing until the end, then one JSON object. |
| `--no-mcp` | Don't start the MCP servers for this request. |
| `--save` | Save the chat, so it shows in the app's chat list. |
| `-h`, `--help` | Show the usage. |

`--provider` and `--model` take a value after a space or after `=` (`--model=qwen3`).

## Modes

The mode decides which tool calls run. It is the same set as a chat's tools menu, spelled for a shell:

| `--mode` | In the app | What runs without asking |
|---|---|---|
| `read-only` | Read only | Read-only tools: reading, searching and git status and diff in the workspace, the time and calculator. File writes, edits and shell commands are not offered to the model at all. |
| `default` | Default permissions | The same as a chat that asks: only the tools that never ask. |
| `auto` | Auto in workspace | Also file writes and edits in the workspace, and shell commands when the Docker sandbox is on. |
| `allow-all` | Allow all | Every tool call. |

Any call that would ask and that the mode doesn't allow is **denied**. The model receives a
refusal in place of the result and can carry on without it, and stderr shows:

```
denied: write_file (use --mode to allow)
```

Nothing is asked and nothing waits for an answer, so the command works in scripts and in CI. The
setting "Tool calls: Run MCP tools and subagents without asking" also applies to command-line
requests, as it does to chats.

Use `allow-all` only where a run may change anything in the workspace or call any MCP server.

## Output

- **Stdout** gets the reply text as it streams, then a final newline.
- **Stderr** gets one short line per tool call: `→ tool_name` when it starts, then `← ok` or
  `← error`. Denied calls, errors and MCP servers that did not connect are listed there too.
- With `--json`, stdout gets one object at the end and nothing before it:

```json
{
  "reply": "The diff renames the setting and updates its test.",
  "tool_calls": [
    {"name": "git_diff", "arguments": "{}", "result": "diff --git …", "is_error": false}
  ],
  "stats": {"prompt_tokens": 2143, "completion_tokens": 58, "...": "..."},
  "error": null
}
```

`reply` is all the reply text, across rounds of tool calls. `arguments` is the call's arguments as
the model wrote them, a JSON string. `stats` are the figures of the last request, the same as the
readout under a reply in the app, or `null` when the provider sent none. `error` is `null`, or the
message of the error that ended the run.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | The reply finished. |
| `1` | The provider or the agent failed (the error is on stderr, and in `error` with `--json`). |
| `2` | Bad arguments, an unknown provider or folder, an empty prompt, or command-line requests are off in Settings. |

## Stdin and pipes

```sh
git diff | cleanrstai exec --mode read-only "review this diff" -
cat notes.md | cleanrstai exec "turn these notes into a checklist" -
cleanrstai exec --json "list the TODO comments in src/" --workspace ~/code/app | jq -r .reply
```

## Chats

A command-line request is a new chat each time, with the one prompt. It is not saved unless you
pass `--save`, or turn on **Save each run's chat** in Settings. A saved chat behaves like any other
chat: it shows in the sidebar, and you can continue it in the app. Files a tool changed stay changed
either way; the undo for file edits is kept in a saved chat only.

## Settings

Settings → Background → **Command line** holds the defaults for every run. See
[Background](../settings/background.html#command-line).

## Windows

The Windows app is built without a console window. `cleanrstai exec` attaches to the console that
started it, so run it from Command Prompt, PowerShell or a script. When the app is started from
Explorer, there is no console to print to, and nothing is shown.

## Sending prompts out

A command-line request sends its prompt, the tools' descriptions, and the workspace content the
model reads to the provider, exactly as a chat does. See
[Data and privacy](../reference/data-and-privacy.html#what-leaves-your-computer).
