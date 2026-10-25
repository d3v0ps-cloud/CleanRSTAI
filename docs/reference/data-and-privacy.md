---
title: Data and privacy
parent: Reference
nav_order: 2
---

# Data and privacy
{: .no_toc }

1. TOC
{:toc}

## What is stored on your computer

| Data | Where |
|---|---|
| Settings | `settings.json` (see [Where settings are stored](../settings/#where-settings-are-stored)) |
| Conversations | one JSON file per chat in the `conversations` folder |
| API keys, header values, MCP environment variable values, OAuth tokens, remote access password, Cloudflare Tunnel token | the system keychain (on macOS, one item named `CleanRSTAI` / `secrets`) |
| Remote access certificate and key (self-signed, when HTTPS uses it) | PEM files in a `remote` folder inside the configuration folder, next to `settings.json` |

On macOS and Linux, settings, export files and the remote access key are created readable only
by you. If the keychain is off or unavailable, secrets are kept in `settings.json` instead
([Secrets](../settings/general.html#secrets)).

Remote access logins are kept in memory only. Nothing is written to disk, and every login ends
when the server restarts ([Remote access](../using/remote-access.html)).

## What leaves your computer

- **Your messages and attachments** go to the provider you chose, along with the system
  prompt and the descriptions of the tools that are on.
- **Tool calls**: MCP tools go to their servers; web fetch requests the URL the model chose.
  Both ask for approval unless you turned that off.
- **Update checks** ask the [update source](../using/updates.html#update-source) for its latest
  release, at startup if enabled and when you check by hand.
- **Web images** in replies are loaded only if **Load web images in replies** is on.
- **Remote access**: while it is on, a browser that has logged in receives your chats, replies,
  tool calls and their results, approval prompts, chat titles, the model and provider names,
  and your name if you set one. It never receives API keys,
  MCP secrets or settings. Over plain HTTP all of this crosses the network unencrypted
  ([Remote access](../using/remote-access.html)).
- **Cloudflare Tunnel**: while it is on, `cloudflared` keeps a connection to Cloudflare, and
  everything a browser exchanges with the app through the tunnel passes through Cloudflare's
  network, which ends the HTTPS connection and so can read it
  ([Reaching it from outside your network](../using/remote-access.html#reaching-it-from-outside-your-network)).
- **Artifacts you open in your browser** are written to a `CleanRSTAI-artifacts` folder in the
  system's temporary folder and run in the browser, where a page can load anything it links to.
  The app only does this when you click **Open in browser**.

Nothing else is sent: there is no telemetry.

## Backups

**Settings → General → Export & import** saves your settings and conversations to files you
choose. See [Export & import](../settings/general.html#export--import).
