# Zahir capability 收敛回执 / Zahir Capability Convergence Receipt（2026-11-25）

- 分支 / Branch：`zahir-capability-convergence`（worktree `.worktrees/zahir-convergence`），基于 main `a4dac8db4`；未合入、未推送。
- 设计 / Spec：`docs/superpowers/specs/2026-11-25-zahir-capability-convergence-design.md`（用户批准；§5 三处修正均经用户确认，第 3 条有一处实施收窄，见下）
- 计划 / Plan：`docs/superpowers/plans/2026-11-25-zahir-capability-convergence.md`
- 执行方式：本会话内联执行（executing-plans），裁定记入 `.superpowers/sdd/2026-11-25-zahir-capability-convergence/progress.md`（git-ignored）。

> 证据层文档：本文是时点记录，不是当前架构；与代码冲突时以代码为准。当前事实见 `docs/reference/FEATURE_LOCATOR.md`「Current state」。

## 1. 做了什么 / What changed

| 段 | 提交 | 内容 |
|---|---|---|
| C 收敛 | `4cb16e5b6` | 删 `backend.rs` 的 `Legacy*` handler registry 与同名重导出、`special.rs` 的 legacy 桥、`ambient.rs` |
| C 收敛 | `8507aff30` | 删 `audit.rs`、`grant.rs`、`grant_integration_tests.rs`、`projection_host` 的测试用 `audit_collector`；`events_scoped_tests.rs` 改用快照 + `TrustedEventScope::admits` |
| C 收敛 | `459c4fe96` | 删 `lifecycle.rs`、`activation.rs`、`activation_reentry_tests.rs`、`ActivationEventHook`、从未发出的 `CapabilityAttached` / `CapabilityDetaching`、scoped 兼容激活入口；修正 `ownership.rs` 过时模块注释 |
| D 依赖可用性 | `ae9438dd5` | `ResolveError::DependencyUnavailable { target, state }`；`resolve_resource` 先查可见性、再查 provider 状态、最后 memory 捕获；事件标签 `dependency_unavailable`；工具调用返回「`<name>` 当前不可用（状态 <State>），稍后重试或请用户启用」 |
| S 重载预检 | `084453b9d` | `mount_inner` 拆为 `prepare_mount`（不写任何东西）+ `commit_mount`；`reload_plugin` 先 `preflight_reload`（解析、身份校验、WASM `CompiledPlugin::new` 只编译）；新增 `MountError::ReloadRefused` |
| F 文档 | 本提交 | FEATURE_LOCATOR 当前现状 + 闭合记录、PLUGIN_SYSTEM 生命周期原语、EXTENSION_SYSTEM Hot Reload（修正过时示例）、AGENTS.md / CLAUDE.md 同步、本回执 |

`src` 相对 `a4dac8db4`：29 个文件，+556 / −4956 行（段 C–S 合计）。

## 2. 实施裁定 / Rulings

1. **撤销 = 从 attachment 列表移除**（段 C，偏离计划的 `revoked: AtomicBool`）。`admit` 只在同一把 `runtime.attachments()` 锁下确认 `Arc::ptr_eq` 成员资格后才放行，所以旧控制器的三项检查（`ttl=None`、自比较的 generation、空 `ConstraintSet`）不可达；`runtime_lifecycle` 等三条拒绝文案随之删除（无测试钉住）。
2. **准入被拒仍拆除**（段 S，收窄计划 §S0 中「被策略拒绝保留旧版」）。保留旧版等于让已被撤销信任的插件继续运行，是闸的放行方向（判据 8、14）。已写入设计 §5 第 3 条。
3. **没有版本在运行时，预检失败退回原路径**：照旧 mount 并在行上记 Error，未挂载插件的重载行为不变。
4. **manifest 缓存保留**：缓存键含 path / size / mtime / ctime / dev / ino，改写即失效；计划中的「绕过缓存」不需要。
5. **`core_panel_lifecycle_failed_reload_never_serves_old_publication` 改写**为 `core_panel_lifecycle_failed_reload_preflight_keeps_old_publication`：预检失败后旧 publication 仍是当前版本、字节不变。拆除后失败撤回旧字节的保证仍由 `core_panel_lifecycle_replacement_failure_disable_and_unmount_withdraw_bytes` 钉住。
6. 被删测试中仍保护生产行为的断言已有生产侧覆盖：无载荷审计 `src/gateway/capability_events_tests.rs:1046,1638`；`ConstraintViolation` `:1466`。

## 3. 验证 / Verification（各段门禁）

| 段 | 命令 | 结果 |
|---|---|---|
| C | `cargo test -p alephcore --lib capability:: tools::scoped::` | 639 passed / 1 ignored |
| C | `cargo check` + `cargo clippy --workspace --all-targets --exclude aleph-desktop-shell` | exit 0；`alephcore` 0 warning（11 条 warning 全在未改动的 `aleph-panel` / `aleph-panel-ui`） |
| D | `cargo test -p alephcore --lib capability:: tools:: gateway:: memory::` | 10174 passed / 4 failed，4 条均为既有环境问题（文件相对 `a4dac8db4` 未改动） |
| S | `cargo test -p alephcore --lib extension::` | 978 passed / 4 failed；其中 3 条在 main `a4dac8db4` 上同样失败，1 条需要未初始化的 `plugins/` 子模块 |
| S | 同 C 的 check + clippy | exit 0，同上 |

`aleph-desktop-shell` 被排除：其 build script 需要 `binaries/aleph-server-aarch64-apple-darwin`，本机不存在（既有环境问题）。

## 4. 没做什么 / Not done

- `ProviderState::Parked` 与依赖恢复后重拉下游、通用资源捕获 trait、单插件双实例切换 + 旧版排空（`with_admitted_completion` 仍无生产消费者）、全量 `reload()` 分阶段化、跨会话 grant、ambient 重新挂载——触发条件见设计 §9。
- 预检覆盖不到：模块能编译但链接/实例化失败、MCP 服务器或 service 启动失败，仍发生在拆除之后。
- 文件监听触发的是全量 `reload()`，不经过单插件预检。
- 四个同名 `ProviderRegistry`、三套 lifecycle 状态机中的另两套（`provider.rs`、`diagnostic_control.rs`）未动（设计非目标）。
- 未做 release / 其他平台 / 真机验证。
