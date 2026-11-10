---
title: MCP Servers
parent: Settings
nav_order: 10
---

# MCP Servers
{: .no_toc }

Model Context Protocol servers that give the model tools, resources and prompts. For an
overview, see [Using the App → MCP servers](../using/mcp.html).

1. TOC
{:toc}

## The server list

The left column lists your servers, each with a status dot. Below it:

- **+ Add** adds a stdio server named `new-server`.
- **Clone** adds a copy of the selected server below it, named `<name>-copy`, with the same
  transport, command or URL, headers, environment and switched-off tools. The copy is a server
  of its own: its secrets are saved for it when you apply, and a server that needs sign-in
  asks the copy to sign in again.
- **－ Remove** removes the selected server.
- **Import JSON…** opens a box where you can paste an `mcpServers` config (Claude Desktop,
  Cursor and others use this format) and click **Import**. The servers are added to the list;
  click **Apply** to connect them.

### Recommended servers

Under the buttons, **Recommended** lists hosted servers you can add in one click. Each is added
as a Streamable HTTP server with its URL filled in and an `Authorization: Bearer ` header ready
for a key; click **Apply** to connect it. A server whose URL is already in your list (added
here, by hand or by import, under any name) shows **Added** instead.

| Server | Name | URL | Notes |
|---|---|---|---|
| **Hugging Face** | `huggingface` | `https://huggingface.co/mcp` | Search models, datasets, Spaces and papers. Sign in when asked, or paste a Hugging Face token after `Bearer`. |
| **Context7** | `context7` | `https://mcp.context7.com/mcp` | Current library and framework documentation. Works without a key; a Context7 API key raises the limits. |
| **Exa** | `exa` | `https://mcp.exa.ai/mcp` | Web search, page contents and code search. An Exa API key raises the limits. |

If the name is taken by another server, the new one gets `-2` (and so on) after it.

## Fields

### Name

Used to prefix the tool names the model sees (`<name>__<tool>`), so keep it short.

### Enabled

Untick to disconnect the server and hide its tools without deleting it.

### Transport

**stdio (local process)** or **Streamable HTTP**.

### stdio fields

| Field | Description |
|---|---|
| **Command** | The program to start: `npx`, `uvx`, `docker` or a path to a server binary. |
| **Arguments** | One argument per row. |
| **Environment** | Variables set for the process, such as API tokens. Values are stored in the keychain. |
| **Working dir** | Optional folder to start the process in. |

### Streamable HTTP fields

| Field | Description |
|---|---|
| **URL** | The server's MCP endpoint, for example `https://example.com/mcp`. |
| **Headers** | Sent with every request. Choosing Streamable HTTP fills in an `Authorization` header with the value `Bearer `, so you only paste the token after it. A header still at `Bearer ` with no token (or with an empty value) is not sent, so a server that signs in with OAuth instead works without removing it. Values are stored in the keychain. |
| **OAuth client ID** | Optional. Only if the server can't register CleanRSTAI automatically and its documentation gives you a client ID. |

## Status and actions

After you apply, the **Status** line shows the connection:

| Status | Meaning |
|---|---|
| Apply settings to connect… | The server has unsaved changes. |
| Connecting… | Starting the process or reaching the URL. |
| Connected · N tools · server info | Ready. |
| Sign-in required | The HTTP server needs OAuth sign-in. |
| Waiting for you to sign in in your browser… | Sign-in is in progress. |
| Error | The error is shown below the status line. |
| Disabled | **Enabled** is off. |

The buttons next to it:

- **Sign in…** (HTTP servers that need it): opens your browser to sign in.
- **Sign out**: forgets the server's tokens.
- **Reconnect**: restarts the connection.
- **Refresh**: reloads the server's tools, resources and prompts.

## Tools, resources and prompts

Once connected, the page lists:

- **Tools**: untick a tool to stop offering it to the model. Hover for its description.
- **Resources** and **Prompts**: what the server offers in **Resources & prompts** (the **+** menu) in
  the message box.
- **Server log (stderr)**: the last lines a stdio server printed, for troubleshooting.
