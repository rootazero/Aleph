# Severed-Wire Audit — `src/utils`

- **Batch:** agents-batch-6
- **Module:** `src/utils` (17 files, 3971 LOC)
- **Date:** 2026-08-16
- **Reviewer:** static (severed-wire-audit skill)
- **Method:** 5-phase severed-wire audit. Scan seams (registration, call-vs-handler, event emit-vs-subscribe, config-reader, path/route, stub sweep), enumerate `DEFINED − CONSUMED`, then **read-first triage** every candidate by grepping the consumer side for a live caller before deciding CONNECT / CUT / DECIDE.

## Result summary

| Severity | Count |
|----------|-------|
| critical | 0 |
| high | 1 |
| medium | 4 |
| low | 2 |
| **total** | **7** |

| Decision | Count |
|----------|-------|
| CONNECT | 0 |
| CUT | 6 |
| DECIDE | 1 |

| Category | Count |
|----------|-------|
| security | 0 |
| logic | 0 |
| architecture | 1 |
| quality | 6 |

This module is a utilities layer, so it has no RPC dispatch arms, event buses, or classifier tables. The dominant severed-wire form here is **form 1 / dead scaffolding** (producers with zero live consumers), which is the primary value the audit targets. All 6 CUTs were verified by grepping the entire workspace (not just `src/`) for a live caller before deciding.

One candidate that *looked* like dead scaffolding was reversed by the read-first rule: `VaultIo` (`src/utils/vault_io.rs`) appeared to have only test consumers, but a deeper grep revealed `src/secrets/vault.rs` imports it (`use crate::utils::vault_io::VaultIo;`) and drives every production vault `read`/`save` through it. **Not a finding.**

---

## Findings

### [HIGH] src/utils/one_or_many.rs:10 — `OneOrMany` / `OneOrManyIter` module is dead scaffolding
- **Category:** quality
- **Decision:** CUT
- **Description:** The whole module (148 lines: `OneOrMany<T>` with serde untagged `Deserialize`, `Default`, `From`, `one`/`many`/`iter`/`len`/`is_empty`/`first`, plus the `OneOrManyIter` iterator) is referenced **only** inside its own file and in the `pub use one_or_many::OneOrMany;` re-export (mod.rs:20). The header says it "replaces `rig::OneOrMany` to remove the rig-core dependency", but the rig dependency was removed without migrating any consumer onto the replacement — the type was left orphaned. No config field, handler, or caller anywhere in the workspace (src/, interfaces/, desktop/, shared/, tests/).
- **Suggested fix:** Delete `one_or_many.rs`, its unit tests, and the `pub use one_or_many::OneOrMany;` line in `mod.rs`. Not re-exported at crate root, so no wider public-API break.

### [MEDIUM] src/utils/text_format.rs:9 — `format_timestamp` has zero callers
- **Category:** quality
- **Decision:** CUT
- **Description:** Whole-workspace grep finds only the definition. The `format_timestamp*` symbols in `interfaces/webchat` are local, unrelated private helpers. No test exercises it either.
- **Suggested fix:** Delete `format_timestamp`. (The five `truncate_*` helpers in the same file are all live — leave them.)

### [MEDIUM] src/utils/text_format.rs:125 — `escape_markdown` has zero callers outside its tests
- **Category:** quality
- **Decision:** CUT
- **Description:** Grep finds only the definition and its two unit tests. No production path escapes markdown through this helper; prompt/telegram code uses its own inline escaping.
- **Suggested fix:** Delete `escape_markdown` and its two tests.

### [MEDIUM] src/utils/paths.rs:242 — `get_memory_db_path` has zero callers
- **Category:** quality
- **Decision:** CUT
- **Description:** Grep finds only the definition. The memory subsystem resolves its directory via `get_note_memory_dir()` (many live callers), and stores open via `utils::sqlite_open::open_sqlite_safe` on explicit paths. This is a side-effecting resolver (`create_dir_all`) with no reader.
- **Suggested fix:** Delete `get_memory_db_path`.

