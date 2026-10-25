---
title: MCP Servers
parent: Settings
nav_order: 3
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
- **－ Remove** removes the selected server.
- **Import JSON…** opens a box where you can paste an `mcpServers` config (Claude Desktop,
  Cursor and others use this format) and click **Import**. The servers are added to the list;
  click **Apply** to connect them.

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
| **Headers** | Sent with every request, such as `Authorization: Bearer …`. Values are stored in the keychain. |
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
- **Resources** and **Prompts**: what the server offers in the MCP menu (books icon) next to
  the message box.
- **Server log (stderr)**: the last lines a stdio server printed, for troubleshooting.
