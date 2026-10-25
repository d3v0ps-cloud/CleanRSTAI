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
- **HTTPS.** The default is a self-signed certificate. The app makes it once and keeps it in a
  `remote` folder in its configuration folder. Use HTTPS unless the network is one you trust
  completely. With **Off (plain HTTP)** the password and chats cross the network unencrypted.
- **Sessions.** A login lasts 30 days, or 7 days without use. Sessions are kept in memory only.
  Restarting the server logs every browser out. Changing any remote access setting, or quitting
  the app, restarts the server.
- **Lockout.** After 5 wrong passwords within 10 minutes, the address that sent them is refused
  for 60 seconds. Each wrong password also takes about half a second to answer.
- **What never leaves the computer.** API keys, MCP secrets and settings are not sent to the
  browser. Model requests, tools and approvals stay on this computer.
- **Network.** Use remote access on a network you trust. Do not forward the port from your
  router to the internet.

### Reaching it from outside your network

Remote access is meant for your own network. A tunnel integration to reach CleanRSTAI from
outside is planned (see the [roadmap](https://github.com/d3v0ps-cloud/CleanRSTAI/blob/main/roadmap.md)).
Until it is available, do not open the port to the internet.

## Limits

- **The desktop app must stay open.** If you quit it, the server stops and browsers cannot
  connect.
- **No attachments from the browser** in this version.
- **One password for everyone.** Anyone who knows it can log in and use every chat.
