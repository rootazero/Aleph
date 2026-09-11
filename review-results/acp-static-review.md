# Static Review: `src/acp/`

**Reviewer**: rust-doctor static-review pass (general-purpose agent)
**Worktree**: `/home/zou/data/workspace/Aleph/.worktrees/review-acp` on `review/acp`
**Date**: 2026-09-08
**Scope**: 18 files, ~6.2k LoC under `src/acp/`
**Method**: Read all source files in scope, cross-referenced callers
(`builtin_tools/acp_tools.rs`, `bin/aleph-server/commands/start/mod.rs`,
`gateway/handlers/acp_config.rs`, `gateway/context.rs`,
`builtin_registry/config.rs`, `builtin_tools/team/member_add.rs`),
applied rust-doctor categories (error handling, async correctness,
resource safety, architecture, security, performance, API design)
plus AGENTS.md redlines R1/R4/R8/R10.

## Module shape (orientation)

The ACP module implements an agent-client-protocol bridge that spawns
external CLI harnesses (Claude Code, Codex, Gemini, custom user tools)
as async subprocesses and exchanges JSON-RPC 2.0 over NDJSON stdio.
It contains:

- `protocol.rs` — JSON-RPC message types + structured `AcpErrorCode`
- `transport.rs` — NDJSON stdio reader task + request/response loop
- `incoming.rs` — agent→client request handler (fs/permission sandbox)
- `session.rs` — per-subprocess lifecycle + cancel handle
- `adapter.rs` / `adapters/` — adapter trait + Generic + Custom impls
- `manager/` — harness CRUD, lifecycle, persistence, session key
- `output_format.rs` — oneshot stdout parser

The module is well-engineered: bounded channels, dedicated spawn_blocking
for blocking fs syscalls, double-latched race-safe spawn paths in
`acquire_live_entry`, file-lock around persistence writes, symlink-aware
path confinement, defense-in-depth on permission grants, lock-ordering
documented in `crate::sync_primitives`. Most findings below are
small structural issues rather than correctness bugs.

---

## Findings (per file)

### `src/acp/incoming.rs`

#### [P1] expect-in-non-test-panics-handler
- **File:** `src/acp/incoming.rs:505`
- **Rule:** error-handling / panics-in-agent-facing-code
- **Context:** `apply_line_window()` is the "never panics" handler for
  agent-side `fs/read_text_file` line-window slicing. The `expect`
  message itself documents the invariant; the comment two lines up even
  says "an attacker-supplied huge `limit` can't overflow `usize`
  (a debug-build panic in this 'never panics' handler)". Using `expect`
  on the very next line contradicts that promise.
- **Before:**
  ```rust
  lines
      .get(start..end)
      // Safe: `start` is guarded by the check above, and `end` is clamped to `lines.len()`.
      .expect("invariant: start/end are within line bounds")
      .join("\n")
  ```
- **After:**
  ```rust
  // `get` returns `None` only if the bounds are inverted — impossible by
  // construction (`start < lines.len()` from the early return above, and
  // `end` is clamped to `lines.len()`). Use `unwrap_or_default` so an
  // unexpected out-of-bounds path never panics this agent-facing handler
  // (the function is documented as "never panics"); the empty-string result
  // is the same observable behaviour the caller would get on a 0-line file.
  lines.get(start..end).unwrap_or_default().join("\n")
  ```
- **Why:** `apply_line_window` runs on every agent `fs/read_text_file`
  call inside a tokio task. A future refactor that breaks the bounds
  invariant would panic the worker thread, killing in-flight prompts
  across all sessions. The empty-string fallback matches the documented
  zero-line semantics and never trips.

**Fixed in this PR.**

#### [P3] inconsistent-fallback-error-handling
- **File:** `src/acp/incoming.rs:118-119`
- **Rule:** style / consistency
- **Context:** `IncomingHandler::new` builds the workspace root from
  `cwd` or current_dir. The two branches use different empty-path
  sentinels for the same failure mode.
