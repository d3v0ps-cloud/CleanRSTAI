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
| Needs | nothing but `cloudflared` | a Cloudflare account and a domain on Cloudflare |
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

A named tunnel needs three things in your Cloudflare account: the **tunnel**, a **route** that
sends your hostname to this computer, and a **DNS record** that points the hostname at the
tunnel. You create the tunnel; the app or you add the route and the DNS record.

### 1. Create the tunnel

1. Log in to the [Cloudflare dashboard](https://dash.cloudflare.com/) and go to
   **Networking → Tunnels**.
2. Select **Create Tunnel**, give it a name such as `cleanrstai`, and select **Create
   Tunnel**.
3. The dashboard shows install commands such as
   `cloudflared service install eyJhIjoi…`. Copy the long `eyJ…` string: that is the
   **tunnel token**. Don't run the command; CleanRSTAI runs `cloudflared` itself.

   To find the token later, open the tunnel and select **Add a replica**.

### 2. Enter it in CleanRSTAI

In **Settings → General → Remote access**, tick **Cloudflare Tunnel**, choose **Named tunnel**,
then:

- **Tunnel token**: paste the `eyJ…` string. It is kept in the system keychain.
- **Public hostname**: the address you want, such as `chat.example.com`. It must be on a domain
  in the same Cloudflare account.

### 3. Publish the hostname

The tunnel token only lets `cloudflared` run the tunnel: it can't add routes or DNS records to
your account. Publish the hostname in one of two ways.

**Let the app publish it (with an API token).** Create a Cloudflare API token that may edit the
tunnel and your domain's DNS:

1. In the dashboard, open your profile (top right) → **My Profile → API Tokens**, or go to
   [dash.cloudflare.com/profile/api-tokens](https://dash.cloudflare.com/profile/api-tokens).
2. Select **Create Token**, then **Create Custom Token → Get started**.
3. Name it, for example `CleanRSTAI tunnel`, and add three permissions:

   | | | |
   |---|---|---|
   | **Account** | **Cloudflare Tunnel** | **Edit** |
   | **Zone** | **DNS** | **Edit** |
   | **Zone** | **Zone** | **Read** |

4. Under **Account Resources**, include your account. Under **Zone Resources**, include the
   domain the hostname is on (**Specific zone**).
5. Select **Continue to summary → Create Token** and copy the token. Cloudflare shows it only
   once.
6. Paste it as the **API token** in CleanRSTAI. It is kept in the system keychain.

When the tunnel starts, the app adds a route for the hostname to the tunnel, keeping any other
routes the tunnel has, and creates a proxied `CNAME` record from the hostname to
`<tunnel-id>.cfargotunnel.com`. If the hostname already has a DNS record that points somewhere
else, the app leaves it alone and says so: remove that record in the dashboard (**DNS →
Records**) or choose another hostname. Publishing again later changes nothing that is already
right.

**Or publish it yourself in the dashboard.** Open the tunnel, go to **Routes**, select **Add
route → Published application**, and enter:

- **Hostname**: your hostname, such as `chat` on `example.com`.
- **Service**: the one **Tunnel status** shows in Settings. With the self-signed certificate
  (the default) that is type **HTTPS**, URL `127.0.0.1:8484`; then expand **Additional
  application settings → TLS** and turn on **No TLS Verify** (the certificate is self-signed).
  With **HTTPS → Off (plain HTTP)** it is type **HTTP**, URL `127.0.0.1:8484`. Use your port if
  you changed it.

The dashboard creates the DNS record for you.

### 4. Apply and connect

Click **Apply**. **Tunnel status** shows *Connecting to Cloudflare…*, then *Online at
https://chat.example.com/* and the number of connections (usually 4), and with an API token
*Published https://chat.example.com/ → https://127.0.0.1:8484*. Open the address on any device
and log in.

## What the status means

| Tunnel status | What to do |
|---|---|
| *cloudflared isn't installed* | Install it (see [Before you start](#before-you-start)) or set its location. |
| *use a password of at least 12 characters* | Choose a longer remote access password. |
| *Paste the tunnel's token* / *Provided Tunnel token is not valid* | Copy the `eyJ…` token again from the dashboard. The app doesn't retry a refused token. |
| *Connecting to Cloudflare…* with an error under it | `cloudflared` can't reach Cloudflare yet. Check the internet connection; a firewall must allow outgoing connections on port 7844. It keeps trying. |
| *Connected, but the tunnel has no public hostname yet* | Publish the hostname (with an API token or in the dashboard). |
| *Publishing failed: … refused the API token* | Check the token's three permissions and that its zone resources include your domain. |
| *Publishing failed: … already has a DNS record* | Remove that record in **DNS → Records**, or choose another hostname. |
| *Publishing failed: No domain on this Cloudflare account matches …* | The hostname must be on a domain in the same account, and the token needs **Zone: Read**. |
| *Trying again shortly* | `cloudflared` stopped; the app starts it again, waiting longer each time. |

In the browser:

- **Error 1033**: Cloudflare can't find a running tunnel. Check that the app is open and
  **Tunnel status** is *Online*.
- **502 Bad Gateway**: the route's service is wrong. Use the one **Tunnel status** shows, with
  **No TLS Verify** on for the self-signed certificate.
- **"unknown host"**: the hostname isn't the one entered in **Public hostname** (or, for a
  quick tunnel, the tunnel restarted with a new address).
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
leave the computer. With an API token, the app also calls Cloudflare's API
(`api.cloudflare.com`) each time the tunnel starts. See
[Data and privacy](../reference/data-and-privacy.html).

## Turning it off

Untick **Cloudflare Tunnel** (or remote access) and click **Apply**, or quit the app: that stops
`cloudflared`, and the address stops working. On Linux `cloudflared` also stops if the app is
killed.

Turning the tunnel off doesn't change your Cloudflare account. To remove everything for a named
tunnel:

1. **Networking → Tunnels**: open the tunnel and delete its route, or delete the tunnel.
2. **DNS → Records**: delete the hostname's `CNAME` record.
3. **My Profile → API Tokens**: delete the API token if you made one.

Changing the tunnel settings restarts only the tunnel, so browsers on your own network stay
logged in.
