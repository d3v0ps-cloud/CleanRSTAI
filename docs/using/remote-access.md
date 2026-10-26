---
title: Remote access
parent: Using the App
nav_order: 6
---

# Remote access
{: .no_toc }

1. TOC
{:toc}

## What it is

CleanRSTAI can serve its chats to a web browser on another device, such as a phone, a tablet
or another computer on your network. The browser shows the same chats as the app.

The app keeps doing the work. Model requests, tools and approvals run on the computer where
CleanRSTAI is open, and the browser only shows them.

Remote access is off by default.

## Turning it on

1. Open **Settings → General** and go to the **Remote access** section
   ([details](../settings/general.html#remote-access)).
2. Tick **Serve the chat interface to browsers on other devices**.
3. Under **Listen on**, choose **This computer only** (the default) or **Local network**.
4. Check the **Port**. It is 8484 unless you change it.
5. Enter a **Password**. The server does not start until there is one.
6. Under **HTTPS**, keep **Self-signed certificate** (the default), choose **Certificate files**
   and enter your own PEM certificate and key, or choose **Off (plain HTTP)**.
7. Click **Apply** or **Save & Close**.

When the server is running, **Status** shows the address, for example
`https://192.168.1.20:8484/` on the local network.

## Connecting from a phone or another computer

Open the address shown under **Status** in the browser on the other device.

- With **This computer only**, only this computer can reach the address. Choose **Local network**
  to reach it from a phone or another computer.
- With the self-signed certificate, the browser warns that the certificate is not trusted the
  first time. Check that the address matches the one under **Status**, then continue. Your own
  certificate, such as one from [mkcert](https://github.com/FiloSottile/mkcert) or your
  organisation, is trusted once its authority is installed on the device.
- With **Off (plain HTTP)**, the browser page shows a red banner. The password and chats cross
  the network unencrypted.

Enter the password on the login form. The browser stays logged in for the time described under
[Security](#security).

Scripts can skip the login form by sending the password in an `Authorization: Bearer <password>`
header. Requests that change something also need the header `X-CleanRSTAI: 1`.

## What the browser can do

- Browse the chat list: pinned chats first, then recent ones. Search the list.
- Start a new chat, send messages, stop a reply and regenerate one.
- Rename, pin and delete chats.
- Choose the provider and model under the message box.
- Answer tool approvals with **Once**, **Always allow this tool**, **Always allow this server**
  or **Deny**. Approval prompts appear in the browser and in the app, so you can answer in
  either place.
- Read tool calls as expandable cards, reasoning in a collapsed section and errors in a banner.
  Replies are shown as Markdown; any HTML in a reply is shown as text.

The page works at phone width. On a narrow screen the sidebar opens from a menu button.

Enter and Shift+Enter follow the **Sending** setting, and Escape stops a reply. The browser
supports the shortcuts listed on the [Keyboard shortcuts](../reference/keyboard-shortcuts.html)
page where the browser allows them.

Attachments are not available from the browser in this version. A message shows the names of
its attachments, but you can't add files from the browser.

## Security

- **Password.** The server refuses to start without one. The password is kept in the system
  keychain with your API keys ([Secrets](../settings/general.html#secrets)).
- **HTTPS.** The default is a self-signed certificate. The app keeps it in a `remote` folder in
  its configuration folder and makes a new one when this computer's network address changes.
  Use HTTPS unless the network is one you trust completely. With **Off (plain HTTP)** the
  password and chats cross the network unencrypted. Strict transport security is sent only with
  your own certificate files (**Certificate files**).
- **Addresses.** The server only answers requests addressed to an IP address, `localhost`, a
  `.local` name or this computer's name, so a web page can't reach it through a look-alike
  domain (DNS rebinding).
- **Sessions.** A login lasts 30 days, or 7 days without use. Sessions are kept in memory only.
  Restarting the server logs every browser out. Changing any remote access setting, or quitting
  the app, restarts the server.
- **Lockout.** After 5 wrong passwords within 10 minutes, the address that sent them is refused
  for 60 seconds. Each wrong password also takes about half a second to answer.
- **What never leaves the computer.** API keys, MCP secrets and settings are not sent to the
  browser. Model requests, tools and approvals stay on this computer.
- **Network.** Use remote access on a network you trust. To reach it from outside, use the
  [Cloudflare Tunnel](#reaching-it-from-outside-your-network). Do not forward the port from your
  router to the internet.

## Reaching it from outside your network

A [Cloudflare Tunnel](https://developers.cloudflare.com/tunnel/) makes the server reachable
from anywhere, without opening a port on your router. CleanRSTAI runs Cloudflare's
`cloudflared` program for you while remote access is on. There are two kinds:

- **Quick tunnel**: a random `https://….trycloudflare.com` address, with no Cloudflare
  account. The address changes every time the tunnel starts, and Cloudflare offers quick
  tunnels for trying things out, with no uptime guarantee.
- **Named tunnel**: your own address such as `https://chat.example.com`, from a tunnel in your
  Cloudflare account on a domain you have on Cloudflare. Use this one to keep using it.

Either way, install `cloudflared` first, from
[Cloudflare's downloads page](https://developers.cloudflare.com/tunnel/downloads/)
(on macOS, `brew install cloudflared`). You don't need to log in with it. If it isn't on your
`PATH`, set its location in the **cloudflared** row.

The tunnel needs a password of at least 12 characters, since anyone on the internet can reach
the login page. **Listen on** can stay at **This computer only**: the tunnel reaches the server
from this computer.

### Quick tunnel

1. In **Settings → General → Remote access**, tick **Cloudflare Tunnel** and choose **Quick
   tunnel**.
2. Optionally, enter **Allowed emails** (for example `me@example.com` or `*@example.com`,
   separated by commas). Cloudflare then asks visitors for a one-time PIN sent to that
   address before they reach the login page. This needs a recent `cloudflared` (2026.10 or
   later).
3. Click **Apply**. When **Tunnel status** says *Online at https://….trycloudflare.com/*, open
   that address on any device and log in.

Quick tunnels hold back the live updates the page normally receives, so through a quick tunnel
the page checks for changes every few seconds (every second while a reply is written) instead.

### Named tunnel

1. In the [Cloudflare dashboard](https://dash.cloudflare.com/), go to **Networking → Tunnels**
   and create a tunnel. Copy its token: the long `eyJ…` string in the install command the
   dashboard shows. You don't need to run that command.
2. In **Settings → General → Remote access**, tick **Cloudflare Tunnel**, choose **Named
   tunnel**, paste the **Tunnel token** and enter the **Public hostname**
   (`chat.example.com`).
3. Publish the hostname, so that Cloudflare sends it to this computer. A tunnel run with a
   token takes its routes from Cloudflare, so this is done there, either way:
   - **Let the app do it:** create an [API token](https://dash.cloudflare.com/profile/api-tokens)
     with **Account → Cloudflare Tunnel → Edit**, **Zone → DNS → Edit** and **Zone → Zone →
     Read**, and paste it as the **API token**. When the tunnel starts, the app adds the route
     for the hostname to the tunnel (keeping any other routes) and creates the DNS record that
     points the hostname at the tunnel. It won't replace a DNS record that already points
     somewhere else; the status says so instead.
   - **Or do it in the dashboard:** on the tunnel's page, add a **published application** route
     for the hostname with the service the status shows: with the self-signed certificate (the
     default), type **HTTPS**, URL `127.0.0.1:8484`, and under **TLS** turn on **No TLS
     Verify**; with **Off (plain HTTP)**, type **HTTP**, URL `127.0.0.1:8484`. Use your port
     if you changed it.
4. Click **Apply**. When **Tunnel status** says *Online at https://chat.example.com/* (and,
   with an API token, *Published …*), open that address on any device and log in.

### What changes with a tunnel

- **Cloudflare carries the traffic.** Cloudflare ends the HTTPS connection to the browser, so
  your chats pass through Cloudflare's network, in the clear inside it, on the way to this
  computer.
- **Logins are limited per visitor.** The lockout uses the visitor's address that Cloudflare
  reports. In addition, after 20 wrong passwords through the tunnel within 10 minutes, from
  any addresses, logins through the tunnel pause for 5 minutes. Logins on your own network
  aren't affected.
- **The tunnel's hostname is accepted** alongside the local addresses, only while the tunnel
  runs.
- **cloudflared restarts on its own** if it stops, after a few seconds and then less often,
  unless Cloudflare refuses the token. A quick tunnel gets a new address when it restarts.

Turning the tunnel off, turning remote access off or quitting the app stops `cloudflared`
(on Linux even if the app is killed). Changing the tunnel settings restarts only the tunnel, so
browsers on your network stay logged in. Turning the tunnel off doesn't remove a published
route or DNS record; delete them in the dashboard if you no longer want them.

## Limits

- **The desktop app must stay open.** If you quit it, the server stops and browsers cannot
  connect.
- **No attachments from the browser** in this version.
- **One password for everyone.** Anyone who knows it can log in and use every chat.
