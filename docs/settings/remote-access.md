---
title: Remote access
parent: Settings
nav_order: 16
---

# Remote access

Use your chats from a browser on another device. See [Remote access](../using/remote-access.html) and [Cloudflare Tunnel](../using/cloudflare-tunnel.html) for how to set it up and use it.

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
  - **Public hostname** (named tunnel): optional, the hostname of the tunnel's route, such as
    `chat.example.com`. The app also learns it from `cloudflared` once the tunnel connects.
  - **Cloudflare Access** (named tunnel only): tick *Require Cloudflare Access in front of the
    tunnel's hostname* to require an Access login before the app's login page. Enter the **Team
    domain** (`name.cloudflareaccess.com`) and the **Audience (AUD) tag** of the Access application.
    *Sign in with Access* also signs the browser in, so the app password is not asked. The status
    line shows the last Access error or a missing team domain or tag. See
    [Cloudflare Access](../using/cloudflare-tunnel.html#cloudflare-access-sign-in-before-the-login).
  - **cloudflared**: the path to `cloudflared`, or empty (the default) to find it on `PATH`,
    with **Browse…**.
  - **Tunnel status**: *Connecting to Cloudflare…*, *Online at* the public address with a copy
    button and the number of connections, or the error (*Trying again shortly* when it will).
    A warning appears while the password is shorter than 12 characters, which the tunnel
    needs. For a named tunnel, a note gives the service to enter in the route in the
    dashboard.

Changes take effect when you click **Apply** or **Save**. The server restarts, which
logs browsers out; a change to the tunnel settings alone restarts only the tunnel. See
[Cloudflare Tunnel](../using/cloudflare-tunnel.html).
