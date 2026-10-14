# Terminal capability parity reconstruction audit / 终端能力对齐重建审计

## Provenance and authorization / 来源与授权

- Base / 起点：`a5300221d27c7a916407d8b96cadccb91cde1bbf`。
- Branch / 分支：`feat/terminal-capability-parity`。
- Worktree / 隔离检出：`/home/zou/data/workspace/Aleph/.worktrees/terminal-capability-parity`。
- At session start the original four handoff commits and 2026-10-06 parity plans/audit were unavailable. The user subsequently confirmed that Windows A0–A2 had not yet been pushed, then authorized synchronization once it was available (m01275). 原始材料最初未同步；下面的重建记录是历史，不伪称原 Windows 日志。
- Original Windows baseline is now authoritative: `origin/main` at `3889702c5e209612a379092b803f250b146a1a80` was merged normally as `0bdd606dce0344937f190dd5f83ae60e06f02b05`. All four original commits, including A2 `1308d76791b241870e4bb079d92d21732884285b`, are verified ancestors. 本地重建生产实现已被原始 A0–A2 替换，不保留第二真源；仅移植 QA selector 与测试 helper 的 manager 借用/锁归属修补。
- User explicitly authorized rebuilding A0–A2 **from the continuation prompt's contracts**, then A3, without restarting design brainstorming. 用户明确授权依据续作契约重建前置，并完成 A3；A4 及后续阶段未授权。
- Verified backup: `/home/zou/data/workspace/Aleph-backups/terminal-parity-local-main-a5300221d.bundle` (807 MiB). It contains existing local history, not the missing handoff commits. 本地 bundle 不是旧交接成果的恢复。
- User additionally approved necessary `src/tools/adapters/mcp_adapter.rs`, `src/gateway/execution_engine/run_loop/inner.rs` and corresponding test wiring for A3, keeping `src/harness/` unchanged. Request-time `ToolCallRequested.identity` is not execution proof; gate proof and execution audit must bind the actually invoked handler. 请求 identity 与实际执行 identity 的审计边界须明确。
- User subsequently approved one additional production file, `src/orchestrator/harness_bridge/replay_adapter.rs`: preserve existing Safe/schema/fingerprint eligibility gates and add a separate sealed replay admission bound to captured identity, call ID and effective input. It must not mint human/operator dispatch proof or flow to child tasks. 用户确认单文件增补；不是扩大 harness 或实施 A4。

## Resource discipline / 资源纪律

Every Cargo command is preceded by `python3 qa/terminal/memory_guard.py -- ...`; minimum available physical memory is exactly **4_294_967_296 bytes**. Linux reads `/proc/meminfo` **MemAvailable** with Python standard library only; Windows reads `GlobalMemoryStatusEx.ullAvailPhys`. Unknown probes fail closed. 每次 Cargo 均真实物理准入；同步后的原始 guard 在低内存时每 5 秒重查，unknown fail-closed。这不是 reservation/watchdog，不提供运行期 RAM 保证。Windows ctypes mock 不等于 Windows 真机 PASS。

Cargo is controller-owned and serial, with process-local settings:

```text
CARGO_BUILD_JOBS=1
CARGO_PROFILE_TEST_DEBUG=0
CARGO_PROFILE_TEST_INCREMENTAL=false
CARGO_TARGET_DIR=<feature-worktree>/target/terminal-parity
```

Filters are listed before execution and must have nonzero matches. Overlapping counts are not added; ignored tests are not PASS. 过滤先 list，重叠不累加，ignored 不计通过。

Evidence is local under `.superpowers/sdd/2026-10-06-terminal-capability-parity/evidence/`; it is ignored execution assistance, not product persistence. 不向产品引入第二状态真源。

## Reconstruction evidence before Windows synchronization / 同步前重建历史实证

The following results describe the pre-sync reconstruction, not validation of the current original baseline or a completed A3. 下表不作同步后 PASS 复用。

