# Gateway Bound Compatibility + Owned Send — Stage C Run Record

**Date / 日期**: 2026-10-28  
**Scope / 范围**: Stage C compatibility and Legacy ExplicitOnly mid-send closure only / 仅 Stage C 兼容闭环与 Legacy ExplicitOnly mid-send 闭合
**Implementation HEAD / 实现提交**: `f7ac10cd957c5d9ed914dc97da8ce4d7051cb8c2`
**Status / 状态**: bounded Stage C closed; required suites GREEN; independent final review `Ready: Yes` / 限定范围 Stage C 已闭合；必需 suites 全绿；独立最终审查 `Ready: Yes`

## Bounded slice / 限定切片

This record covers the existing Bound compatibility closure and the two remaining Legacy ExplicitOnly mid-send contracts. `ChannelRegistry` remains the sole Gateway association authority; `EntryRuntime` retains only entry-local lifecycle bookkeeping; `ProviderHandle` retains provider instance/state/generation; `OwnershipTree` retains owner/lifetime/generation/revoke/dispose. `SendOperation` owns transport handles only and is not another association, policy, owner, or lifecycle authority.

本记录覆盖既有 Bound compatibility closure 与剩余两个 Legacy ExplicitOnly mid-send contracts。`ChannelRegistry` 仍是唯一 Gateway association authority；`EntryRuntime`、`ProviderHandle`、`OwnershipTree` 的既有分工不变。`SendOperation` 只拥有传输句柄，不是第二份 association、policy、owner 或 lifecycle authority。

No public Bound send/ingress route, Stage D resource matrix, Feishu/R2 integration, production mount, Zahir route, EffectClaim producer, Safe Replay, durable scheduler, or new CapabilityKind was added.

未新增 public Bound send/ingress、Stage D resource matrix、Feishu/R2、production mount、Zahir route、EffectClaim producer、Safe Replay、durable scheduler 或新 CapabilityKind。

## Baseline A/B and root cause / 基线对照与根因

Before the fix, the current Stage C tree and a separate detached worktree at `ca03a1206` each ran both focused tests three times. All 12 runs exited 101 at the unchanged five-second watchdog with `no deadlock: Elapsed(())`. The baseline worktree did not reset or modify `main`.

修复前，current Stage C 与独立 detached `ca03a1206` worktree 各对两个 focused tests 串行运行三次，共 12 次均在原五秒 watchdog 处以 exit 101 / `no deadlock: Elapsed(())` 失败；未 reset 或污染 `main`。

Diagnostics on both trees showed old `stop == 0`, the old registration retained, and retry state `Busy`; live queue and dead-letter tables were empty, but enqueue had not been reached. This proved a structural baseline defect exposed by Stage C, not a Stage C regression and not evidence that persistence safety had already been reached.

两棵树诊断均显示旧实例 `stop == 0`、旧注册保留、retry 为 `Busy`；queue/dead-letter 虽为空，但执行尚未到 enqueue。因此这是被 Stage C 暴露的 structural baseline defect，不是 Stage C regression，也不能据此声称已证明持久化安全。

Root cause: `send_on` held the channel read guard across `channel.send(...).await`. `VanishingExplicit::send` awaits unregister/replacement, which needs the same channel's write guard for stop/removal. The same-task read→write cycle deadlocked before cleanup.

根因是 `send_on` 在 `channel.send(...).await` 期间持有 channel read guard；而 `VanishingExplicit::send` 内 await unregister/replacement，需要对同一 channel 获取 write guard 执行 stop/removal，形成同任务 read→write 自锁，cleanup 无法完成。

## Fix and trade-offs / 修复与取舍

The public `Channel::send` contract now returns `SendOperation = Pin<Box<dyn Future<Output = ChannelResult<SendResult>> + Send + 'static>>`. Implementors prepare/clone state under their local guard and return an owned future; transport I/O begins when that future is polled. Registry dynamic paths create the operation under the channel guard, drop the guard, then await. All in-tree implementors and dynamic call paths were migrated atomically.

公开 `Channel::send` 现返回 owned `'static` `SendOperation`。实现者只在本地 guard 下准备/clone 状态，实际 transport I/O 在 future poll 后发生；Registry 在 guard 下创建 operation，释放 guard 后再 await。仓内实现者与动态调用路径已原子迁移。

