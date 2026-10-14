# Task 48 — managed `browser_companion` (PARTIAL)

**Status / 状态：PARTIAL。** 生产 registry 工具 + 沙箱 managed seam + endpoint 握手 + 生命周期加固已实现，并经独立 verifier 复验（最终证据见文末「最终验证」）；**Windows 上 managed spawn 显式 UNSUPPORTED**（包装进程 PID 不是服务 PID），无 managed E2E / 无握手 QA fixture，`managed-companion` selector 仍 BLOCKED；macOS 未编译、Linux unsupported。不声称 production ready，也不是整分支批准。
Production-registry tool, sandbox managed seam, endpoint handshake and lifecycle hardening are implemented and independently verified (final evidence in "Final verification" below). Managed spawn is explicitly UNSUPPORTED on Windows because the wrapper PID is not the service PID. No managed E2E and no handshake-speaking QA fixture; the `managed-companion` selector stays BLOCKED. macOS not compiled, Linux unsupported. No production-ready claim and no whole-branch approval.

## 实现 / What was built

- **工具 / Tool**: `src/builtin_tools/browser_tools/companion/` — `browser_companion {start|status|stop}`，`deny_unknown_fields`；program+argv（无 shell），argv/cwd/超时/生命周期有界；status/stop 只收不透明 `companion_id`，且 session + run + principal 必须与启动者一致（外人得到与"未知 id"相同的答复）。接入真实 registry：`definitions.rs`、`groups.rs`、`struct_def.rs`、`tool_registry_impl.rs`、`constructor/mod.rs`（`.with_approval_policy`）。
- **沙箱 / Sandbox**: `src/sandbox/managed.rs`（`ManagedSandbox`）；`WorkspaceSandbox::execute` 拆出 `prepare_command`，managed spawn 复用**同一**审批、二次 cwd 复核、proxy/DNS、profile 生成。无直接 `Command`。
- **进程表 / Process table**: 扩展既有 `ProcessRegistry`（无第二张表）：managed 条目由 supervisor 任务持有 `Child` 直到 `wait`；`cancel_managed` 仅在 supervisor 回执 `wait` 返回后报告 `Reaped`；`kill()`/`bash process kill`/`shutdown()` **不会**中止 managed 条目（`KillOutcome::Managed`）。
- **握手 / Handshake** (`protocol.rs`)：父进程随机 launch secret 仅经 child stdin 一行下发（不进 argv/env/config/日志，`Zeroizing`）；child 在 ≤4 KiB 的 ready 行报告 endpoint + HMAC；Gateway 对**显式配置**的 endpoint 发全新 challenge，要求 HMAC 应答（无重定向、无代理、状态 200、响应 ≤4 KiB、前后检查 child 存活、常量时间比较）。`EndpointProof` 私有字段、不可反序列化，只能由校验函数生成。
- **发布 / Publish**: `BrowserPresentOperation::from_managed_operation(record, launch)`（`present.rs`）；target 仅取自真实 `open_tab_with_record` 的 `BrowserOperationRecord`；`from_launcher_record` 现为 `#[cfg(test)]`。发布 `ManagedTool` / `DesignCompanion` / `ExplicitOnly` / `Pending`，无 candidates；PID 只来自 driver 声明的服务 PID。
- **终止顺序 / Terminal order**（stop、自然退出、lifetime、守护进程关闭同一条 `finish`）：关闭 resource（撤销 grant、发 close 事件、退役 owner 路由）→ 关 tab → 终止 child 并等待真实退出。回收失败 ⇒ `stop_failed` 并报错，不报成功；启动失败按同序回滚。`aleph-server` 的优雅关闭与强制退出路径均在 bash reaper 之前调用 `shutdown_all_companions()`（有界）。
- **私网 / Private nets**: 无 NetworkGrant；`ProfileManager::check_navigation` 默认拒绝回环/私网（仅 QA 配置可设 `block_private=false`）。
- **Windows**: `WindowsSandboxDriver::managed_spawn_preflight` 返回 `UnsupportedPolicy`——`sandbox-init-windows` 不上报真实服务 PID，包装进程 PID 不能冒充；未关闭 restricted token。工具在任何审批卡片之前即失败。
- **macOS**: `SeatbeltDriver::spawn_managed` 已写（`sandbox-exec` 会 exec 目标，PID 即服务 PID），但**本机无法编译，未验证**。Linux bwrap：unsupported。

