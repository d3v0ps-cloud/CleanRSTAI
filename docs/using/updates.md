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

When a new version is out, the top right of the window shows **Update to vX.Y.Z**:

- **macOS, Linux and portable Windows**: click it to download the release, check it against
  the release's `SHA256SUMS.txt` and replace the installed app. The app keeps working while
  it downloads; then click **Restart to finish updating**.
- **Windows, installed with the `.msi`**: click it to download and check the new installer,
  then **Install vX.Y.Z** closes CleanRSTAI and runs it. Windows asks for permission, and the
  installer replaces the installed version; open CleanRSTAI again when it finishes.
- **When the app can't replace itself**: the link reads **Download vX.Y.Z** and opens the
  release page.

On macOS the app must run from its `.app` bundle to update itself. On Linux, and for the
portable Windows `cleanrstai.exe`, the program is replaced where it is, so keep it in a
folder your user can write to, such as `~/.local/bin`; otherwise the link opens the release
page instead.

## Update source

**Update from** in [General](../settings/general.html#updates) sets where releases come from:

- a GitHub `owner/repo`,
- a `github.com` link, or
- any URL that answers like GitHub's `releases/latest` API (Gitea and Forgejo do).

It defaults to this project's repository and is saved to `settings.json` only once changed, so
a custom source carries over to later installs. **Reset** goes back to the default.

The release must be public: GitHub answers a private repository's release check with a 404.

## Pre-releases

Test builds are published as pre-releases with versions like `v0.8.0-dev.1`. The update check
never offers them: download one from the
[releases page](https://github.com/d3v0ps-cloud/CleanRSTAI/releases) to try it. A pre-release
counts as older than the release it leads up to, so a test build offers that release once it
is out.
