# 下一会话任务 Prompt：Stage B — Source-owned Bound retirement / replacement

你是 Aleph Capability Core 的主实现 agent。请在当前仓库继续推进 Gateway source-owned runtime authority，但本次只做 **Stage B：Bound channel retirement、replacement 和 unregister 语义**。不要进入 Feishu/R2、production mount、Zahir route 或其他子系统迁移。

仓库：`/Volumes/TBU4/Workspace/Aleph`

## 当前基线

本会话已在 `main` 完成并提交：

- `8ff6531bf docs(gateway): authorize source-owned runtime authority`
- `02555b5f1 feat(gateway): own bound channel execution lifecycle`
- `fa1af2f59 docs(gateway): record source-owned runtime evidence`

另有前置 R1 provider binding commits：`12ff9fb3f`、`914d4e974`、`5fde0b3f8`、`d293f8676`、`ec292280e`。

Stage A 已验证一个 Generic Webhook/loopback vertical slice：

- `ChannelRegistry` 既有 entry/map 是唯一 Gateway association authority。
- entry-local operation marker、admitted-work ticket、execution ownership、drain 和 completion handoff 已存在。
- Legacy 和 Bound 真实 I/O 不再持有 `lifecycle_sync`；共享 gate 只保护短 source/map/marker commit。
- `ProviderHandle` 仍拥有 provider instance/generation 和 call admission/completion guard。
- `OwnershipTree` 仍拥有 owner/lifetime/generation/revoke/dispose authority。
- Generic Webhook client 只在显式 start 构造；cold declaration/capture/snapshot 不创建 HTTP client。
- Stage A gateway tests 54/54、capability tests 222/222、check、clippy、fmt、diff check 均通过。
- 非致命 macOS linker warning 仍可能出现：`__eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table`。

R1 仍是 **NO-GO / deferred**。Stage A GREEN 不等价于 R1 GO，也不授权 R2 Feishu。

## 先阅读

必须先读取：

- `AGENTS.md`
- `docs/superpowers/specs/2026-10-26-gateway-source-owned-runtime-authority-design.md`
- `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md`
- `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md`
- `docs/superpowers/runs/2026-10-26-capability-core-r1-capture-ledger.md`
- `src/gateway/channel_registry.rs`
- `src/gateway/channel_registry/binding.rs`
- `src/gateway/channel_registry/test_support.rs`
- `src/gateway/interfaces/webhook/mod.rs`
- `src/capability/provider.rs`
- `src/capability/ownership.rs`

先执行并记录：

```bash
git status --short -uall
git log --oneline --decorate -12
```

## 本阶段目标

证明一个正在执行的 Bound channel 在 retirement、replacement、unregister 和 contract/publication change 过程中：

1. 旧 entry 的 admitted work 能被完整 drain，或在无法证明安全清理时保留真实 residual。
2. 旧 completion 在 successor publication 后一定被拒绝，且不能修改 successor、source revision、provider 或 owner state。
3. successor 必须在 private preparation 和 preflight validation 完成后，才在一个短 source gate commit 中整体发布。
4. replacement/unregister 失败时 incumbent identity、运行状态、mount/dispatch fence 和 residual 保持诚实；不能报成功 no-op。
5. source publication revision exhaustion 在任何 destructive retirement 前 fail-closed；不能半发布、wrap 或删除 incumbent。
6. `ProviderHandle`、`OwnershipTree` 和 `ChannelRegistry` 各自继续持有自己的事实；不能复制 generation、owner map、descriptor map 或 runtime registry。

## 必须先写 RED tests

在真实 authority 的测试模块中先写测试，再运行 focused RED。测试必须观察实际 barrier、loopback request、entry/task/forwarder 状态和 publication boundary，不得用模拟 receipt 或手工修改内部状态伪造效果。

优先覆盖这些测试形状：

- `bound_retirement_discards_original_completion`
  - 旧请求已经到达真实 loopback peer。
  - 在 completion handoff 前 retirement/replacement 发生。
  - 旧 completion 返回 `DiscardedStale`，旧 ticket 只结算一次。
  - successor 不受旧结果影响。

- `bound_replace_publishes_whole_tuple`
  - successor provider、descriptor、contract、ownership 和 entry 私下准备。
  - publication 前旧 entry 仍是唯一可见 incumbent。
  - publication 后 source revision、entry identity、provider instance 和 descriptor fingerprint 一起变化。
  - 不允许出现半更新 tuple。

- `bound_unregister_waits_for_cleanup`
  - 真实 outbound work 持有响应 barrier。
  - unregister/retirement 必须等待原始 work、forwarder 和可观察资源 cleanup。
  - cleanup 未确认时不能移除 incumbent 并报告成功。

