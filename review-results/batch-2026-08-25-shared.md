# Module Review — shared (client / logging / protocol / ui_logic)

- **Date**: 2026-08-25
- **Worktree**: `/home/zou/data/workspace/Aleph/.worktrees/review/interface-shared-mobile-2026-08-25`
- **Branch / baseline**: `review/interface-shared-mobile-2026-08-25` @ `6de033068` (merge: integrate origin/main)
- **Method**: graphify-free, full static reading under four-perspective checklist (security / logic / architecture / quality); threshold ≥ 80% confidence with 3+ concrete anchors per finding.
- **Scope**: `shared/{client,logging,protocol,ui_logic}` + `shared/config/default-config.toml` + `shared/*/{Cargo,clippy}.toml` (read-only)

## File / LOC stats

| Sub-crate | Rust files | LOC (incl. tests) | Largest single file |
|---|---:|---:|---|
| `shared/client` | 7 | 1 782 | `connection.rs` 954 |
| `shared/logging` | 5 | 767 | `retention.rs` 235 |
| `shared/protocol` | 48 | 13 315 | `events.rs` 1 173 |
| `shared/ui_logic` | 13 | 1 790 | `failure.rs` 396 |
| **Total** | **73** | **17 654** | — |

(Counts taken before the in-flight fix on `shared/client/src/gateway_client.rs`; the file grew by +77/-2 lines. The total in-tree now is 17 729 LOC.)

## Findings summary

| Severity | Count |
|---|---:|
| Critical | 0 |
| **High** | **1** |
| Medium | 0 |
| Low | 6 |
| **Total actionable** | **7** |

Per the project protocol the lone High has been fixed in-tree on this branch; the Low items are recorded for follow-up rather than fixed (mostly documentation / oversized-file cleanups that are out of scope for this review or already shipped trade-offs with their own rationale).

---

## Findings (sorted by severity)

### H1 — `shared/client/src/gateway_client.rs:143-159` — `GatewayClient` discards the `connect` handshake's error frame and times out at the method's deadline instead
- **Perspective**: Logic & Correctness (error-handling / contract).
- **Confidence**: 92% (anchor 1: read loop filter on line 152 was strict `id == request_id == 1`; anchor 2: the `connect` frame is sent with `id = 0` per lines 100–115; anchor 3: nothing in the function reads `id == 0` from the wire).
- **Behavior**: `GatewayClient::call_raw` opens a fresh socket, sends the `connect` handshake with `id=0`, then sends the method frame with `id=1`. The read loop filters `id == request_id` (i.e. `== 1`) and discards everything else, including the id=0 frame. When the gateway refuses the handshake — `AUTH_REQUIRED` (`-32000`) for a caller missing credentials, `RATE_LIMITED` (`-32002`), origin gate `403` re-encoded as an error, etc. — the error frame arrives with `id=0` and `error`, is silently dropped, and the loop waits the full `DEFAULT_TIMEOUT_MS` (30 s) for an id=1 frame that will never come. The caller sees `CliError::Timeout("Read timeout: ...")` and has no way to know the real reason is in the server log.
- **Why this matters**: `aleph-server gateway call` is the documented public entrypoint to drive a one-shot RPC against a remote gateway. A non-loopback caller with no token hits this branch every time and learns nothing about *why*.
- **Fix applied**: Replaced the strict `if id == request_id` filter with a three-arm match: `id == 0 && json.error` → return that frame immediately (the existing `if let Some(error)` block below the loop reads it and produces `CliError::Rpc`); `id == request_id` → return as before; everything else (notifications, a successful handshake with no payload) → continue. A `#[tokio::test]` (`a_handshake_error_is_surfaced_immediately_not_at_the_method_timeout`) spins up a loopback server that replies to `connect` with an `AUTH_REQUIRED` error and asserts the call fails in <5 s, not at the 30 s timeout.
- **Commit**: `150ee4fdc shared/client: surface connect-handshake error in GatewayClient instead of waiting for method timeout`.

### L1 — `shared/protocol/src/events.rs` — 1 173 LOC, exceeds the 500-LOC guideline by ~2.3×
- **Perspective**: Quality (oversized file).
- **Confidence**: 100% (anchor 1: `wc -l shared/protocol/src/events.rs`; anchor 2: AGENTS.md "oversized files (>500 LOC)" guidance; anchor 3: the file is pure type definitions + 3 small helpers).
- **Notes**: The growth is honest — every variant has a doc explaining why it is `StreamEvent`-shaped and which gateway frame it mirrors. The doc comments are load-bearing for the wire contract. Splitting `AgentTraceEvent`'s 20 variants into a sibling module would be a structural improvement, but the risk of splitting wrong is non-zero and is a candidate for the next round of protocol work rather than a same-day fix. **Not fixed in this batch.**

