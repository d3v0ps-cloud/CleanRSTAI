---
title: Cloudflare Tunnel
parent: Using the App
nav_order: 7
---

# Cloudflare Tunnel
{: .no_toc }

Use your chats from anywhere, not only on your own network, without opening a port on your
router.

1. TOC
{:toc}

## How it works

[Remote access](remote-access.html) serves your chats to browsers on your network. A
[Cloudflare Tunnel](https://developers.cloudflare.com/tunnel/) makes that same page reachable
from the internet: CleanRSTAI runs Cloudflare's `cloudflared` program, which keeps an outgoing
connection to Cloudflare. Visitors open an `https://` address at Cloudflare, and Cloudflare
sends their requests down that connection to the app. Nothing on your network accepts incoming
connections.

The app starts `cloudflared` when remote access and the tunnel are on, shows its state in
Settings, starts it again if it stops, and stops it when you turn the tunnel off or quit.

## Quick or named?

| | Quick tunnel | Named tunnel |
|---|---|---|
| Address | random, such as `https://brave-new-words.trycloudflare.com` | your own, such as `https://chat.example.com` |
| Changes | every time the tunnel starts | never |
| Needs | nothing but `cloudflared` | a Cloudflare account, a domain on Cloudflare, and a tunnel you create in the dashboard |
| Updates in the page | every few seconds | live |
| Meant for | trying it out | keeping it |

Cloudflare gives quick tunnels no uptime guarantee, limits them to 200 requests at a time, and
holds back the live updates the page normally receives, so through a quick tunnel the page
checks for changes every few seconds instead (every second while a reply is written).

## Before you start

1. **Turn on remote access** in **Settings → General → Remote access** (see
   [Remote access](remote-access.html#turning-it-on)). **Listen on** can stay at **This
   computer only**: the tunnel reaches the server from this computer.
2. **Use a password of at least 12 characters.** Anyone on the internet can reach the login
   page, so the tunnel won't start with a shorter one.
3. **Install `cloudflared`** from
   [Cloudflare's downloads page](https://developers.cloudflare.com/tunnel/downloads/):
   - macOS: `brew install cloudflared`
   - Windows: `winget install --id Cloudflare.cloudflared`
   - Linux: the package for your distribution from the downloads page, or the single binary.

   You don't need to log in with it. If it isn't on your `PATH`, enter its location in the
   **cloudflared** row (or pick it with **Browse…**).

## Quick tunnel

1. In **Settings → General → Remote access**, tick **Cloudflare Tunnel** and choose **Quick
   tunnel** (the default).
2. Optionally, fill in **Allowed emails**: addresses or whole domains, separated by commas,
   such as `me@example.com, *@example.org`. Cloudflare then asks visitors for a one-time PIN
   sent to one of those addresses before they reach the login page. This needs `cloudflared`
   2026.10 or later. Leave it empty to let anyone with the address reach the login page.
3. Click **Apply**. **Tunnel status** shows *Connecting to Cloudflare…*, then
   *Online at https://….trycloudflare.com/* with a copy button.
4. Open that address on your phone or another computer and log in with your remote access
   password.

The address is new each time the tunnel starts (including when the app restarts), so copy it
again from Settings.

## Named tunnel

A named tunnel lives in your Cloudflare account. You create the tunnel and its route (the
hostname it serves) in the Cloudflare dashboard; the app only needs the tunnel's token to run
it.

### 1. Create the tunnel

1. Log in to the [Cloudflare dashboard](https://dash.cloudflare.com/) and go to
   **Networking → Tunnels**.
2. Select **Create Tunnel**, give it a name such as `cleanrstai`, and select **Create
   Tunnel**.
3. The dashboard shows install commands such as
   `cloudflared service install eyJhIjoi…`. Copy the long `eyJ…` string: that is the
   **tunnel token**. Don't run the command; CleanRSTAI runs `cloudflared` itself.

   To find the token later, open the tunnel and select **Add a replica**.

### 2. Add the route

The route tells Cloudflare which hostname the tunnel serves and where `cloudflared` sends its
requests. On the tunnel's page, go to **Routes**, select **Add route → Published
application**, and enter:

- **Hostname**: the address you want, such as `chat` on `example.com`. The domain must be on
  Cloudflare, in the same account.
- **Service**: this server. With the self-signed certificate (the default) that is type
  **HTTPS**, URL `127.0.0.1:8484`; then expand **Additional application settings → TLS** and
  turn on **No TLS Verify** (the certificate is self-signed). With **HTTPS → Off (plain
  HTTP)** it is type **HTTP**, URL `127.0.0.1:8484`. Use your port if you changed it. The
  **Tunnel status** note in Settings shows the exact service to enter.

Select **Add route**. Cloudflare also creates the DNS record that points the hostname at the
tunnel.

### 3. Enter the token in CleanRSTAI

In **Settings → General → Remote access**, tick **Cloudflare Tunnel**, choose **Named tunnel**,
and paste the **Tunnel token**. It is kept in the system keychain.

**Public hostname** is optional: the app learns the route's hostname from `cloudflared` once
the tunnel connects. Fill it in if the status shows *Connected, but the tunnel has no public
hostname yet* even though the route exists.

### 4. Apply and connect

Click **Apply**. **Tunnel status** shows *Connecting to Cloudflare…*, then *Online at
https://chat.example.com/* and the number of connections (usually 4). Open the address on any
device and log in.

## What the status means

| Tunnel status | What to do |
|---|---|
| *cloudflared isn't installed* | Install it (see [Before you start](#before-you-start)) or set its location. |
| *use a password of at least 12 characters* | Choose a longer remote access password. |
| *Paste the tunnel's token* / *Provided Tunnel token is not valid* | Copy the `eyJ…` token again from the dashboard. The app doesn't retry a refused token. |
| *Connecting to Cloudflare…* with an error under it | `cloudflared` can't reach Cloudflare yet. Check the internet connection; a firewall must allow outgoing connections on port 7844. It keeps trying. |
| *Connected, but the tunnel has no public hostname yet* | Add the route in the dashboard (see [Add the route](#2-add-the-route)), or enter its hostname in **Public hostname**. |
| *Trying again shortly* | `cloudflared` stopped; the app starts it again, waiting longer each time. |

In the browser:

- **Error 1033**: Cloudflare can't find a running tunnel. Check that the app is open and
  **Tunnel status** is *Online*.
- **502 Bad Gateway**: the route's service is wrong. Use the one **Tunnel status** shows, with
  **No TLS Verify** on for the self-signed certificate.
- **"unknown host"**: the app doesn't know the hostname yet: enter it in **Public hostname**
  (or, for a quick tunnel, the tunnel restarted with a new address).
- **"too many failed logins"**: see [Logins through the tunnel](#logins-through-the-tunnel).

## Logins through the tunnel

- The login page is on the internet, so use a strong password; the tunnel requires at least 12
  characters.
- After 5 wrong passwords within 10 minutes, the visitor's address (as Cloudflare reports it)
  is refused for 60 seconds.
- After 20 wrong passwords through the tunnel within 10 minutes, from any addresses, all logins
  through the tunnel pause for 5 minutes. Logins on your own network aren't affected.
- For a quick tunnel, **Allowed emails** adds Cloudflare's email check in front of the login
  page.

## Privacy

Cloudflare ends the HTTPS connection to the browser, so everything the page exchanges with the
app (your chats, replies, tool calls and approvals) passes through Cloudflare's network, in the
clear inside it, on the way to this computer. API keys, MCP secrets and settings still never
leave the computer. See
[Data and privacy](../reference/data-and-privacy.html).

## Turning it off

Untick **Cloudflare Tunnel** (or remote access) and click **Apply**, or quit the app: that stops
`cloudflared`, and the address stops working. On Linux `cloudflared` also stops if the app is
killed.

Turning the tunnel off doesn't change your Cloudflare account. To remove a named tunnel, delete
it (or its route) in **Networking → Tunnels**, and the hostname's `CNAME` record in **DNS →
Records** if it remains.

Changing the tunnel settings restarts only the tunnel, so browsers on your own network stay
logged in.
