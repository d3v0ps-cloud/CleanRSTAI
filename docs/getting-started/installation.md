---
title: Installation
parent: Getting Started
nav_order: 1
---

# Installation

## From a release

Download the file for your platform from the
[latest release](https://github.com/d3v0ps-cloud/CleanRSTAI/releases/latest):

- **macOS**: `CleanRSTAI-macos-universal.zip` (or the `.dmg`), then move `CleanRSTAI.app`
  to Applications.
- **Linux**: `CleanRSTAI-linux-<arch>.tar.gz`, which holds the `cleanrstai` binary.

`SHA256SUMS.txt` lists the checksums of every file.

## From source

Requires Rust 1.95 or newer (`rustup update`).

```sh
cargo run --release
```

### macOS app bundle

```sh
cargo install cargo-bundle
cargo bundle --release
open target/release/bundle/osx/CleanRSTAI.app
```
