# Comprehensive Findings & Triage Matrix — `src/config`, `src/context`, `src/diagnostics`

- **Review Date**: 2026-11-08
- **Scope**: 157 files, 68,503 LOC across `src/diagnostics/**`, `src/context/**`, `src/config/**`
- **Total Findings**: 126 (2 P0, 31 P1, 46 P2, 47 P3)
- **Triage Model (Jev Role)**:
  - `Fix-Now` (High-leverage / high-confidence safety and correctness defects): 22 findings
  - `Verify-Then-Fix` (Subtle boundary conditions and robustness defects): 28 findings
  - `Defer` (Public API shifts, major refactorings, or low-risk cosmetic cleanups): 76 findings

---

## 1. Findings Distribution by Batch

| Batch | Scope | Files | LOC | P0 | P1 | P2 | P3 | Total | Clean Files |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| **B1** | `src/diagnostics/**` | 27 | 11,458 | 0 | 3 | 3 | 4 | 10 | 19 |
| **B2** | `src/context/fabric/**` & `src/context/*.rs` | 9 | 6,105 | 0 | 2 | 4 | 6 | 12 | 5 |
| **B3** | `src/context/budget/**` & `retrieval/**` | 9 | 6,629 | 0 | 4 | 4 | 6 | 14 | 0 |
| **B4** | `src/context/compact/**` (Part 1 Core) | 5 | 7,824 | 0 | 2 | 2 | 4 | 8 | 0 |
| **B5** | `src/context/compact/**` (Part 2 Strategies) | 15 | 4,869 | 0 | 2 | 6 | 5 | 13 | 6 |
| **B6** | `src/config/types/policies/**` & `generation/**` | 16 | 6,601 | 0 | 4 | 4 | 4 | 12 | 9 |
| **B7** | `src/config/types/memory/**` & `types/*.rs` Core | 21 | 6,814 | 0 | 5 | 6 | 4 | 15 | 0 |
| **B8** | `src/config/types/*.rs` Extended, `agent_*` | 25 | 6,158 | 1 | 4 | 6 | 5 | 16 | 17 |
| **B9** | `src/config/*.rs` Root Engine & Lifecycle | 17 | 10,147 | 1 | 5 | 6 | 3 | 15 | 9 |
| **B10**| `src/config/tests/**` Integration Test Suite | 13 | 3,115 | 0 | 1 | 5 | 5 | 11 | 7 |
| **Total** | | **157** | **69,720** | **2** | **32** | **46** | **46** | **126** | **72** |

---

## 2. Priority 0 & Priority 1 Findings Matrix