| Stage / 阶段 | Actual result / 实际结果 |
|---|---|
| A0 initial guard | Controller unittest: exit 0, 36 PASS; `a0-unittest.log`. Initial missing-module RED was import failure, not behavioral RED. |
| A0 independent review | Reviewer found explicit separator with empty command and check-only+command success no-ops. Repair: 46 tests, 9 assertion failures (exit 1) before fix; controller fresh rerun 46 PASS (exit 0), plus real empty-command exit 2 and real child exit 3 smoke. Command exit-code ambiguity is distinguished by stderr result/command_exit markers; passthrough is retained. |
| A1 wire RED | Guarded `cargo test -p aleph-protocol --lib terminal::tests -- --list`: exit 101, 44 missing DTO errors; compile RED, no assertions ran. |
| A1 wire GREEN | Same list: exit 0, 16 hits. Actual `-- --test-threads=1`: exit 0, 16 PASS, 0 ignored, 371 filtered. Core response DTO wiring belongs to A2. |
| Shared/client baseline | Guarded `just test-shared` exit 0: shared-ui-logic 221 PASS in each feature shape; CLI 252 library + 3 integration PASS; protocol 387 PASS and 2 ignored docs; TUI 454 library + 1 CLI-definition PASS. Guarded `cargo test -p aleph-client` exit 0, 25 PASS, 0 ignored. |
| Core test build | Guarded `cargo test -p alephcore --lib --no-run`: exit 0, 38m 54s. This is compilation, not full library execution. |
| A2 lifecycle RED | List: exit 0, 1 hit. Actual run: exit 101, 1 failed, 0 ignored, 20675 filtered. `src/builtin_tools/terminal/tests.rs:825`: stale Blocked agent row + removed real PTY returned Reached instead of Gone. Fixtures use real PTY and Drop cleanup. |
| A2 GREEN / regressions | Commit `21909dfcd2ae839bfe806f274ca01dc07a24d7f2` (`terminal: converge observations over shared runtime`), normal commit, `git show --check` exit 0. Guarded stale-PTY: 1 PASS/0 ignored/20680 filtered; terminal module 25 PASS/0 ignored; direct `gateway::pty::runtime::tests`: 10 PASS/0 ignored; gateway::pty 138 PASS + 1 documented ignored; gateway::runtime 42 PASS; handlers runtime 3 PASS. A2 no-run after implementation exit 0. |
| A2 direct runtime coverage | Tests cover caller ownership matrix, cancellation, registry-first Gone against matching/nonmatching stale rows, default/empty until, timeout clamp/zero, and exact newline tail-12; guarded execution exit 0. |
| A2 global PTY census repair | Commit `ce41b25d21a6f8f62b676227fafd5c18ae410c41`: singleton access moved into tagged test bodies; helpers/Drop explicitly borrow that same manager. Guard rules and production behavior unchanged. Fresh guarded list/run: census 1 PASS; terminal 25 PASS; runtime 10 PASS, all exit 0/0 ignored. Normal commit and `git show --check` exit 0. |
| Full Core baseline / BLOCKED | Guarded full Core library execution exit 101: **20645 PASS, 27 FAIL, 19 ignored**, 1012.19s. Exactly three A3 draft failures; the remaining **24** required attribution, not automatic waiver. Subsequently four missing-submodule tests passed and the A2 global-PTY-test regression was fixed/tested (below). The other **19** failures remain recorded, not waived or reported PASS; no fresh full-suite GREEN is claimed. They include the existing harness budget (5615 > 5545); no harness fix is authorized. |
| A3 resumed / 重启授权后 | User authorized continued handling of baseline failures and A3. Initial draft rewrite and partial-write retry were behavior RED (exit 101); child draft first failed at parent approval, and two weak green drafts were not proof. Tests-only work then strengthened same-service replacement/handler identity, same-name parallel verdicts and child fixtures. The intermediate missing-API compile RED and missing-verdict assertion RED were subsequently resolved by the current implementation; see the current verification row below. |
| Checkout and bins / 检出与二进制 | Restored fixed gitlinks, not updated versions: skills `75db7ea32692c613afde17734c970803da8828fc`, plugins `b35ad47fe8eec3196b2dee70e51eadeacf3a620c`; init exit 0. Guarded `cargo test -p alephcore --bins` exit 0, 113 PASS, 0 ignored. Four missing-submodule failures now each have fresh guarded list exit 0/one hit and actual execution exit 0/one PASS: bundled config paths, self-skill keys, plugin installer validation, and side-question command words. These isolated reruns do not establish full-suite GREEN. |
| Real agent E2E / 实机观察 | `codex-cli 0.160.1` and Claude Code `2.1.281` run through real Aleph PTYs. `SKIP_BUILD=1 KEEP=1 QA_REAL_AGENT_NAME=codex bash qa/terminal/run.sh real` and corresponding `claude` run both exit 0, five checks each: direct binary and offline-staged npx wrapper identify correct agent/program, program is one word and contains no environment assignment. Server freshly built with guarded `cargo build --profile test --bin aleph-server`, exit 0. This tests real startup/foreground identification, not model task execution or A3 proof. |

Independent A0/A1 review found no substantive wire mismatch. Its Windows assessment is read-only/mocked, not platform validation. 审阅不替代命令结果。

Real-agent QA keeps HOME/ALEPH_HOME in task scratch directories; no model prompt was submitted and no global CLI configuration was changed. Startup may contact external services; zero network/cost is not asserted. The fixture had cleared the requested agent before reading it; `qa/terminal/run.sh` now preserves the requested selector before resetting result variables. Normal commit `1cab9cf20ec55438750345ec1417b14d37b37e42`; `bash -n` and `git show --check` exit 0. Codex spawned a scratch managed daemon; its PID, creation time and executable were verified before task-specific termination. No global process kill was used. 实机证据不代表 TUI/Panel 渲染、输入 grants、模型工作或 Windows 已验证。