The sending operation retains the actual instance's captured policy. A same-ID Automatic successor cannot reclassify stale ExplicitOnly work. The public telecom final cancellation fence remains after the last lock wait and before first poll; the transport future is not wrapped in a cancelling `select!`.

operation 保留实际发送实例的 policy；同 ID Automatic successor 不会把陈旧 ExplicitOnly work 变成 queue permission。Public telecom 的 final cancellation fence 仍在最后一次锁等待之后、首次 poll 之前，transport future 不被取消型 `select!` 包裹。

`Channel::stop` performs synchronous resource cleanup but does not drain/join in-flight send operations; it may wait for a provider-local resource lock held by a transport write. Thus an already-created operation may outlive stop, while unregister/replacement still completes actual old-instance cleanup before returning. No detached cleanup, fake receipt, operation-drain authority, unsafe, or self-referencing guard was introduced.

`Channel::stop` 同步清理资源但不 drain/join 在飞 send operations，也可等待 provider-local transport write 锁。因此已创建 operation 可能晚于 stop 结束，但 unregister/replacement 返回前旧实例 cleanup 已真实完成。未引入 detached cleanup、fake receipt、operation-drain authority、unsafe 或 self-referencing guard。

Rejected alternatives: retaining the borrowed future or draining under the registry guard recreates self-reentrant deadlock; detached/deferred cleanup would make unregister/replacement return before cleanup and require another lifecycle authority. The owned-operation migration is intentionally breaking for downstream/out-of-tree `Channel` implementors.

未采用保留 borrowed future 或在 registry guard 下 drain 的方案，因为它们会重建自重入死锁；detached/deferred cleanup 会让 lifecycle 在清理完成前返回并需要第二份 authority。**发布说明必须提示：downstream/out-of-tree `Channel` implementors 必须迁移到 breaking `SendOperation` 签名。**

An additional review fix makes an iMessage stop before a later attachment return `SendFailed`, because an earlier body/attachment may already be on the wire. It is terminal `Ambiguous`, not retryable or operator-redrivable. At a zero-prior-part boundary this conservatively prefers a fresh instruction over possible duplicate delivery.

额外审查修复将 iMessage 在后续 attachment 前停止的错误改为 `SendFailed`：此前 body/attachment 可能已送达，因此归为 terminal `Ambiguous`，不自动重试也不可 redrive。即使此前零部件数为零也采取保守处理，宁可要求新指令，也不冒重复投递风险。

## Behavioral RED/GREEN / 行为红绿证据

Commit `a24b73094` strengthened the two existing tests without deleting assertions or extending the watchdog:

`a24b73094` 在不删除断言、不延长 watchdog 的前提下强化两个既有测试：

- `explicit_only_channel_unregistered_mid_send_is_never_persisted`
- `explicit_only_channel_replaced_mid_send_is_never_persisted`

They now prove old stop exactly once, old send attempt exactly once, unregister removal, Automatic successor availability and a real successor-visible transport send, and no old plaintext in live queue or dead letters. Both were behavioral RED before production changes. After the migration, each focused command ran three consecutive times with exit 0 and `1 passed; 0 failed; 0 ignored; 17 filtered out`:

它们证明旧实例只 stop/send 一次、unregister 后注册消失、Automatic successor 可用且真实发送 successor-visible payload、旧 plaintext 不进入 queue/dead-letter。生产修改前均为 behavioral RED；迁移后以下命令各连续三次 exit 0，均为 `1 passed; 0 failed; 0 ignored; 17 filtered out`：

```bash
cargo test -p alephcore --test sms_delivery_policy_test explicit_only_channel_unregistered_mid_send_is_never_persisted -- --exact
cargo test -p alephcore --test sms_delivery_policy_test explicit_only_channel_replaced_mid_send_is_never_persisted -- --exact
```

The iMessage classifier regression test was also behavioral RED before its fix (`NotConnected` incorrectly remained retryable), then GREEN (`SendFailed` → no enqueue + `DeadLetterReason::Ambiguous`).

iMessage classifier 回归测试同样先证明旧 `NotConnected` 会错误重试的行为 RED，再验证 `SendFailed` 不 enqueue 且归为 `DeadLetterReason::Ambiguous`。

