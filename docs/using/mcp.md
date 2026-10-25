---
title: MCP servers
parent: Using the App
nav_order: 3
---

# MCP servers
{: .no_toc }

1. TOC
{:toc}

[Model Context Protocol](https://modelcontextprotocol.io) servers give the model tools
(files, databases, the web and more), plus resources you can attach and prompts you can
insert. CleanRSTAI connects to:

- **stdio** servers: a local process started with `npx`, `uvx`, `docker` or any binary, with
  arguments, environment variables and a working directory.
- **Streamable HTTP** servers: a URL, with custom headers (such as `Authorization`) and OAuth
  sign-in.

![MCP server settings]({{ '/settings-mcp.png' | relative_url }})

## Adding a server

Open **Settings → MCP Servers** and either:

- click **+ Add** and fill in the fields (see [MCP Servers settings](../settings/mcp-servers.html)), or
- click **Import JSON…** and paste the `{"mcpServers": {...}}` config used by Claude Desktop,
  Cursor and others.

Then click **Apply**. The dot next to each server shows its state: green when connected,
amber while connecting, red on an error, and grey when disabled or not connected.

### Example config

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/Documents"]
    },
    "fetch": { "command": "uvx", "args": ["mcp-server-fetch"] },
    "remote": {
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

Entries with `command` become stdio servers and entries with `url` become HTTP servers.

## Tool names

Tools are shown to the model as `<server>__<tool>`, for example `filesystem__read_file`, so
give servers short, descriptive names.

To hide a tool from the model, untick it under **Tools** on the server's settings page, or
switch it off for one chat in the tools menu next to the message box.

## Signing in (OAuth)

When a remote server needs sign-in, its status reads **Sign-in required** and the top bar
warns that an MCP server needs you to sign in. Click **Sign in…** on the server's settings
page: your browser opens, and CleanRSTAI completes the standard MCP authorization flow
(discovery, automatic client registration, PKCE and a loopback redirect).

Tokens are kept in the keychain and refreshed automatically. **Sign out** forgets them. For
servers that can't register CleanRSTAI automatically, enter the **OAuth client ID** from the
server's documentation.

## Resources and prompts

The MCP menu (books icon) next to the message box lists each connected server's:

- **Resources**: click one to attach it to your message. PDFs and Office documents are
  attached as text, like file attachments.
- **Prompts**: click one to insert it. If it takes arguments, a form asks for them first.

Tool, resource and prompt lists refresh automatically when a server reports a change.

## Troubleshooting

- **Error status**: the error is shown under the status line. For stdio servers, open
  **Server log (stderr)** to see what the process printed.
- **Command not found on macOS**: CleanRSTAI imports your login shell's `PATH` at startup, so
  check the command works in a new terminal window, then restart the app.
- **Changes don't take effect**: click **Apply**. Only servers whose connection settings
  changed are reconnected; use **Reconnect** to restart one by hand, or **Refresh** to reload
  its tools.
- **Approvals**: MCP tools ask before each call. See [approving tool calls](tools.html#approving-tool-calls).
