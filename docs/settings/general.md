---
title: General
parent: Settings
nav_order: 1
---

# General
{: .no_toc }

The system prompt, your name, the send key and updates. Theme, size, motion and how replies look are on the [Appearance](appearance.html) page.

1. TOC
{:toc}

## System prompt

The instructions sent at the start of every chat. Default: *You are a helpful assistant.*

## Your name

The name in the greeting on a new chat and in the sidebar's footer. Empty (default) uses your
account's user name, from its first part (`aaron.b` greets *Aaron*).

## Sending

- **Enter sends, Shift+Enter for newline** (default)
- **Ctrl/⌘+Enter sends, Enter for newline**

**⌘/Ctrl + Enter** sends in both modes.

## Updates

- **Check GitHub for a new release at startup** (default on).
- **Update from**: where releases come from: an `owner/repo`, a `github.com` link or a URL
  that answers like GitHub's `releases/latest` API. **Reset** restores the default.
- **Check for updates now** checks the source as typed, before you save, and shows the
  installed version. If a newer release can be installed, **Install update** appears.
- **Install a version**: **List releases** fetches every release from the same source (the
  newest 50, dev builds included, marked **dev**; the installed one is marked **installed**).
  Pick one and click **Install vX.Y.Z** to move to that build, newer or older. It installs the
  same way as an update, checked against the release's `SHA256SUMS.txt`. **Release notes**
  opens the release's page. Install is greyed out for the installed version, and for a
  release without a build this copy can install itself.

See [Updates](../using/updates.html).
