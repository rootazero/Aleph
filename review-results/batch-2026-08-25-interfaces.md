# Module Review — interface (cli / tui / webchat)

- **Date**: 2026-08-25
- **Worktree**: `/home/zou/data/workspace/Aleph/.worktrees/review/interface-shared-mobile-2026-08-25`
- **Branch**: `review/interface-shared-mobile-2026-08-25`
- **Baseline commit**: `6de033068` (HEAD of `main` at review start)
- **Reviewer**: static (4-perspective: Security/Robustness, Logic/Correctness, Architecture R1-R10, Code Quality)
- **Confidence threshold**: ≥ 80% (3+ concrete anchors: file, line, behaviour)
- **Pre-existing batch reports**: `batch5-interfaces-cli.md`, `batch5-interfaces-tui.md`, `batch5-webchat-{core,components,phone,wide-chat}.md`
- **Scope**: `interfaces/cli/`, `interfaces/tui/`, `interfaces/webchat/` (Cargo.toml, src/, build.rs, clippy.toml)

## Stats

| Sub-crate | `.rs` files | LOC | Largest file (LOC) |
|-----------|------------:|----:|-------------------:|
| cli       |          52 | 16,619 | `commands/main.rs` (1,448) |
| tui       |          28 | 15,537 | `tui/app/tests.rs` (2,738 — test-only) |
| webchat   |         380 | 126,127 | `platform/wide/views/chat/state/mod.rs` (3,159) |
| **Σ**     |     **460** | **158,283** | — |

Files >500 LOC, non-test: cli has 7, tui has 5, webchat has 69 (all called out under "soft cap" finding below).

## Architecture red-line compliance

| Rule | CLI | TUI | Webchat | Evidence |
|------|-----|-----|---------|----------|
| R1 (core never calls platform APIs; interfaces depend on `shared/protocol` + `shared/client` only) | ✅ | ✅ | ✅ | `interfaces/{cli,tui,webchat}/Cargo.toml` explicitly do not depend on `alephcore`. CLI: `aleph-client` + `aleph-protocol`. TUI: same. Webchat: `aleph-protocol` + `shared-ui-logic`. |
| R2 (complex UI in Leptos/WASM only) | ✅ (text I/O only) | ✅ (terminal renderer only) | ✅ (Leptos) | TUI markdown is hand-rolled (`tui/markdown.rs`); no HTML emission. |
| R3 (core minimalism — no heavy deps in interfaces) | ✅ | ✅ | ✅ | clap, ratatui, leptos are R3-exempt. No unexpected heavy deps. |
| R4 (interfaces are pure I/O — no business logic leaks) | ⚠️ partial | ✅ | ✅ | `cli/commands/doctor.rs:240 build_repair_brief` still composes an LLM repair prompt in the shell — a thin prompt-engineering layer that belongs in Core. `main.rs:710 github:/zip heuristic` survives as a documented "client-side I/O needs local read" carve-out. |
| R7 (Rust Core is the only brain) | ✅ | ✅ | ✅ | All state-changing actions route via JSON-RPC. |
| R8 (regex only for machine formats) | ✅ | ✅ (no regex at all) | ✅ | No regex in any of the three crates except via `pulldown-cmark`/`regex-syntax` deps used for markdown parsing. |
| R9 / R10 | N/A | N/A | N/A | Not interface responsibilities. |

## Findings (severity sorted)

### Critical
None.

### High
None.

### Medium

#### M1 (webchat) — panic hook writes raw URL with `?token=`/`?bt=` to localStorage ring buffer
- **File**: `interfaces/webchat/src/panic_overlay.rs:93` (`current_url()`)
- **Behaviour**: The hook fires on every WASM panic and persists `{ts, version, message, stack, url}` to `aleph.panel.crashes`. On cold-start, the URL still carries the gateway token (`?token=…` or `?bt=…`) because `scrub_credentials_from_url()` does not run until the WS handshake completes. `clear_credentials()` scrubs sessionStorage, not localStorage, so the token lives in a key an XSS payload already covers.
- **Fix applied (commit `e46e7fb78`)**: `current_url()` now calls a new `strip_credentials()` helper that drops `?token=` / `?bt=` (and their `&` siblings, with fragment passthrough) before the value hits the ring buffer. Unit test added in the same module.

