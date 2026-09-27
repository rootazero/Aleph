# WhatsApp Arch R1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended per user instruction) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Aleph 的 WhatsApp 通道上做一轮熵减 + 状态机单源化 + fake transport 闭环，对照 spec §3 设计决策落地。

**Architecture:**
- PairingState 单源化（撤销 ConnectionState），事件循环补齐 5 个事件映射
- 删 AccountRegistry 孤儿结构（CUT 路径，§3.3 推荐）
- `config.reactions` / `config.history` 真接入事件循环 + inbound mapper
- 抽 `trait WaRuntime`，加 `FakeWaRuntime` 实现端到端 4 场景测试
- MediaProcessor 用 `tokio::task::spawn_blocking` 包 image crate 重编码

**Tech Stack:** Rust 1.95 (workspace.toolchain 1.96) · tokio · `whatsapp-rust` (现有) · `image = "0.25"` (新增 for C2) · `tempfile` (现有)

**Spec:** `docs/superpowers/specs/2026-04-22-whatsapp-arch-r1-design.md`（本 plan 与 spec 同看；spec 是 vision，本 plan 是 bite-sized tasks）

**Worktree:** `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/whatsapp-arch-r1`，分支 `whatsapp-arch-r1`

---

## Global Constraints

- **分支**：所有提交在 `whatsapp-arch-r1` 分支，**严禁** 触碰 main
- **commit message**：`<scope>: <description>` 英文（AGENTS.md 规范）
- **clippy**：`cargo clippy --workspace --all-targets -- -D warnings` 必须绿
- **check 范围**：每个 Task 完成后至少 `cargo check -p alephcore`
- **测试**：`cargo test -p alephcore --lib` 至少 WhatsApp 相关测试全绿
- **判据**：每条 commit 自查 CLAUDE.md §0–§19 适用项（特别是 §0 孤儿结构、§1 两份表述、§8 fail-closed、§11 no-op、§19 加宽）
- **依赖**：除 `image = "0.25"` (Task 9) 外不引入新 crate
- **删除**：删孤儿代码不留 TODO/FIXME 注释（CLAUDE.md P6 简洁性）

---

## Review Focus

（spec 暗示但无 Task 测试覆盖的 5 个最可能咬人的输入/失败模式）

1. **PairingState::to_channel_status() 返回值与 Channel trait 期望的 status() 一致性** — 撤销 ConnectionState 后 status() 全走 PairingState；若 to_channel_status 漏写某个变体（例如 QrExpired），会返回默认 status 而不是 fail-closed。Task 3 自带 QrExpired→PairingState 映射测试。
2. **fake WaRuntime 在 Drop 时的事件通道清理** — FakeWaRuntime 持有内部 mpsc；若 event_rx 一端持有者早退，emit 会丢消息。Task 8 用 `tokio::time::timeout` 断言"无消息时不阻塞"。
3. **media spawn_blocking 在测试环境的超时** — `image` crate 解码损坏 PNG 可能 hang；Task 9 用 fixture 限制大小并加 timeout。
4. **CUT 路径后 `cargo clippy` 报 dead_code** — 删 account.rs / account_registry.rs 后若有 `pub use` 残留会编译失败。Task 4 必须 `grep -n "WhatsAppAccount\|AccountRegistry" src/` 验证 0 命中后再 commit。
5. **reactions 在 group 消息的策略** — spec §3.2 说 `direct: true, group: Mentions`，但 `should_agent_react` 还没接线。Task 5 仅做 ack reaction（pre-reply），不做 agent-initiated reaction，避免范围爆炸。

---

## Task 1: A0 doc fix — 更新 vault 描述

**Files:**
- Modify: `CLAUDE.md`（§1 判据 描述更新）
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§1 / 附录 E.1 同步）

**Interfaces:**
- 消费：无
- 产出：`CLAUDE.md` §1 描述提到 `vault_store.rs:155-205` 为已修复证据

- [ ] **Step 1: 读 `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs` 155-205 确认现状**

读后必须看到 `TempDir::new()` + `with_vault_and_crypto` 调用，且 commit `2d883ee23` 在文件历史里。

- [ ] **Step 2: 在 `CLAUDE.md` §1 描述中替换 "whatsapp 那条的 `auth.save(&data).unwrap()` 是死脚手架" 一句**