## Final command ledger / 最终命令记录

Final verification ran at `f7ac10cd9`. Every Cargo command was individually preceded by `python qa/browser_live/check_memory.py`, exit 0 / `ALLOW`. Test binaries emitted only the non-fatal macOS linker `__eh_frame` warning.

最终验证在 `f7ac10cd9` 执行。每条 Cargo 命令均单独前置 `python qa/browser_live/check_memory.py`，exit 0 / `ALLOW`。测试二进制仅出现非致命 macOS linker `__eh_frame` warning。

| Command / 命令 | Exit | Result / 结果 |
|---|---:|---|
| `cargo test -p alephcore --test sms_delivery_policy_test` | 0 | 18 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib gateway::channel_registry::tests::` | 0 | 63 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib gateway::webhook_receiver::tests::` | 0 | 22 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib gateway::delivery_queue::tests::` | 0 | 52 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib gateway::channel_health_monitor::tests::` | 0 | 14 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib tools::registry::tests::` | 0 | 54 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib gateway::interfaces::webhook::` | 0 | 51 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib capability::` | 0 | 222 passed, 0 failed, 0 ignored |
| `cargo check -p alephcore --tests` | 0 | passed |
| `cargo clippy -p alephcore --tests -- -D warnings` | 0 | passed |
| `cargo fmt --all --check` | 0 | passed |
| `git diff --check` | 0 | passed |

Named regression coverage includes Bound denial/cold/source lifecycle, failed-cleanup incumbent retention, Legacy positive controls, stale ExplicitOnly handle rejection, replay policy revalidation, and all three public telecom fences/custody tests. No zero-test filter is counted as a pass.

具名回归覆盖 Bound denial/cold/source lifecycle、cleanup failure 保留 incumbent、Legacy positive controls、stale ExplicitOnly handle 拒绝、replay policy 复检及三项 public telecom fence/custody 测试；未把零测试 filter 计为通过。

An earlier full run had all suites GREEN but clippy exit 101 for bare `#[must_use]` (`double_must_use`); `612c1516c` added an explicit reason before the final full rerun. Additional individually gated checks for `aleph-panel`, `panel-tab-terminal-pilot`, `panel-tab-terminal`, and `aleph-terminal-ui` all exited 0. They retained warning-only unused `t_string` and dead-code `MountHandle::new` diagnostics; those packages were not put under a new `-D warnings` gate.

较早一次全集 suites 全绿但 clippy 因裸 `#[must_use]` 报 `double_must_use` / exit 101；`612c1516c` 增加明确说明后重跑全集。额外四个 panel/terminal package check 均独立 memory-gated 且 exit 0，保留既有 warning-only `t_string` unused 与 `MountHandle::new` dead-code，不新增 `-D warnings` 闸。

## Independent review / 独立审查

Task 2 review found three Important issues: retry after partial iMessage delivery, inaccurate stop wording, and unspecified preparation side effects. Commit `5f59ce8c7` and scoped re-review `del_mvrolh9o_4fzv` closed all three with `Ready: Yes`.

Task 2 独立审查发现三项 Important：iMessage 部分投递后重试、stop 文档不准确、prepare side-effect 契约缺失；`5f59ce8c7` 与 scoped 复审 `del_mvrolh9o_4fzv` 全部闭合，结论 `Ready: Yes`。

Fresh final reviewer `del_mvrpim6o_xl9e` inspected the complete `ca03a1206..612c1516c` diff, including Stage C and unrelated range content. It found no Critical and two Important: type-level deadlock hardening and undocumented unbounded Bound restart drain. Scoped re-review `del_mvrpueoi_ya8x` accepted the existing non-vacuous mid-send behavioral tests as the regression guard, downgrading only optional future type/CI hardening, and accepted the explicit Bound drain documentation. Final verdict: `Ready: Yes`, no unresolved Critical/Important.

 fresh 最终 reviewer `del_mvrpim6o_xl9e` 审查完整 `ca03a1206..612c1516c` diff，未发现 Critical，提出两项 Important：类型层死锁防线与未说明的 Bound restart 无界 drain。Scoped 复审 `del_mvrpueoi_ya8x` 接受现有非空洞 mid-send 行为测试作为回归守卫，仅保留可选的未来类型/CI 加固，并接受明确 Bound drain 文档。最终结论 `Ready: Yes`，无未解决 Critical/Important。

