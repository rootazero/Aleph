# Next-session prompt — Gateway Stage C verification closure

你是 Aleph Capability Core 的主实现 agent。请在 Aleph 仓库继续工作；以当前 `main` 的 git root 为准，不假设其他机器上的绝对路径。

## 架构终极目标

Everything externally composable is a Capability.

一切需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都是 Capability。把 Aleph 从“一个集成很多功能的进程”演进成“一个轻量 Capability Core，加上一组按需启动、可停止、可替换的能力提供者”。Gateway 社交通道、终端、记忆星云图和 sandbox 关闭后不应初始化其线程、连接、数据库或缓存；单个能力失败不应拖垮 Core；停用后旧调用句柄必须失效。

## 开始前重述目标

请先用你自己的话重述：当前不是直接开始 Stage D，而是先闭合 Stage C 留下的两个 Legacy ExplicitOnly mid-send integration 超时，确认它们是否造成隐私/队列/生命周期违规，并取得独立 Stage C review。此次不得扩大到 Stage D resource matrix、Feishu/R2 或 production mount。

## 当前基线与进度

- 当前 `main` 已包含 Stage C 实现和证据；先读取实际 HEAD，不照搬历史 hash。
- Stage C implementation HEAD：`66489d94b`；evidence commit：`dc278a932`。
- Stage C required suites 已 GREEN：Registry 63、Webhook receiver 22、delivery 52、health 14、tools 54、Webhook interface 51、capability 222。
- `ChannelRegistry` 是唯一 Gateway association authority；`EntryRuntime` 仅持有 entry-local lifecycle bookkeeping；`ProviderHandle` 持有 provider instance/state/generation；`OwnershipTree` 持有 owner/lifetime/generation/revoke/dispose。
- R1 仍为 NO-GO/deferred；Stage D/E、Feishu/R2 和所有生产组合能力仍未授权。
- 独立 reviewer `del_mvri61gw_y936` 因 `No API key found for anthropic.` 未执行；手工 review 不等于独立通过。
- 首个未闭合 contract：
  - `explicit_only_channel_unregistered_mid_send_is_never_persisted`
  - `explicit_only_channel_replaced_mid_send_is_never_persisted`
- 两者当前均在五秒 watchdog 处报 `no deadlock: Elapsed(())`。没有 clean baseline reproduction，因此不得直接称为既有问题。

## 必读材料

- `AGENTS.md`
- `docs/reference/FEATURE_LOCATOR.md`
- `docs/reference/GATEWAY.md`
- `docs/reference/ARCHITECTURE.md`
- `docs/reference/DEVELOPMENT.md`
- `docs/reference/REDLINES.md`
- `docs/superpowers/runs/2026-10-28-gateway-bound-compatibility-closure-run.md`
- `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md`
- `docs/superpowers/plans/2026-10-28-gateway-stage-c-verification-closure-plan.md`
- `tests/sms_delivery_policy_test.rs`
- `src/gateway/channel_registry.rs`
- `src/gateway/channel_registry/binding.rs`
- `src/gateway/delivery_queue.rs`
- pure slot `Channel` contract 及实际 Legacy unregister/register/stop 路径

## 执行方式

1. 确认 `main` 和当前 worktree 状态。若使用开发 worktree，从当前 `main` 创建短分支 `mid-send-closure`；所有生产 writer 串行，结束后先合并再清理 worktree。
2. 先完成 plan Task 1 的 current/baseline A/B 与 lock census，再提出 2–3 个真实修复路径及取舍；不要直接改生产代码。
3. baseline 使用 `ca03a1206` 的独立临时 worktree，不得 reset 或污染 `main`。只有实际 A/B 证据才能区分 Stage C regression 与 baseline defect。
4. 当前已知 `send_on` 在 `channel.send(...).await` 期间持有 channel read guard，而 `VanishingExplicit::send` 内会 await `unregister`。必须验证完整调用链，不只凭这一片段下结论。
5. 不得删除、放宽或单纯延长两个失败测试。 unregister case 必须证明旧注册消失且旧实例 cleanup 完成；replacement case 必须证明 Automatic successor 可用、旧 ExplicitOnly 实例只发送一次且 cleanup 完成，二者都不得持久化旧 payload。
6. 保留执行此次 send 的原始 channel instance policy；同 ID successor 或注册消失不能变成 queue permission。
7. 先写可执行的行为 RED，再改生产行为。parser/import/compile/setup failure 不算 contract RED。
8. 不允许 unsafe、self-referencing guard、未跟踪的 detached cleanup、假 receipt、手工 active-work 权限或第二份 authority。unregister/replacement 不得在实际 cleanup 未完成时声称成功。
9. 如果唯一安全方案需要 broad `Channel` trait/API migration、改变公开 unregister 语义或新增 deferred-cleanup authority，停下来请求架构决策，不得静默扩大。
10. `lifecycle_sync` 和 source publication gate 只保护短 identity/marker/publication commit；channel/transport/stop/close/join I/O 必须在锁外。
11. 每次 Cargo 命令前运行并记录：

```bash
python qa/browser_live/check_memory.py
```

要求 exit 0 / `ALLOW`。零测试 filter 不算通过。

## 必需验证

至少执行完整 SMS suite 和 Stage C regression set：

```bash
cargo test -p alephcore --test sms_delivery_policy_test
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::webhook_receiver::tests::
cargo test -p alephcore --lib gateway::delivery_queue::tests::
cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
cargo test -p alephcore --lib tools::registry::tests::
cargo test -p alephcore --lib gateway::interfaces::webhook::
cargo test -p alephcore --lib capability::
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo fmt --all --check
git diff --check
```

每条 Cargo 命令都要单独前置 memory gate。两个 focused mid-send tests 连续串行运行三次；记录真实 exit、passed/failed/ignored 和非致命 linker warning。

## Review 与文档

完成后对本次修复及完整 Stage C diff 做一次独立只读 review，重点检查：

- read/write lock 是否造成同任务或跨任务 deadlock；
- unregister/replacement 返回时旧资源是否仍处于未清理状态；
- ExplicitOnly policy 是否始终来自实际发送实例；
- 是否有 plaintext queue/dead-letter/replay 泄露；
- Bound denial、cold read、source lifecycle 和 Legacy positive controls 是否回归；
- 是否新增第二份 authority 或进入 deferred subsystem。

reviewer 不可用时记录完整错误并执行同一 checklist 的手工 review，不得声称独立通过。只有完整 SMS/Stage C suites GREEN 且没有未解决 Critical/Important finding，才更新双语 run record、source plan 和 `FEATURE_LOCATOR.md`；Stage C 未闭合时不得写成完成。

最终报告必须列出：baseline A/B、root cause、方案取舍、RED/GREEN 命令与实际结果、修改文件、authority duplication 审查、review 可用性、首个未闭合 contract、仍 deferred 的 provider/kind、未运行验证，以及没有执行的 destructive/external action。

本阶段闭合后，下一任务才是源总计划 Stage D：真实 Webhook 加 test-only polling/socket 的 cold/active resource matrices 与 positive controls；它不是本任务的隐含授权。

现在先完成读取、current/baseline 复现、lock census 和方案比较；不要直接写生产代码。
