# Findings & Remediation: Batch IDEN2 (Identity Record, Schema, Artifact, Export & Verify)

**Commit**: `8b1931279`  
**Files Touched**:
- `src/identity/record.rs`
- `src/identity/artifact.rs`
- `src/identity/export.rs`
- `src/identity/verify.rs`

## Remediation Applied

1. **Eager Compile-Time Invariant Enforcement (`IDEN2-1`)**:
   - Hoisted inherent associated constants `_ALL_LEN_MATCHES` on `LedgerAction` and `LedgerOutcome` to module-level anonymous constants `const _: () = assert!(...);` ensuring guaranteed eager evaluation by rustc.

2. **Zero-Copy SQLite Row Deserialization (`IDEN2-2`)**:
   - In `row_to_record`, replaced `row.get(6)?` and `row.get(8)?` (which allocated two heap `String`s per row) with borrowed string slices `row.get_ref(6)?.as_str()?` and `row.get_ref(8)?.as_str()?`, completely avoiding 2 heap allocations per record read from SQLite.

3. **Streaming Artifact File Hashing (`IDEN2-3`)**:
   - In `artifact::hash_file`, replaced `std::fs::read` (which allocated up to 256 MiB in memory) with a 16 KiB buffered streaming reader into `Sha256`, strictly bounding heap allocation to a constant 16 KiB while enforcing the exact same byte limit and generating the exact same digest.

4. **Move Semantics on Chain Export (`IDEN2-4`)**:
   - Implemented `From<LedgerRecord> for ExportedRecord` and consumed `records` and `keys` by value via `.into_iter()`, eliminating 5 string clones per record and 1 per key during export generation.

5. **In-Place Hex Validation (`IDEN2-5`)**:
   - In `PinnedHead::parse`, replaced `hex::decode(hash).is_err()` with direct in-place validation (`hash.len() % 2 == 0 && hash.bytes().all(|b| b.is_ascii_hexdigit())`), eliminating intermediate heap vector allocation.

6. **Binary Search & Stack Comparison on Pin Check (`IDEN2-6`, `IDEN2-7`)**:
   - In `HeadPin::check`, replaced linear O(N) scan with O(log N) `binary_search_by_key(&pin.seq, |r| r.seq)`.
   - Used stack buffer for 32-byte SHA-256 hash comparison, eliminating heap string allocation.

7. **Export Keyring Pre-Allocation (`IDEN2-8`)**:
   - Initialized `ExportKeyring` with `HashMap::with_capacity(doc.keys.len())` to eliminate incremental reallocations.

8. **Single-Probe Foreign Key Cache (`IDEN2-9`)**:
   - Refactored `Keyring::check` in `verify.rs` to use `self.others.entry(fingerprint.to_string())`, collapsing 3 hash map lookups and redundant string conversions into a single probe.

9. **Flattened Signature Verification Expression (`IDEN2-10`)**:
   - Simplified `verify_artifact` return match.