| Finding ID | Severity | Location | Description | Disposition |
|---|---|---|---|---|
| **B8-01** | **P0** | `src/config/agent_resolver/mod.rs:289-328` | When `workspace_path == agent_dir`, `old_sessions` equals `new_sessions`; `remove_dir_all` wipes live session history. | **Fix-Now (Wave 1)** |
| **B9-01** | **P0** | `src/config/save.rs:228-235` | `merge_sections` probes root `t.get("ssrf")` instead of `[security.ssrf]`, destroying SSRF security rules on save. | **Fix-Now (Wave 1)** |
| **B1-01** | **P1** | `src/diagnostics/checks/duplicate_instance.rs:33` | `duplicate_instance` check is unconditionally skipped in `doctor` command. | **Fix-Now (Wave 2)** |
| **B1-02** | **P1** | `src/diagnostics/checks/disk_space.rs:93` | Blocking `statvfs` syscall on async Tokio worker thread. | **Verify-Then-Fix** |
| **B1-03** | **P1** | `src/diagnostics/checks/discord_channel_health.rs:226` | Read failure mapped to `Severity::Info`, masking diagnostic failure. | **Fix-Now (Wave 2)** |
| **B2-01** | **P1** | `src/context/fabric/runtime.rs:1174` | Strict token/byte equality check rejects compliant conservative external plugins. | **Fix-Now (Wave 2)** |
| **B2-02** | **P1** | `src/context/file_ops.rs:88` | `canonicalize_path_string` strips leading slash on absolute paths, causing collision with relative paths. | **Fix-Now (Wave 2)** |
| **B3-01** | **P1** | `src/context/retrieval/content_index.rs:205` | SQLite busy/lock contention error during check drops and destroys FTS5 index. | **Fix-Now (Wave 3)** |
| **B3-02** | **P1** | `src/context/budget/cheap_passes/image_stripping.rs:77` | Strips all user images when tool result contains an image. | **Fix-Now (Wave 3)** |
| **B3-03** | **P1** | `src/context/budget/preflight.rs:175` | Base64 image payload charged as 800k tokens instead of estimate in preflight. | **Fix-Now (Wave 3)** |
| **B4-01** | **P1** | `src/context/compact/compactor.rs:1176` | Async cancellation collapsed into deterministic truncation in `summarize_slice`. | **Verify-Then-Fix** |
| **B4-02** | **P1** | `src/context/compact/manual.rs:663` | Cut selection retires open `RunStarted` without closers, causing `FinishWithoutStart`. | **Fix-Now (Wave 3)** |
| **B5-01** | **P1** | `src/context/compact/summary_utils.rs:116` | Tag stripping searches closing tag from offset 0, misparsing commentary tags. | **Fix-Now (Wave 3)** |
| **B5-03** | **P1** | `src/context/compact/preserve.rs:55` | False positive prefix match drops user messages starting with `[Context Summary]`. | **Fix-Now (Wave 3)** |
| **B6-001**| **P1** | `src/config/types/policies/tool_permissions.rs:120` | Swallows glob syntax errors and fails open to Allow. | **Fix-Now (Wave 4)** |
| **B6-002**| **P1** | `src/config/types/generation/config.rs:142` | Cross-modality provider name collision drops speech/transcription providers. | **Verify-Then-Fix** |
| **B7-01** | **P1** | `src/config/types/acp.rs:341` | Uninitialized presence flags in `AcpAdapterEntry` cause preset hydration to wipe custom user args. | **Fix-Now (Wave 4)** |
| **B7-03** | **P1** | `src/config/types/dispatcher/core.rs:74` | Unclamped zero values cause dispatcher task queue deadlocks or CPU spinning. | **Fix-Now (Wave 4)** |
| **B7-04** | **P1** | `src/config/types/phase6_wiring.rs:312` | Substring match on provider names causes false collision (`mini` in `gemini`). | **Fix-Now (Wave 4)** |
| **B7-05** | **P1** | `src/config/types/memory/mod.rs:188` | Memory top-level defaults silently overwrite `[memory.assembler]` user config. | **Fix-Now (Wave 4)** |
| **B8-02** | **P1** | `src/config/agent_manager/crud.rs:569` | Standard table insertion inside array of tables produces invalid TOML syntax. | **Fix-Now (Wave 1)** |
| **B8-03** | **P1** | `src/config/agent_manager/crud.rs:474` | `validate_filename` permits `/` and `\` path traversal across nested dirs. | **Fix-Now (Wave 1)** |
| **B8-04** | **P1** | `src/config/agent_manager/agent_files.rs:68` | Lexical path containment allows reading/writing through symlinks outside agent dir. | **Fix-Now (Wave 1)** |
| **B8-05** | **P1** | `src/config/agent_manager/toml_ops.rs:62` | 5-second sleep spin-loop and blocking fsync run on async Tokio thread. | **Verify-Then-Fix** |
| **B9-02** | **P1** | `src/config/save.rs:257` | `preserve_external_sections` resurrects omitted `Config` sections, preventing deletion. | **Fix-Now (Wave 1)** |
| **B9-03** | **P1** | `src/config/patcher.rs:383` | Holds `RwLockReadGuard` across async PTY manager deactivation (deadlock hazard). | **Fix-Now (Wave 1)** |
| **B9-05** | **P1** | `src/config/patcher.rs:572` | `check_conflict` permanently locks patcher when mtime changes externally. | **Verify-Then-Fix** |
| **B9-06** | **P1** | `src/config/patcher.rs:846` | `deep_merge` inserts `null` into Config fields, breaking serde deserialization. | **Fix-Now (Wave 1)** |
| **B10-01**| **P1** | `src/config/tests/serialization.rs:425` | `test_embedding_survives_save_incremental` simulates check in-memory without testing file save. | **Fix-Now (Wave 5)** |
