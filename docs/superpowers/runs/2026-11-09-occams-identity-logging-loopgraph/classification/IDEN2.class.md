# Jev Classification Report: Batch IDEN2

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-rev-identity-logging-loopgraph`  
**Target Module**: `src/identity` (Record, Schema, Artifact, Export & Verify)  
**Files Audited**:
- `src/identity/record.rs`
- `src/identity/schema.rs`
- `src/identity/artifact.rs`
- `src/identity/export.rs`
- `src/identity/verify.rs`

---

## 1. Executive Summary

Batch IDEN2 covers the identity ledger record structures, database schema definitions, agent artifact signing envelopes (`<path>.aleph-sig.json`), self-contained portable chain export/import, and the end-to-end cryptographic chain verifier (`walk_chain`).

The subsystem demonstrates rigorous architectural rigor: strict fail-closed verification, anti-tampering guards against tail truncation, prefix deletion, undeclared signers, and foreign key substitution, along with cross-platform canonical JSON export validation.

Static Occam's Razor analysis reveals key optimization and simplification opportunities:
1. **Memory Cap Enforcement & Zero-Copy Hashing**: `artifact::hash_file` previously read entire files up to 256 MiB into heap memory with `std::fs::read` solely to compute a 32-byte SHA-256 digest. Neither caller (`sign_artifact`, `verify_artifact`) ever uses the raw bytes. Streaming through a chunked reader (e.g. 16 KiB buffer) eliminates up to 256 MiB heap allocations and removes OOM attack surfaces.
2. **Export Move Semantics**: In `export_chain`, owned `records` (`Vec<LedgerRecord>`) and `keys` (`Vec<AgentKey>`) were iterated by reference, deep-cloning 5 heap `String`s per record and 1 per key, only to discard the source vectors immediately. Consuming vectors via `into_iter()` eliminates thousands of redundant string allocations on large exports.
3. **Database Row Decoding Zero-Copy String Borrowing**: `row_to_record` unconditionally allocated two heap `String`s (`row.get(6)?`, `row.get(8)?`) solely to parse `LedgerAction` and `LedgerOutcome`. Borrowing slices directly via `row.get_ref(i)?.as_str()?` eliminates two heap allocations per row decoded across queries and verifications.
4. **Compile-time Assertion Eagerness**: In `record.rs`, associated `const _ALL_LEN_MATCHES: () = assert!(...);` inside inherent impl blocks are unreferenced and therefore not eagerly evaluated by rustc. Hoisting to module-level anonymous constants `const _: () = assert!(...);` guarantees true compile-time enforcement of variant array completeness.
5. **Collection & Cache Optimization**: Foreign key resolution in `verify::Keyring` performed 3 hash map lookups (`contains_key`, `insert`, `get`) with redundant `owner.to_string()` clones. Converting to the `Entry` API collapses this to a single probe. `ExportKeyring` initializes with default capacity instead of pre-sizing to `keys.len()`. `HeadPin::check` performed a linear O(N) scan on strictly monotonically sequenced rows instead of O(log N) binary search.
6. **Preservation of Wire-Critical Canonicalization**: Identified that `export_preimage` must retain its `serde_json::to_value` (BTreeMap alphabetical key sorting) pipeline to ensure byte-for-byte signature verification compatibility across exports.

---

## 2. Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---|:---|:---|
| **IDEN2-1** | `src/identity/record.rs:84-87, 131-134` | `CORRECTNESS / INVARIANT-TIGHTENING` | Associated constants `const _ALL_LEN_MATCHES: () = assert!(Self::ALL.len() == 8, ...);` on `LedgerAction` and `LedgerOutcome` inside inherent impls are unreferenced and not eagerly evaluated by `rustc`. | **FIX-NOW** | Hoist to module-level anonymous constants `const _: () = assert!(LedgerAction::ALL.len() == 8);` and `const _: () = assert!(LedgerOutcome::ALL.len() == 3);` to guarantee eager compiler enforcement. |
| **IDEN2-2** | `src/identity/record.rs:323-324, 335, 337` | `REDUNDANT-ALLOCATION` | In `row_to_record`, `let action_str: String = row.get(6)?;` and `let outcome_str: String = row.get(8)?;` allocate two heap `String`s on every row decoded from SQLite, only to call `.parse::<LedgerAction>()` and `.parse::<LedgerOutcome>()`. | **FIX-NOW** | Read borrowed string slices directly via `row.get_ref(6)?.as_str()?` and `row.get_ref(8)?.as_str()?`, and parse via `LedgerAction::from_str` / `LedgerOutcome::from_str`. Zero string allocations. |
| **IDEN2-3** | `src/identity/artifact.rs:201-223` | `REDUNDANT-ALLOCATION / PANIC-RISK` | `hash_file` uses `std::fs::read(path)` to load up to `MAX_ARTIFACT_BYTES` (256 MiB) into an in-memory `Vec<u8>` just to calculate `Sha256::digest(&bytes)`. Callers only ever use the 32-byte digest. Loading up to 256 MiB invites memory spikes and OOM. | **FIX-NOW** | Stream the file in 16 KiB chunks via `std::io::BufReader` directly into `Sha256`, tracking total bytes read against `MAX_ARTIFACT_BYTES`. Reduces maximum heap allocation from 256 MiB to a fixed chunk buffer while generating the exact same SHA-256 digest. |
| **IDEN2-4** | `src/identity/export.rs:448-456` | `REDUNDANT-ALLOCATION` | In `export_chain`, `records` is an owned `Vec<LedgerRecord>` and `keys` is an owned `Vec<AgentKey>`. The mapping `records.iter().map(ExportedRecord::from_record)` and `keys.iter().map(...)` deep-clones 5 heap `String`s per record (`signer_fp`, `target`, `args_fp`, `detail`, `principal`) and 1 per key, then immediately drops the source vectors. | **FIX-NOW** | Consume vectors by value: implement `From<LedgerRecord> for ExportedRecord` to move `signer_fp`, `target`, `args_fp`, `detail`, and `principal` without cloning. Move `k.fingerprint` via `keys.into_iter()`. Eliminates thousands of string allocations on large exports. |
| **IDEN2-5** | `src/identity/export.rs:140` | `REDUNDANT-ALLOCATION` | In `PinnedHead::parse`, `hex::decode(hash).is_err()` allocates an unnecessary intermediate `Vec<u8>` on the heap purely to validate that the string contains valid hex. | **FIX-NOW** | Validate hex in-place: `hash.len() % 2 != 0 || !hash.bytes().all(|b| b.is_ascii_hexdigit())`. Zero heap allocations. |
| **IDEN2-6** | `src/identity/export.rs:195` | `CONTROL-FLOW / INDIRECTION` | `HeadPin::check` executes an O(N) linear scan: `rows.iter().find(\|r\| r.seq == pin.seq)`. Ledger rows are strictly monotonically ordered by `seq`. | **FIX-NOW** | Use binary search: `rows.binary_search_by_key(&pin.seq, \|r\| r.seq)` for O(log N) row lookup. |
| **IDEN2-7** | `src/identity/export.rs:201` | `REDUNDANT-ALLOCATION` | In `HeadPin::check`, `hex::encode(&row.hash).eq_ignore_ascii_case(&pin.hash)` allocates a 64-byte `String` on the heap to perform a case-insensitive hex comparison against the pinned hash. | **FIX-NOW** | Compare against a stack buffer (`let mut buf = [0u8; 64]; hex::encode_to_slice(&row.hash, &mut buf)`) or decode `pin.hash` to bytes and compare directly `row.hash == decoded`. Zero heap allocations. |
| **IDEN2-8** | `src/identity/export.rs:488-495` | `REDUNDANT-ALLOCATION` | In `ExportKeyring::load`, `let mut keys = HashMap::new();` is initialized with default 0-capacity, triggering incremental rehashing and reallocations as keys are inserted. | **FIX-NOW** | Pre-allocate with known capacity: `HashMap::with_capacity(doc.keys.len())`. |
| **IDEN2-9** | `src/identity/verify.rs:251-266` | `CONTROL-FLOW / INDIRECTION` | In `Keyring::check`, foreign key resolution does 3 separate hash table lookups (`contains_key`, `insert`, and `get`) and unconditionally clones `owner.to_string()` when already populated. | **FIX-NOW** | Use the `Entry` API: `match self.others.entry(fingerprint.to_string())` to perform a single lookup and cache probe, moving `owner` into `Signer::Foreign` without redundant lookups. |
| **IDEN2-10** | `src/identity/artifact.rs:324-330` | `CONTROL-FLOW / INDIRECTION` | Verbose closure/match pattern: `Ok(if verify_signature(...).is_ok() { ArtifactVerdict::Valid } else { ArtifactVerdict::BadSignature })`. | **FIX-NOW** | Flatten expression to `Ok(match verify_signature(...) { Ok(()) => ArtifactVerdict::Valid, Err(_) => ArtifactVerdict::BadSignature })`. |
| **IDEN2-11** | `src/identity/verify.rs:398-412` | `CORRECTNESS / INVARIANT-TIGHTENING` | Anchor verification in `walk_chain` checks `last.seq < anchor.seq` (`TailTruncated`) and `last.seq == anchor.seq` (`AnchorMismatch`), but if `last.seq > anchor.seq`, no fault is raised, silently ignoring chains that have outgrown their anchor. | **FIX-CAREFULLY** | Audit anchor semantics. When anchor is strict authoritative head, `last.seq != anchor.seq` should fail closed. However, partial offline verification or historical snapshots may intentionally test extensions (`HeadPin::Extends`). Verify test invariants before tightening. |
| **IDEN2-12** | `src/identity/verify.rs:459` & `src/identity/export.rs:627` | `REDUNDANT-ALLOCATION` | `walk_chain` allocates a new `Vec<ChainFault>`, which is then extended into the caller's newly allocated `faults` vector (`faults.extend(walk_chain(...)?)`). | **FIX-CAREFULLY** | Allow passing `&mut Vec<ChainFault>` or pre-sizing capacity to eliminate intermediate vector allocation on verification paths. |
| **IDEN2-13** | `src/identity/export.rs:394-406` | `CORRECTNESS / INVARIANT-TIGHTENING` | In `export_preimage`, `serde_json::to_value` then `serde_json::to_vec` creates intermediate JSON AST allocations. Suggestion to serialize directly. | **REJECT** | Wire-critical cryptographic canonicalization. As explicitly documented, `serde_json::to_value` guarantees struct keys are sorted alphabetically via `BTreeMap` before compact serialization. Bypassing this would change preimage key ordering and break compatibility with existing signed exports. |
| **IDEN2-14** | `src/identity/artifact.rs:189-190` | `PANIC-RISK / DEFENSIVE-CHECK` | `u32::try_from(agent.len()).expect("agent id exceeds u32 length in signature preimage")` panics if agent ID exceeds 4 GiB. | **REJECT** | Documented cryptographic framing invariant. Silent truncation would permit length-extension and preimage confusion attacks. Agent IDs are short strings in practice; failing loud enforces unambiguous framing. |
| **IDEN2-15** | `src/identity/verify.rs:290-305` | `CONTROL-FLOW / INDIRECTION` | `declared_signers` performs a dedicated preliminary pass over `rows` to collect all `IdentityCreated` and `IdentityRotated` fingerprints into a `HashSet<&str>` before `walk_chain` begins. | **REJECT** | Deliberate design for async queueing tolerance. Records are enqueued asynchronously; an action issued before key rotation may land after the rotation record in SQLite. Requiring immediate adjacency would flag benign concurrency races as tampering. Dual-pass set membership check is essential for semantic correctness. |
| **IDEN2-16** | `src/identity/schema.rs:32-93` | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | Review of schema constants: `IDENTITY_SCHEMA`, `LEDGER_ADD_PRINCIPAL_SQL`, `LEDGER_COLUMNS`. | **REJECT** | Confirmed live and necessary. All 3 constants are actively consumed by `SecurityStore` migration arms, identity queries, and row decoding. Preserving the domain-owns-shape / store-owns-file architectural boundary. |

---

## 3. Invariant Safety Pre-Flight

1. **Public API & Trait Contracts**:
   - `row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<LedgerRecord>` signature and returned struct remain identical.
   - `sign_artifact`, `read_envelope`, `verify_artifact`, `envelope_path` signatures and error types (`ArtifactError`, `ArtifactVerdict`) remain identical.
   - `export_chain`, `verify_export`, `PinnedHead::parse`, `ExportPins`, `ExportReport`, `HeadPin` signatures and wire representations remain untouched.
   - `walk_chain`, `verify_chain`, `ChainReport`, `ChainFault` public surface remains stable.
2. **Cryptographic Preimages & Canonicalization**:
   - `DOMAIN` (`b"aleph-agent-artifact-v1"`), length prefix, agent ID, and 32-byte digest layout in `artifact::preimage` remain 100% byte-for-byte identical.
   - `export_preimage` canonical serialization using alphabetical `BTreeMap` sorting via `serde_json::to_value` is preserved; existing signature envelopes continue to verify cleanly.
   - Streaming SHA-256 file hashing in `artifact::hash_file` produces the exact same 32-byte digest as loading the full file into memory.
3. **Behavioral Equivalence**:
   - Offline verification without vault/daemon (`verify_export`) retains identical fault reporting.
   - Anti-tampering invariants (prefix deletion, sequence gap, tail truncation, anchor mismatch, foreign signer, undeclared signer) continue to catch all anomalies in test suites.
   - Asynchronous queueing tolerance across key rotations via `declared_signers` remains intact.
4. **Performance & Memory Impact**:
   - Caps memory consumption during artifact hashing to a 16 KiB buffer, eliminating up to 256 MiB allocations per artifact operation.
   - Eliminates 5 String allocations per record during `export_chain` via move semantics (`into_iter()`).
   - Eliminates 2 String allocations per row during `row_to_record` SQLite deserialization.
   - Reduces foreign key cache lookups in `Keyring::check` from 3 to 1 via `Entry` API.
   - Accelerates `HeadPin::check` from linear O(N) to O(log N) binary search on sorted ledger rows.