### L2 — `shared/protocol/src/trace_presentation.rs` — 993 LOC; `present_agent_trace_event` is one 340-line match (lines 223–555)
- **Perspective**: Quality.
- **Confidence**: 100%.
- **Notes**: Same "load-bearing for the contract" reasoning as L1. The function grows by one arm per `AgentTraceEvent` variant, so splitting is best done alongside the events.rs refactor. Two long-lived fixes in this file (`MoaAdvisor` rendering its `text` round-2 W3a, `MoaAdvisorSpend` rendering its `cost_usd`) were both about restoring parity with the webchat renderer — splitting now would land them in modules that immediately drift again. **Not fixed in this batch.**

### L3 — `shared/config/default-config.toml` — stale sample config, NOT loaded by any code
- **Perspective**: Documentation / quality.
- **Confidence**: 90% (anchor 1: `grep -rn 'default-config' shared/ src/ interfaces/ mobile/ desktop/ 2>/dev/null` returns zero hits; anchor 2: every concrete claim about paths or PII format contradicts current code).
- **Stale specifics**:
  - References "Phase 7" features; the codebase has been through many phases since.
  - PII redaction tokens shown as `[EMAIL_REDACTED]` / `[PHONE_REDACTED]` / `[CARD_REDACTED]`; the real PII scrubber (`shared/logging/src/pii.rs:69-93`) emits `[EMAIL]` / `[PHONE]` / `[CREDIT_CARD]` (no `_REDACTED` suffix).
  - References `~/.config/aleph/`; the real code resolves `~/.aleph/` via `shared/protocol/src/paths.rs`.
  - Mentions `[trigger]` double-tap hotkey system — that is a desktop/macOS concern, not a shared default config.
  - Mentions `[providers.*]` blocks with `provider_type`, `api_key`, `fallback_providers` — `shared/protocol/src/providers/wire.rs` defines the contract, but the production config loader lives in `alephcore` (out of scope).
- **Why not fixed**: task brief marks this file read-only ("do NOT modify content, may flag if outdated").
- **Recommendation**: either delete it (no caller) or move it to `desktop/macos/` (the only consumer that ever needed a typed hotkey) where the staleness would be local to a shell rather than spreading across the shared tree.

### L4 — `shared/logging/src/pii.rs:28-52` — 9 `.expect("static PII regex is valid")` in production code
- **Perspective**: Quality (AGENTS.md code style).
- **Confidence**: 100%.
- **Notes**: AGENTS.md says "Never `unwrap()` in production." Static-regex `expect` is the canonical exception (literals cannot fail to compile at runtime), but no clause in `AGENTS.md` carves it out. Same finding was already raised in the batch5 review (`shared-rest` → L6) and was not acted on because the resolution belongs to `AGENTS.md` rather than to a specific file. **Not fixed in this batch.** Adding one sentence to `AGENTS.md` ("Static-regex `expect` is permitted for compile-time-constant patterns") would close this finding permanently.

### L5 — `shared/client/src/config.rs:91-95` — `default_path()` falls back to a relative `PathBuf::from(".")` when `dirs::config_dir()` returns `None`
- **Perspective**: Quality / cross-platform correctness.
- **Confidence**: 80% (anchor 1: `dirs::config_dir()` returns `None` on a chrooted / no-home process; anchor 2: the fallback is a literal `"."`, not `~/.aleph/...`; anchor 3: no caller surfaces a clearer error).
- **Behavior**: `CliConfig::default_path()` does `dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("aleph-cli")`. On a system without `$HOME` and without the platform fallback, the result is `./aleph-cli/config.toml` — a relative path that, if `save()` is later called, writes to the process's CWD. The `save()` path does not `canonicalize` first, so a `cli.toml` in the wrong CWD is the failure mode.
- **Why not fixed**: the practical fallback surface (a process with no home and no platform config dir) is narrow, and the fix ("surface `CliError::Config` instead of silently using `.`") changes the public API of `default_path()` enough to want its own commit. **Not fixed in this batch.**

### L6 — `shared/protocol/src/voice_text.rs:132-138` — `is_token_loop` normalises via `normalize()` per token, but `filter_transcript` compares the *whole* normalised string against `HALLUCINATION_PHRASES`
- **Perspective**: Logic consistency (low).
- **Confidence**: 85% (anchor 1: the list contains both `"OK"`-lowered entries and `"ok"`; anchor 2: `normalize()` lowercases; anchor 3: no test covers the cross-case "OK" → match).
- **Notes**: Reading the code carefully: every entry in `HALLUCINATION_PHRASES` is pre-lowercased, and `normalize()` lowercases before the contains check, so the lookup is consistent. The note here is that this contract is implicit and only one test (`nulls_chinese_boilerplate`) checks CJK round-tripping — adding `filter_transcript("OK")` → `""` and `filter_transcript("OK.")` → `""` as explicit tests would lock the contract in. **Not fixed in this batch.**

---

## Architecture Red Lines compliance (R1 / R3 / R4 / R7 / R8)