- **Before:**
  ```rust
  None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
  ```
  and earlier:
  ```rust
  let abs = if p.is_absolute() {
      p.to_path_buf()
  } else {
      std::env::current_dir().unwrap_or_default().join(p)
  };
  ```
  The first uses `unwrap_or_default()` (yields `PathBuf::new()` — empty
  path) while the second uses `unwrap_or_else(|_| PathBuf::from("."))`.
  For an empty `PathBuf::new()`, `lexical_normalize` then pushes "." as
  a component and the resulting root is `"."`, which is still
  functional but inconsistent.
- **After:** unify on `unwrap_or_else(|_| PathBuf::from("."))`.
- **Why:** Two error fallbacks for the same syscall; one yields an
  empty path that downstream `lexical_normalize` repairs, the other
  yields the explicit `"."`. Pick one and document it.
- **State:** Not fixed in this PR — would require either a unit test
  assertion to lock the behaviour or an audit of every other call
  site that relies on the implicit empty-path repair. Left as a noted
  nit.

### `src/acp/manager/harness_admin.rs`

#### [P1] duplicated-kill-emit-loop-across-three-sites
- **File:** `src/acp/manager/harness_admin.rs` (3 sites)
- **Rule:** architecture / DRY / lock-ordering drift
- **Context:** `unregister_harness`, `update_harness` (disable branch),
  and `update_harness` (enable branch) all repeated the same
  18-line block: iterate `removed`, drop the per-session mutex via
  `entry.session.lock().await`, call `session.kill().await`, then
  iterate again to emit `AcpSessionEvent::Removed` for each key. The
  block is preceded by the same `drop(configs); drop(adapters);
  drop(sessions);` triple in all three sites — and a typo in any one
  site (e.g. forgetting a `drop`, getting the emit payload wrong,
  swapping the `is_empty` branch) silently changes harness teardown
  semantics for one path while the others still work.
- **Before:** (excerpt from `unregister_harness`)
  ```rust
  drop(configs);
  drop(adapters);
  drop(sessions);
  for (_, entry) in &removed {
      let mut session = entry.session.lock().await;
      session.kill().await;
  }
  for (key, _) in removed {
      self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
          harness_id: key.harness_id,
          cwd: key.cwd.to_string_lossy().into_owned(),
          session_name: if key.name.is_empty() {
              None
          } else {
              Some(key.name)
          },
      })
      .await;
  }
  ```
  Same 18 lines duplicated in `update_harness` twice.
- **After:** extracted helper
  ```rust
  async fn kill_and_emit_removed(
      &self,
      removed: Vec<(SessionKey, SessionEntry)>,
  ) {
      for (_, entry) in &removed {
          let mut session = entry.session.lock().await;
          session.kill().await;
      }
      for (key, _) in removed {
          self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
              harness_id: key.harness_id,
              cwd: key.cwd.to_string_lossy().into_owned(),
              session_name: if key.name.is_empty() {
                  None
              } else {
                  Some(key.name)
              },
          })
          .await;
      }
  }
  ```
  Each call site now ends with:
  ```rust
  drop(configs);
  drop(adapters);
  drop(sessions);
  self.kill_and_emit_removed(removed).await;
  ```
- **Why:** Three divergent copies are a maintenance trap. The helper
  centralises the lock-then-emit semantics and the empty-name
  normalisation; the doc-comment on it explicitly forbids running it
  under the outer write locks, which the call sites must respect.

**Fixed in this PR.**

### Cross-module deferred findings (not acp-internal)

#### [P2] public-enums-missing-non-exhaustive
- **Files:** `src/acp/mod.rs:28` (`AcpSessionEvent`), `src/acp/protocol.rs:319`
  (`AcpErrorCode`), `src/acp/protocol.rs:416` (`AcpSessionState`),
  `src/acp/adapter.rs:13` (`AdapterMode`), `src/acp/incoming.rs:29`
  (`PermissionPolicy`), `src/acp/output_format.rs:9` (`OutputFormat`)
- **Rule:** architecture / API stability
- **Context:** Six public enums exported from `src/acp/`. None carry
  `#[non_exhaustive]`. Adding a variant is a breaking change for every
  downstream exhaustive `match` site.