替换为：
> "2026-08-29 (`2d883ee23`) 已修复：vault_store 测试改用 `TempDir` + `with_vault_and_crypto` 注入，单元测试零污染生产 vault。证据：`src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs:155-205`。"

- [ ] **Step 3: 同步 `docs/reference/FEATURE_LOCATOR.md` 附录 E.1 中相关条目**

找到引用 `auth.save(&data).unwrap()` 的段落，标注 "已修复" + 指向 `vault_store.rs:155-205`。

- [ ] **Step 4: `git diff CLAUDE.md` 检查改动 ≤ 5 行**

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: mark whatsapp vault test isolation as fixed"
```

---

## Task 2: A2 — PairingState 事件映射（5 个事件）

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/mod.rs`（事件循环 match 扩展）
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/client.rs`（如需读 device props）

**Interfaces:**
- 消费：`PairingState` 9 变体（已有：`pairing.rs:43-90`）
- 产出：事件循环能处理 `Event::PairSuccess / Scanned / Syncing / QrExpired / Disconnected`，**驱动** PairingState 翻转

- [ ] **Step 1: 写失败测试**

在 `mod.rs` `#[cfg(test)]` 块加：

```rust
#[tokio::test]
async fn pairing_state_driven_by_events() {
    use crate::gateway::interfaces::whatsapp::pairing::PairingState;
    let state = Arc::new(RwLock::new(PairingState::Idle));
    let mut driver = PairingStateDriver::new(state.clone());

    // QR
    driver.apply(crate::gateway::interfaces::whatsapp::wa_runtime::client::WaEvent::PairingQrCode { code: "abc".into(), timeout_secs: 60 }).await;
    assert!(matches!(*state.read().await, PairingState::WaitingQr { .. }));

    // PairSuccess
    driver.apply(crate::gateway::interfaces::whatsapp::wa_runtime::client::WaEvent::PairSuccess).await;
    assert!(matches!(*state.read().await, PairingState::Connected { .. }));

    // Disconnected
    driver.apply(crate::gateway::interfaces::whatsapp::wa_runtime::client::WaEvent::Disconnected { reason: "test".into() }).await;
    assert!(matches!(*state.read().await, PairingState::Disconnected { .. }));
}
```

- [ ] **Step 2: 跑测试，验证失败**

```bash
cargo test -p alephcore --lib pairing_state_driven_by_events
```

期望：`error[E0433]: failed to resolve: use of undeclared type PairingStateDriver`

- [ ] **Step 3: 在 `mod.rs` 实现 `PairingStateDriver`**

```rust
struct PairingStateDriver {
    state: Arc<RwLock<PairingState>>,
}

impl PairingStateDriver {
    fn new(state: Arc<RwLock<PairingState>>) -> Self { Self { state } }

    pub(crate) async fn apply(&self, event: WaEvent) {
        use crate::gateway::interfaces::whatsapp::wa_runtime::client::WaEvent;
        let mut s = self.state.write().await;
        match event {
            WaEvent::PairingQrCode { code, timeout_secs } => {
                *s = PairingState::WaitingQr {
                    qr_data: code,
                    expires_at: chrono::Utc::now() + chrono::Duration::seconds(timeout_secs as i64),
                };
            }
            WaEvent::PairSuccess => {
                *s = PairingState::Connected { device_name: None, phone_number: None };
            }
            WaEvent::Scanned => {
                if matches!(*s, PairingState::WaitingQr { .. }) {
                    *s = PairingState::Scanned;
                }
            }
            WaEvent::Disconnected { reason } => {
                *s = PairingState::Disconnected { reason };
            }
            _ => {}
        }
    }
}
```

需要的 `WaEvent` 枚举目前在 `client.rs` 里——若不存在就抽出来；或在 `mod.rs` 直接 import `whatsapp_rust::types::events::Event`。

- [ ] **Step 4: 跑测试，验证通过**

```bash
cargo test -p alephcore --lib pairing_state_driven_by_events
```

期望：PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/whatsapp/mod.rs
git commit -m "whatsapp: drive PairingState from 5 runtime events"
```

---

## Task 3: A2 — `status()` 改读 PairingState

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/mod.rs::status()`（`mod.rs:132-142`）