| Red line | Status | Evidence |
|---|---|---|
| **R1** (core never calls platform APIs; `shared/logging` + `shared/protocol` are platform-free; `shared/client` uses tokio-tungstenite only) | ✅ | `shared/client/Cargo.toml` declares only `tokio`, `tokio-tungstenite`, `native-tls`, `futures-util`, `serde`, `serde_json`, `toml`, `thiserror`, `tracing`, `dirs` + `aleph-protocol`. No `alephcore` / `desktop` / `mobile` deps. `shared/logging/Cargo.toml` is tracing-family + regex + chrono + dirs only. `shared/protocol/Cargo.toml` is chrono + schemars + serde only. |
| **R3** (core minimalism; no uuid/rand/async runtime in `shared/protocol`) | ✅ | `grep -rn 'uuid\|rand\|tokio' shared/protocol/` returns zero hits in `.rs` (mentions are in doc-comments only). The id generator (`shared/protocol/src/ids.rs`) deliberately uses `AtomicU64` to keep the crate off the uuid→rand dep chain. |
| **R4** (interfaces use shared; shared doesn't import interfaces) | ✅ | `shared/*` declare zero deps on `interfaces/*` / `desktop/*` / `mobile/*`. |
| **R7** (shared crates are reusable across many shells; no `alephcore` dep) | ✅ | Same evidence as R1; no shell-to-shell coupling. |
| **R8** (regex ONLY for machine formats) | ✅ | `shared/logging/src/pii.rs` regex targets email / phone / SSN / credit-card / AWS keys / GitHub PATs / `Basic` + `Bearer` headers / `key=value` credentials. All machine formats. `shared/protocol/src/scope.rs::belongs_to_project` uses exact string comparison, not regex. `shared/ui_logic/src/safety/prompt_injection.rs` uses substring heuristics by design (regex WASM size cost is documented at lines 37-39). |
| R2, R5, R6, R9, R10 | N/A | These red lines govern UI shells / config / prompt design; the shared crate has no UI, no menu entry point, no configurable knobs exposed as tools, no prompt authoring. |

---

## Out-of-scope but flagged for the next batch (no in-tree action)

| Item | Anchor | Why out of scope |
|---|---|---|
| `shared/protocol/src/desktop_bridge/methods/pim.rs:380-413` — `MailMessageDetail` and `MailGetResult` are byte-for-byte duplicates | 9 fields per struct, identical shape | Removing `MailGetResult` requires the out-of-scope `desktop/macos/src/pim.rs` to migrate to `MailMessageDetail`. Flagged in the batch5 review too; coordinate with the desktop batch. |
| `shared/protocol/src/desktop_bridge/errors.rs:9-14` — error-code constants overlap with `shared/protocol/src/jsonrpc.rs:33-66` (`-32001` / `-32002` etc. each have two definitions) | grep of `i32 = -320` in both files | The two protocol families are independent on the wire; the overlap is symbolic, not functional. Worth a `// distinct from jsonrpc::AUTH_REQUIRED` cross-reference but no semantic fix needed. |

---

## What was NOT reviewed (residual risk)

- `shared/logging/src/file_appender.rs:67-71` and `shared/logging/src/pii_filter.rs` were read but not stress-tested: log-rotation interaction with active writers (does `RollingFileAppender::new(Rotation::DAILY, ...)` rename a file a writer has open?) and the `PiiScrubbingFormat`'s span attribute rendering were not exercised. The unit tests cover the static patterns; the rotation behavior is upstream-tracing-appender's responsibility.
- The WASM connector (`shared/ui_logic/src/connection/wasm.rs`) was not compiled against `wasm32-unknown-unknown`; review of its `Closure::forget()` leaks (standard wasm-bindgen idiom, documented at lines 65-80) is logical only. A native `cargo check -p shared-ui-logic --target wasm32-unknown-unknown` would be the actual cross-check.
- `shared/protocol/src/providers/wire.rs` (692 LOC) and `shared/protocol/src/desktop_bridge/methods/{ax,input,media,screen,perm,pim}.rs` were read for correctness against their doc comments and serde round-trip tests, but the contract against `alephcore` (handler-side) was not exercised because `alephcore/` is out of scope. Wire-drift bugs of the kind `wire.rs`'s module doc warns about could still exist on the handler side.
- `shared/protocol/src/trace_replay.rs`, `shared/protocol/src/btw.rs` (small, well-tested), `shared/protocol/src/receipt.rs` (small, well-tested), `shared/protocol/src/team_topic.rs` (small, well-tested), `shared/protocol/src/scope.rs` (small, well-tested), `shared/protocol/src/queue.rs` (trivial), `shared/protocol/src/audit.rs` (medium), `shared/protocol/src/tool_permissions.rs` (small, key-constants-only) were spot-checked but not deeply audited — they are stable contracts with rich test coverage and the recent batch8 (`batch8-interface-shared.md`) confirmed no dead code or empty modules remain.

## Fixes applied

| Commit | Subject |
|---|---|
| `150ee4fdc` | shared/client: surface connect-handshake error in GatewayClient instead of waiting for method timeout |

That is the only commit made by this review on this branch. The fix is localised to `shared/client/src/gateway_client.rs` (+77/-2 lines), adds a regression guard (`#[tokio::test]`), and uses no new dependencies. No other files in scope were modified.