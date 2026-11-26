# Shell Capability 重构验证 / Shell Capability rework verification

- 对照基线 / Product baseline: `7fb540b444e7aa4223a70147849a3dff374a5b3b`；受验产品提交 / verified product commit: `fdfa1077d`。两边使用相同的 `plugins` / `skills` Git 子模组。 / Both used the same pinned and initialized submodules.
- 设计与计划 / Design and plan: [design](../specs/2026-11-26-shell-capability-rework-design.md), [plan](../plans/2026-11-26-shell-capability-rework.md).
- 证据范围 / Evidence scope: isolated Linux worktree, HOME, ALEPH_HOME and loopback fixture ports. No main-branch merge or push.

## Stage 2 决策：不开放通用续写 / Stage 2 decision: no generic follow-up stdin

| 必须证明 / Required proof | 现状与决定 / Finding and decision |
| --- | --- |
| 命令策略等价 / Command-policy equivalence | `src/sandbox/factory.rs` 在配置关闭可调策略后仍挂硬性规则。`src/sandbox/command_policy/mod.rs::command_text` 和 `src/sandbox/security_kernel_hook.rs` 检查启动命令，含随启动提供的 stdin；它们没有已运行解释器的完整解析状态，也不消费后续字节流。对每块数据单独审批，不能替代这一约束。 / The hard-block hook remains mounted when tunable policy is disabled. The hooks see the launch command and any prebuilt stdin, not the interpreter's subsequent state or stream. Per-chunk approval is not an equivalent command floor. The existing safe `custom_patterns_see_a_script_arriving_on_stdin` regression passed; it checks the initial-request predicate only, not any ability to police later writes. |
| 审批后实时授权 / Fresh authorization after approval | 本次异步任务只继承原始启动所需的上下文；观察操作重新核对当前完整会话身份。这不是写操作的授权来源，没有把启动授权或 PID 当作授权令牌。 / The detached task inherits only the context for its original admission. Observation rechecks the current full session identity; neither launch authority nor a PID is a grant to write. |
| 原生所有权 / Native containment | 平台路径确认的是 driver 拥有的直接子进程。Linux 的 `--unshare-pid` 依配置而定；不能据此把所有配置或平台承诺成整棵进程树。 / The verified acknowledgement is for the driver-owned direct child. Linux PID namespace use is conditional and is not a cross-platform or process-tree guarantee. |

未添加 `keep_stdin_open`、`write_stdin`、`close_stdin`、`input` 或未连接的 writer。启动脚本通过初始 stdin 的既有方式不变。 / No `keep_stdin_open`, `write_stdin`, `close_stdin`, `input`, or dormant writer ships. Existing startup-script stdin remains supported.

## 验证记录 / Verification record

| 检查 / Check | 结果 / Result |
| --- | --- |
| T1 real child regressions | 旧实现三个有界回归复现失败：初始输入与输出互相等待、提前退出后继承管道拖住 drain、初始写入不受执行时限约束。修复后 Linux Core common 27、CodeExec 40、CodeCheck 19，零失败。 / All three regressions failed on the old common runner; after the fix the named Linux suites passed 27, 40, and 19 cases respectively. |
| Linux Core focused suites | 15 组过滤器均通过：334 次执行，323 项不同测试，零失败、零忽略、无 Rust 编译告警。包括 Bash 36、registry 36、workspace 45、common I/O 27、live tail 18、partial-output filter 13、journal 30、浏览器 companion 37，以及现有目录描述字节上限。`yield_during_approval_then_cancel_never_reaches_the_driver` 经过真实 workspace 审批请求，证明 `starting` 后取消不进 driver。 / All 15 filters passed: 334 executions, 323 distinct tests, no failures, skips, or Rust warnings. The new approval-parking regression observes `starting` through the real Workspace gate, then proves cancellation prevents entering the driver. |
| Native Linux Gateway session | `fdfa1077d` 构建的 `aleph-server` 通过隔离的真实 Gateway、Agent 工具循环、Workspace sandbox 和 Bubblewrap；仅模型传输用固定回包。快速命令在首批等待内返回最终 stdout；长命令返回 `running`，续读无重复，终态仍接受原游标；`kill` 先返回 `stopping`，确认后 `wait` 返回 `killed`。两条原生子进程记录分别证明了主动停止和 SIGTERM 关机回收：关机前后一条 PID 从存在到消失，最终 journal 标记为 `killed`。 / The binary built from `fdfa1077d` exercised the real Gateway, Agent tool loop, Workspace sandbox, and Bubblewrap with a scripted model transport. It verified fast completion, native `running`, non-repeating cursor reads through settlement, and `stopping` followed by `killed`. Journaled native PIDs and `/proc` confirmed explicit cancellation and daemon SIGTERM cleanup. |
| Linux workspace and capability integration | `cargo test --locked -p alephcore --test exec_workspace_jail --test sandbox_capability_approval -- --test-threads=1`：两组各 6 项，零失败。 / Both integration suites passed all 6 cases each. |
| Wiring and formatting | Rust formatting and `git diff --check` pass; exec/executor/tool/seam/Gateway audits pass. `just check-wiring` has the same three RPC records (`first`, `second`, `unanswered`); `just check-doc-wiring` has the same six unrelated capability-symbol records. Both failures are byte-for-byte identical to the pinned baseline. / No new recorded wiring failures. |

原始日志、固定回包脚本和 Gateway 回执保存在本地隔离工作树的 `.superpowers/sdd/2026-11-26-shell-capability-rework/`，不进入版本控制。 / Raw logs, scripted model fixture, and Gateway receipts are in the isolated worktree's untracked `.superpowers/sdd/2026-11-26-shell-capability-rework/`.

## 未覆盖 / Not covered

- macOS、Windows 真机和交叉编译未执行；Linux 结论不外推到其他平台的进程树或隔离能力。 / No native macOS/Windows run or cross-compile, and no claims about their process trees or containment based on Linux evidence.
- 复审子会话因 Pi 宿主包缺少文件未启动；方案阶段审查不充当实现终审。 / Pi's final-review child could not start because its host package was incomplete. The design review is not an implementation approval.
