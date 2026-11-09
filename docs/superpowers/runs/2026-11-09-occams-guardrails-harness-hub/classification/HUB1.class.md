# HUB1 Classification Report: Foundation Types and Validation

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-guard-harness-hub`  
**Target Files**:
- `src/hub/mod.rs`
- `src/hub/types.rs`
- `src/hub/origin.rs`
- `src/hub/trust.rs`
- `src/hub/secrets.rs`
- `src/hub/verify.rs`
- `src/hub/catalog_client.rs`
- `src/hub/primer.rs`

---

## 1. Jev Tier Classification Matrix

| ID | Location | Taxonomy | Severity | Route | Summary |
|---|---|---|---|---|---|
| HUB1-01 | `src/hub/types.rs`, `src/hub/origin.rs:172-176` | `REDUNDANT-ALLOCATION` | Low | `FIX-NOW` | Deserializing `ExtensionKind` in `row_to_origin` via `serde_json::Value::String` allocates unnecessary JSON Value and String. Implement `FromStr` on `ExtensionKind` and parse directly from sqlite column ref. |
| HUB1-02 | `src/hub/origin.rs:192-195` | `REDUNDANT-ALLOCATION` | Low | `FIX-NOW` | `all_origins` dynamically formats SQL on every call via `format!("SELECT {SELECT_COLS} ...")` instead of using a `const &str`. |
| HUB1-03 | `src/hub/origin.rs:218-271` | `REDUNDANT-ALLOCATION` | Med | `FIX-NOW` | `forget_installed` clones `entry_id` into `exact: Vec<String>` and `leaf_matches: Vec<String>`, then loops over `doomed: Vec<String>`. Ambiguity checks ensure at most 1 item; borrowing `&str` avoids all String clones and vector allocations. |
| HUB1-04 | `src/hub/origin.rs:52-58` | `REDUNDANT-ALLOCATION` | Low | `FIX-NOW` | `spec_digest` serializes spec via `serde_json::to_string` performing unnecessary UTF-8 validation and String allocation before hashing. `serde_json::to_vec` is more efficient. |
| HUB1-05 | `src/hub/trust.rs:40-48` | `REDUNDANT-ALLOCATION` | Low | `FIX-NOW` | `command_display` allocates intermediate `Vec<String>` and clones every argument before calling `.join(" ")`. Can append into a single String directly. |
| HUB1-06 | `src/hub/trust.rs:176-189` | `REDUNDANT-ALLOCATION` | Med | `FIX-NOW` | `scan_for_injection` unconditionally performs `text.replace('\u{00A0}', " ")`, compound invisible stripping, and 3 lowercase allocations. On clean text, fast-path single lowercase pass eliminates 4 allocations per call without altering detection semantics. |
| HUB1-07 | `src/hub/secrets.rs:40-52` | `REDUNDANT-ALLOCATION` / `CONTROL-FLOW` | Low | `FIX-NOW` | `id_hash` manually formats 8 bytes into hex using `std::fmt::Write` loop and preallocated String. `hex::encode(&digest[..8])` replaces 13 lines with a standard 1-line call. |
| HUB1-08 | `src/hub/verify.rs:66-86`, `151-206` | `CONTROL-FLOW` / `REDUNDANT-ALLOCATION` | Low | `FIX-NOW` | `health_note` string formatting duplicated across match arms in `verdict_with_health`. `verify_install` deeply nested (5 levels of match). Flatten with `let-else` guards and extract `health_note`. |
| HUB1-09 | `src/hub/catalog_client.rs:227-236` | `REDUNDANT-ALLOCATION` | Low | `FIX-NOW` | `ingest` joins tags via `he.tags.join(" ")` inside `format!(...)`, allocating twice per entry. Append tags directly to `scan_blob`. |
| HUB1-10 | `src/hub/catalog_client.rs:86-94` | `DEFENSIVE-CHECK` | Low | `FIX-CAREFULLY` | `sanitize_generated_at` performs duplicate digit checks on fixed-width slots (already verified in loop). Keep clean structural check. |
| HUB1-11 | `src/hub/primer.rs:17-20` | `REDUNDANT-ALLOCATION` | Low | `FIX-NOW` | `prime_official_catalog_if_empty` extends entries vector sequentially without pre-allocating combined capacity. |
| HUB1-12 | `src/hub/origin.rs:199` | `DEAD-CODE` | Info | `DEFER` | `delete_origin` is public and tested; keep for external maintenance / ledger cleanup. |

---

## 2. Invariant & Safety Review

1. **Fail-Closed Verification**:
   - `verify.rs` directory checks rely on `symlink_metadata` and fail-closed on symlinks or non-directories.
   - `verdict_with_health` keeps `Degraded` as running while `Unhealthy`, `Restarting`, `Dead`, `Stopped` fail closed.
2. **Trust & Injection Rails**:
   - `scan_for_injection` maintains exact multi-pass detection for NBSP and zero-width split attacks (`lower_stripped`, `lower_nbsp`, `lower_compound`).
3. **Ledger Integrity**:
   - `origin.rs` preserves exact ambiguity rejection when multiple ledger rows match backend ID or directory leaf.
