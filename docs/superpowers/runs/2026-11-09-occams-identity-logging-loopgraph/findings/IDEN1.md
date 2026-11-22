# Findings & Remediation: Batch IDEN1 (Identity Core, Keystore & Ledger)

**Commit**: `e9473d2d9`  
**Files Touched**:
- `src/identity/ledger.rs`
- `src/identity/keystore.rs`
- `src/identity/hash.rs`
- `src/identity/mod.rs`

## Remediation Applied

1. **Zero-Allocation Hot-Path Ledger Logging (`IDEN1-1`)**:
   - `record()` previously did `let agent_id = new.agent_id.clone();` on every tool call hot path.
   - Now on error, retrieves `agent_id` directly from `e.0` (`LedgerJob::Append(r) => &r.agent_id`), eliminating heap string allocation on all successful tool dispatches.

2. **In-Place Seed Decoding (`IDEN1-2`)**:
   - `AgentKeystore::seed` previously called `hex::decode` into a heap `Vec<u8>` before copying to `[u8; 32]`.
   - Now uses `let mut seed = Zeroizing::new([0u8; 32]); hex::decode_to_slice(...)`, decoding directly into the stack buffer and avoiding heap allocation.

3. **Preimage Hash Branch Flattening (`IDEN1-3`)**:
   - Simplified `match p.prev_hash { Some(prev) => h.update(prev), None => h.update(GENESIS_HASH) }` to `h.update(p.prev_hash.unwrap_or(&GENESIS_HASH));`.

4. **Idiomatic Result Mapping (`IDEN1-4`, `IDEN1-5`)**:
   - Replaced noisy `Ok(self.store....?)` wrapping in `AgentKeystore::identity`, `list`, `keys_of`, and `AgentLedger::recent` with direct `.map_err(Into::into)`.

5. **Zero-Cost Existence Probe (`IDEN1-6`)**:
   - Added `is_installed() -> bool { LEDGER.get().is_some() }` in `ledger.rs` and re-exported in `mod.rs` to allow checking ledger readiness without atomic refcount increments.