### [MEDIUM] src/utils/paths.rs:257 — `get_skills_dir_string` is dead (its UniFFI consumer was removed)
- **Category:** quality
- **Decision:** CUT
- **Description:** Documented "for UniFFI export", but UniFFI was removed (`Cargo.toml:105` — "UniFFI removed - using Gateway WebSocket architecture"; no `.udl`, no uniffi dep). Grep finds it only at its definition and in the crate-root re-export `pub use crate::utils::paths::{get_skills_dir, get_skills_dir_string};` (lib.rs:247). `get_skills_dir` in that same re-export **is** live.
- **Suggested fix:** Remove `get_skills_dir_string` (the function and its name in the lib.rs re-export list), keeping `get_skills_dir`.

### [LOW] src/utils/instance_lock.rs:38 — unused public methods `lock_path` / `holder_pid` / `into_file`
- **Category:** quality
- **Decision:** CUT
- **Description:** Grep for `.lock_path(`, `.holder_pid(`, `.into_file(` finds no caller anywhere. `rewrite_holder_pid` (line 62) is live (bin/aleph-server/main.rs:212, the daemonize path) and writes the `holder_pid` field internally, but nothing ever reads `holder_pid()` back; `lock_path()` and `into_file()` are likewise never called. Speculative public API with no consumer.
- **Suggested fix:** Delete the three methods; keep `rewrite_holder_pid` and the internal field.

### [LOW] src/utils/atomic_io.rs:21 — two atomic-write primitives with divergent semantics
- **Category:** architecture
- **Decision:** DECIDE
- **Description:** Not a severed wire — both `atomic_io::write_atomic` (sync) and `atomic_write::atomic_write_file` (async) have many live callers — but a design smell. The async helper preserves the destination's existing permission bits before rename (with a comment + test guarding the executable-bit case); the sync `write_atomic` does not, so overwriting an executable file through it silently strips the mode. Today no `write_atomic` caller targets an executable (vault sidecars, JSON stores, pid files — all data), so it is latent, not a live bug.
- **Suggested fix:** Product/architecture call: (a) add permission preservation to `write_atomic` for parity, (b) document the divergence, or (c) consolidate the sync/async pair. Present the trade-off rather than silently picking.

---

## What was verified clean (not findings)

- `VaultIo` — **reversed**: appears test-only, but is the production vault persistence layer (`src/secrets/vault.rs` uses it for every `open`/`save`).
- All five `truncate_*` helpers — live (many callers).
- `extract_json_robust` — live (memory notes pipeline).
- `sanitize_filename` / `MAX_FILENAME_CHARS` / `FALLBACK_FILENAME` — live (artifacts store, media cache).
- `no_window` — live (MCP/ACP/sandbox/runtimes spawns).
- `sqlite_open` (safe + readonly) — live (many stores).
- `scratch_root` / `keep_until_exit` / `reap_on_exit` — live (test harnesses; `reap_on_exit` used by probe harnesses in tests/).
- `fifo_cache::remember` — live (gateway event_visibility, teams broadcast).
- `panic_message` — live (subagent_spawner).
- `is_path_within` — live (skill_reader).
- `process_alive` (`is_process_alive` / `process_start_time` / `process_matches`) — live (instance_lock, daemon).
- `instance_lock` (`try_acquire` / `diagnose_holder` / `rewrite_holder_pid` / `AcquireOutcome` / `HolderDiagnostic`) — live (main, cli/policy, stale_lock diagnostic).
- `paths` (`equivalent`, `display_string`, `get_home_dir`, `get_config_dir`, `get_data_dir`, `get_note_memory_dir`, `get_*_db_path` (except memory), `get_scratchpad_bindings_path`, `tool_usage_path`, `get_background_*_dir`, `get_runtimes_dir`, `find_git_root`, `get_all_skills_dirs`, `get_agent_config_dir`, `migrate_legacy_db_files`, `legacy_home_aleph_path`, `private_temp_root`, plugin-skill-dir publish/read) — all live.
- Stub sweep: no `todo!` / `unimplemented!` / `// TODO` / `FIXME` in any of the 17 files.

## Negative (explicitly not done / not covered)

- READ-ONLY: no source was edited; only `summary.json` and `REPORT.md` were written.
- No CONNECT findings: no candidate had a live caller AND a genuinely dark wanted feature.
- Not verified by compilation (`cargo test --no-run`) — the CUT recommendations should be compile-checked by the implementer, since deleting `OneOrMany` / `get_skills_dir_string` touches public API and re-exports.
- Cross-references outside `src/utils` were used only for caller verification and `related` links; all findings are anchored in `src/utils`.
- The `graphify-out` graph.json was not relied on for completeness (only as a navigation hint), per the skill's guidance.