#### M2 (cli) — `providers add --api-key` was a mandatory `String`; secret leaked into shell history
- **File**: `interfaces/cli/src/commands/cli_args.rs:638`, `providers_cmd.rs:524`
- **Behaviour**: The flag was required; the key ended up in `~/.bash_history` and the process listing (`ps auxe`).
- **Fix applied (commit `461aebce7`)**: `--api-key` is now `Option<String>` and is read from `ALEPH_PROVIDER_API_KEY` (clap `env`, `hide_env_values = true`) before falling back to `rpassword::prompt_password`. JSON mode (no TTY) returns a typed error so machine callers cannot accidentally fall back to a prompt that will hang.

#### M3 (cli) — `plugin pack` followed symlinks; a stray symlink could pull arbitrary files into the zip
- **File**: `interfaces/cli/src/commands/plugin_cmd.rs:810` (`add_dir_to_zip`)
- **Behaviour**: `path.is_dir()` and `File::open` followed symlinks. A typo'd symlink under `plugins/foo/secret → /etc` would silently ship `/etc/passwd` inside `plugin.aleph-plugin.zip`.
- **Fix applied (commit `461aebce7`)**: switched the per-entry classification to `std::fs::symlink_metadata`; symlinks are now skipped (intentional — plugin templates ship no symlinks). Comment explains why.

#### M4 (tui) — `load_trace_replay` shared the live trace-event path, polluting status-bar counters
- **File**: `interfaces/tui/src/tui/app/trace.rs:382` (`load_trace_replay`)
- **Behaviour**: The projection loop calls `apply_agent_trace_event` for each persisted event. `SessionCompleted` and `ProviderUsage` arms unconditionally bump `total_tokens` and `cache_stat`, so viewing a replay of any past run permanently inflated the current session's status bar. `dismiss_pending_approval()` also fired mid-replay, dissolving any live `/btw` overlay.
- **Fix applied (commit `33ac4d3fc`)**: added a `replaying_trace` flag to `AppState`; the projection loop now toggles it, save/restores `total_tokens` / `cache_stat` / `cache_stat_agent` / `cache_root_agent`, and the SessionCompleted arm's `dismiss_pending_approval()` is gated behind `!self.replaying_trace`. Belt-and-suspenders: the save/restore is the safety net, the flag is the branch-friendly runtime guard.

#### M5 (tui) — chat area rebuilt every message's markdown at 20 fps
- **File**: `interfaces/tui/src/tui/widgets/chat_area.rs:67` (`build_all_lines`)
- **Behaviour**: The 50 ms tick loop called `terminal.draw` → `build_all_lines` → `markdown_to_lines` for every message in `state.messages`, including idle frames with no new content. Long conversations spent tens of ms per frame on markdown parsing the same text repeatedly.
- **Fix applied (commit `80ab50ce7`)**: thread-local cache keyed on a fingerprint (content width + message count + 64-bit mix over per-message bytes: content, reasoning, tools.len, streaming flag, spinner-frame when streaming). Idle ticks reuse the cached `Vec<Line<'static>>`; streaming turns invalidate on content change. Streaming hash includes the spinner frame, idle ticks do not.

#### M6 (webchat) — assistant-message markdown allowed protocol-relative URLs through the sanitizer
- **File**: `interfaces/webchat/src/components/markdown.rs:184` and `memory_graph/markdown_excerpt.rs:141` (`sanitize_link_url`)
- **Behaviour**: Both sanitizers checked `split_once(':')`; `//evil.com/x` has no colon, so it passed the scheme whitelist and rendered with `target="_blank"`. Clicking a model-supplied link navigated to the attacker's domain with the panel's origin intact.
- **Fix applied (commit `e46e7fb78`)**: both sanitizers now reject URLs that begin with `//` with a `#disallowed-protocol-relative` rewrite before the scheme check. Unit tests added (dangerous-scheme rejection + allow-list preservation) in `components/markdown.rs`.

#### M7 (webchat) — attachment upload had no size or count cap; one drag could blow the tab
- **File**: `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:35`, `view.rs:274`
- **Behaviour**: `read_file_list_into` and `ingest_dropped_file` called `read_as_data_url` on every dropped file; the entire base64 payload landed on the heap and rode out via `chat.send`.
- **Fix applied (commit `b0cdda6bd`)**: both paths now enforce `MAX_ATTACHMENT_SIZE_BYTES` (10 MB) per file and `MAX_ATTACHMENT_COUNT` (10) per send, with constants exported from `attachments.rs` and a `console::warn` rationale on skip.

