---
title: Data & privacy
parent: Settings
nav_order: 4
---

# Data & privacy
{: .no_toc }

Where secrets are kept, what replies may load from the web, export and import, and where the app keeps its files. See also [Data and privacy](../reference/data-and-privacy.html) for everything that leaves the computer.

1. TOC
{:toc}

## Secrets

**Keep secrets in the system keychain** (default on) keeps API keys, HTTP header
values, MCP environment variable values and the remote access password in the macOS Keychain, Windows Credential Manager
or the Secret Service (GNOME Keyring, KWallet) on Linux. `settings.json` keeps only blank
placeholders, and keys saved in plain text by earlier versions move to the keychain on first
launch.

On macOS every secret is kept in one Keychain item, so a new install or update asks for your
password once rather than once per key; choose **Always Allow** to stop the prompt until the
next update. Secrets that earlier versions kept as separate items move into it on first launch.

If no keychain is available, a warning says so and secrets stay in `settings.json`. On macOS
and Linux that file is readable only by you.

## Load web images in replies

Off by default. When off, images from the web show as links. Loading them tells the image's
server that the chat was opened, and a reply could put chat contents in the image address.

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
  listed so you can fill them in. Then click **Apply** or **Save**.
- **Import conversations…** adds chats from a conversations export and skips any that already
  exist.

Both files record the app version and export time, and are created readable only by you on
macOS and Linux.

## File locations

The bottom of the page shows where the settings file and conversations folder are. See
[Where settings are stored](index.html#where-settings-are-stored).