## 验证 / Evidence (Windows, `-p alephcore`, jobs=1, debug=0, offline)

日志 / logs: `.analysis/browser-live-runtime/task48-{check1,check2,clippy1,clippy2,test1..test5}.log`

- `cargo check --lib --tests`：通过 / passes（check2）。
- `cargo clippy -p alephcore --lib --bin aleph-server --tests`：0 warning（clippy2，修复 1 条 `redundant_guards` 后）。
- 最终定向 `cargo test -p alephcore --lib -- browser_tools:: companion executor::builtin_registry session_mode process_registry sandbox::workspace::tests::managed managed_spawn launcher_without`：**483 passed / 0 failed**（test5）。涵盖：协议（畸形/超长/错误 endpoint/错误 nonce/challenge 绑定）、managed 注册表（kill 拒绝、`cancel_managed` Reaped/TimedOut/NotFound/他人 session、shutdown 跳过）、真实子进程 supervisor 取消+回收、沙箱 managed 审批（拒绝则不 spawn；批准后返回 driver 声明 PID；预置 stdin 被拒；默认 driver unsupported）、Windows driver 拒绝、manager 生命周期（成功发布并核对 target=收据、PID=driver PID；stop 顺序 resource→tab→reap；自然退出；回收失败 `stop_failed` 可重试；所有者校验；proof/open_tab/publish 失败回滚顺序；回滚回收不确定的报告；unsupported 不启动；无 producer 不启动；shutdown 顺序）、工具 schema/参数界限、registry/census 接线。
- catalog 描述上限 117_174→117_368 B 与"无无条件 schema"pin 134→135 已按文档化流程调整。
- 未运行 `cargo fmt` 全仓（仅 rustfmt 新文件）；未构建/重启 server；未触碰 `~/.aleph`。

## 加固 / Hardening pass (lifecycle + security) — still PARTIAL

**中文**

