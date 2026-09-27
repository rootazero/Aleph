# WhatsApp Channel Arch R1 — 修复 + 连线 + 增强

> **Status**: Draft · awaiting human review
> **Created**: 2026-04-22
> **Branch**: `whatsapp-arch-r1` (worktree)
> **Supersedes (partially)**: `docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md` (2026-04-06, 描述已严重过时)
> **Spec convention**: 本 spec 是 architectural 路径的"设计+取舍"文档；writing-plans 步骤会把它拆成可执行的 implementation plan。

---

## TL;DR

经过 3 个只读 subagent 对现状的全仓 grep + 文件级行号定位，发现**8 项用户提的改进中有 2 项已被前人修复**（vault 测试隔离 / 通道契约撒谎），**3 项是孤儿结构**（account / reactions / history_buffer 的字段和模块存在但无消费者），**2 项是状态机未联动**（PairingState 与 ConnectionState 各走各的），**1 项是 mock 骨架不全**（test_mode 只旁路 Channel trait 层）。

R1 的目标是：**熵减为主、增强为辅**。先消除孤儿结构与未联动的状态机；再用最小代码把"已存在但未消费"的字段真正接到事件循环；最后补 fake transport 让 R2 的多账户/路由改动有可观测闭环。

**不做**：完整多账户接入（5–7 PR 体量，超出 R1 范围；详见 §6）。

---

## 1. 上下文与动机

### 1.1 旧设计文档已过时

`docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md`（2026-04-06）描述的"当前用 Go bridge，需要迁到 Baileys" 与现状不符：
- 当前 `wa_runtime/` 用的是 `whatsapp-rust` crate（Rust 原生），**不是 Go bridge**，不是 Baileys
- 文档列的"Phase 1-5"中**大部分已实现**（policy、chunking、reactions、history_buffer、media、account_registry）
- 真正未接线/有问题的不在文档里，而在 2026-04 之后 100+ 次 commit 中悄然留下

**R1 的副产物**：改写 `WHATSAPP_ARCHITECTURE_DESIGN.md` 为现状 + 修复记录（§9.2），或直接 retire 它。

### 1.2 ALEPH 判据对照（CLAUDE.md §0–§19）

按 CLAUDE.md 判据打分（5 分制，越高越严重）：

| 判据 | 涉及项 | 现状打分 | 备注 |
|------|--------|---------|------|
| §0 两端完整而中间没线 | G1/G2/G4 | 5/5 | AccountRegistry / ReactionHandler / GroupHistoryBuffer 全孤儿 |
| §1 同一事实的两份表述 | G3/G6 | 2/5 | 已修（G3 测试隔离，G6 capability 诚实） |
| §8 fail-closed 反转成许可 | G7 | 3/5 | `PairingState::to_channel_status()` 死代码，status() 走 ConnectionState 路径 |
| §11 报成功的 no-op | G2 reactions/history | 5/5 | config.reactions / config.history 解析后丢弃 |
| §19 一次加宽 | G4 多账户 | 4/5 | accounts HashMap 字段存在但完全没计数循环 |

### 1.3 openclaw 对照的真正差距

之前声称的"openclaw feature parity"清单（多账户 / reactions / history_buffer）实际差距是 **"openclaw 已接线，Aleph 留了结构没接线"**——不是参考项目做不到的事，是我们自己 4 月份留下的脚手架从未合拢。

参考项目的 fake transport（`adapter.runtime.ts`）是 R2 多账户路由的必要前置；R1 只做骨架的最小扩展。

---

## 2. 范围（In-Scope vs Out-of-Scope）

### 2.1 In-Scope

