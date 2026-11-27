# Zahir desktop discovery and operator approval / 桌面发现与运维审批验收

Branch / 分支：`refactor/computer-use` · Product base / 产品基线：`6a351411f` · [Design / 设计](../specs/2026-11-26-computer-use-zahir-operator-approval-design.md) · [Plan / 计划](../plans/2026-11-26-computer-use-zahir-operator-approval.md)

## What the branch proves / 分支验证什么

- **Discovery is not authority.** A projection host mounted before builtin registration converges on one `desktop` descriptor. Discovering or resolving it does not call the platform. An admitted read crosses the existing builtin router once; denying admission or disposing the registration does not trigger another read. / 投影宿主先启动、内置工具后注册，最终只有一个 `desktop` 描述符；发现和解析不调用平台，受管读取只经过原路由一次，拒绝和卸载均不会额外读取。
- **The held call owns its proof.** The approved leaf keeps the original handler, backend lease, effective payload and delivery rail. Canonical registry views recreate their provider wrappers; comparing the underlying published handler distinguishes an unchanged binding from equal-schema replacement. / 暂挂动作保留原 handler、租约、有效载荷与投递轨；比较底层发布实例而非每次新建的 provider 包装，才能识别同 schema 替换而不误伤无关注册。
- **Only an eligible desktop `Ask` holds for an operator.** No desktop lock is held while the card is pending. The wait expires no later than 60 seconds or the original deadline, whichever is earlier. After `AllowOnce`, Core tries the original session lock, rechecks the refusal conditions and hands off at most once. The existing `Full` tier lift and the raw path are unchanged. / 只有符合条件的桌面 `Ask` 等待运维者；等候时不占锁，最多 60 秒且不得超过原调用截止时间。单次批准后尝试取得原会话锁，重新检查拒绝条件，只投递一次。保留既有 `Full` 层级提升与原始调用路径。

## Validation / 验证

The first fake integration run used `Full`, whose tier is already carried into the owned task and lifts `Ask` to `Allow`. The approval fixture uses `Auto` to exercise an actual leaf `Ask`; the branch does not redefine the tier rule. / 初次集成测试使用 `Full`，而既有执行任务会保留该层级并按原规则将 `Ask` 提升为 `Allow`。改用 `Auto` 验证确实需要审批的叶动作，不改变层级规则。

| Gate / 门禁 | Result / 结果 |
|---|---|
| `desktop_` Core filter / Core 组合筛选 | 32 passed / 32 通过 |
| Late registration and exact admitted capture / 迟注册、原调用捕获 | 1 each, independent reruns / 各独立复测 1 项通过 |
| `builtin_tools::desktop` | 294 passed / 294 通过 |
| `operator_once`; `gateway::handlers::exec_approvals` | 17 and 18 passed / 分别 17、18 项通过 |
| Native process evidence and Escape flag / 进程实例证据和 Escape 标志 | 1 each, synthetic macOS tests / macOS 合成测试各 1 项通过 |
| `cargo check --offline -p alephcore --bin aleph-server`; `cargo check --offline -p aleph-desktop-macos` | Both passed; integrated check repeated after import-only formatting / 两项均通过，调整 import 顺序后集成检查再次通过 |
| `git diff --check 6a351411f`; `cargo fmt --all --check` | Diff check passed. Workspace formatter still reports 99 hunks in 36 unchanged files; none in this branch's changed files. / diff 检查通过；全仓格式检查仍报告 36 个未改动文件中的 99 处既有差异，本分支修改文件无格式差异。 |

The fake desktop fixture checks an exact original-payload dispatch and scoped post-state once. A sibling approval remains independent; denied, cancelled, busy-lock, changed policy, changed PID instance, window-frame drift, stale handler lease and Escape cases cause no further input. The record and event frames contain no test payload. / 替身桌面验证原始载荷只投递一次，且保留定向事后状态；并发审批互不影响，拒绝、取消、锁被占用、策略或进程实例变更、窗口几何漂移、handler 租约失效及 Escape 都不再产生输入。记录和事件均不包含测试载荷。

The independent read-only review identified unbound pointer locations; the final code requires a verified, owned window and a point within its frame. The approval card intentionally shows the action category and verified target, not the raw input. / 独立只读审阅发现无窗口绑定的指针坐标缺少证据；最终实现要求已证实的所属窗口且坐标落在其帧内。审批卡只展示动作类别与已验证目标，不展示原始输入。

## Boundaries / 未覆盖边界

Evidence uses fake input and synthetic macOS identity and Escape state. No real UI, live CGEvent delivery, TCC or permissions changes, Windows/Linux run, or operator Panel interaction was performed. The checks are pre-handoff snapshots, not an atomic guarantee about OS delivery or UI state. / 本轮使用替身输入及合成的 macOS 进程身份和 Escape 状态，未操作真实桌面、发出 CGEvent、变更 TCC/权限、运行 Windows/Linux 或实际操作运维 Panel。投递前自检不是对 OS 投递结果或 UI 状态的原子保证。
