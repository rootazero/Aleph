# src/memory (top-level) — occams-r5 Review

## Summary
- **8 findings**: 1 crit / 1 high / 3 med / 3 low
- Files reviewed (line counts):

| File | Lines | Assessment |
|------|-------|------------|
| `insights.rs` | 842 | Well-structured; core pure, store fetch gated |
| `project_scope.rs` | 811 | Well-documented; pure functions, good test coverage |
| `embedding_provider.rs` | 502 | Clean trait; `validate_api_base` SSRF guard is solid |
| `streaming_scrubber.rs` | 499 | Tight streaming FSM; edge cases well-handled |
| `reembed.rs` | 455 | Clean migration logic; idempotent by design |
| `content_scanner.rs` | 431 | Correct delegation to `unicode_guard` SSOT |
| `embedding_manager.rs` | 347 | Clean lock ordering; documented requeue semantics |
| `embedding_resolver.rs` | 283 | Pure routing; small, well-bounded |
| `tool_signal_sink.rs` | 243 | Correct fire-and-forget contract |
| `session_memory_mode.rs` | 199 | Clean dial resolution; `all_covers_every_variant` |
| `loom_concurrency.rs` | 192 | ⚠ Dead models; see Finding 1 |
| `proptest_enums.rs` | 132 | Trivially small |
| `embedding_signature.rs` | 107 | Trivially small; minor `.to_string()` noise |
| `mod.rs` | 103 | Clean re-exports |
| `namespace.rs` | 59 | Correctly gutted; surviving surface is honest |
| `explain.rs` | 51 | Correctly gutted; surviving surface is honest |

No `TODO`/`FIXME`/`XXX`/`HACK` found. No `pub`/`pub(crate)` visibility issues. No lock-across-`await`. No oversize files (>700L — `insights.rs` and `project_scope.rs` are close but justified by their doc comments).

---

## Findings

### [CRIT] `loom_concurrency.rs` — 4 of 5 test models are orphaned or incorrect

- **Location**: `src/memory/loom_concurrency.rs:1-192`
- **Category**: dead code / correctness
- **Evidence**:
  - `loom_daemon_singleton_init`: models `DreamDaemon` compare_exchange — `DreamDaemon` is now in `dreaming/mod.rs` (directory module), so the named pattern still exists but the loom model is a thin proxy at best.
  - `loom_compression_trigger_race`: models `compression/scheduler.rs Mutex + AtomicU32` — referenced file exists, this model is live.
  - `loom_activity_timestamp_update`: comment says `memory/dreaming.rs LAST_ACTIVITY_TS` — **that file does not exist** (the module is `dreaming/` directory).
  - `loom_metrics_counter_accuracy`: comment says `memory/cortex/dreaming.rs total_processed/total_extracted` — **`cortex/` directory does not exist**.
  - `loom_embedding_provider_swap`: models `memory/embedding_manager.rs RwLock<Provider>` — actual field is `RwLock<Option<Arc<dyn EmbeddingProvider>>>`; the model uses `RwLock<String>` and clones the whole string on every read (`name.clone()`), not the Arc-wrapped trait object. This models the wrong thing.
- **Suggested fix**: Delete `loom_concurrency.rs` entirely. The `loom` feature and dev-dependency should be removed. The test comments document patterns that are either stale (`cortex/`, `dreaming.rs`), partially accurate (`DreamDaemon` moved to a dir), or wrong (`RwLock<String>` vs `RwLock<Option<Arc<dyn...>>>`). If the team wants to keep loom tests, each model must be verified against live code with a comment pointing to the exact file:line being modelled.
- **Risk**: Low in production (dev-only, behind `#[cfg(all(test, feature = "loom"))]`), but the wrong model misleads future developers about what concurrency behavior is actually being verified.

---

### [HIGH] `content_scanner.rs` — `SSH_ACCESS_RE` regex misses `~/.ssh` paths and has no `owner`

