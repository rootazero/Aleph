# Next-session prompt — Stage C Gateway Bound compatibility closure

你是 Aleph Capability Core 的主实现 agent。请在仓库 `/Volumes/TBU4/Workspace/Aleph` 继续工作。

开始前，请用你自己的话重述：当前要解决的是 Gateway Bound channel 如何避免从旧 Legacy compatibility surfaces 获得未授权执行、资源初始化或副作用；本次只推进 Stage C compatibility closure，不扩大到 Stage D resource matrix 或 Feishu/R2。

当前基线：

- 当前本地 HEAD：`95f388c41 docs(gateway): record bound retirement evidence`
- Stage B code commits：`a6d122bf4`、`df513848c`、`255ebc8a6`
- Stage B 已完成并验证 bounded Generic Webhook/loopback composition。
- `ChannelRegistry` 是唯一 Gateway association authority；`EntryRuntime` 只持有 entry-local lifecycle bookkeeping；`ProviderHandle` 持有 provider instance/state/generation；`OwnershipTree` 持有 owner/lifetime/generation/revoke/dispose。
- Stage B run record：`docs/superpowers/runs/2026-10-27-gateway-bound-retirement-run.md`
- Stage C 计划：`docs/superpowers/plans/2026-10-27-gateway-bound-compatibility-closure-plan.md`
- 源总计划中的 Stage C：`docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md`
- R1 仍是 NO-GO/deferred。

先读取：

- `AGENTS.md`
- `docs/reference/FEATURE_LOCATOR.md`
- `docs/reference/GATEWAY.md`
- `docs/reference/ARCHITECTURE.md`
- `docs/reference/DEVELOPMENT.md`
- `docs/reference/REDLINES.md`
- `docs/superpowers/runs/2026-10-27-gateway-bound-retirement-run.md`
- 上述两个 Stage C plan
- `src/gateway/channel_registry.rs`
- `src/gateway/channel_registry/binding.rs`
- `src/gateway/channel_registry/test_support.rs`
- `src/gateway/handlers/channel.rs`
- `src/gateway/handlers/discord_panel.rs`
- boot `subsystems.rs`、health monitor、delivery queue、inbound router、builtin channel tools、webhook receiver、pure slot `Channel` contract

工作纪律：

1. 先做实际 compatibility census，再提出 2–3 个实现路径；不要静默假设调用方行为。
2. 必须先写 RED tests，再改生产行为。parser/import/compile/test setup failure 不算 contract RED；先修正到测试能执行，再记录真实行为 RED。
3. 只使用现有 `ChannelRegistry`、entry-local runtime、`ProviderHandle` 和 `OwnershipTree`。禁止新增 registry、owner map、descriptor map、runtime table、CapabilityKind、generalized envelope 或 public Bound route。
4. Bound raw `Arc`、Legacy effect helper、webhook verify/direct handler、health/restart、delivery queue、inbound forwarding 等路径必须在创建 ticket、资源、队列记录或实际 effect 之前 fail-closed。
5. cold/Discovered/Disabled 的 list/status/health/directory/snapshot/capture/read 路径不能启动 transport、client、forwarder 或 provider runtime。
6. Legacy positive controls 必须保持原行为。
7. source gate 只保护短 identity/publication/marker commit；任何 channel/transport/stop/close/join I/O 必须在锁外。
8. 不得进入 Stage D polling/socket/SQLite resource matrix，不得进入 Feishu/R2、production mount、Zahir route、public send/ingress、EffectClaim、Safe Replay、durable recovery 或 scheduler。
9. 每次 cargo 命令前运行并记录：

```bash
python qa/browser_live/check_memory.py
```

要求它 exit 0 / `ALLOW`。

优先写入以下五个真实 contract tests：

- `bound_compatibility_effect_paths_are_denied`
- `bound_held_webhook_handler_denies_dispatch`
- `bound_cold_lists_never_call_transport`
- `bound_compat_lifecycle_uses_source`
- `legacy_positive_controls_unchanged`

测试必须观察真实 loopback、Webhook handler、queue/effect boundary、entry/provider/forwarder 状态和 source publication boundary；不得用 fake receipt 或手动修改内部 active-work 来伪造权限或完成。

实现后至少执行：

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::webhook_receiver::tests::
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::delivery_queue::tests::
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib tools::registry::tests::
python qa/browser_live/check_memory.py
cargo check -p alephcore --tests
python qa/browser_live/check_memory.py
cargo fmt --all --check
git diff --check
```

如果改动触及 Webhook 或 capability 边界，再运行对应 `gateway::interfaces::webhook::` 和 `capability::` suites。记录每个命令的真实 exit、passed/failed/ignored 数量和非致命 linker warning。

完成实现后进行一次独立只读 review，重点检查：

- Bound bypass 是否在副作用前拒绝；
- Option/bool refusal 是否被 caller 当成成功或 fallback permission；
- raw handle 是否泄露；
- cold reads 是否真的没有 transport/resource 初始化；
- lifecycle 是否走 source-owned path；
- Legacy positive control 是否回归；
- 是否新增了第二份 authority 或扩大到 deferred subsystem。

reviewer 服务不可用时，记录完整错误并执行同一 checklist 的手工 review，不要声称独立 review 通过。

若测试和 review 全部通过，再更新 Stage C 双语 run record、source plan 和相关 `FEATURE_LOCATOR.md`/reference 条目。最终报告必须列出：bounded slice、RED/GREEN 命令与实际结果、修改文件、authority duplication 审查、首个未闭合 contract、仍 deferred 的 provider/kind、未运行验证，以及没有执行的 destructive/external action。

现在先完成读取、census 和方案比较；不要直接写生产代码。