- `bound_contract_update_retires_old_publication`
  - 只改变 descriptor/schema/fingerprint 或 lifecycle contract 也必须使旧 capture/lease 失效。
  - 同一 textual ID 不能让旧 provider/entry proof 继续被接受。

- `bound_failed_cleanup_retains_incumbent`
  - 注入真实 stop/close/join failure。
  - incumbent 保留，operation marker/residual 保留，successor 不发布。
  - 不能把 failed cleanup 当成 clean success 或自动 Ready。

- `bound_revision_exhaustion_never_half_publishes`
  - 在 destructive retirement 前耗尽 source publication revision。
  - 失败必须没有 teardown、删除、successor publication 或 revision wrap。
  - 需要覆盖 cleanup 期间另一 entry 消耗 revision 的情况。

RED 失败必须是 contract RED。parser、import、test insertion 或格式错误先修正，不计为 contract RED。

## 实现边界

只有 RED 精确证明现有 authority 缺少 Stage B 所需事实时，才添加最小实现。优先扩展现有 `ChannelRegistry` entry/runtime 和已有 `binding.rs`，不得新增并行 authority。

允许：

- 在现有 entry-local runtime 中补充 retirement phase、successor preparation、drain/join residual 或 source-owned publication marker。
- 复用 `ProviderHandle` 的 generation/call completion guard。
- 复用 `OwnershipTree::with_current_binding`、revoke/dispose/generation 和已有 descriptor canonical fingerprint。
- 在真实 ChannelRegistry 测试支持中增加 deterministic barriers/loopback failure injection，只用于观察真实边界。
- 修改现有 Legacy/Bound compatibility wrapper，使 Bound replacement/unregister 不再走错误的 Legacy success path。

禁止：

- 新增 registry、adapter、facade glue、owner map、handler map、generalized envelope 或 durable store。
- 新增 `CapabilityKind`。
- 泛化 `BackendLease::canonical_entry`。
- 修改 `src/harness/**`。
- 进入 Feishu、生产 Bound opt-in、production mount、Zahir route 或 public send/ingress API。
- 实现 EffectClaim producer、Safe Replay、external exactly-once、universal scheduler、durable recovery 或 crash recovery。
- 用 test-only receipt 作为生产权限、ownership 或 completion authority。
- 用 `Drop` 推断真实 network operation 已经结束。

## 线性化与锁纪律

- source gate 只保护短 identity/publication/marker commit；任何 channel/factory/transport/stop/close/join I/O 必须在锁外。
- retirement marker 必须覆盖 unlocked drain/cleanup phases，并在最后一个成功 publication/失败 residual commit 后释放。
- completion 验证顺序保持 source → tree → provider → original entry；顺序不能被 successor lookup 替代。
- old completion 必须使用 original entry/ticket，不能按 textual ID 重新取 successor。
- publication 必须一次性提交完整 successor tuple：entry identity、provider instance/generation、descriptor/fingerprint、contract、ownership、desired intent 和 source revision。
- 任何失败路径都必须保留可观察 residual，不能返回“成功但没有效果”。

## 验证要求

每次 cargo 命令前先运行：

```bash
python qa/browser_live/check_memory.py
```

Focused RED 后至少运行：

```bash
cargo fmt --all --check
git diff --check
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::channel_registry::binding::tests::
cargo test -p alephcore --lib gateway::interfaces::webhook::
cargo test -p alephcore --lib capability::
```

所有命令必须记录实际结果、测试数量、失败数量和非致命 warning。不要把未运行的验证写成通过。

完成前执行一次独立只读 code review；如果 reviewer 服务不可用，要明确记录原因，不能声称独立 review 通过。最后检查：

```bash
git diff --check
git status --short -uall
git diff --stat
```

## Stage B 完成标准

Stage B 只能在以下全部成立时标记完成：

- 上述 retirement/replacement/unregister RED tests 均有真实 GREEN 结果。
- 旧 completion 不会修改 successor 或任何新 generation/owner state。
- failure path 保留 incumbent 和真实 residual。
- revision exhaustion 不会 destructive、half-publish 或 wrap。
- 没有新增 Gateway authority、CapabilityKind、production route 或 deferred subsystem。
- 实际 tests/check/clippy/fmt/diff 结果已写入对应 bilingual plan/spec/run record。
- R1 仍单独评估为 NO-GO/deferred，除非后续 resource matrix、compatibility closure 和所有既定 gate 另行证明；Stage B 完成不授权 Feishu/R2。

最终报告必须列出：bounded slice、每个 RED/GREEN 命令和实际结果、修改文件、authority duplication 审查、首个未闭合 contract、仍 deferred 的 provider/kind、未运行的验证，以及没有执行的 destructive/external action。