| ID | 项 | 类型 | 预计 diff |
|----|----|------|----------|
| **A0** | 修正 CLAUDE.md §1 + FEATURE_LOCATOR §1 关于 vault 描述（已修复） | doc | ≤ 10 行 |
| **A2** | PairingState 与 ConnectionState 联动：5 个事件映射 + `status()` 改读 PairingState | 修复 | ~80 行 |
| **A3** | AccountRegistry 决策：**待用户裁定**（CUT 路径为推荐；§6 留完整接入草案） | 修复 | CUT: ~120 行删除 / 完整: ~300 行 |
| **B1** | `config.reactions` 字段在事件循环消费：构造 `ReactionHandler` + `ReactionSender` adapter | 连线 | ~60 行 |
| **B2** | `config.history` 字段在 inbound mapper 消费：`GroupHistoryBuffer::add` + 下游读 `get_context` | 连线 | ~50 行 |
| **C1** | `trait WaRuntime` 抽出 + `FakeWaRuntime` 实现，让 `for_test()` 真正跑闭环 | 增强 | ~150 行 |
| **C2** | `MediaProcessor` 自动重编码：图像 → JPEG/PNG 限幅，音频 → opus，**用 `tokio::task::spawn_blocking` 包 CPU 密集** | 增强 | ~120 行 |
| **D1** | 更新 `WHATSAPP_ARCHITECTURE_DESIGN.md` 为现状；更新 `FEATURE_LOCATOR.md` 增加 WhatsApp 段 | doc | ~80 行 |

### 2.2 Out-of-Scope（显式不做）

- **完整多账户接入**（chat_id → account_id 路由、event_rx fan-in、HashMap<AccountId, WaRuntime>）：5–7 PR 体量，超出 R1。
- **替换 `whatsapp-rust` 为 Baileys**：R7/LLM 友好方案已在用，无替换理由。
- **修改 `Channel` trait**：守住 R4 契约稳定。
- **引入新 async runtime / 新三方 crate**（除 `image` for C2 外不引新依赖）：CLAUDE.md 禁用清单。
- **e2e 真机 QA 套件**：R1 仅补 fake transport；真机 `qa/channels/run.sh` 加 whatsapp 段是 R2 任务。

---

## 3. 关键设计决策

### 3.1 D1：PairingState 单源化（替代"两条状态机"）

**现状**：两条独立状态机并存，事件映射残缺。
- `wa_runtime/state.rs::ConnectionState`：5 变体，atomic 后端，由 `Event::Connected/Disconnected/PairingQrCode` 驱动
- `pairing.rs::PairingState`：9 变体 FSM，有完整 `to_channel_status()` + `validate_transition()`，**30+ 单测覆盖**所有合法/非法转换，但**生产代码 0 个事件源驱动它**

**决策**：**保留 PairingState，撤销 ConnectionState**（单源）。理由：
- PairingState 的 9 变体 + 校验 + 测试已就位，是更细粒度的真相
- ConnectionState 只是 PairingState 的粗化子集
- `WaRuntime::connection_state()` 重命名为 `pairing_phase()`，调用方改读 PairingState::to_channel_status()
- 删 `wa_runtime/state.rs`（ConnectionState + AtomicConnectionState）

**风险**：`ConnectionState::Connecting` 与 `PairingState::Initializing` 语义不完全等价（前者更粗）。需要 grep 所有 `ConnectionState` 引用点逐一核对。

**回退闸**：保持 `ConnectionState` 作为派生指标（`PairingState::to_connection_state()` 函数），仅 `status()` 走 PairingState。

### 3.2 D2：`config.reactions` / `config.history` 真接入，**不引入 trait 抽象**

**现状**：
- `reactions.rs::ReactionHandler` 已存在，需要 `Arc<dyn ReactionSender>`——设计上是为 trait 抽象预留的
- `history_buffer.rs::GroupHistoryBuffer` 是具体 struct，直接持有即可

**决策**：
- **reactions**：抽最小 `trait ReactionSender: Send + Sync { async fn send_reaction(...) }`，生产实现就是 `WaRuntime::send_reaction` 的 thin wrapper，测试实现是 `FakeWaRuntime`。**不抽完整 `WhatsAppRuntime` trait**——那是 C1 的事。
- **history**：直接在 `WhatsAppChannel` 持 `Arc<GroupHistoryBuffer>`，mapper 签名扩为 `map_event_to_inbound(event, channel_id, buffer)`。

