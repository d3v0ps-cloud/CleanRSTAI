# Review: CleanRSTAI

_Static review of the whole codebase (all 35 source modules, workflows, docs, vendored
crate notes). `cargo test`/clippy were not run; the code is consistent with the CI gates
described in CLAUDE.md._

## Overall

This is an unusually high-quality codebase for a project at v0.7. ~15k lines of Rust, a
single well-factored binary, and almost every module carries real unit/integration tests
(mocked OpenAI server, real temp folders, LibreOffice-made document fixtures, a full OAuth
flow driven in-process against a fake auth server). The architecture is clean and the docs
(README, CLAUDE.md, roadmap) are accurate and unusually complete. No broken core logic was
found; the issues below are mostly edges, one real (if small) UX bug, a couple of security
considerations, and a performance wart.

## What stands out

- **Threading model** (`main.rs`, `app.rs`, `agent.rs`): UI never awaits; background work
  posts `AppEvent`s over an unbounded channel + `request_repaint()`. The per-chat `Run`
  (task handle + pending approvals + live subagent reports) and the "late events ignored
  once the run is gone" guard in `handle_chat_event` make Stop/cancel race-free.
- **History repair** (`llm::to_api_messages`): unanswered tool calls (user hit Stop
  mid-call) get "Tool call was cancelled." placeholders so the API never rejects the next
  request. Cancelled prompts are kept for editing but filtered out of history. This is
  exactly the kind of edge most chat clients get wrong.
- **Workspace sandboxing** (`tools/workspace.rs`): `..` rejected lexically, absolute paths
  allowed but canonicalized and checked against the root, symlink-out refused, `writable()`
  walks to the nearest existing ancestor — and it's all covered by tests including the
  symlink cases.
- **Secrets** (`secrets.rs`, `config.rs`): keychain externalize/internalize with
  plain-text fallback *without blanking* on failure, stale-entry cleanup, one-time
  migration, `write_private` (0600, atomic rename), `.bak` on a corrupt settings file. The
  `MemoryStore` tests exercise all of it.
- **OAuth** (`mcp/oauth.rs`): spec-correct (RFC 9728 → 8414 → 7591 → PKCE S256 → 8707),
  random high-entropy state/verifier, loopback listener with state check and a
  human-readable result page, refresh with "someone else already refreshed" detection. The
  in-process test even validates the verifier against the captured challenge.
- **SSE parser** (`sse.rs`): handles chunk splitting mid-UTF-8, CRLF, comments, multi-line
  data, unterminated trailing event — with tests for each.
- **`ToolCallBuilder`** tolerates missing `index`, streamed name/argument fragments, and
  non-standard already-parsed argument objects.
- Defensive details throughout: zip-entry size caps, 5 MB fetch cap with truncation note,
  `catch_unwind` around the young mermaid renderer, 3-second deadline around the
  login-shell `PATH` probe, `shutdown_timeout` on the tokio runtime, Windows
  process-tree kill for `npx`.

## Issues

### Bugs / functional

1. **`src/app.rs` — `insert_prompt` drops the prompt's last message if a run is active.**
   The final user message is `pop`ped *before* the "wait for the current response to
   finish" guard; when the guard fires, the `return` discards it. So a multi-message MCP
   prompt clicked mid-generation silently loses its final text (a single-message prompt
   still works, which makes the behavior inconsistent). Move the pop after the guard, or
   re-append on failure.

2. **`src/testutil.rs:118` — mock server writes `HTTP/1.1 {status} X`** as the status line
   (`HTTP/1.1 200 X`). It happens to parse fine in reqwest, but it's an obvious leftover;
   make it `OK`.

3. **`src/tools/workspace.rs` — `shell()` timeout only kills the direct child on Unix.**
   `kill_on_drop` kills `sh`, but grandchildren survive: `sleep 100 | cat` keeps `sleep`
   running after "the command didn't finish within N s and was stopped". Run the command in
   its own process group (`setsid`/`pre_exec` → `setpgid`) and `kill(-pgid)` on timeout.
   (The stdio MCP side at least has the Windows tree-kill; Unix shell has no equivalent.)

4. **`src/mcp/http.rs` — session expiry is manual and invisible.** A 404-with-session
   returns "MCP session expired (HTTP 404); reconnect the server", but the server stays
   `Connected` in the UI and every subsequent tool call fails with the same message until
   the user finds the Reconnect button in Settings. Either auto-reconnect once (the full
   connect path already exists) or flip the status to `Error`.