| A3 focused GREEN / A3 聚焦绿 | Guarded `bash .superpowers/sdd/2026-10-06-terminal-capability-parity/verify-a3-green.sh` exited 0 after final changes. The script listed every filter first and required nonzero hits. Actual results: Core lib `--no-run` exit 0; A3 tests 11 PASS/0 ignored; registry 22 PASS/0 ignored; replay 11 PASS/0 ignored; boundary 15 PASS/0 ignored; retry 6 PASS/0 ignored; adapters 29 PASS/0 ignored; handlers 39 PASS/0 ignored; terminal 27 PASS/0 ignored; `gateway::pty::runtime::tests` 9 PASS/0 ignored; broader scoped 141 PASS/0 ignored. MCP bridge regression filter: 6 PASS/0 ignored. Fresh full Core library run: exit 101, 20669 PASS/18 FAIL/19 ignored; failures were retained as out-of-scope/baseline, not waived. Test-helpers no-run, bins (113 PASS), shared/client/panel, and Linux/Windows/macOS desktop checks exited 0. Workspace all-target clippy remains blocked by unchanged `interfaces/cli/src/commands/open_cmd.rs:118` dead code. A3 production changes remain uncommitted. Independent review `8795564a-c90c-47c` found no confirmed security/concurrency defect and made no test claim. This is focused A3 evidence only, not full Core/workspace/clippy/platform runtime or parity completion. |

## Original Windows integration / 原始 Windows 基线融合

- Pre-sync files and ignored evidence were preserved in `/home/zou/data/workspace/Aleph-backups/terminal-parity-pre-windows-sync-1cab9cf20.T2Fp2l`; SHA-256 verification passed. Protection branch and the A3 stash are retained. Main worktree was not merged or edited.
- Seven expected independently-reconstructed-file merge conflicts were resolved using original Windows A0/A1/A2. The original guard already rejects empty commands; the reconstruction-only guard API and DTO/runtime implementations are not retained.
- Fresh original-baseline guard: 15 PASS, exit 0. Guarded Core `--lib --no-run`: exit 0, 15m26s. This compiles tests, not full-suite execution.
- Original A2 had a real global-PTY census failure too: runtime fixture construction/Drop and terminal test adapters obtained the singleton outside attributed test bodies. Fresh list: one hit; actual RED exit 101. Explicit manager dependency injection in cfg-test code and one missing lock tag repaired it; production prefixes and guard rules were unchanged.
- Fresh list-before-run GREEN, all exit 0: census 1; runtime 9; terminal 27; PTY 147 PASS + 1 ignored (148 hits); exact `gateway::runtime::` 25; exact runtime handler 1; protocol terminal 5. These overlapping counts are not added; ignored is not PASS. Evidence: `windows-sync-*.log/.exit`.
- Merge commit `0bdd606dce0344937f190dd5f83ae60e06f02b05`, normal commit; `git show --check` exit 0. Upstream formatting/dependency commits are inherited, not independent local fixes. No local harness delta relative to upstream.
- A3 stash restored; one documentation conflict kept the original A2 reference because the reconstruction-only draft named a now-obsolete method. No unresolved conflicts. A3 is still uncommitted and compilation still fails at `src/tools/scoped/mod.rs:380`, `implementation of std::marker::Send is not general enough` (`&'0 str` versus `&'1 str`). No GREEN claim.
- Pre-sync independent concurrency/SideQuestion review findings require behavior tests on the integrated code. Full library suite, bins, test-helpers, shared siblings, workspace clippy and real-client QA must be reclassified/revalidated; prior failure counts and real-agent startup results remain historical, not fresh acceptance.

## Validation allocation / 验证安排

The seven-command discipline in `docs/reference/DEVELOPMENT.md` remains applicable; `cargo check` does not substitute for test compilation or execution. Historical handoff counts are not fresh results.

### Before A3 / A3 开工前

- Complete A2 GREEN and relevant terminal/PTY/runtime filters with actual Unix PTYs.
- Core lib test build (above), actual Core bins baseline, full Core lib execution and test-helpers integration compilation. Genuine baseline failures require recording and user arrangement, not unrelated repair or exemption.

### After A3 / A3 改动后

- Actual registry/scoped replacement, per-call proof, unsafe retry, Safe replay, boundary-repair, hook rewrite and concurrent approval regressions; Core library/bins and test-helpers compilation.
- Shared protocol/client/TUI/CLI and shared-ui-logic validations are required after replacing reconstructed A1 with the original synchronized contract. Historical A1 PASS is not reused.
- Workspace all-target clippy after declared shell placeholders, plus targeted formatting and diff checks. Unrelated formatting blockers must not be silently widened or bypassed.

### Later client/platform stages / 后续客户端与平台阶段

Desktop/platform checks, Panel library checks, real TUI/Panel reachability and multi-platform QA remain outstanding and must be allocated explicitly. Deferred means **not PASS**, not exempt, and not full parity completion. Unix test coverage does not establish Windows process/PTY behavior.

## Remaining scope / 剩余范围

A4–A7, B1–B6 and C1–C6 are not implemented or authorized here. In particular production attach RPC wiring, grants/human raw input/agent control, process-reaping settlement, clients/renderers, BSP/workspaces/worktrees, recovery and full real-machine parity QA remain separate stages.

## Commit discipline / 提交纪律

The current common git directory has no `pre-commit` hook; absence does not mean checks passed. Do not create/modify hooks or use `--no-verify`. Normal commit attempts and their actual checks/results must be reported, and any source/hook blocker retained for explicit user arrangement.