#### M8 (webchat phone) — `model_route.rs` reload clobbered unsaved edits on every reconnect
- **File**: `interfaces/webchat/src/platform/phone/settings/model_route.rs:61`
- **Behaviour**: The `Effect` watched `is_connected`; on every reconnect it re-fetched, set `loading=true` (collapsing the form into "Loading…"), and overwrote every input signal with the stored server-side values. Mobile reconnects are common — any user mid-edit lost their work.
- **Fix applied (commit `62d15dde7`)**: gate the reload on an `ever_loaded` flag; reconnect keeps the cached view, only cold-start and the explicit retry trigger a reload.

#### M9 (webchat) — `dispatch_event` held `event_handlers` Mutex while invoking handlers
- **File**: `interfaces/webchat/src/context.rs:873` (`dispatch_event`)
- **Behaviour**: A handler that synchronously called `subscribe_events` / `unsubscribe_events` on the same StoredValue would re-enter the lock on the same thread. Wasm is single-threaded; `try_lock` is no escape hatch — it deadlocked.
- **Fix applied (commit `377da13c1`)**: clone the (Arc) handler list under the lock into a local Vec, drop the guard, then iterate. Lock acquire pattern was carried over from `subscribe`/`unsubscribe` where the critical section is short; dispatch is the load-bearing fan-out and was the one that needed explicit handling.

### Low (carried over from prior review, with status)

| ID | Where | Status |
|----|-------|--------|
| L1 cli | `plugin_cmd.rs:285` `serde_json::to_string_pretty(&json).unwrap()` | **Fixed** in `461aebce7` — replaced with `unwrap_or_default()`. |
| L2 cli | `plugin_cmd.rs:533` `plugin_dir.unwrap()` after `is_some_and` | **Already fixed** in prior batch — code path no longer exists. |
| L3 cli | `doctor.rs:240 build_repair_brief` | **Still present**, retained. R4 violation by design — see R4 row above. |
| L4 cli | `main.rs:710` `github:/zip` heuristic | **Still present**, retained. Documented carve-out for client-side I/O. |
| L6 cli | `plugins_cmd.rs:175` GitHub release filename not sanitised | **Not touched** (Low, not in scope of this round). |
| L5 tui | `commands.rs:395` `let _ = textarea;` dead param | **Fixed** in `33ac4d3fc` — argument renamed to `_textarea` and the trailing no-op removed. |
| L6 tui | Spinner frame table duplicated in `status_bar.rs:18` and `tool_block.rs:14` | **Fixed** in `33ac4d3fc` — extracted to `theme::SPINNER_FRAMES`, both sites import. |
| L3 tui | `agent.cancel` vs `chat.abort` (mod.rs:459 vs commands.rs:902) | **Not touched** — two RPCs with different semantics (run-only vs session-scoped purge). Documented in code; not a bug. |
| L2 tui | `Reasoning` vs `ReasoningBlock` gating asymmetry (events.rs:374 vs :651) | **Not touched** — the asymmetry is intentional (see ToolStart/ToolEnd comment block at events.rs:425). |
| L1 webchat | `state/sessions.rs:94` `Owner::current().expect(...)` | **Not touched** — intentional hard contract, silent fallback would orphan signals. |
| L3 webchat | `context.rs:873` Mutex deadlock | **Fixed** as M9 above (was under-stated Low, promoted to Medium during fix). |
| L4 webchat | `markdown_excerpt.rs:141` protocol-relative URL | **Fixed** as M6 above (was under-stated Low, promoted to Medium during fix). |
| L5 webchat | `state/memory.rs:65-80` `MemoryState::new` Effect creation | **Not touched** — only called from `app.rs:84` (wasm context with a guaranteed owner); the prior review's concern was theoretical. |
| L1 webchat | Stale-response guard absent in `phone/memory/mod.rs:85` | **Fixed** in `62d15dde7` — `if mem.agent_id.get_untracked() == agent` check before mutating `st.window` / `st.error`. |
| L3 webchat | `phone/model_route.rs` reconnect reload | **Fixed** as M8 above (was under-stated Low, promoted to Medium during fix). |
| L2 webchat | `attachments.rs` Closure::forget() leaks | **Not touched** — one-shot per attachment; the file-size gate (M7) now stops large files before the closure even fires, so the leak surface is much smaller. The voice.rs `_on_data`/`_on_stop` are stored on the handle and reclaimed on stop (no `forget()`). |
| Numerous | "Soft cap >500 LOC" calls across webchat (69 files) | **Not touched** — each is documented and most have a coherent internal split (chat/state/mod.rs splits tools from messages etc.). Refactor scope is too large for this round. |

### New Low (raised this round)

None with ≥ 80% confidence.