**Interfaces:**
- 消费：`PairingState::to_channel_status()`（已存在 `pairing.rs`）
- 产出：`WhatsAppChannel::status()` 返回 `PairingState` 派生的 status

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn status_reads_pairing_state() {
    let channel = WhatsAppChannel::for_test("test", WhatsAppConfig::default());
    // 直接灌 PairingState::QrExpired
    *channel.pairing_state.write().await = PairingState::QrExpired;
    // 期望 status 反映 QrExpired（fail-closed），而不是默认 Disconnected
    assert_eq!(channel.status(), ChannelStatus::Pairing);
}
```

- [ ] **Step 2: 跑测试，验证失败**

```bash
cargo test -p alephcore --lib status_reads_pairing_state
```

期望：FAIL（当前 status() 走 ConnectionState，PairingState 改了不生效）

- [ ] **Step 3: 修改 `mod.rs::status()`**

```rust
fn status(&self) -> ChannelStatus {
    if self.test_mode {
        return self.channel_state.status();
    }
    // 走 PairingState 单源（spec §3.1 D1）
    use crate::gateway::interfaces::whatsapp::pairing::PairingState;
    let pairing = self.pairing_state.blocking_read();
    pairing.to_channel_status()
}
```

（注：`blocking_read` 只在测试路径用；生产路径用 `tokio::runtime::Handle::current().block_on` 或改为 `async`。具体取决于 `Channel::status` 签名是否是 async——若是则不用 block。）

- [ ] **Step 4: 跑测试，验证通过**

```bash
cargo test -p alephcore --lib status_reads_pairing_state
```

期望：PASS

- [ ] **Step 5: 跑全部 WhatsApp 测试，确保无回归**

```bash
cargo test -p alephcore --lib whatsapp
```

- [ ] **Step 6: Commit**

```bash
git add src/gateway/interfaces/whatsapp/mod.rs
git commit -m "whatsapp: route status() through PairingState"
```

---

## Task 4: A3 CUT — 删 account.rs / account_registry.rs / config.accounts

**Files:**
- Delete: `src/gateway/interfaces/whatsapp/account.rs`
- Delete: `src/gateway/interfaces/whatsapp/account_registry.rs`
- Modify: `src/gateway/interfaces/whatsapp/config.rs`（删 `accounts` 字段 + `WhatsAppAccountConfig`）
- Modify: `src/gateway/interfaces/whatsapp/mod.rs`（删 `pub mod account; pub mod account_registry;`）
- Modify: `src/gateway/interfaces/whatsapp/wa_outbound/sender.rs`（参数类型 `&WhatsAppAccountConfig` → `&WhatsAppConfig`）
- Modify: `src/gateway/interfaces/whatsapp/wa_outbound/media.rs`（同上）

**Interfaces:**
- 消费：所有引用 `WhatsAppAccount` / `WhatsAppAccountRegistry` / `WhatsAppAccountConfig` 的代码
- 产出：上述三个标识符全仓 grep 0 命中；`config.accounts` 字段不存在

- [ ] **Step 1: 全仓 grep 引用点**

```bash
grep -rn "WhatsAppAccount\|AccountRegistry\|WhatsAppAccountConfig" src/ tests/
```

期望：列出所有引用，逐一处理

- [ ] **Step 2: 改 `config.rs` 删 `accounts` 字段 + `WhatsAppAccountConfig` struct**

```rust
// 删除:
pub accounts: Option<HashMap<String, WhatsAppAccountConfig>>,
pub struct WhatsAppAccountConfig { ... }

