---
title: Updates
parent: Using the App
nav_order: 5
---

# Updates

CleanRSTAI checks GitHub for a newer release at startup (you can turn this off) and when you
click **Check for updates now** in [Settings → General](../settings/general.html#updates) or
choose **Check for updates** in the command palette.

## Installing an update

When a new version is out, the top bar shows **Update to vX.Y.Z**:

- **macOS and Linux**: click it to download the release, check it against the release's
  `SHA256SUMS.txt` and replace the installed app. The app keeps working while it downloads;
  then click **Restart to finish updating**.
- **Elsewhere**: the link reads **Download vX.Y.Z** and opens the release page.

On macOS the app must run from its `.app` bundle to update itself. On Linux the `cleanrstai`
binary is replaced where it is, so keep it in a folder your user can write to, such as
`~/.local/bin`.

## Update source

**Update from** in [General](../settings/general.html#updates) sets where releases come from:

- a GitHub `owner/repo`,
- a `github.com` link, or
- any URL that answers like GitHub's `releases/latest` API (Gitea and Forgejo do).

It defaults to this project's repository and is saved to `settings.json` only once changed, so
a custom source carries over to later installs. **Reset** goes back to the default.

The release must be public: GitHub answers a private repository's release check with a 404.