- **Location**: `src/memory/content_scanner.rs:72-76`
- **Category**: correctness / security coverage gap
- **Description**: The `SSH_ACCESS_RE` pattern is:
  ```
  (?:\$HOME|~)/\.ssh\b
  ```
  The `~` is a literal tilde in the regex (not expanded by Rust's `Regex` crate). The `~/.ssh` pattern only matches when `~` is followed by a word-boundary char — but `~` itself is **not a word character**, so `~/.ssh` followed by `/` fails: the word boundary `\b` fires between `~` and `/` because `/` is not a word char. Result: `~/.ssh/config` and `~/.ssh/id_rsa` are **not flagged**. Only `~ .ssh` (with a space between) or tilde at end-of-string would match.
- **Suggested fix**:
  ```
  r"(?:\$HOME|~)/\.ssh"
  ```
  Drop `\b` — the slash is a sufficient terminator. Optionally add a word boundary after `.ssh`:
  ```
  r"(?:\$HOME|~)/\.ssh(?:/|\b)"
  ```
- **Risk**: Benign false negative — legitimate content about SSH configs gets past the scanner. A credential-theft attempt embedding `~/.ssh` is not caught. Does not introduce a false positive.

---

### [MEDIUM] `embedding_signature.rs:51-54` — Redundant `.to_string()` in `Mismatch` variant

- **Location**: `src/memory/embedding_signature.rs:51-54`
- **Category**: unnecessary allocation
- **Description**:
  ```rust
  Some(s) => SignatureStatus::Mismatch {
      stored: s.to_string(),      // s is already &str, to_string() on String field = OK but s is &str
      current: current.to_string(),
  },
  ```
  The fields are `String`, the inputs are `&str`. The `.to_string()` calls are syntactically necessary but a caller could instead pass owned `String`s directly and avoid the allocation. More importantly, `current` comes from `compare(current: &str)` which is itself called as `compare(Some(s), "openai:m:1536")` with a string literal — the literal coerces to `&str` and then gets re-allocated to `String`. Callers of `compare` do not need to change, but the API design silently allocates.
- **Suggested fix**: Keep as-is for now (low severity). If the API is refactored, consider having callers pass owned `String`s or changing the fields to `&str` (requires lifetime juggling).
- **Risk**: Trivial memory noise on the hot path (only called during `reembed_all` init and provider switch).

---

### [MEDIUM] `content_scanner.rs:122-123` — Redundant `.to_string()` on `ScanVerdict::Rejected`

- **Location**: `src/memory/content_scanner.rs:122-123`
- **Category**: unnecessary allocation
- **Description**:
  ```rust
  return ScanVerdict::Rejected {
      reason: "Content contains prompt injection pattern".to_string(),
      pattern: "prompt_injection",  // &'static str, no to_string needed
  };
  ```
  8 instances of `reason: "...".to_string()` where `"..."` is a string literal coerced to `String`. Two instances of `pattern: "..."` where `"..."` is `&'static str` assigned to a `&'static str` field — zero cost but noisy.
- **Suggested fix**: Make `ScanVerdict::Rejected` fields `&'static str`:
  ```rust
  pub enum ScanVerdict {
      Rejected { reason: &'static str, pattern: &'static str },
  }
  ```
  Then replace all 8 `reason: "...".to_string()` with `reason: "..."`. This removes the heap allocation in the rejection path of the content scanner, which is on the hot write path for every memory fact.
- **Risk**: Requires changing `ScanVerdict::Rejected` fields to `&'static str`. Affects all call sites that construct this variant (only within this file). Straightforward mechanical change.

---

### [MEDIUM] `reembed.rs:191` — `#[allow(clippy::too_many_arguments)]` on `reembed_agent_notes`

- **Location**: `src/memory/reembed.rs:191`
- **Category**: code smell
- **Description**: Function has 10 parameters. Clippy flags this because beyond ~4 parameters, callers must remember argument order and position, which is a maintenance hazard. The `#[allow]` suppresses the warning without justification.
- **Suggested fix**: Group the state-mutating output params into a struct:
  ```rust
  struct ReembedCounters {
      total_notes: usize,
      total_updated: usize,
      total_skipped: usize,
      errors: Vec<String>,
  }
  ```
  Pass `&mut ReembedCounters` instead of four separate `&mut` params. This also makes the caller in `reembed_all` cleaner. The function's purpose (batch-reembed one agent's notes) is clear enough that the struct name documents intent.
- **Risk**: Refactoring churn; well-contained to `reembed.rs`.

---

