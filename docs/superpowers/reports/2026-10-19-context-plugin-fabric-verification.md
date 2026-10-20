# Context Plugin Fabric Verification / 上下文插件验证记录

本记录对应隔离分支 `codex/context-fabric-plugin`，起点 `79a2494f6`。源码修改与提交均在 `D:/Workspace/Aleph/.worktrees/context-fabric-plugin`；没有向 main 应用本次源码变更。

This report covers the isolated Context Plugin Fabric branch, not an upstream Pi plugin compatibility or token-savings benchmark.

## Delivered path / 已连通的路径

- Shared wire contract → mounted context Capability declarations → explicit host/session grants → the existing registry → dynamic WASM handler resolution → ContextFabric → root/child preflight, proactive/reactive/rescue/manual compaction and scoped tool-result admission.
- Builtin remains the default. Replace avoids a duplicate native summary; Compose/Interceptor use ordered stages. Budget, fingerprint, grant/instance changes and cancellation are checked before installing output; invalid external results use bounded native fallback.
- Original tool output, fold records and recovery reuse the existing result store and `session_events`. Legacy owner=None remains builtin. Foreign folds without verifiable original runtime identity recover through explicit degraded Core recovery; a same-named new plugin is not substituted for the historical producer.
- Provider replay policy is bound to the actual response-producing request. Ordinary hidden thinking is dropped from persistence/public answers; required protocol fields remain Core-only. Plugin snapshots exclude Thinking and tool signatures. Core reattaches private fields only to retained original frames whose public content and tool identity match.
- Actual context WASM mounts disable WASI, suppress guest logging/dumps, apply memory/fuel/deadline limits and reject ambient effects. A bounded blocking permit lives until native execution really exits, including cancellation of the awaiting future.

核心实现复用既有插件系统、事件库、索引与 compactor；没有新增 JS 宿主、MCP server、第二份事件真源或生产挂载 ZahirFacade。

## Validation setup / 验证设置

Native Windows; package `alephcore` v26.9.1. Cargo uses `CARGO_BUILD_JOBS=4`, `CARGO_PROFILE_DEV_DEBUG=0` and one shared worktree target directory. Before each Cargo command, `Win32_OperatingSystem.FreePhysicalMemory` must be at least 4,194,304 KiB; otherwise the runner rechecks after five seconds. Earlier default-debug test builds exhausted LLVM memory, so their timeout/OOM is not represented as a test pass.

The final runner is `.superpowers/sdd/2026-10-19-context-plugin-fabric/validate-final.ps1`; its versioned logs and exit files are local execution evidence. Filters overlap and must not be summed as distinct tests.

## Executed checks / 已执行检查

Focused checks and strict compilation/lint checks passed on the final tree. Repository-wide acceptance remains blocked by the 16 failures listed below; this report does not call the full suite green.

| Command / 检查 | Actual result / 实际结果 |
|---|---|
| `cargo test -p aleph-slot-api --lib ai_reasoning` | 27 passed |
| `cargo test -p aleph-slot-api --test ai_compat` | 12 passed |
| `cargo test -p alephcore --lib context` | 852 passed |
| `cargo test -p alephcore --lib session::projection` | 17 passed |
| `cargo test -p alephcore --lib signature_only` | 3 passed |
| `cargo test -p alephcore --lib harness::tests::budget` | 2 passed |
| `cargo test -p alephcore --lib extension::manifest` | 163 passed |
| `cargo test -p alephcore --lib extension::registry` | 42 passed |
| `cargo fmt --check` | Passed, including the final isolated rerun |
| `cargo check -p alephcore --lib --tests` | Passed |
| `cargo clippy -p alephcore --lib -- -D warnings` | Passed after removing the unused private helper and redundant attribute |
| `cargo test -p alephcore --lib bytes_ratchet` | 5 passed |
| `cargo test -p alephcore --lib builtin_tools::plugin_manage` | 8 passed |
| `cargo test -p alephcore --lib extension::runtime::wasm` | 60 passed |
| Full `cargo test -p alephcore --lib` | **21,492 passed, 16 failed, 25 ignored**, 313.20 s |

Native R10 measurement is 12 production files / 5,535 lines, down from the branch point's 5,570. The existing ceiling was lowered from 5,545 to 5,535; no spare headroom was introduced. Tool-description/schema byte ceilings were not raised: description bytes are 117,900; registry schema bytes are 107,358 across 61 tools, within the existing 107,406-byte schema ceiling. Non-catalogue schema bytes are 10,253. These are prompt-footprint ratchets, not a workload compression benchmark.

## Full-suite failure accounting / 全量失败归因

The v5 full suite reported 21,480 passed, 28 failed and 25 ignored. Twelve failures were resolved before v8; no new ignore or skip was added:

- Missing `skills`/`plugins` submodules were initialized at their pinned commits. Four failures explicitly reported an empty checkout, including the bundled skill census and installer validation.
- The context feature's tool-catalogue/schema growth was addressed by removing repeated argument documentation, rather than raising byte limits. The ordinary WASM unload fixture was corrected to match the existing stale-binding error contract.
- Four telecom source-census failures compared Windows backslashes with slash-based source names. Only their test-path rendering/exclusion was normalized; exact caller counts and expected files remain unchanged.
- The offload-marker test now compares the recovery handle using the same raw-versus-JSON-escaped representation as its supplied input. It still asserts no second persistence, retention of the original blob, reduction of surrounding text and a complete recovery handle.

