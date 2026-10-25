---
title: Development
parent: Reference
nav_order: 3
---

# Development
{: .no_toc }

1. TOC
{:toc}

## Building

Requires Rust 1.95 or newer.

```sh
cargo run --release
```

## Checks

CI runs these on every push and pull request:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                  # unit tests
cargo test -- --ignored     # end-to-end tests: mock LLM + the reference MCP "everything"
                            # server over stdio and Streamable HTTP (needs Node/npx)
```

## Source layout

```
src/
  main.rs         startup: PATH fix, tokio runtime, eframe window
  app.rs          main UI: sidebar, chat view, input, event handling
  ui/             theme and icons, chat view, markdown and drawings, settings page, palette
  agent.rs        generation loop: stream, run tool calls, repeat
  approval.rs     tool-call approvals and the per-chat "Always allow" list
  tools/          built-in tools (time, calculator, task list, fetch, workspace), per-chat switches
  subagent.rs     the delegate tool: parallel subagents with their own model/tool loop
  llm.rs          OpenAI-compatible /chat/completions and /models client
  sse.rs          incremental Server-Sent Events parser
  mcp/            MCP client: stdio and Streamable HTTP transports, OAuth, connection manager
  config.rs       settings model, persistence, mcpServers JSON import
  export.rs       settings / conversations export and import
  secrets.rs      OS keychain storage for API keys and tokens
  attach.rs       image, document and text file attachments
  documents.rs    text extraction for PDF, Office, OpenDocument and RTF files
  stats.rs        prefill / decode speed and token statistics
  update.rs       GitHub Releases update check and in-place install
  remote/         remote access: web server, login sessions, TLS, and the browser client (web/)
  types.rs        messages and conversation storage
  env_path.rs     login-shell PATH import for GUI launches
  testutil.rs     tiny scripted HTTP server for tests
```

The UI is built with [egui](https://github.com/emilk/egui) and runs on the main thread. Network
and MCP I/O run on a tokio runtime and send events back over a channel.

## Releasing

1. Bump `version` in `Cargo.toml` (and `Cargo.lock`, via `cargo build`) in a pull request.
2. Run **Actions → Release → Run workflow** with `version: vX.Y.Z`, or push the tag:

   ```sh
   git tag vX.Y.Z && git push origin vX.Y.Z
   ```

The tag must equal `v` + the `Cargo.toml` version. The workflow builds the macOS universal
`.dmg` and `.zip`, the Windows `.zip` and `.msi`, and both Linux tarballs, then creates the
GitHub Release with `SHA256SUMS.txt`. Tags containing `-` (such as `v1.0.0-dev.1`) are marked
as pre-releases, which the in-app update check skips; their `.msi` gets the plain numeric
version (`1.0.0`), since Windows Installer versions can't carry a suffix.

The browser client in `src/remote/web/` is plain JavaScript embedded in the binary. Its
markdown renderer has tests of its own: `node src/remote/web/markdown.test.js`.

`scripts/release-local.sh [--publish] [--skip-tests]` does the same from a local machine; see
the [README](https://github.com/d3v0ps-cloud/CleanRSTAI#releasing-without-github-actions) for
what each target needs, and for code signing.

## Writing these docs

This site is built by GitHub Pages from the `docs/` folder with the
[Just the Docs](https://just-the-docs.com) theme. Add a page by adding a Markdown file with
`title` front matter, plus `parent` (matching the parent page's title exactly) to nest it.

To preview locally (needs Ruby):

```sh
cd docs
bundle install
bundle exec jekyll serve
```

Then open `http://localhost:4000`.
