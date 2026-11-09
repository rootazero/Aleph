# HUB1 Static Review and Occam's Razor Refactoring

**Batch**: HUB1 — Hub Foundation Types, Provenance Ledger, Trust & Verification  
**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-guard-harness-hub`  
**Files Audited**:
- `src/hub/mod.rs`
- `src/hub/types.rs`
- `src/hub/origin.rs`
- `src/hub/trust.rs`
- `src/hub/secrets.rs`
- `src/hub/verify.rs`
- `src/hub/catalog_client.rs`
- `src/hub/primer.rs`

---

## 1. Executive Summary

Batch HUB1 audits the foundation types, install provenance ledger, trust rails, secret naming, and post-install verification logic of the Aleph Hub subsystem. Applying Occam's razor, we eliminated redundant heap allocations (intermediate string cloning, vector allocations, dynamic SQL formatting, and repeated string lowercasing) and flattened deeply nested control flow while strictly preserving all security boundaries and fail-closed invariants.

---

## 2. Invariant Analysis & Guarantees

1. **Origin & Ledger Ambiguity Handling**:
   - `origin::forget_installed` requires matches to be unambiguous. If more than 1 exact match or more than 1 directory leaf match exists, it logs a warning and returns `Ok(0)` rather than clobbering multiple rows. Refactoring to borrow `&str` preserved this exact behavior without allocating multiple `Vec<String>`.
2. **Fail-Closed Verification**:
   - `verify::verify_on_disk_path` uses `std::fs::symlink_metadata` so symlinks are never dereferenced and are rejected fail-closed.
   - `verify::verify_install` maintains strict exhaustive matching on `HealthStatus`: `Healthy` and `Degraded` are recognized as running, whereas `Unhealthy`, `Restarting`, `Dead`, and `Stopped` fail closed. Flattening the control flow with `let-else` preserves exact diagnostic detail strings and failure paths.
3. **Injection & Trust Rails**:
   - `trust::scan_for_injection` maintains its multi-pass detection (stripped, NBSP-replaced, and compound stripped passes) to defeat zero-width and invisible-character splitting attacks. The new fast-path checks `!text.contains('\u{00A0}') && removed == 0` to avoid 4 redundant string allocations on benign inputs while running the complete compound passes whenever suspicious characters are present.
4. **Secret Key Collision Resistance**:
   - `secrets::field_key` uses domain-separated SHA-256 prefixes to ensure distinct extension IDs that sanitize to identical strings do not collide in the vault. `id_hash` was simplified to standard `hex::encode` with zero change to wire format or key derivation.
5. **Public DTO Compatibility**:
   - All serialization tags, wire fields, and JSON schemas remain byte-for-byte identical.

---

## 3. Surgical Code Repairs

### 3.1 `src/hub/types.rs`
- **Implementation**: Added `std::str::FromStr` for `ExtensionKind` mapping `"skill"`, `"plugin"`, and `"mcp"`.
- **Tests**: Added parser test assertions ensuring exact roundtrip and failure on unknown strings.

### 3.2 `src/hub/origin.rs`
- **`row_to_origin`**: Replaced `serde_json::Value::String(kind)` and `serde_json::from_value` with direct `kind_str.parse::<ExtensionKind>()`.
- **`all_origins`**: Replaced per-call `format!("SELECT {SELECT_COLS} ...")` heap string allocation with a compile-time static `const ALL_ORIGINS_SQL: &str`.
- **`forget_installed`**: Borrowed `&str` candidate slices from `all` instead of cloning strings into `exact: Vec<String>` and `leaf_matches: Vec<String>`, directly executing `delete_origin(conn, doomed)` on the single resolved entry.
- **`spec_digest`**: Switched from `serde_json::to_string` to `serde_json::to_vec` to eliminate UTF-8 validation and intermediate string allocation.

### 3.3 `src/hub/trust.rs`
- **`command_display`**: Replaced `vec![command.clone()]` + `args.iter().cloned()` + `.join(" ")` with direct sequential string formatting, eliminating intermediate `Vec<String>` and argument cloning.
- **`scan_for_injection`**: Added fast-path branch when text contains neither invisible characters nor NBSP (`!has_nbsp && removed == 0`), eliminating 4 string allocations and extra string folding on benign catalog entries.

### 3.4 `src/hub/secrets.rs`
- **`id_hash`**: Replaced manual 13-line `write!(&mut out, "{byte:02x}")` loop with `hex::encode(&hasher.finalize()[..8])`.

### 3.5 `src/hub/verify.rs`
- **`verdict_with_health`**: Extracted duplicate `health_note` formatting across match arms.
- **`verify_install`**: Flattened 5 levels of nested `match` statements into linear `let-else` guards for manager availability, server listing, and server lookup.

### 3.6 `src/hub/catalog_client.rs`
- **`ingest`**: Appended tag strings directly to `scan_blob` instead of calling `he.tags.join(" ")` inside `format!(...)`, saving intermediate string allocation per catalog entry.

### 3.7 `src/hub/primer.rs`
- **`prime_official_catalog_if_empty`**: Pre-allocated combined vector capacity (`mcp.len() + skills.len() + plugins.len()`) before extending.

---

## 4. Verification Note

In accordance with instructions, `cargo check` was not run in this batch. All refactoring was performed with strict semantic equivalence and minimal surgical diffs.