- **取消安全 / Cancellation**：`CompanionManager::start` 在 `launcher.launch` 返回后**同步**（其间无 await）登记一个 `Phase::Starting` 的 companion、创建 `StartGuard` 并启动 manager 持有的 watcher；后续 proof / open_tab / publish 任一 await 被丢弃，`StartGuard::drop`（同步 `Notify::notify_one`，非 async Drop）唤醒 watcher，由其走同一条 `finish`：按序关闭已附着的 resource / tab，并**必有** `stop_and_reap`。`shutdown_all` 通过同一注册表看到 `Starting` 条目并关闭它；迟到的 `start` 之后 attach 失败（`ClosedDuringStart`），关掉自己刚开的 tab，不发布。`process.rs` 另加 `LaunchAbortGuard` + `kill: Notify`：握手期间 `launch` future 被丢弃时 supervisor 自行 kill+wait。
- **容量 / Capacity**：`MAX_LIVE_COMPANIONS` 在 launch **之前**经同一 manager 互斥量原子预留（`Reservation` 守卫，Drop 释放；install 时转为条目）；`shutdown_all` 之后拒绝新启动（`ShuttingDown`）。
- **resource 关闭失败 / Close failure**：`finish` 中 resource 关闭未被确认 ⇒ 记 `resource_close_failed`、状态 `stop_failed`、返回错误，**不**关 tab、**不**杀 child（杀掉会释放端口而 viewer grant/WS 可能仍指向它）；`stop` / `shutdown_all` / watcher（最多 3 次重试）可重试，已完成的步骤有标志位不重复。
- **后代进程 / Descendants**：**不声称 tree-kill**。托管服务契约为“单进程”：`browser_companion` 的 capabilities 现为 `spawn_subprocess:false`（⇒ `allow_fork=false`）；`WorkspaceSandbox::spawn_managed` 在审批前及 `prepare_command` 之后两次拒绝任何 `spawn_subprocess:true` 的 managed 请求（`UnsupportedPolicy{feature:"managed_spawn_fork"}`）。OS 层禁止 fork 依赖 driver：macOS seatbelt 在 `allow_fork=false` 时输出 `(deny process-fork)`，**本次未在 macOS 实机验证**；Linux/Windows 仍整体 Unsupported。契约写入 `src/sandbox/managed.rs`、`src/sandbox/driver.rs`。
- **验证 / Evidence**（Windows，`-p alephcore`，jobs=1，debug=0，offline，每次前跑内存闸门）：`task48-hardening-check.log`（`cargo check --lib --tests` 通过、0 warning）；`task48-hardening-clippy.log`（`cargo clippy --lib --tests` 0 warning）；`task48-hardening-test.log`：定向过滤器 `browser_tools:: companion executor::builtin_registry session_mode process_registry sandbox::workspace::tests::managed managed_spawn launcher_without launcher_capabilities` → **491 passed / 0 failed**，含新增：`cancelled_start_after_launch_reaps_the_child_and_publishes_nothing`、`cancelled_start_during_proof_reaps_the_child`、`shutdown_all_closes_an_in_flight_start_and_it_does_not_publish`、`resource_close_failure_fails_closed_and_the_retry_finishes_in_order`、`watcher_retries_a_failed_close_before_reaping`、`capacity_is_reserved_atomically_before_launch`、`launcher_capabilities_never_grant_fork`、`managed_spawn_refuses_a_forking_policy_and_spawns_nothing`。测试后对 `manager.rs`/`process.rs` 做了 rustfmt（纯格式），并重跑同一过滤器：`task48-hardening-test2.log` 同为 491 passed / 0 failed（check/clippy 未在 fmt 后重跑）。
- **残余 / Residual**：被取消的 `open_tab` 若已在浏览器中建 tab 但尚未返回 id，该 tab 可能泄漏（child 仍被回收）；resource 关闭首次失败后，`lifecycle.close` 已撤销 grant 并关闭注册表，重试只会看到 `AlreadyClosed`，因此首次失败尝试中丢失的 close 事件不可恢复（错误会如实上报，重试继续后续步骤）；独立静态复核还指出 supervisor 在 registry cancel 已消费且返回错误的极窄路径不会再次发 kill、shutdown 不等待所有 handshake 中的 start，以及 watcher 三次 close 失败后不再自动重试，这些均未在本轮修复；fork 拒绝在非 macOS 未验证；无 tree-kill；Windows 仍 Unsupported；仍无 managed E2E。

**English**

- **Cancellation**: after `launcher.launch` returns, `start` registers a `Phase::Starting` companion, creates a `StartGuard` and spawns the manager-owned watcher in one synchronous block (no await between). If the future is dropped in proof/open_tab/publish, the sync `StartGuard::drop` (`Notify::notify_one`, no async Drop) wakes the watcher, which runs the same `finish`: close any attached resource/tab in order, then always `stop_and_reap`. `shutdown_all` sees `Starting` entries through the same registry; a late `start` fails attach (`ClosedDuringStart`), closes its own tab and publishes nothing. `process.rs` also gets `LaunchAbortGuard` + `kill: Notify` so a `launch` future dropped during the ready handshake makes the supervisor kill and reap the child itself.
- **Capacity**: the cap is reserved atomically before launch under the manager mutex (`Reservation` guard, released on drop, converted on install); no new starts after `shutdown_all` (`ShuttingDown`).
- **Close failure**: an unconfirmed resource close in `finish` records `resource_close_failed`, sets `stop_failed` and returns an error without closing the tab or killing the child (killing would free the port while viewer grants/WS may still reference it). `stop`, `shutdown_all` and the watcher (up to 3 attempts) retry; completed steps are flagged and not repeated.
- **Descendants**: **no tree-kill is claimed.** The managed contract is single-process: capabilities now set `spawn_subprocess:false` (so `allow_fork=false`) and `WorkspaceSandbox::spawn_managed` refuses any `spawn_subprocess:true` request before approval and again after `prepare_command` (`UnsupportedPolicy{feature:"managed_spawn_fork"}`). OS-level fork denial is the driver's job: macOS seatbelt emits `(deny process-fork)` when `allow_fork=false` — **not verified on a real macOS host here**; Linux/Windows stay Unsupported. Contract documented in `src/sandbox/managed.rs` and `src/sandbox/driver.rs`.
- **Evidence** (Windows, `-p alephcore`, jobs=1, debug=0, offline, memory gate before each run): `task48-hardening-check.log` (`cargo check --lib --tests` passes, 0 warnings); `task48-hardening-clippy.log` (`cargo clippy --lib --tests`, 0 warnings); `task48-hardening-test.log`: the targeted filter above (plus `launcher_capabilities`) → **491 passed / 0 failed**, including the eight new tests listed above. rustfmt (format only) was applied to `manager.rs`/`process.rs` after that run and the same filter re-run: `task48-hardening-test2.log` also 491 passed / 0 failed (check/clippy not re-run after fmt).
- **Residual**: a cancelled `open_tab` that already created a browser tab but had not returned its id may leak that tab (the child is still reaped); after a failed first resource close, `lifecycle.close` has already revoked grants and closed the registry, so a retry only sees `AlreadyClosed` and a close event lost on the failed attempt is unrecoverable (the error is surfaced, later steps proceed on retry); independent static review also found a narrow supervisor path where a consumed registry cancel error does not issue a later kill, shutdown does not await every handshake-in-flight start, and the watcher stops automatic retries after three close failures; these were not fixed in this pass; fork denial is unverified off macOS; no tree-kill; Windows Unsupported; still no managed E2E; status remains PARTIAL.