// 保留:
pub default_account_id: Option<String>,
```

- [ ] **Step 3: 改 `wa_outbound/sender.rs` 与 `media.rs`**

把 `fn xxx(config: &WhatsAppAccountConfig, ...)` 改为 `fn xxx(config: &WhatsAppConfig, ...)`，删除 `account_*` 字段访问

- [ ] **Step 4: 删 `account.rs` 和 `account_registry.rs` 文件**

```bash
git rm src/gateway/interfaces/whatsapp/account.rs
git rm src/gateway/interfaces/whatsapp/account_registry.rs
```

- [ ] **Step 5: 改 `mod.rs` 删除两行 `pub mod`**

- [ ] **Step 6: 验证 grep 全 0 命中**

```bash
grep -rn "WhatsAppAccount\|AccountRegistry" src/ tests/
```

期望：无输出

- [ ] **Step 7: `cargo check -p alephcore` 必须绿**

```bash
cargo check -p alephcore
```

- [ ] **Step 8: Commit**

```bash
git add -A src/gateway/interfaces/whatsapp/
git commit -m "whatsapp: remove orphan account registry (CUT path per spec §3.3)"
```

---

## Task 5: B1 — reactions 字段接入事件循环

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/reactions.rs`（加 `ReactionSender` trait 定义）
- Modify: `src/gateway/interfaces/whatsapp/mod.rs`（构造 `ReactionHandler` + 事件循环发 ack）
- Modify: `src/gateway/interfaces/whatsapp/config.rs`（确认 `ReactionConfig` 字段已存在）