### 3.3 D3：AccountRegistry 走 CUT 路径（推荐）

**现状**：`account.rs` + `account_registry.rs` 全孤儿，`config.accounts: Option<HashMap>` 字段被 serde 解析后丢弃。

**决策（推荐）**：**CUT**。
- 删 `account.rs`、`account_registry.rs` 两个文件
- 删 `WhatsAppConfig::accounts`、`WhatsAppAccountConfig` 类型
- 保留 `WhatsAppConfig::default_account_id: Option<String>`，单账户路径不变

**理由**：
1. CLAUDE.md 判据 §0 + P6（简洁性）都主张"先简化再复杂化"
2. 没有产品需求证据（用户从未提及"我需要在同一 Aleph 实例跑多个 WhatsApp 号码"）
3. 完整接入是 5-7 PR 工作量，会冲淡 R1 的核心修复目标
4. 如果未来真有多账户需求，可以在新 spec 里提，"先删后建"比"先建后删"安全（CLAUDE.md §0 "改问『这段字是谁写的』『不在我这张表上的那部分呢』"）

**替代路径**（详见 §6）：完整接入多账户——R1 不走，但提供"已批准的设计草案"留档。

### 3.4 D4：`WaRuntime` trait 抽出（C1）

**现状**：
- `WaRuntime` 是具体 struct（27 字段）
- 仓库不用 mockall
- `test_mode: bool` 旁路只到 `Channel::send/start` 层，不深入 WaRuntime

**决策**：抽最小 `trait WaRuntime: Send + Sync` 含 `pairing_phase / start / shutdown / send_message / send_typing / mark_read / send_reaction / take_event_receiver`，生产实现 `RealWaRuntime`（包装当前 `WaRuntime`），测试实现 `FakeWaRuntime`（内部 mpsc，受控 emit `Event::PairingQrCode/Connected/Disconnected/PairSuccess/Message`，可断言内部状态）。

**关键**：trait 边界必须**接缝于 runtime 内部事件通道**，不能接缝于 Channel trait（否则跟 test_mode 重复了）。

### 3.5 D5：MediaProcessor 用 spawn_blocking 跑 CPU 密集

**现状**：`wa_outbound/media.rs` 已有 MIME 路由骨架，但图像解码/重编码在 async 任务里跑会阻塞 tokio worker。

**决策**：
- 图像：`image` crate 解码 → resize → JPEG 编码，整个 pipeline 在 `tokio::task::spawn_blocking` 中
- 音频 opus 重编码：本期只做 MIME 标记正确（`audio/ogg; codecs=opus`），不真做编码转码（whatsapp-rust 接受已编码好的 opus 流）
- 视频：passthrough + size 校验，不在本期做 transcode

**依赖**：`image = "0.25"` 是 CLAUDE.md 允许的三方（已用于 desktop 模块的截图处理，需核对是否已在 workspace Cargo.lock）。

---

## 4. 模块改动详情

### 4.1 文件级 diff 清单（按 A2 → B1 → B2 → C1 → C2 → D 顺序）