- **Why deferred:** Same shape as the `shared/protocol` 58-enum gap
  already deferred in `review-results/aggregate.md`. Touching these
  here without coordinated migration of `bin/aleph-server`,
  `builtin_tools/acp_tools.rs`, `gateway/handlers/acp_config.rs`,
  and the panel would either:
  1. be no-op (no variant is currently being added), or
  2. require a crate-spanning `try_match!` migration like the
     shared/protocol change.
- **Recommended path:** Same dedicated migration as
  `shared/protocol` — single change covering all `alephcore` public
  enums, with a wildcard-arm migration plan.

#### [P2] AcpOperationError-public-fields
- **File:** `src/acp/protocol.rs:373-378`
- **Rule:** API design
- **Context:** `pub struct AcpOperationError { pub code, pub message,
  pub remote_error }` — public fields allow external code to construct
  an `AcpOperationError` whose message doesn't match its code (e.g.
  `code: Timeout, message: "session lost"`). Constructors exist
  (`new`, `with_remote`) and are used everywhere inside the crate.
- **Why deferred:** Existing call sites all go through `new` /
  `with_remote`. Tightening visibility is a tightening, not a fix;
  defer to the enum migration above so the change lands coherently.

### Confirmed-safe patterns (no finding, recorded for context)

The following were inspected and require no change:

- **Bounded channels**: `transport.rs:event_rx = mpsc::channel(256)`,
  `protocol.rs:MAX_NOTIFICATIONS = 1024`, `incoming.rs:MAX_FS_READ_BYTES
  = 32 MiB`, `MAX_FS_WRITE_BYTES = 16 MiB`. All three bounds are
  sized to prevent OOM via crafted agent input.
- **`spawn_blocking` discipline**: `incoming.rs::confine` correctly
  wraps blocking `std::fs::canonicalize` / `symlink_metadata` /
  `read_link`; `manager/persistence.rs::wire_persistence` correctly
  spawns the initial `load_persisted_sessions` into a blocking pool.
- **Lock ordering**: `crate::sync_primitives.rs` documents the
  conventions; `harness_admin.rs` comments call out the sessions →
  harnesses → configs acquisition order explicitly in both
  `unregister_harness` and `update_harness`. Consistent.
- **`std::sync::RwLock` use in `session.rs`**: `acp_session_id` is
  read with `read().unwrap_or_else(|e| e.into_inner()).clone()` in
  every site; guard never held across `.await`. No UB risk.
- **Cancel-race correctness**: `manager/lifecycle.rs::remove_if_same`
  uses `Arc::ptr_eq` to avoid evicting a respawned entry under a
  racing `acquire_live_entry`. `restore_sessions` mirrors the same
  pattern with explicit logging (ACP-R4-01 comment).
- **Race-safe spawn**: `acquire_live_entry` checks existing entry
  under read lock, evicts if dead under write lock with `Arc::ptr_eq`,
  spawns outside any lock, then double-checks after acquiring the
  write lock and kills its own session if it lost the race.
- **Permission policy defence-in-depth** (`incoming.rs:266-298`):
  `ApproveReads` blocks auto-approve when `toolCall` carries any of
  `path`/`file_path`/`command`/`shell`/`exec`/`url`, closing the
  bypass where an agent sends a `read`-kind label with a write/exec
  payload. `pick_option` uses word-boundary matching, not substring,
  so `"disallow"` can never satisfy `"allow"`. `is_auth_word` does
  similar word-boundary filtering for error-classification.