The earlier historical reviewer failure `del_mvri61gw_y936` / `No API key found for anthropic.` is superseded for this closure by the successful independent reviews above; no manual-only approval is being claimed.

早期 `del_mvri61gw_y936` 的 `No API key found for anthropic.` 是历史失败；本次闭合已取得上述成功独立审查，不声称 manual-only approval。

## Files and commits / 文件与提交

Core changes are in `crates/aleph-slot-api/src/channel.rs`, `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/binding.rs`, `src/gateway/delivery_queue.rs`, `src/gateway/inbound_router/mod.rs`, `src/gateway/channel.rs`, `src/gateway/webhook_receiver.rs`, `src/capability/provider.rs`, `tests/sms_delivery_policy_test.rs`, and `tests/link_acl_probe/mock_channel.rs`. Mechanical `Channel` migrations cover the CLI, Discord, Email, Feishu API shape only, iMessage/BlueBubbles, IRC, LINE, Matrix, Mattermost, Nostr, QQ, Signal, Slack, SMS, Telegram, voice_call, Webhook, WeChat, WhatsApp, XMPP and related execution/test fixtures. Slot compatibility tests and `examples/modularization/channel/src/lib.rs` were migrated as downstream positive controls.

核心修改见上述 registry/queue/provider/slot/test 文件；机械迁移覆盖仓内所有 channel implementors 与 execution/test fixtures。Feishu 仅适配公开 API 形状，不包含 Feishu/R2 功能集成。Slot compatibility tests 与 modularization example 作为 downstream positive controls 同步迁移。

Closure commits: `7ced31c11` (design), `162cc1bde` (plan), `a24b73094` (behavioral RED), `7e6286658` (owned operations), `5f59ce8c7` (partial-iMessage retry fence), `612c1516c` (must-use diagnostic), `f7ac10cd9` (review documentation/comment closure).

闭合提交依次涵盖设计、计划、行为 RED、owned operation、iMessage 重试闸、must-use 诊断与审查文档/注释闭环。

## Residuals, unclosed work and deferred scope / 残余、未闭合与延期

No contract remains unclosed within the approved Stage C slice. The next source-plan task is Stage D: real Webhook plus test-only polling/socket cold/active resource matrices and positive controls; it was not started or implicitly authorized here.

批准的 Stage C 范围内没有未闭合 contract。下一项源总计划任务是 Stage D：真实 Webhook 加 test-only polling/socket 的 cold/active resource matrices 与 positive controls；本阶段未开始，也不构成隐含授权。

Registry-level Bound lifecycle stop still drains admitted source work without timeout; a wedged Bound transport can block restart/health monitor. This is distinct from `Channel::stop`, which does not drain send operations. Production Bound mount remains deferred, so this dormant path was documented rather than given a new cancellation/deferred-cleanup authority.

Registry-level Bound lifecycle stop 仍无超时 drain admitted source work，wedged Bound transport 可阻塞 restart/health monitor；这与不 drain send operations 的 `Channel::stop` 不同。由于 production Bound mount 仍 deferred，本次仅如实记录，没有新增 cancellation/deferred-cleanup authority。

R1 remains NO-GO/deferred. Stage D/E, Feishu/R2, production mount/Zahir, public Bound send/ingress, EffectClaim, Safe Replay, durable recovery/scheduler, terminal, memory graph, sandbox, and all remaining provider/kind backends remain deferred. No full-workspace test run, real external-provider delivery, Stage D matrix, production mount, or external service validation was performed.

R1 继续 NO-GO/deferred；Stage D/E、Feishu/R2、production mount/Zahir、public Bound send/ingress、EffectClaim、Safe Replay、durable recovery/scheduler、terminal、memory graph、sandbox 及其余 provider/kind backends 均 deferred。未运行 full-workspace tests、真实外部 provider 投递、Stage D matrix、production mount 或外部服务验证。

No destructive or external action was performed: no reset/clean, merge, worktree cleanup, push, deployment, external message, production mount, or external service mutation.

未执行 destructive/external action：没有 reset/clean、merge、worktree cleanup、push、部署、外部消息、production mount 或外部服务变更。