| 文件 | 改动类型 | 行数估计 |
|------|---------|---------|
| `pairing.rs` | **不删**（保留 9 变体 + 测试）；新增 `PairingState::to_connection_state()` 派生 | +20 |
| `wa_runtime/state.rs` | **仅当用户批准 D1 时删**（撤销 ConnectionState + AtomicConnectionState）；否则保留作为 PairingState 的派生指标 | -40（或 +20 派生函数） |
| `wa_runtime/client.rs` | `WaRuntime::connection_state()` → `pairing_phase()`；所有内部 atomic 改读 PairingState | ~±40 |
| `wa_runtime/mod.rs` | `WaRuntime` 重导出；`pairing_phase()` 改返回 `PairingState` | ~±10 |
| `mod.rs` | 事件循环加 arm（PairSuccess/Scanned/Syncing/QrExpired/Failed）；`status()` 改读 `pairing_state.to_channel_status()`；构造 `ReactionHandler` + `GroupHistoryBuffer` | ~±100 |
| `reactions.rs` | 删掉假 trait 引用；保留 ReactionHandler + 加 `ReactionSender` trait 定义 | ~±20 |
| `history_buffer.rs` | 不改 | 0 |
| `wa_inbound/mapper.rs` | 签名扩 `buffer: &GroupHistoryBuffer`；内部 `buffer.add()` | +15 |
| `wa_outbound/media.rs` | spawn_blocking + image crate | +90 |
| **新文件** `wa_runtime/traits.rs` | `trait WaRuntime` 定义 | +50 |
| **新文件** `wa_runtime/fake.rs` | `FakeWaRuntime` 实现 + `MockEventBus` | +180 |
| **新文件** `wa_runtime/real.rs` | `RealWaRuntime`（拆 client.rs 当前实现） | +0（仅迁移） |
| `account.rs` | **仅当用户批准 D3 CUT 时删**（CUT 路径）；否则按 §6 完整接入 | -120（或 +~250 完整接入） |
| `account_registry.rs` | **仅当用户批准 D3 CUT 时删** | -90（或 +~50 完整接入） |
| `config.rs` | **仅当用户批准 D3 CUT 时**删 `accounts: Option<HashMap>` + `WhatsAppAccountConfig`；否则保持 | -30（或 0） |
| `mod.rs` | `pub mod account; pub mod account_registry;` 删除（CUT 路径） | -2 |
| `mod.rs` | `for_test` 改用 FakeWaRuntime | +10 |

**总 diff（按推荐 CUT 路径）**：约 -300 / +600 = 净 +300 行（含 fake transport 与新测试）。
**总 diff（按完整接入多账户）**：约 -50 / +1000 = 净 +950 行（R1 不推荐）。

### 4.2 测试策略

| 测试类型 | 现有 | R1 新增 |
|---------|------|--------|
| PairingState FSM（unit） | 30+ 测试 | 不动 |
| vault_store（unit）| TempDir 隔离测试 | 不动 |
| Channel contract（unit）| test_mode 旁路 | 增加 FakeWaRuntime 真闭环 |
| **新**：R1 端到端 | 0 | 4 场景：QR / Connected / PairSuccess / Reconnect |
| **新**：media spawn_blocking | 0 | 图像重编码 round-trip + size 校验 |

**回归测试**：所有现有 `tests/whatsapp_contract_test.rs` + `tests/whatsapp_protocol_test.rs` 必须保持绿。

---

## 5. 不做的事（State the Negative）

按 AGENTS.md 规则显式列出未做项：

1. **多账户路由**：R1 走 CUT 路径（§3.3）。如果未来要做，详见 §6 的设计草案留档。
2. **PairingState → ChannelStatus 的转换**：当前 `to_channel_status()` 在 PairingState 上存在但未被 `WhatsAppChannel::status()` 调用——R1 修复**调用路径**，不修改转换函数本身（CLAUDE.md §1 "同一事实两份表述"风险：函数已是真相，`status()` 改读它即可）。
3. **e2e 真机 QA**：`qa/channels/run.sh` 加 whatsapp 段是 R2 任务，R1 仅 fake transport。
4. **PairedResult / phone-linking wizard UI**：R5 不在本期。
5. **历史消息回填（HistorySync 事件）**：whatsapp-rust 会推送历史同步事件，但目前 mapper 只接 `Event::Message`；R1 仅做事件→PairingState 映射，不做历史注入到 LLM context（那是 memory/recall 范畴）。
6. **可达性测试**：spec 表里写了 250-400 行估算，**实际 diff 必须 ≤ 估算**；否则回头修订 spec。
7. **PairingState 单源化是单向棘轮**：一旦 R1 删除 ConnectionState，R2 想恢复 ConnectionState 必须重新设计场景。

---

## 6. 多账户完整接入设计草案（仅留档，R1 不实施）

> 本节是 §3.3 替代路径的留档，**R1 不实施**。

### 6.1 触发条件