- **Path confinement**: `canonicalize_within_root` walks components
  one-at-a-time so a symlink pointing outside the workspace is
  rejected before the syscall that would otherwise follow it. The
  Windows `\\?\` verbatim-prefix issue is explicitly fixed.
- **Secure auth logging**: `AcpSession::authenticate` logs `method_id`
  only; the credential is never traced. `transport.rs::send` logs
  only `method` and `id`, never params.

---

## Headline counts

| File | P0 | P1 | P2 |
|------|----|----|----|
| `src/acp/incoming.rs` | 0 | 1 | 0 |
| `src/acp/manager/harness_admin.rs` | 0 | 1 | 0 |
| `src/acp/protocol.rs` | 0 | 0 | 2 |
| `src/acp/mod.rs` | 0 | 0 | 1 |
| `src/acp/adapter.rs` | 0 | 0 | 1 |
| `src/acp/output_format.rs` | 0 | 0 | 1 |
| `src/acp/transport.rs` | 0 | 0 | 0 |
| `src/acp/session.rs` | 0 | 0 | 0 |
| `src/acp/manager/persistence.rs` | 0 | 0 | 0 |
| `src/acp/manager/lifecycle.rs` | 0 | 0 | 0 |
| `src/acp/manager/session_key.rs` | 0 | 0 | 0 |
| `src/acp/adapters/*` | 0 | 0 | 0 |
| `src/acp/mock_server.rs` | 0 | 0 | 0 |
| **total** | **0** | **2** | **5** |

No P0 findings. No hard-redline violations.

## Findings fixed in this PR

1. **`incoming.rs:505` — `apply_line_window` `expect()` → `unwrap_or_default()`** —
   non-test agent-facing handler no longer panics on out-of-bounds slice.
2. **`manager/harness_admin.rs` — duplicated kill+emit pattern → extracted
   `kill_and_emit_removed` helper** — three sites now share one
   implementation; lock-ordering requirement documented on the helper.

## Findings deferred / cross-module

1. **Six `pub enum`s without `#[non_exhaustive]`** in `src/acp/`
   (`AcpSessionEvent`, `AcpErrorCode`, `AcpSessionState`, `AdapterMode`,
   `PermissionPolicy`, `OutputFormat`) — same shape as the
   `shared/protocol` 58-enum gap deferred in `aggregate.md`. Needs a
   coordinated wildcard-arm migration across
   `bin/aleph-server`, `builtin_tools/acp_tools.rs`,
   `gateway/handlers/acp_config.rs`, and panel consumers.
2. **`AcpOperationError` public fields** — should be tightened to private
   fields + getters in the same migration as item 1.
3. **`incoming.rs:118-119` inconsistent `unwrap_or_default` vs
   `unwrap_or_else`** — pure style; either unify or document why
   the empty-path branch is intentional.

## State of negative (explicit non-actions)

- **Not** running `cargo check` or `cargo build` (per task instructions;
  unified check runs after all module reviews finish).
- **Not** running `cargo clippy` (out of scope for this static pass).
- **Not** running `cargo test` (per task instructions).
- **Not** adding `#[non_exhaustive]` to the six acp public enums
  (deferred to the coordinated migration — see `aggregate.md` §2).
- **Not** refactoring `request`/`request_streaming` in `transport.rs`
  into a shared timeout+loop helper. Both methods carry the same drain
  stale-after-timeout block, but they branch on different
  notification-handling semantics (collect vs forward-callback) and the
  duplication is shallow — extracting risks coupling the two paths
  more than it saves.
- **Not** extracting the detach-then-kill pattern from
  `unregister_harness` (which uses `iter().filter().collect()` then a
  separate `remove` loop) and `update_harness` (which uses a closure
  that does both in one pass). The two patterns are intentionally
  different and unifying would obscure the per-call-site invariant.
- **Not** removing the redundant `start_kill` in `session.rs::Drop`
  (`kill_on_drop(true)` already kills on drop). Removing it would
  change observable behaviour: `kill_on_drop` defers to tokio's runtime
  shutdown ordering, while an explicit `start_kill` in our `Drop` runs
  synchronously when `AcpSession` itself goes out of scope (which is
  earlier than the runtime shutdown). Kept intentionally.
- **Not** addressing the documented stale-snapshot race in
  `manager/persistence.rs::wire_persistence` — the file-lock stops the
  rename race but does not enforce temporal ordering between
  `spawn_blocking` calls. The code's own comment acknowledges this;
  fixing it requires a write-queue or per-event version stamping,
  which is out of scope.
