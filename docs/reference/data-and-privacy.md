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
| API keys (including the web search key), header values, MCP environment variable values, OAuth tokens, remote access password, Cloudflare Tunnel token | the system keychain (on macOS, one item named `CleanRSTAI` / `secrets`) |
| Compaction summaries, file diffs and undo checkpoints (the previous content of files that `write_file` and `edit_file` changed, for the latest 20 turns by default) | in the chat's file in the `conversations` folder |
| Remote access certificate and key (self-signed, when HTTPS uses it) | PEM files in a `remote` folder inside the configuration folder, next to `settings.json` |

On macOS and Linux, settings, export files and the remote access key are created readable only
by you. If the keychain is off or unavailable, secrets are kept in `settings.json` instead
([Secrets](../settings/general.html#secrets)).

Remote access logins are kept in memory only. Nothing is written to disk, and every login ends
when the server restarts ([Remote access](../using/remote-access.html)).

## What leaves your computer

- **Your messages and attachments** go to the provider you chose, along with the system
  prompt and the descriptions of the tools that are on. With the OpenAI Responses API the app
  asks OpenAI not to store the response (`store: false`); the model's reasoning then comes back
  encrypted and is kept in the chat file to send back. With the Anthropic Messages API, the
  model's signed thinking is kept in the chat file the same way, and prompt caching (on by
  default) lets Anthropic hold the cached prompt briefly (minutes) to reuse it.
- **Tool calls**: MCP tools go to their servers; web fetch requests the URL the model chose;
  web search sends the model's query to the search service chosen in
  [Built-in Tools](../settings/built-in-tools.html#web-search) (DuckDuckGo by default; Brave,
  Tavily and Kagi also get your API key). They ask for approval unless you turned that off.
- **Context compaction** sends the older messages of a long chat (shortened where they are
  long) to the provider and model chosen in [Context & files](../settings/context.html), to
  have them summarised.
- **Project instructions**: `AGENTS.md`, `CLAUDE.md` or the other files you list, from a
  chat's workspace folder, go to the provider with the system prompt.
- **Skills**: the names and descriptions of the skills (from your skills folder and the chat's
  `.cleanrstai/skills` folder) go to the provider with every message while skills are on and
  some exist. A skill's text, and any other file of the skill that the model asks for, go to
  the provider when the model loads it. Nothing else is read from a skill folder.
- **Hooks** are commands you set in [Skills & hooks](../settings/skills-and-hooks.html#hooks).
  They run on this computer (or in the Docker sandbox, when it is on), with the chat's working
  folder as their directory. Each one gets the tool's name, the call's arguments as JSON and,
  for an after-tool hook, the tool's result, in environment variables. What a hook prints is
  kept in the chat, and an after-tool hook's output goes to the provider with the result. A
  command can do anything you could do in a terminal, so only set hooks you have read.
- **Code review** sends the working folder's git diff (up to the size you set, and the names of
  new files) to the provider and model chosen in
  [Context & files](../settings/context.html#code-review), when you choose Review changes.
- **Update checks** ask the [update source](../using/updates.html#update-source) for its latest
  release, at startup if enabled and when you check by hand.
- **Notifications** when a reply finishes, fails or needs approval are handed to the system's notification service (Notification Center, Windows notifications, or D-Bus on Linux). They contain the chat's title and what happened, never the reply, and the system may show them on your other devices. Off in [Background](../settings/background.html) settings, or per chat.
- **Web images** in replies are loaded only if **Load web images in replies** is on.
- **Docker sandbox**: while it is on, the app runs the `docker` command on your computer to run
  shell commands in containers. Docker may download images from a registry (Docker Hub unless
  the image names another one; the published full image comes from GitHub's registry,
  `ghcr.io`), and a custom Dockerfile's `FROM` and `RUN` lines download what they name. With
  **Only the hosts and ports below**, the app also builds a small network image from
  `ubuntu:24.04` and Ubuntu's package mirrors. Containers get only the chat's workspace folder, not the rest of your files, and no
  API keys or other secrets. With **Normal network (can reach this computer, your local network and the internet)** on, commands can reach the internet;
  with **Only the hosts and ports below**, only the hosts and ports you list, and the domains in
  those rules are looked up through your computer's DNS servers
  ([Docker sandbox](../settings/docker.html)).
- **Remote access**: while it is on, a browser that has logged in receives your chats, replies,
  tool calls and their results, approval prompts, chat titles, the model and provider names,
  and your name if you set one. It never receives API keys,
  MCP secrets or settings. Over plain HTTP all of this crosses the network unencrypted
  ([Remote access](../using/remote-access.html)).
- **Cloudflare Tunnel**: while it is on, `cloudflared` keeps a connection to Cloudflare, and
  everything a browser exchanges with the app through the tunnel passes through Cloudflare's
  network, which ends the HTTPS connection and so can read it
  ([Cloudflare Tunnel](../using/cloudflare-tunnel.html#privacy)).
- **Artifacts you open in your browser** are written to a `CleanRSTAI-artifacts` folder in the
  system's temporary folder and run in the browser, where a page can load anything it links to.
  The app only does this when you click **Open in browser**.
- **Web pages previewed in the panel** (macOS and Windows) can't load anything from the network
  unless **Let pages load from the network** is on in
  [General](../settings/general.html#web-page-preview).

Nothing else is sent: there is no telemetry.

## Backups

**Settings → General → Export & import** saves your settings and conversations to files you
choose. See [Export & import](../settings/general.html#export--import).
