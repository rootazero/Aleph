# Severed Wire Audit Report: src/cli/ — Batch 1

**Audit Date:** 2026-08-22  
**Auditor:** pi (severed-wire-audit skill)  
**Prior Report:** `review-results/cli.md` (2026-08-12)  
**Scope:** `src/cli/` (endpoint.rs, ipc_client.rs, policy.rs, mod.rs)  
**Graph:** `graphify-out/GRAPH_REPORT.md`

---

## Summary

| Category | Count |
|----------|------:|
| Commits Made | 0 (all fixes applied in prior batch) |
| New Severed Wires Found | 0 |
| Prior Findings Status | All 6 FIXED |
| New Findings | 0 |
| DECIDE Items Carried Forward | 3 |

---

## Phase 1: Scan Results

### Seams Checked

| Seam Type | Defined | Consumed | Delta |
|-----------|---------|---------|-------|
| CLI → Policy dispatch | `run_no_lock`, `with_policy`, `try_with_policy` | `secret.rs`, `gateway.rs`, `plugins.rs`, `resume.rs`, `bootstrap_runtime` | 0 |
| CLI → Endpoint file ops | `write_endpoint`, `read_endpoint`, `remove_endpoint` | `daemon.rs`, `start/mod.rs`, `ipc_client.rs` | 0 |
| Policy → IPC client | `forward_to_server` | `policy.rs:163`, `resume.rs:27` | 0 |
| HTTP client → TLS gate | `build_client`, `is_loopback_host` | `call_once` | 0 |
| Error handling | `truncate_error_body` | `finalize`, 401 arm | 0 |
| Lock error types | `LockHeldError` | `with_policy`, `Display` impl, tests | 0 |

**Result:** No severed wires found.

---

## Phase 2: Candidate List

No candidates — all prior findings were already addressed in commits:
- `6efb6605c` (2026-08-11): H1, H2, M1, M2, L1, L2
- `ee0731e1e` (2026-08-18): cli-endpoint-01 CUT, cli-ipc-01 CONNECT, cli-policy-01 CONNECT

---

## Phase 3: Triage Summary

### Prior Findings — All Resolved

| ID | Finding | Commit | Evidence |
|----|---------|--------|----------|
| H1 | TLS cert unconditional accept | 6efb6605c | `is_loopback_host` + test coverage (6 tests) |
| H2 | chmod failure leaves file | 6efb6605c | `remove_file` on chmod failure (endpoint.rs:68-75) |
| M1 | LockOrIpc TOCTOU | 6efb6605c | Retry in `try_with_policy` (policy.rs:157-167) |
| M2 | PID 0 orphaned | 6efb6605c | `orphaned = pid == 0` (policy.rs:96) |
| L1 | Error body truncation | 6efb6605c | `truncate_error_body` uniform (ipc_client.rs:52-66) |
| L2 | cfg(not(windows)) guard | 6efb6605c | Guard removed from `try_with_policy_lock_only_returns_err_when_held` |

### New Findings — None

No new severed wires identified. The 7-lens scan confirmed:
- All CLI commands wired to handlers (main.rs match exhaustive)
- All policy dispatch functions have callers
- All endpoint file operations wired
- All IPC forward paths verified
- Error handling uniform across code paths
- Test coverage comprehensive

---

## Phase 4: Fixes

**No new fixes required** — all prior audit findings were resolved in previous batch.

---

## Phase 5: Guard Status

No CI grep-diff guard implemented for this module (out of scope for this audit).

---

## DECIDE Items (Carried Forward)

| Item | Description | Rationale | Recommendation |
|------|-------------|-----------|-----------------|
| D1 | `atomic_io::write_atomic` temp file permissions | Requires changing `write_atomic` signature (8+ call sites) | Defer to atomic_io pass; current code correct on supported filesystems |
| D2 | `with_policy` calls `std::process::exit(64)` | Deliberate UX choice documented in docstring | Not a defect; flag for completeness only |
| D3 | No Windows ACL tightening for `.ipc-endpoint.json` | Windows not a supported production target for IPC channel | Defer to Windows hardening pass |

---

## Architecture Compliance

| Redline | Status | Notes |
|---------|--------|-------|
| R1 (core no platform APIs) | ✓ | All `#[cfg(unix)]` blocks bounded to permission tightening |
| R4 (interface layer pure I/O) | ✓ | `ipc_client`: HTTP forwarding; `endpoint`: file I/O; `policy`: dispatch only |
| R7 (one core, many shells) | ✓ | CLI is the wire between binary and admin HTTP API |
| R8 (regex for machine formats only) | ✓ | No regex in this module |
| R10 (intelligence in prompt) | ✓ | No prompt engineering in this module |

---

## Test Coverage Verification

| Test File | Finding | Platform | Status |
|-----------|---------|----------|--------|
| `endpoint.rs` tests | H2, basic ops | Unix | ✓ 11 tests |
| `ipc_client.rs` tests | H1, L1 | All | ✓ 11 tests |
| `policy.rs` tests | M1, M2, L2 | All | ✓ 6 tests (L2 guard removed) |

---

## Conclusion

The `src/cli/` module is fully wired. All findings from the 2026-08-12 audit have been resolved. The code demonstrates:

1. **Secure defaults**: TLS loopback-only trust (H1)
2. **Defensive file handling**: chmod failure cleanup (H2)
3. **Robust retry logic**: TOCTOU retry on lock contention (M1)
4. **Honest error messages**: PID 0 treated as orphaned (M2)
5. **Privacy-preserving output**: Error body truncation (L1)
6. **Cross-platform correctness**: cfg guard removed (L2)

No new severed wires were identified in this audit pass.

---

*Report path: `review-results/cli-batch-1/REPORT.md`*  
*Graph path: `graphify-out/GRAPH_REPORT.md`*  
*Graph lines: ~450 (4K+ with full wire tracing)*