仅当以下任一条件成立时才考虑：
1. 用户在生产环境中确认需要 Aleph 同时跑多个 WhatsApp 号码（多 sim 卡、多用户）
2. PairingState 单源化 + FakeWaRuntime 落地后，仍无法解决"为什么我需要第二账户"的产品需求
3. R2 用户反馈"我现在用 openclaw 是因为它支持多账户"

### 6.2 设计要点

```rust
pub struct WhatsAppChannel {
    info: ChannelInfo,
    config: WhatsAppConfig,
    channel_state: ChannelState,
    accounts: Arc<WhatsAppAccountRegistry>,  // 多账户容器
    runtimes: Arc<RwLock<HashMap<AccountId, Arc<dyn WaRuntime>>>>,  // 每个账户一个 runtime
    // ...
}

impl WhatsAppChannel {
    async fn start(&mut self) -> ChannelResult<()> {
        let account_configs = self.config.accounts.clone()
            .unwrap_or_else(|| /* single-account fallback */);
        for (account_id, account_config) in account_configs {
            let auth = WaAuthManager::with_vault_and_crypto(
                self.vault.clone(), &account_id, self.crypto.clone()
            );
            let runtime = RealWaRuntime::new(auth, self.event_tx.clone()).await?;
            self.runtimes.write().await.insert(account_id.clone(), Arc::new(runtime));
        }
    }

    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendResult> {
        let account_id = self.resolve_account_id(&msg.conversation_id)?;
        let runtime = self.runtimes.read().await.get(&account_id).unwrap().clone();
        runtime.send_message(msg).await
    }
}
```

### 6.3 风险

- 跨账户的 inbound 路由：chat_id(jid) → account_id 需要 hashmap 缓存 + 反向查找（用户首次 DM 时建立）
- 事件 fan-in：每个 runtime 各自的 event_rx 需要合并到 channel 层的统一 event_rx
- 资源消耗：每个 WaRuntime 独立 WhatsApp 连接，N 个账户 = N 倍内存 + N 个 push notification token
- 与 R5"AI comes to you"冲突：多账户意味着消息从多个号码涌入，AI 主动到达的"工作通道"语义模糊

### 6.4 决策

**R1 不做**。若 R2 触发条件成立，新开 spec。

---

## 7. 实施顺序（依赖图）

```
A0 doc fix ──────────────┐
                         │
A2 PairingState 单源 ───┐ │
                        ├─┤
A3 CUT account/registry ┘ │
                          │
        ┌─────────────────┤
        ▼                 ▼
B1 reactions 接入 ─┐
                   ├─→ C1 FakeWaRuntime ─┐
B2 history_buffer  ─┘                     ├─→ C2 media spawn_blocking
                                          │
                                          └─→ D1 doc 更新
```

**合并策略**：
- Round 1（无依赖）：A0 + A2 + A3 CUT 可并行
- Round 2：B1 + B2 并行
- Round 3：C1（依赖 B1 + B2，因为 trait 抽象要包含两者）
- Round 4：C2（独立）
- Round 5：D1（最后汇总）

每个 Round 后做 `cargo check -p alephcore` + `cargo test -p alephcore --lib` 局部验证。

---

## 8. 验收标准

### 8.1 必须满足

1. `cargo check -p alephcore` 全绿
2. `cargo test -p alephcore --lib` 全绿，无新增 `--ignored`
3. `cargo clippy --workspace --all-targets` 无新增 warning（已有 warning 必须保持或减少）
4. R1 新增的 4 个端到端 fake 测试全绿
5. vault_store 测试仍走 TempDir 隔离（验证 CLAUDE.md §1 描述与代码一致）
6. `WhatsAppChannel::status()` 在 fake runtime emit `Event::PairSuccess` 后正确返回 `ChannelStatus::Connected`
7. AccountRegistry / WhatsAppAccount / WhatsAppAccountConfig 三个标识符**在 `rg` 全仓搜 0 命中**（CUT 路径证据）
8. PairingState 9 变体在生产代码路径上**至少有 5 个能被 fake event 触发**（R1 仅触发 PairingQrCode → WaitingQr → Connected；R2 再补 Scanned/Syncing/QrExpired）