The remaining 16 failures are:

A read-only comparison against `79a2494f6` found no diff in the source files that own the three HOME tests, the MCP fixture, the project-skill test, or any of the eleven PTY/terminal tests. The two independent failure reviews also found no Context Fabric call path in those tests. This is evidence against a regression, not proof that the environment is healthy.

| Group / 分组 | Count | Evidence / 证据 |
|---|---:|---|
| File-operation HOME refusal guards | 3 | `src/builtin_tools/file_ops/path_utils.rs:1740`, `:1781`, `:1878`; each fails again when run alone. Native `tempdir()`/`canonicalize()` fixtures were confirmed, so the Unix-path-fixture hypothesis was rejected and no refusal assertion was weakened. |
| MCP executable-path fixture | 1 | `src/extension/mcp_config/tests.rs:371`; `/usr/bin/python3` resolves to absent `C:/usr/bin/python3` on this host. Fails alone too. |
| Project-skill classification | 1 | `src/gateway/execution_engine/run_loop/tests.rs:3291`; global skills unexpectedly produce a project block. Fails alone too; cause not established. |
| Live PTY/screen/foreground/session tests | 11 | `src/builtin_tools/terminal/tests.rs`, `src/gateway/handlers/pty.rs`, `src/gateway/pty/{foreground,manager,session}.rs`, `src/gateway/runtime/tests.rs`. Representative `a_child_write_reaches_the_server_held_screen` also fails alone after 6.28 s. Independent review found the common symptom is that ConPTY child bytes do not reach the server-held screen/OSC7/title; the owning files are unchanged. |

The six representative reruns each matched and executed one test; none turned green in isolation. Their log is `validation-residuals.log`. The same runner also re-executed the 27 reasoning tests, 12 compatibility tests and final format check successfully.

所有剩余失败的源文件与 `79a2494f6` 比较均未改变；但没有在该旧提交重新构建并运行，因此不能据此声称全部是已证明的历史基线。HOME、项目技能与其余 PTY 原因仍待定位，未将其归为无害失败，也未修改相关生产逻辑以凑全绿。完整验收仍受阻。

Remaining failing test names / 剩余失败名称：

```text
builtin_tools::file_ops::path_utils::tests::a_home_entry_compiled_under_one_home_is_not_served_under_another
builtin_tools::file_ops::path_utils::tests::a_home_with_glob_metacharacters_is_still_a_literal_location
builtin_tools::file_ops::path_utils::tests::code_built_entries_are_literal_locations_under_any_home
builtin_tools::terminal::tests::explain_reads_the_live_session_screen
extension::mcp_config::tests::bare_and_absolute_commands_are_kept_as_written
gateway::execution_engine::run_loop::tests::project_skill_block_none_when_only_global_skills
gateway::handlers::pty::tests::spawn_with_no_cwd_actually_chdirs_the_child_into_the_first_root
gateway::runtime::tests::a_changed_sample_reaches_the_bus_an_unchanged_one_does_not_and_exit_publishes_once
gateway::pty::foreground::tests::a_real_child_is_reported_as_the_foreground_program
gateway::runtime::tests::cwd_prefers_osc7_then_foreground_then_spawn
gateway::pty::session::tests::a_child_write_reaches_the_server_held_screen
gateway::runtime::tests::a_session_that_exits_leaves_the_table
gateway::pty::manager::tests::spawning_a_pty_journals_intent_then_pid_and_its_exit_settles_the_row
gateway::runtime::tests::a_real_agent_started_after_spawn_is_identified
gateway::pty::manager::tests::the_intent_row_names_the_resolved_shell_and_a_close_records_the_screen
gateway::pty::session::tests::a_child_that_exits_settles_the_session_without_needing_terminal_eof
```

## Independent review / 独立复核

Focused independent reviews covered Task 1 admission, session-scoped registry/bundles/descriptor binding, fold ownership and hard-forget, root/child wiring, actual WASM mount/grant/revoke identities, WASI/log/dump restrictions, private protocol-frame restoration and observer Notify admission. Reviews of these repaired boundaries returned APPROVED. Static review is recorded separately from executable test results.

## Boundaries / 未做及边界

- The external executable phases in this version are Admission, Preflight, Compaction and Lifecycle. Recovery, general search and status/discovery retain their Core services. `session_shutdown` is a host-owned notification seam; turn_end does not destroy a persistent session.
- The fixture is a real Aleph WASM guest and normal mount/grant/invoke/revoke path. Upstream Pi/billion-context/context-mode JavaScript extensions are not loaded directly; they need an adapter to this contract.
- No live paid-provider request, process-level crash/restart exercise, macOS/Linux build, Panel interaction, release build or same-workload performance comparison was run in this verification.
- No 98% output reduction, 5× token saving, universal replay/exactly-once effect guarantee or complete detection of arbitrary unknown secrets is claimed.
