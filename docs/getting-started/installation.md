---
title: Installation
parent: Getting Started
nav_order: 1
---

# Installation
{: .no_toc }

1. TOC
{:toc}

## Download a release

Every release on the [releases page](https://github.com/d3v0ps-cloud/CleanRSTAI/releases/latest)
has these files:

| File | Platform |
|---|---|
| `CleanRSTAI-macos-universal.dmg` | macOS disk image, Apple Silicon and Intel |
| `CleanRSTAI-macos-universal.zip` | the same `.app`, zipped |
| `CleanRSTAI-linux-x86_64.tar.gz` | Linux x86_64 |
| `CleanRSTAI-linux-aarch64.tar.gz` | Linux ARM64 |
| `CleanRSTAI-windows-x86_64.msi` | Windows x86_64 installer |
| `CleanRSTAI-windows-x86_64.zip` | Windows x86_64 portable `.exe` |
| `SHA256SUMS.txt` | checksums of every file |

To check a download against the checksums:

```sh
shasum -a 256 -c SHA256SUMS.txt --ignore-missing   # macOS
sha256sum -c SHA256SUMS.txt --ignore-missing       # Linux
```

## macOS

1. Open the `.dmg` (or unzip the `.zip`).
2. Drag **CleanRSTAI.app** to **Applications**.
3. Open it from Applications or Launchpad.

{: .note }
> If a release isn't signed and notarized, macOS blocks it on first launch. Right-click the
> app and choose **Open**, or run:
>
> ```sh
> xattr -dr com.apple.quarantine /Applications/CleanRSTAI.app
> ```

GUI apps on macOS start with a minimal `PATH`. CleanRSTAI imports the `PATH` from your login
shell at startup, so MCP servers started with `npx`, `uvx`, `docker` or Homebrew tools are
found the same way your terminal finds them.

## Linux

The tarball holds a single `cleanrstai` binary. Put it somewhere on your `PATH` that your
user can write to, such as `~/.local/bin`, so the [in-app updater](../using/updates.html) can
replace it later:

```sh
tar -xzf CleanRSTAI-linux-x86_64.tar.gz
mkdir -p ~/.local/bin
mv cleanrstai ~/.local/bin/
cleanrstai
```

Use `CleanRSTAI-linux-aarch64.tar.gz` on ARM64. The binary is built against glibc 2.28, so it
runs on any distribution with glibc 2.28 or newer.

At runtime you need a desktop with X11 or Wayland (`libxkbcommon-x11` and a GL or Vulkan
driver, which standard desktops already have). The keychain uses the Secret Service (GNOME
Keyring or KWallet), and save dialogs use the desktop portal (`xdg-desktop-portal`).

## Windows

- **Installer**: run `CleanRSTAI-windows-x86_64.msi`. It installs to Program Files and adds
  a Start menu shortcut; a newer installer upgrades it in place.
- **Portable**: unzip `CleanRSTAI-windows-x86_64.zip` and run `cleanrstai.exe` from
  anywhere.

{: .note }
> Unless a release is signed, Windows SmartScreen warns on first launch. Choose
> **More info → Run anyway**.

Both update themselves from the app: the portable `.exe` in place (keep it in a folder you
can write to), and the installed one by running the new installer. See
[Updates](../using/updates.html). `npx` resolves to `npx.cmd` automatically for MCP servers.

To build it yourself, see below: `cargo build --release` produces
`target/release/cleanrstai.exe`, which doesn't open a console window.

## Build from source

Requires Rust 1.95 or newer (`rustup update`) and a C compiler.

```sh
git clone https://github.com/d3v0ps-cloud/CleanRSTAI.git
cd CleanRSTAI
cargo run --release
```

### macOS app bundle

```sh
cargo install cargo-bundle
cargo bundle --release
open target/release/bundle/osx/CleanRSTAI.app
```

## Next step

[Set up a provider and send your first message](first-chat.html).