### 8.2 可接受妥协

- `cargo clippy` 允许"已存在的 warning"清单不变
- `cargo test -p alephcore --lib` 不要求 100% pass（只要求 WhatsApp 相关测试全绿，其他模块失败需单独评估）
- FakeWaRuntime 不实现"WhatsApp Cloud API 的消息撤回/编辑"（capabilities 已声明 false）

---

## 9. 文档副产物

### 9.1 CLAUDE.md §1 描述更新

把"whatsapp 那条的 `auth.save(&data).unwrap()` 是死脚手架"改为：
> "2026-08-29 (`2d883ee23`) 已修复：vault_store 测试改用 `TempDir` + `with_vault_and_crypto` 注入，单元测试零污染生产 vault。证据：`src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs:155-205`。"

### 9.2 `WHATSAPP_ARCHITECTURE_DESIGN.md` 改写

把 1306 行的过时文档**压缩为 ≤ 200 行**的"现状 + 修复记录"：
- 删"Phase 1-5"计划（已实施）
- 删"Baileys migration"段（未做且不做）
- 新增"R1 修复与连线"段，引用本 spec
- 标 Status: Superseded by `2026-04-22-whatsapp-arch-r1-design.md`

### 9.3 FEATURE_LOCATOR.md 增加 WhatsApp 段

在附录 E 判据分组后新增"§11 WhatsApp 通道"段，列出：
- 已知孤儿结构清单（修复后应清空）
- PairingState 单源化前后对比
- FakeWaRuntime 与 test_mode 的关系

---

## 10. 引用与索引

- `docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md`（将被 §9.2 压缩）
- `docs/reference/FEATURE_LOCATOR.md` §1（vault 描述，将被 §9.1 更新）
- `docs/reference/CLAUDE.md` 判据 §0/§1/§8/§11/§19
- `src/gateway/interfaces/whatsapp/mod.rs:59-67`（WhatsAppChannel 结构）
- `src/gateway/interfaces/whatsapp/wa_runtime/state.rs:4-10`（待删的 ConnectionState）
- `src/gateway/interfaces/whatsapp/pairing.rs:43-90`（保留的 9 变体 FSM）
- `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs:155-205`（已修复的测试隔离）
- `/Volumes/TBU4/Github/openclaw/extensions/whatsapp/adapter.runtime.ts`（fake transport 参考）
- `/Volumes/TBU4/Github/openclaw/extensions/qa-lab/src/live-transports/whatsapp/scenario-implementations.ts`（端到端场景测试参考）

---

## 11. 决策日志

| 日期 | 决策 | 替代方案 | 理由 |
|------|------|---------|------|
| 2026-04-22 | PairingState 单源化，撤销 ConnectionState | 保留两条互为派生 | 判据 §1 + §8 风险 |
| 2026-04-22 | AccountRegistry 走 CUT（推荐，待用户批准） | 完整接入多账户（§6 草案留档） | 判据 §0 + P6 + 无产品需求 |
| 2026-04-22 | MediaProcessor 用 spawn_blocking 包装 image crate | 直接在 async 任务里跑 | tokio worker 阻塞 |
| 2026-04-22 | WaRuntime trait 抽最小集（接缝于事件通道） | 全量 trait 抽象 | 与 test_mode 区分 |
| 2026-04-22 | R1 不做真机 QA | 加 qa/channels/run.sh whatsapp 段 | 工作量控制 |
| 2026-04-22 | R1 不做 e2e 多账户 | mock runtime 跑多账户场景 | R1 边界 |

---

## 12. 等待人类审查

- [ ] §3.1 D1 PairingState 单源化——同意还是保留 ConnectionState？
- [ ] §3.3 D3 AccountRegistry CUT 路径——同意还是走 §6 的完整接入？
- [ ] §3.5 D5 image crate 引入——允许还是只做 MIME 标记不实编？
- [ ] §4.1 diff 估算 +300 行——是否可接受？
- [ ] §7 实施顺序——同意还是调整？

如审查通过，下一步走 writing-plans skill 拆分实施步骤。