5. **`src/mcp/http.rs` — 401 on the GET server-push stream is treated as "not supported"
   and the listener exits permanently.** If the stream 401s while a (stale-then-
   refreshable) token is in play, `list_changed` pushes are lost until a full reconnect.
   Minor, but a `Listen::AuthFailed` variant with retry/backoff like other errors would be
   more robust.

### Security (consider, not necessarily fix)

6. **`fetch` tool SSRF surface** (`tools/fetch.rs`): any http/https URL is allowed —
   `http://169.254.169.254/latest/meta-data/` (cloud instance credentials),
   `http://localhost:<port>` (local services), RFC1918 hosts — and reqwest follows
   redirects by default. Approval-gating makes this acceptable by design, but with
   *auto-approve* on (a user setting) a prompt-injected page or a careless model can read
   metadata endpoints. Consider refusing/warning on link-local + loopback + private
   ranges, or at least disabling redirect-following after a cross-host redirect.

7. **Subagents run MCP tools without approval** (documented in `config.rs` and the
   settings copy). It's a deliberate trade-off, but it means the per-tool approval policy
   the user set for the chat doesn't apply inside delegation. Fine as-is; I'd just make
   sure the "Subagents may use the chat's MCP tools" checkbox copy carries the "without
   asking" consequence prominently.

8. **`documents.rs` zip-bomb cap is a truncation, not a failure**: `f.take(MAX_ENTRY_BYTES)`
   happily allocates 200 MB of `String` for a 2 GB XML entry. Failing when the read reaches
   the cap (or checking the entry's declared size first) would be cheaper and more honest.

### Performance

9. **`src/app.rs` — `self.mcp.snapshot()` is called up to 3× per frame.** `top_bar`,
   `tools_menu` and `mcp_menu` each build a full snapshot (cloning every server's tools,
   resources, prompts and 200-line stderr tail) on every repaint, and repaints happen at
   high frequency while streaming. `connected_tool_count` already shows how to read the
   entries without cloning; the composer menus could use the same (counts + a cached
   snapshot refreshed on `on_change`/frame-dirty). This is the one real CPU cost worth
   fixing before the tool lists get large.

10. **`ui/attachments.rs` `ImageCache` is unbounded** (decoded image bytes keyed by URI,
    never evicted). `DrawingCache` caps at 256 and clears; images don't. A long session
    with many 20 MB attachments will accumulate them. A similar cap (LRU or
    clear-on-overflow) would keep memory bounded.

### Nits

- `src/llm.rs` — `llama_cpp_context_size` issues a `GET /props` on **every generation**
  for llama.cpp providers; cache it per provider until settings change.
- `src/export.rs` — `conversation_markdown` matches tool results by a *global* call-id
  map, while the UI deliberately matches "tool messages directly following the assistant
  message" (see `results_after` in `chat_view.rs` and the CLAUDE.md note about servers
  reusing ids like `call_0`). With id-reusing servers the Markdown export can pair a call
  with the wrong result.
- The GitHub repo `d3v0ps-cloud/CleanRSTAI` is hard-coded in two unrelated places
  (`update.rs`, `fetch.rs` user-agent) — a single constant would do.
- `state/` at the repo root is an empty directory (git can't track it — probably local
  cruft; delete or ignore it).
- `Conversation::load_all` parses every conversation (up to 1 M chars of attachment text
  each) on the UI thread at startup; fine at desktop scale, worth knowing about.
- `agent.rs` e2e `mock_llm()` hand-rolls an HTTP request reader that
  `testutil::serve` already provides — the two implementations have drifted (hence the `X`
  status line above). One shared helper would be less code to maintain.

## Suggested priority order

1. Fix `insert_prompt` dropping the final message (#1) — the only real user-facing bug.
2. Add the process-group kill for workspace shell timeouts (#3) — it's a correctness gap
   in a tool that runs arbitrary commands.
3. Cache/cheapen the per-frame MCP snapshot (#9).
4. Decide the fetch-tool private-address policy (#6) before auto-approve users hit it in
   anger.
5. The rest are polish.

Overall: this is well above average in testing, documentation, and edge-case handling — the
vendor patch is even catalogued in `VENDORED.md`. The project is in good shape to keep
shipping; the findings above are the kind of thing that would show up only after real-world
use, so addressing 1–4 now is cheap insurance.