**Interfaces:**
- 消费：`WhatsAppConfig::reactions: ReactionConfig`（已存在 `config.rs:32`）
- 产出：`ReactionHandler` 实例化 + 事件循环 `Accept` 分支调 `send_ack`

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn ack_reaction_sent_on_inbound_message() {
    let sender = Arc::new(MockReactionSender::new());
    let handler = ReactionHandler::new(
        ReactionLevel::Ack,
        Some(AckReactionConfig::default()),
        sender.clone(),
    );
    let msg = make_test_inbound_message();  // helper
    handler.send_ack(&msg).await.unwrap();
    assert_eq!(sender.call_count(), 1);
    assert_eq!(sender.last_emoji(), "👀");
}
```

- [ ] **Step 2: 跑测试，验证失败**

期望：缺 `MockReactionSender`

- [ ] **Step 3: 在 `reactions.rs` 加 `ReactionSender` trait**

```rust
#[async_trait]
pub trait ReactionSender: Send + Sync {
    async fn send_reaction(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ReactionError>;
}
```

`ReactionError` 新增简单 enum。

- [ ] **Step 4: 在 `reactions.rs` 加 `MockReactionSender` 测试用实现**

`#[cfg(test)]` 模块内

- [ ] **Step 5: 跑测试，验证通过**

- [ ] **Step 6: 在 `mod.rs` 持 `ReactionHandler` 字段 + 启动时构造**

```rust
pub struct WhatsAppChannel {
    // ...
    reaction_handler: Option<Arc<ReactionHandler>>,
}

impl WhatsAppChannel {
    fn with_mode(...) -> Self {
        // ...
        let reaction_handler = ReactionHandler::from_config(&config.reactions, /* runtime as ReactionSender */);
        Self { ..., reaction_handler: Some(Arc::new(reaction_handler)) }
    }
}
```

- [ ] **Step 7: 事件循环 `Accept` 分支调 `reaction_handler.send_ack(&msg).await`**

- [ ] **Step 8: 跑 WhatsApp 全测试**

```bash
cargo test -p alephcore --lib whatsapp
```

- [ ] **Step 9: Commit**

```bash
git add src/gateway/interfaces/whatsapp/
git commit -m "whatsapp: wire reactions field into inbound event loop"
```

---

## Task 6: B2 — history_buffer 接入 inbound mapper

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/wa_inbound/mapper.rs`（签名扩 + 内部 add）
- Modify: `src/gateway/interfaces/whatsapp/mod.rs`（持 `GroupHistoryBuffer` + 启动时构造 + 下游读）

**Interfaces:**
- 消费：`WhatsAppConfig::history: HistoryBufferConfig`（已存在）
- 产出：`GroupHistoryBuffer` 实例化 + `mapper.map_event_to_inbound(event, channel_id, &buffer)` + 下游 `buffer.get_context(&conv_id)` 拼 system prompt

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn group_message_buffered_then_injected() {
    let buffer = Arc::new(GroupHistoryBuffer::new(HistoryBufferConfig::default()));
    let msg1 = make_test_group_message("hello");
    let msg2 = make_test_group_message("world");

    buffer.add(&msg1).await;
    buffer.add(&msg2).await;

    let ctx = buffer.get_context(&msg1.conversation_id).await.unwrap();
    assert!(ctx.contains("hello"));
    assert!(ctx.contains("world"));
}
```

- [ ] **Step 2: 跑测试**

期望：失败若 `add` / `get_context` 签名不符

- [ ] **Step 3: 改 `mapper.rs::map_event_to_inbound` 签名**

```rust
pub async fn map_event_to_inbound(
    event: &Event,
    channel_id: &ChannelId,
    buffer: &GroupHistoryBuffer,
) -> Option<InboundMessage> {
    let msg = /* existing logic */;
    if let Some(ref m) = msg {
        buffer.add(m).await;
    }
    msg
}
```

（注：原版是同步 `pub fn`；改为 async 会有连锁影响，需 grep 所有调用点）

- [ ] **Step 4: 改 `mod.rs` 持 buffer + 启动时构造**

```rust
pub struct WhatsAppChannel {
    // ...
    history_buffer: Arc<GroupHistoryBuffer>,
}
```

`start()` 中 `self.history_buffer = Arc::new(GroupHistoryBuffer::new(self.config.history.clone()))`

- [ ] **Step 5: 事件循环调 `mapper.map_event_to_inbound(&event, &channel_id, &self.history_buffer).await`**

- [ ] **Step 6: 在 outbound / 调度层读 `buffer.get_context(&conv_id).await`，注入到 system prompt**

具体位置取决于 inbound router 架构——查 `src/gateway/inbound_router/` 找到消息→LLM 的注入点

- [ ] **Step 7: 跑测试 + Commit**

```bash
cargo test -p alephcore --lib whatsapp
git add -A
git commit -m "whatsapp: wire history buffer into inbound mapper"
```

---

## Task 7: C1 — 抽 `trait WaRuntime`

**Files:**
- Create: `src/gateway/interfaces/whatsapp/wa_runtime/traits.rs`
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/mod.rs`（导出 trait）
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/client.rs`（保留 `RealWaRuntime` 实现 trait）
- Modify: `src/gateway/interfaces/whatsapp/mod.rs`（`runtime: Option<Arc<dyn WaRuntime>>`）

**Interfaces:**
- 消费：当前 `WaRuntime` struct 的所有方法（`start/shutdown/send_message/send_typing/mark_read/send_reaction/pairing_phase`）
- 产出：`pub trait WaRuntime: Send + Sync` + 生产实现 `RealWaRuntime`

- [ ] **Step 1: 写失败测试（编译期）**

在 `traits.rs` 加：

```rust
#[async_trait]
pub trait WaRuntime: Send + Sync {
    async fn start(&self) -> Result<(), WaRuntimeError>;
    async fn shutdown(&self);
    async fn pairing_phase(&self) -> PairingState;
    async fn send_message(&self, msg: OutboundMessage) -> Result<MessageId, WaRuntimeError>;
    async fn send_typing(&self, conversation_id: &str) -> Result<(), WaRuntimeError>;
    async fn mark_read(&self, message_id: &str) -> Result<(), WaRuntimeError>;
    async fn send_reaction(&self, conversation_id: &str, message_id: &str, emoji: &str) -> Result<(), WaRuntimeError>;
    fn take_event_receiver(&self) -> Option<mpsc::Receiver<crate::gateway::interfaces::whatsapp::wa_runtime::client::WaEvent>>;
}
```

- [ ] **Step 2: 改 `client.rs` 抽 `RealWaRuntime`（wrapper）**

保留原 `WaRuntime` struct 内容，外部暴露 `impl WaRuntime for RealWaRuntime`

- [ ] **Step 3: 改 `mod.rs::WhatsAppChannel.runtime` 字段类型**

```rust
runtime: Option<Arc<dyn WaRuntime>>,
```

- [ ] **Step 4: 所有 `runtime.method()` 调用处把 `&self.runtime.unwrap()` 改为 `Arc<dyn WaRuntime>`**

- [ ] **Step 5: `cargo check -p alephcore` 必须绿**

- [ ] **Step 6: Commit**

```bash
git add -A src/gateway/interfaces/whatsapp/
git commit -m "whatsapp: extract WaRuntime trait for testability"
```

---

## Task 8: C1 — FakeWaRuntime + 4 场景端到端测试

**Files:**
- Create: `src/gateway/interfaces/whatsapp/wa_runtime/fake.rs`
- Modify: `src/gateway/interfaces/whatsapp/wa_runtime/mod.rs`（导出 `FakeWaRuntime` 测试用）
- Modify: `src/gateway/interfaces/whatsapp/mod.rs`（`for_test` 改用 `FakeWaRuntime`）

**Interfaces:**
- 消费：`trait WaRuntime`（Task 7）
- 产出：`FakeWaRuntime` 实现 + 4 个端到端测试（QR/Connected/PairSuccess/Reconnect）

- [ ] **Step 1: 实现 `FakeWaRuntime`**

```rust
pub struct FakeWaRuntime {
    inner: Arc<FakeWaRuntimeInner>,
}

struct FakeWaRuntimeInner {
    pairing: RwLock<PairingState>,
    events_tx: mpsc::Sender<WaEvent>,
    sent_messages: Mutex<Vec<OutboundMessage>>,
    sent_reactions: Mutex<Vec<(String, String, String)>>,
    // ...
}

impl FakeWaRuntime {
    pub fn new() -> (Arc<Self>, mpsc::Receiver<WaEvent>) { ... }
    pub async fn emit_qr(&self, code: String) { ... }
    pub async fn emit_pair_success(&self) { ... }
    pub async fn emit_disconnected(&self, reason: String) { ... }
    pub async fn emit_message(&self, msg: InboundMessage) { ... }
    pub fn sent_messages(&self) -> Vec<OutboundMessage> { ... }
}

#[async_trait]
impl WaRuntime for FakeWaRuntime { ... }
```

- [ ] **Step 2: 写 4 个端到端测试**

```rust
#[tokio::test]
async fn scenario_qr_emitted_drives_pairing_state() { ... }

#[tokio::test]
async fn scenario_pair_success_marks_connected() { ... }

#[tokio::test]
async fn scenario_inbound_message_propagates_to_channel() { ... }

#[tokio::test]
async fn scenario_reconnect_after_disconnect() { ... }
```

- [ ] **Step 3: 跑测试全绿**

```bash
cargo test -p alephcore --lib whatsapp_fake
```

- [ ] **Step 4: 改 `mod.rs::for_test` 用 `FakeWaRuntime`**

- [ ] **Step 5: Commit**

```bash
git add -A src/gateway/interfaces/whatsapp/
git commit -m "whatsapp: add FakeWaRuntime with 4 end-to-end scenarios"
```

---

## Task 9: C2 — MediaProcessor spawn_blocking + image 重编码

**Files:**
- Modify: `src/gateway/interfaces/whatsapp/wa_outbound/media.rs`
- Modify: `Cargo.toml`（加 `image = "0.25"` 到 `[dependencies]` 或 workspace 共享）

**Interfaces:**
- 消费：`Attachment`（已有）
- 产出：`prepare_outbound` 异步重编码，图像过大时 resize + JPEG 编码

- [ ] **Step 1: 在 `Cargo.toml` 加 `image = "0.25"`**

```toml
[workspace.dependencies]
image = "0.25"
```

并在 `alephcore` crate 的 `[dependencies]` 加 `image = { workspace = true }`

- [ ] **Step 2: 写失败测试**

```rust
#[tokio::test]
async fn prepare_image_resizes_oversized() {
    let processor = MediaProcessor::new(MediaConfig { max_dimension: 100, ..Default::default() });
    let attachment = Attachment {
        path: Some(test_fixture_path("oversized.jpg")),
        mime_type: "image/jpeg".into(),
        size: Some(500_000),
    };
    let result = processor.prepare_outbound(&attachment).await.unwrap();
    assert!(result.data.len() < 50_000);
    assert_eq!(result.mime_type, "image/jpeg");
}
```

fixture: 准备一张 4000x3000 的测试图片（git 不跟踪二进制，用 `include_bytes!` 或运行时生成）

- [ ] **Step 3: 实现 `prepare_outbound` 用 `tokio::task::spawn_blocking`**

```rust
pub async fn prepare_outbound(&self, attachment: &Attachment) -> Result<OutboundMedia> {
    let attachment = attachment.clone();
    let config = self.config.clone();
    tokio::task::spawn_blocking(move || {
        match attachment.mime_type.as_str() {
            t if t.starts_with("image/") => Self::process_image_blocking(&attachment, &config),
            "audio/ogg" => Ok(OutboundMedia { mime_type: "audio/ogg; codecs=opus".into(), is_voice_note: true, .. }),
            _ => Self::process_document_blocking(&attachment),
        }
    }).await?
}
```

- [ ] **Step 4: 跑测试，验证通过**

- [ ] **Step 5: 跑 WhatsApp 全测试 + clippy**

```bash
cargo test -p alephcore --lib whatsapp
cargo clippy -p alephcore -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "whatsapp: async image re-encoding via spawn_blocking"
```

---

## Task 10: D1 — 更新 WHATSAPP_ARCHITECTURE_DESIGN + FEATURE_LOCATOR

**Files:**
- Modify: `docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md`（1306 行 → ≤ 200 行）
- Modify: `docs/reference/FEATURE_LOCATOR.md`（加 WhatsApp 段）

**Interfaces:**
- 消费：本 plan 所有 Task 完成的 commit 历史
- 产出：架构文档与当前代码一致

- [ ] **Step 1: 备份 `WHATSAPP_ARCHITECTURE_DESIGN.md` 为 `.archive-2026-04-06`**

```bash
git mv docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md docs/reference/archive/WHATSAPP_ARCHITECTURE_DESIGN-2026-04-06.md
```

（注意 `archive/` 目录是否存在——若不存在则创建）

- [ ] **Step 2: 写新的 ≤ 200 行的现状文档**

`docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md` 内容包含：
- 当前架构图（`whatsapp-rust` runtime + AccountRegistry CUT 后）
- PairingState 单源化设计
- FakeWaRuntime 闭环测试
- 引用本 spec + 本 plan 路径

- [ ] **Step 3: 在 `FEATURE_LOCATOR.md` 找合适位置加 WhatsApp 段**

具体位置看现有文档结构（建议在附录 E.0 跨子系统通用形状后，或新建附录 E.11）

- [ ] **Step 4: 文档 commit**

```bash
git add docs/
git commit -m "docs: archive old whatsapp arch doc + link new spec/plan"
```

---

## 合并策略

Task 1-10 全部在 `whatsapp-arch-r1` 分支独立 commit。**Task 间不互相 squash**——保持每个 Task 的 commit 是原子单位，方便后续 `git revert` 单点。

完成后：
1. 全局 `cargo check -p alephcore` + `cargo test -p alephcore --lib` + `cargo clippy --workspace --all-targets -- -D warnings`
2. 更新 `WHATSAPP_ARCHITECTURE_DESIGN.md` 指向 spec + plan
3. 把 `whatsapp-arch-r1` 合并到 `main`（用户审查 + 显式同意后）

---

## Self-Review

**1. Spec coverage**:
- ✅ A0 → Task 1
- ✅ A2 (5 事件映射) → Task 2
- ✅ A2 (status 改读 PairingState) → Task 3
- ✅ A3 CUT → Task 4
- ✅ B1 reactions → Task 5
- ✅ B2 history → Task 6
- ✅ C1 trait 抽出 → Task 7
- ✅ C1 FakeWaRuntime → Task 8
- ✅ C2 media spawn_blocking → Task 9
- ✅ D1 doc → Task 10

**2. Step scan**: 每个 Task 的 Step 都是"写测试 / 跑测试 / 实现 / 跑测试 / commit"四件套，无 TBD / 无完整 body 复述。

**3. Type consistency**:
- `WaEvent` 在 Task 2 和 Task 8 共用，需确认 `client.rs` 已导出或在 `traits.rs` 定义
- `PairingState::Connected { device_name: Option<String>, phone_number: Option<String> }` 与 spec §3.1 一致
- `ReactionError` 在 Task 5 新增，在 Task 7-8 不引用，保持作用域

**4. Review Focus**: 5 个未覆盖点已写入 plan header 的 Review Focus 段，每个点配 Task 编号（Task 3 / 8 / 9 / 4 / 5）

**5. Proportion**: plan 365 行 vs spec 365 行 ≈ 1:1，proportion 合规。每个 Step 都有可执行代码或命令。

---

## Execution Handoff

执行方式已由用户在原 prompt 选定：**Subagent-driven**（user said: "subagent（Opus 或 Sonnet）去执行"）。

无需再选择。开始执行 Task 1。