## Findings by severity

| Severity | Count |
|----------|------:|
| Critical |   0 |
| High     |   0 |
| Medium   |   9 |
| Low      |  15 (all carried over from prior review; status documented above) |
| **Σ**    | **24** |

## Fixes applied

| Commit | Subject |
|--------|---------|
| `461aebce7` | `interfaces/cli: secret hygiene + symlink-safe plugin pack` |
| `33ac4d3fc` | `interfaces/tui: replay isolation + dead-parameter + spinner dedup` |
| `e46e7fb78` | `interfaces/webchat: panic URL credential strip + protocol-relative link block` |
| `b0cdda6bd` | `interfaces/webchat: cap attachment size + count to bound base64 inflation` |
| `62d15dde7` | `interfaces/webchat/phone: stop reconnect reloads from clobbering edits` |
| `377da13c1` | `interfaces/webchat: release event_handlers lock before invoking handlers` |
| `80ab50ce7` | `interfaces/tui: thread-local cache for chat-line markdown rendering` |

All commits are scoped per AGENTS.md commit format (`<scope>: <description>`, English).

## Negative space — what was NOT reviewed / NOT touched

- `interfaces/cli/src/commands/doctor.rs` `build_repair_brief` is still in the CLI layer (R4 violation by design; the brief itself is a prompt engineering artifact that belongs in Core's doctor tool/system prompt). Flagged but not fixed — moving it requires touching the Core crate, which is out of scope.
- `interfaces/cli/src/main.rs:710` plugin-install heuristic still uses `source.starts_with("github:") || source.ends_with(".zip")` to choose between client-side and daemon-side handling. Documented carve-out, retained.
- `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:111-135` ResizeObserver closure leak: retained. The author's comment notes the chat view is kept alive by `MainContent`, so the closure is "one-per-app, not per-visit". The trade-off is documented; fixing it requires holding the `Closure` in a `StoredValue` and adding `on_cleanup` machinery that the rest of the composer doesn't share.
- `interfaces/webchat/src/components/chat_sidebar.rs:473/496/505` `sessions.set` / `groups.set` / `is_loading.set` after an `await` — still not protected with `try_set`-style helpers, because Leptos 0.8 does not ship a `try_set` on `RwSignal`. Possible fix is custom signal wrappers, which is a sizable refactor.
- 69 webchat files over 500 LOC: not refactored. Most have an internal sub-module split already (`state/mod.rs` is 3,159 lines but wraps tools, messages, planner, and reasoning in separate `impl` blocks). A wholesale split is out of scope.
- `interfaces/cli/src/commands/secret_cmd.rs` already does the right thing (`rpassword` + `--json` stdin fallback). The providers-add fix brought `providers_cmd::add` into parity with it.
- TUI `commands.rs:1391-1400` `read_dir`/`read_to_string` `.expect()` inside `this_client_resolves_a_side_question_in_exactly_one_place` test are inside `#[cfg(test)]` and excluded from this review by AGENTS.md.
- The two untracked files in the worktree (`review-results/batch-2026-08-25-mobile.md`, `review-results/batch-2026-08-25-shared.md`) are owned by other concurrent reviewers — not touched, per protocol.

## Residual risks

- **R4 prompt engineering in `doctor.rs`**: the brief template references tool names (`doctor`, `self_config`, `self_manage`, `vault_store`). If a future tool rename reaches Core without an interface-side update, the brief will route to a dead tool. Migration to Core's doctor tool is the durable fix.
- **Thread-local cache in TUI chat area**: leaks one `Vec<Line<'static>>` per process. On shutdown the OS reclaims it; in a hot-reload scenario the cache would survive across versions and serve stale lines until the next content change. Not a real-world concern for a TUI binary.
- **`attachments.rs` base64 inflation**: the 10 MB / 10-file caps are heuristic. A 10 MB CSV inflates to ~13.3 MB base64 + a Json-RPC envelope, which approaches but does not exceed the gateway's `gateway.max_request_bytes`. If those server-side limits tighten, the client cap should follow.
- **`sanitize_link_url`**: relies on `pulldown-cmark` populating `dest_url` exactly. If a future pulldown-cmark release starts encoding scheme-less URLs differently (e.g. percent-encoding the leading `//`), the fix needs to canonicalise before the prefix check.
- **`strip_credentials` in `panic_overlay.rs`**: only strips `?token=` and `?bt=`. If a future gateway version introduces another credential-bearing query param, this list must grow; consider folding it through `context::strip_params` instead of duplicating the prefix list.