### [LOW] `streaming_scrubber.rs:499L` — File approaches 500-line heuristic; internal `feed` function is the natural split

- **Location**: `src/memory/streaming_scrubber.rs:1-499`
- **Category**: file size
- **Description**: 499 lines (just under the 500-line rough heuristic). The test module is 140 lines. The production logic (~360 lines) fits comfortably in one file but the `feed` FSM (the core algorithm) is ~80 lines and logically separable from the scrubber construction (`with_tags`, `with_tag_set`, `reset`, `in_span`, `flush`) and the ASCII helpers (`find_ascii_ci`, `max_partial_suffix_ascii_ci`).
- **Suggested fix**: No immediate action. If the scrubber gains features (e.g., a replacement-rule mode), split along: `streaming_scrubber.rs` (construction/flush) + `streaming_scrubber/fsm.rs` (the `feed` loop) + `streaming_scrubber/ascii_helpers.rs` (the two pure helpers). This is low priority.
- **Risk**: None; file is currently within the heuristic limit.

---

### [LOW] `embedding_manager.rs:54` — `#[allow]` on `excessive_clone`

- **Location**: `src/memory/embedding_manager.rs:54`
- **Category**: unnecessary suppression
- **Description**:
  ```rust
  let decision = resolve(&settings);  // rust-doctor-disable-next-line excessive-clone
  (
      decision.effective.map(|c| c.id.clone()),  // line 54
      decision.eason,
      decision.effective.cloned(),  // clones the Option<&Config> to Option<Config>
  )
  ```
  Two clones: `c.id.clone()` (the id field) and `.cloned()` (the whole config). Both are necessary — `resolve` returns borrowed `&'a EmbeddingProviderConfig` and the caller needs owned copies before the settings lock is released. The `#[allow]` is correctly placed.
- **Suggested fix**: No change needed. The suppression is justified. Document the reason in the `#[allow]` comment:
  ```rust
  // Settings lock must not be held across provider creation (I/O).
  // Clones capture what we need before releasing the lock.
  #[allow(clippy::excessive_clone)]
  let (effective_id, reason, config) = /* ... */;
  ```
- **Risk**: None.

---

### [LOW] `namespace.rs` — Surviving surface is honest; no findings

- **Location**: `src/memory/namespace.rs:1-59`
- **Category**: informational
- **Description**: The module doc is a correct, honest accounting of the earlier overclaim. The surviving surface (`NamespaceScope::Owner` + `to_namespace_value() -> "owner"`) is used by `memory/reflector/recall_signals`. This is a well-gutted module that correctly identifies its own former overreach. No action needed.

---

## Cross-cutting

### Embedding subsystem: clean boundaries
`embedding_provider.rs` (trait + impl), `embedding_manager.rs` (lifecycle + queue), `embedding_resolver.rs` (pure routing), `embedding_signature.rs` (signature strings). No feature triangle — each module has one job. No meaningful duplication across the four files. The `resolve` function is small and pure; `EmbeddingManager` owns the async lifecycle; `RemoteEmbeddingProvider` owns the HTTP/reqwest layer. Clean separation.

### `explain.rs` — Surviving surface is honest; no findings
`FactExplanation` and `ExplainedEvent` are constructed by `events/traveler.rs::explain_fact`. The module doc correctly identifies the deleted audit types. No dead types or unused exports.

### Loom dependency
`loom = { version = "0.7", optional = true }` in `Cargo.toml`. Only compiled when `cargo test --features loom`. The `#[cfg(all(test, feature = "loom"))]` gate in `mod.rs` is correct. The concern is the content of the tests (see CRIT finding above), not the dependency itself.

### No dead code outside `loom_concurrency.rs`
No unused `pub fn`, no unreachable code, no `#[cfg]` stubs, no `impl` blocks with no callers. All `pub use` items in `mod.rs` resolve to live types.

---

## Out of scope
- Subdirs: `assembler/`, `compression/`, `context/`, `dreaming/`, `events/`, `notes/`, `store/`, `transcript_indexer/`, etc. — separate batch.
- Runtime behavior of `validate_api_base` SSRF checks — only reviewed the code shape.
- Database schema / query correctness.
- `proptest_enums.rs` (trivially small, internal test helpers).
