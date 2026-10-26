---
title: General
parent: Settings
nav_order: 1
---

# General
{: .no_toc }

Appearance, behaviour, secrets and data.

1. TOC
{:toc}

## System prompt

The instructions sent at the start of every chat. Default: *You are a helpful assistant.*

## Theme

**System** (default) follows your operating system; **Light** and **Dark** fix it. You can
also switch from the command palette.

## UI scale

Scales the whole interface from 0.5× to 2.5× (default 1.0×).

## Sending

- **Enter sends, Shift+Enter for newline** (default)
- **Ctrl/⌘+Enter sends, Enter for newline**

**⌘/Ctrl + Enter** sends in both modes.

## Secrets

**Store API keys and secrets in the system keychain** (default on) keeps API keys, HTTP header
values, MCP environment variable values and the remote access password in the macOS Keychain, Windows Credential Manager
or the Secret Service (GNOME Keyring, KWallet) on Linux. `settings.json` keeps only blank
placeholders, and keys saved in plain text by earlier versions move to the keychain on first
launch.

On macOS every secret is kept in one Keychain item, so a new install or update asks for your
password once rather than once per key; choose **Always Allow** to stop the prompt until the
next update. Secrets that earlier versions kept as separate items move into it on first launch.

If no keychain is available, a warning says so and secrets stay in `settings.json`. On macOS
and Linux that file is readable only by you.

## Remote access

Serves the chat interface to a browser on another device. Off by default. The rows below
appear when it is on. See [Remote access](../using/remote-access.html) for setup and security.

- **Remote access**: the checkbox *Serve the chat interface to browsers on other devices*
  (off by default).
- **Listen on**: **This computer only** (the default, address 127.0.0.1) or **Local network**
  (all network addresses, 0.0.0.0).
- **Port**: a number from 1 to 65535 (default 8484).
- **Password**: a password field with a **Show** toggle, needed to log in from a browser. While
  it is empty the server does not start, and the status says *Set a password to start the
  server*. It is kept in the keychain like your API keys.
- **HTTPS**: **Self-signed certificate** (the default), **Certificate files** or **Off (plain
  HTTP)**. With **Certificate files**, enter the **Certificate file** and **Key file** paths
  (PEM), or pick them with **Browse…**. With **Off (plain HTTP)** a warning says the password
  and chats cross the network unencrypted.
- **Status**: *Running at* the address, one line per address with a copy button, or *Stopped*,
  or the error. With the self-signed certificate, a note says browsers warn about it the first
  time.
- **Cloudflare Tunnel**: the checkbox *Reach this computer from outside the network through a
  Cloudflare Tunnel* (off by default). When it is on:
  - **Tunnel type**: **Quick tunnel** (the default: a random `trycloudflare.com` address, no
    account needed) or **Named tunnel** (your own domain, with a Cloudflare account).
  - **Allowed emails** (quick tunnel): optional email addresses or `*@domain` patterns,
    separated by commas, that Cloudflare lets through with a one-time PIN. Empty lets anyone
    with the address reach the login page.
  - **Tunnel token** (named tunnel): the tunnel's token from the Cloudflare dashboard, with a
    **Show** toggle. It is kept in the keychain like your API keys.
  - **Public hostname** (named tunnel): the hostname for the tunnel, such as
    `chat.example.com`.
  - **API token** (named tunnel): optional. A Cloudflare API token with *Cloudflare Tunnel:
    Edit*, *DNS: Edit* and *Zone: Read*, which publishes the hostname for you (its route and
    DNS record). Kept in the keychain.
  - **cloudflared**: the path to `cloudflared`, or empty (the default) to find it on `PATH`,
    with **Browse…**.
  - **Tunnel status**: *Connecting to Cloudflare…*, *Online at* the public address with a copy
    button and the number of connections, or the error (*Trying again shortly* when it will);
    with an API token, *Publishing the hostname…*, *Published …* or why publishing failed.
    A warning appears while the password is shorter than 12 characters, which the tunnel
    needs. For a named tunnel without an API token, a note gives the service to point the
    hostname at in the dashboard.

Changes take effect when you click **Apply** or **Save & Close**. The server restarts, which
logs browsers out; a change to the tunnel settings alone restarts only the tunnel. See
[Reaching it from outside your network](../using/remote-access.html#reaching-it-from-outside-your-network).

## Updates

- **Check GitHub for a new release at startup** (default on).
- **Update from**: where releases come from: an `owner/repo`, a `github.com` link or a URL
  that answers like GitHub's `releases/latest` API. **Reset** restores the default.
- **Check for updates now** checks the source as typed, before you save, and shows the
  installed version. If a newer release can be installed, **Install update** appears.

See [Updates](../using/updates.html).

## Reasoning

How a reasoning model's **Thought process** section starts out:

- **Collapsed** (default)
- **Expanded**
- **Expanded while thinking**: open while the model reasons, collapsed once it answers.

## Stats

**Show prefill/decode speed and token counts under responses** (default on).

## Images

**Load web images in replies** (default off). When off, images from the web show as links.
Loading them tells the image's server that the chat was opened, and a reply could put chat
contents in the image address.

## Artifacts

**Open web pages, drawings and long code in a side panel** (default on). When a reply makes an
[artifact](../using/chats.html#artifacts), the panel opens beside the chat. When off, open it with
**Open in panel** by its link in the reply, or the panel button in the top bar.

## Tool calls

**Run MCP tools and subagents without asking for approval** (default off).

{: .warning }
> This skips every approval, including web fetch and the workspace tools that write files and
> run shell commands.

## Max tool rounds

How many times the model may call tools before it must answer: 1 to 200, default 20.

## Tool timeout

How long one tool call may take: 5 to 3600 seconds, default 120.

## Export & import

- **Export settings…** saves providers, MCP servers and preferences as last saved (apply
  first to include unsaved changes). API keys, header values and MCP environment variable
  values are left blank, with their names kept, unless you tick **Include API keys and
  secrets** for a complete backup.
- **Export conversations…** saves every chat, including tool calls and results, to one file.
- **Import settings…** loads a settings export into the settings page for review:
  - **Merge** adds the imported providers and MCP servers, updating ones that already exist,
    and keeps your other preferences.
  - **Replace** uses the imported settings as they are.

  Blank secrets are filled from matching entries you already have, and any still blank are
  listed so you can fill them in. Then click **Apply** or **Save & Close**.
- **Import conversations…** adds chats from a conversations export and skips any that already
  exist.

Both files record the app version and export time, and are created readable only by you on
macOS and Linux.

## File locations

The bottom of the page shows where the settings file and conversations folder are. See
[Where settings are stored](index.html#where-settings-are-stored).
