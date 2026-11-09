# Jev Classification Report: Batch IDEN1

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-rev-identity-logging-loopgraph`  
**Target Module**: `src/identity` (Core, Keystore & Ledger)  
**Files Audited**:
- `src/identity/mod.rs`
- `src/identity/actor.rs`
- `src/identity/hash.rs`
- `src/identity/keystore.rs`
- `src/identity/ledger.rs`

---

## 1. Executive Summary

Batch IDEN1 covers the agent identity core, task-local delegation actor tracking, cryptographic preimage hashing, per-agent Ed25519 keystore, and the append-only signed operation ledger.
The subsystem exhibits strong architectural discipline: strict fail-closed semantics, tamper-evident hash chaining, resilient key lifecycle sequencing (mint → declare → activate), and clean single-writer FIFO synchronization with loss tracking.
Static review identified key Occam's Razor opportunities:
1. **Hot-path allocation elimination**: In `ledger::record()`, every tool dispatch unconditionally heap-allocates an owned `String` via `new.agent_id.clone()` solely to format an error log if the channel write fails. In >99.999% of calls this allocation is immediately discarded. Leveraging the returned `SendError(job)` recovers the string on failure with zero allocations on the successful path.
2. **Zero-copy secret loading**: In `keystore::seed()`, reading private seeds from the encrypted vault uses `hex::decode`, allocating an unnecessary intermediate heap `Vec<u8>`. Replacing with `hex::decode_to_slice` into an in-place `Zeroizing<[u8; 32]>` eliminates the heap allocation entirely.
3. **Idiomatic control flow & error forwarding**: Eliminating redundant `match` boilerplate in preimage hash calculation (`unwrap_or(&GENESIS_HASH)`) and replacing noisy `Ok(...?)` unwrap/rewrap patterns with direct `.map_err(Into::into)` in store wrappers.
4. **Existence oracle optimization**: Multiple hot dispatch call sites query `ledger::global().is_none()` purely as an existence check, inducing atomic refcount churn on `Arc<AgentLedger>`. Exposing a zero-cost `is_installed()` query allows clean existence checks without touching the handle.

---

## 2. Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---|:---|:---|
| **IDEN1-1** | `src/identity/ledger.rs:584` | `REDUNDANT-ALLOCATION` | `let agent_id = new.agent_id.clone();` unconditionally heap-allocates an owned `String` on every single tool execution hot path, solely to log `agent_id = %agent_id` in `tracing::warn!` if `tx.send().await` returns `Err`. | **FIX-NOW** | Remove unconditional clone. On `Err(SendError(job))`, borrow `agent_id` directly from `job` (`LedgerJob::Append(r) => &r.agent_id`). Zero allocation on the normal execution path. |
| **IDEN1-2** | `src/identity/keystore.rs:268-276` | `REDUNDANT-ALLOCATION` | `AgentKeystore::seed` calls `hex::decode(secret.expose())`, allocating an intermediate heap `Vec<u8>` before copying 32 bytes into `[u8; 32]` and deallocating/zeroizing the vector. | **FIX-NOW** | Use `let mut seed = Zeroizing::new([0u8; 32]); hex::decode_to_slice(secret.expose(), seed.as_mut()).map_err(...)?;`. Decodes in-place with zero heap allocations. |
| **IDEN1-3** | `src/identity/hash.rs:101-104` | `CONTROL-FLOW / INDIRECTION` | 4-line `match p.prev_hash { Some(prev) => h.update(prev), None => h.update(GENESIS_HASH), }` in `compute_hash`. | **FIX-NOW** | Simplify to `h.update(p.prev_hash.unwrap_or(&GENESIS_HASH));`. Eliminates verbose branching without changing hash preimage layout. |
| **IDEN1-4** | `src/identity/keystore.rs:167, 171, 175` | `CONTROL-FLOW / INDIRECTION` | Methods `identity()`, `list()`, and `keys_of()` use noisy `Ok(self.store....?)` wrapping, unlike adjacent `revoke()` which uses `.map_err(Into::into)`. | **FIX-NOW** | Simplify to `self.store.get_agent_identity(agent_id).map_err(Into::into)` etc., removing redundant unwrap and re-wrap. |
| **IDEN1-5** | `src/identity/ledger.rs:260` | `CONTROL-FLOW / INDIRECTION` | `AgentLedger::recent` wraps store invocation in `Ok(self.keys.store().ledger_recent(agent_id, limit)?)`. | **FIX-NOW** | Simplify to `self.keys.store().ledger_recent(agent_id, limit).map_err(Into::into)`. |
| **IDEN1-6** | `src/identity/ledger.rs:563-565` | `REDUNDANT-ALLOCATION` | Hot-path dispatch gates query `identity::global().is_none()` as an existence oracle, invoking `LEDGER.get().cloned()` and churning atomic reference counts on `Arc<AgentLedger>`. | **FIX-CAREFULLY** | Add `pub fn is_installed() -> bool { LEDGER.get().is_some() }` to `ledger.rs` and re-export in `mod.rs`. Preserves `global() -> Option<Arc<AgentLedger>>` for callers requiring ownership. |
| **IDEN1-7** | `src/identity/actor.rs:49-51` | `REDUNDANT-ALLOCATION` | Call sites checking `current_actor().is_some()` clone the task-local `String`. Suggestion to introduce speculative `has_actor() -> bool`. | **REJECT** | Public function `current_actor() -> Option<String>` is used across 15+ call sites. Adding speculative methods without an architectural mandate violates P6 (Occam's razor). Call frequency is bounded to turn/dispatch entry. |
| **IDEN1-8** | `src/identity/hash.rs:66` | `PANIC-RISK / DEFENSIVE-CHECK` | `lp()` uses `u32::try_from(bytes.len()).expect("preimage field exceeds u32 length")`. | **REJECT** | Deliberate cryptographic framing invariant. As explicitly documented in code, silent truncation would allow collision attacks across length boundaries; failing loud on fields >4 GiB enforces unambiguous preimage framing. |
| **IDEN1-9** | `src/identity/keystore.rs:240-248` | `CONTROL-FLOW / INDIRECTION` | `activate()` calls `get_agent_identity` before `upsert_agent_identity`, and calls `get_agent_identity` again afterwards. | **REJECT** | In SQLite, `INSERT ... ON CONFLICT DO UPDATE` does not return rows in rusqlite. The subsequent read guarantees returning the exact row as persisted by the database (including updated timestamps and reset `revoked_at`). |
| **IDEN1-10** | `src/identity/ledger.rs:239-240` | `REDUNDANT-ALLOCATION` | `write()` converts fixed `[u8; 32]` and `[u8; 64]` into heap `Vec<u8>` for `LedgerRecord.hash` and `signature`. | **DEFER** | `LedgerRecord` is a public serialization struct with custom hex serialization, shared across `verify.rs`, `export.rs`, `rusqlite` storage, and tools. Changing fields to arrays is an API-breaking contract change. |
| **IDEN1-11** | `src/identity/ledger.rs:444-450` | `PANIC-RISK / DEFENSIVE-CHECK` | `install()` uses `if LEDGER.get().is_some() { return None; }` followed by `!LEDGER.install(...) || !WRITER.install(...)`. | **REJECT** | Standard fail-closed atomic registration pattern for `CapabilitySlot`. Short-circuit guarantees concurrent callers cleanly fail without creating dangling sender channels. |
| **IDEN1-12** | `src/identity/ledger.rs:301-310` | `ACCIDENTAL-COMPLEXITY` | `orphan_chains()` collects known identities into a `BTreeSet<String>` for membership check instead of `HashSet`. | **REJECT** | The number of distinct agent identities on a daemon is small (typically single digits); `BTreeSet` maintains deterministic iteration order and avoids SipHash overhead for negligible dataset sizes. |

---

## 3. Invariant Safety Pre-Flight

1. **Public API Contracts**:
   - `AgentLedger::append`, `recent`, `verify`, `verify_all`, `agent_ids`, `orphan_chains`, `lost`, `keys` signatures remain identical.
   - `AgentKeystore::ensure`, `signing_identity`, `revoke`, `sign`, `identity`, `list`, `keys_of`, `mint_key`, `activate` signatures remain identical.
   - `current_actor() -> Option<String>` and `as_actor(...)` signatures remain untouched.
   - `Preimage` and `compute_hash` signatures and byte-framing logic remain 100% byte-for-byte identical.
   - All error types (`KeyError`, `LedgerCommandError`) retain existing variants.
2. **Cryptographic & Ledger Integrity**:
   - The chain preimage algorithm (`DOMAIN || agent_id || seq || at_ms || action || outcome || target || args_fp || detail || signer_fp || prev_hash || [principal]`) is completely preserved; compatibility test `a_principal_less_row_hashes_exactly_as_it_did_before_the_field_existed` remains satisfied.
   - Key lifecycle invariant (mint → declare → activate) in `perform_rotate` and (declare → mark) in `perform_revoke` remains strictly sequential within the single writer loop.
   - Vault key retrieval continues to zeroize private seed bytes upon decode and cache drop.
3. **Behavioral Equivalence**:
   - Normal append, rotation, revocation, and flush commands follow the exact same execution order and error propagation.
   - Error logging on writer failure in `record()` preserves the exact same `agent_id` and error context.
4. **Performance Impact**:
   - Eliminates 1 heap `String` allocation per tool dispatch on the hot path (`record()`).
   - Eliminates 1 intermediate heap `Vec<u8>` allocation per vault seed fetch (`AgentKeystore::seed`).
   - Reduces branching and unwrap overhead on the chain preimage hashing path (`compute_hash`).
   - Provides a zero-cost existence query (`is_installed()`) avoiding atomic refcount increments when probing ledger readiness.