## 最终验证 / Final verification (independent verifier; unit/compile level)

Windows, `-p alephcore`，日志均在 / logs under `.analysis/browser-live-runtime/`：

- `task48-finalverify-check.log`：`Finished dev profile ... in 3m 39s`，无 error / warning 行 / no error or warning lines。
- `task48-finalverify-test.log`：`test result: ok. 81 passed; 0 failed; 0 ignored; 0 measured; 20647 filtered out`（`alephcore` lib）。覆盖 `companion::{manager 18, process 4, protocol 6, tool 5}`、`process_registry` 32、`sandbox::platforms::windows::driver` 12（含 `managed_spawn_is_refused_because_the_wrapper_pid_is_not_the_service_pid`）、`sandbox::workspace` 4（managed 审批 / 无 service PID 驱动 unsupported / 拒绝 fork 策略）。该日志**未记录**所用过滤器字符串，故不复述。 The log does not record the filter string, so it is not restated here.
- `task48-finalverify-clippy.log`：`Finished dev profile ... in 5m 25s`，日志无 warning 行（0 warnings）；该日志同样未记录 clippy 参数。
- 较早 hardening 阶段（更宽过滤器，非最终 verifier）：`task48-hardening-test.log` / `task48-hardening-test2.log` 均 491 passed / 0 failed；`task48-hardening-check.log`、`task48-hardening-clippy.log` 通过且无 warning 行（rustfmt 之后 check/clippy 未在 hardening 阶段重跑）。独立复核的局部切片日志另存为 `task48-independent-{companion,executor,procreg,sandbox}.log`（各自 `ok`，不另作汇总声称）。
- 覆盖的是单测 / 编译级；**不是** managed E2E。未运行 `cargo fmt` 全仓、未构建 / 重启 server、未运行全 workspace 测试或严格 `-D warnings` clippy。

## 缺口 / Not done, not proven

- **无 managed E2E**；无真实服务实现该握手的 QA fixture（此前「无生产 producer」的说法现仅指无 QA fixture / Windows Unsupported；生产 producer 代码已存在）；`managed-companion` 仍 BLOCKED。No real handshake-speaking QA fixture; the production producer code now exists, the blocker is fixture + Windows Unsupported.
- Windows managed spawn 需 trusted `sandbox-init-windows` 上报真实 PID 并并入 wait——**架构缺口**，未发明替代。
- macOS 路径未编译；Linux 不支持。
- 同 session 内后续 run 无法 stop 先前 run 的 companion（严格 run 所有权；靠 lifetime≤1h 与关闭兜底）。
- 沙箱 `AllowHosts` 网络仅 macOS/Linux 有 proxy 改写；Windows AppContainer 对回环的限制未验证。
- 仍缺 Native/TUI/OS/TTL 等 PARTIAL 项（见 `BROWSER_LIVE_VIEWER.md`）。不声称 #48 完成。
