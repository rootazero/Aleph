# Sandbox execution boundary — verification record / 沙箱执行边界验证记录

Design: [approved design](../specs/2026-11-25-sandbox-execution-boundary-design.md) · [implementation plan](../plans/2026-11-25-sandbox-execution-boundary.md). The production changes and regression tests were committed together in `6b82702f6` on `sandbox-capability-refactor`. This is a dated verification record, not the current architecture reference. / 本文是时点证据，以 [SANDBOX.md](../../reference/SANDBOX.md) 为当前架构说明。

## Reference comparison / 参考实现对比

The Graphify indexes located the main paths; the comparison below was checked against source at the indicated refs. / Graphify 用于定位，结论以所列版本源码为准。

| Area / 维度 | `sandbox-runtime` `f7a0f19` | `codex` `1885417cd4` | Aleph change / Aleph 改动 |
|---|---|---|---|
| Policy and launch / 策略与启动 | `src/sandbox/sandbox-manager.ts` coordinates platform policy, launch wrapping, and cleanup. | `codex-rs/sandboxing/src/manager.rs` transforms permission inputs into a platform launch; `codex-rs/protocol/src/models.rs` carries typed profiles. | Reuse `WorkspaceSandbox::prepare_command` and mounted drivers. The trusted checkout determines policy root; validated `cwd` determines launch location. |
| Backend failure / 后端故障 | `src/sandbox/linux-sandbox-utils.ts` distinguishes hard dependencies from warnings. | `codex-rs/core/src/tools/orchestrator.rs` owns retry policy above the sandbox boundary. | Separate known unavailability, disabled posture, policy refusal, and native launch outcome. Do not add an unsandboxed retry. |
| Runtime ownership / 运行期所有权 | `src/sandbox/sandbox-manager.ts` owns cleanup for launch-related resources. | The sandbox manager accepts explicit workdir and execution policy instead of deriving the policy from display state. | Carry a Git checkout lease across actual execution and cancellation. Do not store it in durable project paths or the passive active-run snapshot. |

## Verification / 验证

Host: macOS 27.2.1 (26B131), Darwin arm64. Commands ran in the isolated `sandbox-capability-refactor` checkout. / 以下结果来自隔离工作树；各过滤组可能重复计数，不相加。

| Verification / 验证 | Result / 结果 |
|---|---|
| Test-first owned-worktree slice | Before connecting preparation and carrier: three behavioral failures (policy root followed child `cwd`, removed checkout was recreated/launched, carrier lost ownership), one existing grant control passed. |
| Core build gates | `cargo test -p alephcore --lib --no-run` and `cargo check -p alephcore --all-targets` passed. `cargo test -p alephcore --bins` passed all 151 tests. |
| Library test filters `sandbox::`, `owned_worktree`, `subagent_spawner`, `teams::dispatcher`, `the_harness_spawn`, `operating_envelope`, `run_slot_releases` | 544, 9, 58, 107, 5, 14, 1 passed respectively; no failures. The explicit native test is ignored by the normal filters and was run separately. |
| Tool and gateway filters `bash_exec::`, `code_exec::`, `code_check::`, `browser_tools::companion`, `busy_queue::`, `execution_engine::steering`, `resume_coordinator::` | 34, 43, 19, 37, 62, 29, 46 passed respectively; no failures, one pre-existing Code Exec test ignored. |
| Additional library filters `orchestrator::`, `agents::subagent_tool` | 319 and 115 passed respectively; no failures. |
| Gateway execution-engine group | All 511 passed after populating the pinned `skills` and `plugins` submodules and rebuilding. The earlier binary compiled without bundled skills failed the command census; the census itself is unchanged. |
| Focused integration with `test-helpers` | All 15 passed: `cancellation_chain` (3), `exec_workspace_jail` (6), `subagent_deps_inherit` (1), `worktree_isolation` (5). |
| Full integration compile with `test-helpers` | Blocked by E0432 in `tests/resume_coordinator_integration.rs:1361`: the test imports `set_channel_config_snapshot`, which is already absent on base `f4864b17e`. Neither that test nor the channel policy was changed in this patch. |
| macOS CLI diagnostics | `sandbox-debug --show-summary` reported `macos/seatbelt (workspace-write)` and `Network: denied`. `sandbox-debug --network none -- /usr/bin/true` returned `exit_code: Some(0)`. Both used an isolated `ALEPH_HOME`; this exercises the ordinary CLI entry, not Team or subagent dispatch. |
| `owned_worktree_seatbelt_native_write_boundary --ignored` | Passed. Real linked checkout, child `cwd`, injected `CARGO_TARGET_DIR`, successful write to `<checkout>/target/sandbox-smoke`; denied write left no sentinel in the parent checkout outside system scratch. This is not an actual Cargo build or a claim about two checkouts inside the same writable scratch area. |
| Rust formatting and lint | `cargo fmt -p alephcore -- --check`, `git diff --check`, and `cargo clippy --workspace --all-targets` passed. Clippy emitted warnings, none located on changed lines. Workspace-wide formatting reported 33 files, none in this patch. |
| Independent review / 独立审阅 | Rechecked lock release before the harness's first poll, non-owning active registry and abort cleanup, persistence/recovery, and lease lifetime. A final follow-up found no blocker in ordinary Team delegation after restoring its member-default fallback. |

## Evidence boundaries / 证据边界

- The production path reuses scoped Tool Capabilities; there is no new model-selectable sandbox backend. / 复用既有 Tool Capability；无模型可选的新后端。
- A recording driver checks inputs and ordering, not kernel enforcement. Native Seatbelt evidence applies to the exact launch and file effects tested. System scratch allowances are unchanged; this does not prove independent per-worktree scratch or complete native process-tree reap. / 记录型驱动验证参数和顺序，不代表内核执行；Seatbelt 证据只覆盖该次启动和文件效果，不能推导临时目录完全隔离或原生进程树已全部回收。
- The Team and subagent producer tests, worktree integration tests, and Seatbelt native launch check cover separate boundaries. No single test drives a complete Team or subagent turn through an LLM tool call into the native driver. / 入口、工作树生命周期和 Seatbelt 真机执行分别有证据；没有单条测试覆盖完整的「Team 或子代理 → 模型工具调用 → 原生驱动」链路。
- Linux and Windows were not tested on native hosts. There was no performance benchmark comparison or sandboxed Cargo build. / 未在 Linux、Windows 真机验证，未做前后性能基准比较或沙箱内 Cargo 构建。
