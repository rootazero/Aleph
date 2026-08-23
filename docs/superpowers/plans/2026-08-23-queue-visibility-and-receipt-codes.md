# 队列可见性统一 + 错误码跨端收口 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让「排队等待」成为 wire 上的一等状态（客户端不再拿「运行中」冒充它），并把错误分类从 Panel 的关键词猜测收口到服务端已经在发的 `error_code`。

**Architecture:** 新增**一个** `StreamEvent::RunQueued{run_id, session_key, ahead}`，由每个等待者在 `deliver_with_ticket` **已有**的唤醒点自报位置——零新增唤醒边、零轮询、静态车道表里不存 emitter。`RunAccepted` 保持不变，从此是"准入"边。空闲会话一帧都不多发。第二支柱把 `i18n::ReceiptKind::code()` 的码表下沉到 `aleph_protocol::receipt`，Panel 改读 `error_code`，关键词分类器降级为兜底。

**Tech Stack:** Rust（tokio · serde · schemars）· Leptos/WASM（Panel）· 跨 crate 共享类型 `aleph-protocol`

**Spec:** `docs/superpowers/specs/2026-08-23-queue-visibility-and-receipt-codes-design.md`

## Global Constraints

- **分支**：`worktree-queue-visibility-round11`，基线 `064d036fc`。**严禁触碰 main。**
- **R10**：`src/harness/` **零改动**。本轮任何任务都不得在该目录下增删一行。
- **R4**：所有逻辑落在 gateway I/O 边界与 Panel，不下沉到 domain。
- **提交信息**：`<scope>: <description>`，英文。scope 用 `gateway` / `panel` / `protocol` / `docs`。
- **注释与 doc comment 一律英文**；本计划的中文说明不进代码。
- **wire code 只能加不能改拼写**（客户端 switch 得到）。现有 8 个：`TIMEOUT` `CANCELLED` `AGENT_BUSY` `RATE_LIMITED` `AUTH` `PROVIDERS_UNREACHABLE` `FAILED` `SPEND_EXHAUSTED`。注意 `Unreachable` 的码是 `PROVIDERS_UNREACHABLE`，**与变体名不同**。
- **每条新守卫写完必须手动破坏一次**，确认它红且点得出文件行号。变异结果四分法**按此顺序**判：`running 0 tests` ⇒ VACUOUS → `test result: FAILED` ⇒ RED → `test result: ok` ⇒ GREEN → 剩下的（连 `test result:` 行都没有）才是 BUILD-ERROR。**cargo 对测试失败也打 `^error:`**，不要按它排序。
- **验证命令**（判据清单 §10 最小验证集，任务里逐条指定用哪些）：
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p aleph-panel --lib --no-run
  cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
  cargo clippy --all-targets
  ```
  第 3 条是**唯一编译出厂形态**的命令（`--lib` 测试构建里 `cfg(test)` 为真，看不见出厂形态的错误）。

---

## File Structure

**服务端（`src/`）**

| 文件 | 责任 | 任务 |
|---|---|---|
| `src/gateway/busy_queue/mod.rs` | 车道仍是纯候车室；新增两个**只读**查询 `TicketGuard::ahead()` 与 `pending_for()` | 1, 4 |
| `src/gateway/busy_queue/deliver.rs` | 等待循环新增 `report` 参数 + 去重 | 3 |
| `src/gateway/busy_queue/spawn.rs` | Panel/CLI 到达路径：构造 report 闭包 | 3 |
| `src/gateway/inbound_router/executor.rs` | channel 到达路径：同一个闭包 | 3 |
| `src/gateway/event_emitter/types.rs` | `StreamEvent::RunQueued` | 2 |
| `src/gateway/event_emitter/redacting.rs` | pass-through 臂（**穷尽 match，不加会编译错**） | 2 |
| `src/gateway/event_emitter/mod.rs` | CUT `emit_run_error` | 10 |
| `src/gateway/events/frame.rs` | `GatewayEventFrame::RunQueued` + `From` 臂 + `stream_method` 臂 | 2 |
| `src/gateway/handlers/chat.rs` | `chat.history` 响应加 `pending` | 4 |
| `src/gateway/i18n.rs` | `ReceiptKind::code()` 改为读协议表 | 5 |

**协议（`shared/protocol/`）**

| 文件 | 责任 | 任务 |
|---|---|---|
| `shared/protocol/src/receipt.rs` | **新建**：`ReceiptCode` 枚举 + `as_wire()`/`from_wire()`/`ALL` | 5 |
| `shared/protocol/src/lib.rs` | `pub mod receipt;` | 5 |

**Panel（`interfaces/webchat/`）**

| 文件 | 责任 | 任务 |
|---|---|---|
| `.../chat/state.rs` → `.../chat/state/mod.rs` | `git mv`，其余内容不动 | 6 |
| `.../chat/state/send_error.rs` | **新建**：`ChatSendErrorCode` + `ChatSendError` + `from_wire_code` | 6 |
| `.../chat/state/run_phase.rs` | **新建**：`ChatPhase`（加 `Queued`） | 7 |
| `.../chat/events.rs` | `run_queued` 路由与渲染 + `run_error` 传 `error_code` | 8 |
| `.../chat/messages.rs` · `reasoning.rs` · `composer/mod.rs` · `phone/chat/composer.rs` · `team_events.rs` | 穷尽 match 逼出的 5 个读者 | 7 |
| `interfaces/webchat/src/api/chat.rs` | `SessionHistory.pending` | 9 |

**文档**：`docs/reference/FEATURE_LOCATOR.md` · `docs/reference/GATEWAY.md`（任务 11）

**依赖顺序**：1 → 2 → 3 → 4 独立成链；5 → 6 独立成链；7 → 8 → 9 依赖 2；10、11 最后。**任务 1–4 与 5–6 可并行**。

---

### Task 1: `TicketGuard::ahead()` —— 车道的只读位置查询

**Files:**
- Modify: `src/gateway/busy_queue/mod.rs`（`impl TicketGuard` 块内，紧接 `is_front` 之后；`#[cfg(test)] mod tests` 内加测试）

**Interfaces:**
- Consumes: 无（纯新增）
- Produces: `pub fn TicketGuard::ahead(&self) -> u16`

**背景**：`Lane.tickets` 是 `VecDeque<Ticket>`，位置天然可导出。`snapshot()` 的 doc 已经立了规矩——**cancelled 的票不算**（"`total_waiting` means what it says: messages that may still run"）。`ahead()` 必须沿用同一口径，否则同一个数在两个面上不一样。

- [ ] **Step 1: 写失败测试**

在 `src/gateway/busy_queue/mod.rs` 的 `#[cfg(test)] mod tests` 里追加：

```rust
/// `ahead` counts messages that may still run, matching `snapshot`'s
/// `total_waiting` contract — a cancelled ticket ahead of me will never
/// become a run, so reporting it would tell the user to wait for something
/// that is already gone.
#[test]
fn ahead_counts_only_tickets_that_may_still_run() {
    let s = "sess-ahead-live";
    let a = register(s, CAP, "run-a").expect("a");
    let b = register(s, CAP, "run-b").expect("b");
    let c = register(s, CAP, "run-c").expect("c");

    assert_eq!(a.ahead(), 0, "front ticket has nobody ahead of it");
    assert_eq!(b.ahead(), 1);
    assert_eq!(c.ahead(), 2);

    // Cancelling the middle one must shrink what `c` is told to wait for.
    assert!(cancel_queued_run("run-b"));
    assert_eq!(c.ahead(), 1, "a cancelled predecessor is not a wait");
    assert_eq!(a.ahead(), 0);
    drop((a, b, c));
}

/// Fail-open, same posture as `is_front` / `drain_epoch`: a ticket whose
/// lane or entry is gone reports "nobody ahead". Reporting a stale positive
/// would park a client's UI on a wait that no longer exists.
#[test]
fn ahead_fails_open_when_the_ticket_left_the_lane() {
    let s = "sess-ahead-gone";
    let a = register(s, CAP, "run-a").expect("a");
    let b = register(s, CAP, "run-b").expect("b");
    assert_eq!(b.ahead(), 1);

    // `mark_admitted` withdraws a ticket without touching its guard.
    mark_admitted(s, "run-a");
    assert_eq!(b.ahead(), 0, "the ticket ahead was withdrawn");

    mark_admitted(s, "run-b");
    assert_eq!(b.ahead(), 0, "own ticket withdrawn reads as fail-open 0");
    drop((a, b));
}
```

- [ ] **Step 2: 跑测试确认它红**

```
cargo test -p alephcore --lib busy_queue::tests::ahead_ -- --nocapture
```
Expected: **BUILD-ERROR**（`no method named ahead`）。这是本任务唯一允许的非 RED 起点——方法还不存在。

- [ ] **Step 3: 实现**

在 `impl TicketGuard` 里，紧接 `is_front` 之后插入：

```rust
    /// How many messages ahead of this one may still run.
    ///
    /// The wire value behind `StreamEvent::RunQueued.ahead`, so it answers the
    /// only question a waiting user asks. Counts with `snapshot`'s contract,
    /// not the raw deque length: a cancelled ticket ahead of me will never
    /// become a run, and telling the user to wait for it is the same class of
    /// lie `mark_admitted` removed on the other side.
    ///
    /// Fails **open** (`0`) when this ticket is no longer in its lane —
    /// withdrawn by `mark_admitted`, dropped, or the lane garbage-collected —
    /// the same posture as [`Self::is_front`] and [`Self::drain_epoch`]. `0`
    /// renders as "about to start", and whatever happens next (`RunAccepted`
    /// or `RunError`) overwrites that phase, so a stale open answer costs one
    /// frame, never a stuck UI.
    ///
    /// Saturates at `u16::MAX`; the lane cap (`max_per_session`, default 32)
    /// is three orders of magnitude below it.
    #[must_use]
    pub fn ahead(&self) -> u16 {
        let map = lock();
        let Some(lane) = map.get(&self.session_key) else {
            return 0;
        };
        let mut ahead = 0u16;
        for t in &lane.tickets {
            if t.id == self.ticket {
                return ahead;
            }
            if !t.cancelled {
                ahead = ahead.saturating_add(1);
            }
        }
        0
    }
```

- [ ] **Step 4: 跑测试确认它绿**

```
cargo test -p alephcore --lib busy_queue::tests::ahead_ -- --nocapture
```
Expected: `test result: ok. 2 passed`

- [ ] **Step 5: 变异证伪**

把 `if !t.cancelled {` 临时改成 `if true {`，重跑上面的命令。
Expected: **RED**，且失败点名 `ahead_counts_only_tickets_that_may_still_run`。确认后改回。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/busy_queue/mod.rs
git commit -m "gateway: add TicketGuard::ahead, the lane's read-only position query"
```

---

### Task 2: `RunQueued` 帧穿过整条管线

**Files:**
- Modify: `src/gateway/event_emitter/types.rs`（`enum StreamEvent`，紧接 `RunAccepted` 之后）
- Modify: `src/gateway/event_emitter/redacting.rs:241`（pass-through 臂）
- Modify: `src/gateway/events/frame.rs`（`enum GatewayEventFrame` + `From<StreamEvent>` + `stream_method`）
- Modify: `shared/protocol/src/events.rs`（**`aleph_protocol::StreamEvent` 的孪生变体 + `run_id()` 的臂**——见下方裁定 R7）
- Test: `src/gateway/events/frame.rs` 的 `#[cfg(test)] mod tests`
- Test: `src/gateway/event_visibility.rs` 的 `#[cfg(test)] mod tests`

**⚠️ 控制器裁定 R7（补计划初稿的一个缺口）**：本仓有**两个** `StreamEvent`——`crate::gateway::StreamEvent`（服务端内部）与 `aleph_protocol::StreamEvent`（TUI / CLI / `shared/client` 解码的那个）。`events/frame_census.rs` 的 `every_stream_method_has_a_typed_twin_in_the_protocol_enum` 要求每个 `stream.*` method 在**协议**那个枚举里有孪生，否则每个终端客户端在 `connection.rs` 静默丢帧。初稿只说"`RunQueued` 有 `StreamEvent` 孪生"，那句话对内部枚举成立、对协议枚举不成立。**所以本任务多一处改动**（3f），而 `PANEL_ONLY_STREAM_METHODS` 仍然**不要**碰——那张表是给真的没有孪生的三个帧用的，且它有一条"只减不增"的守卫。

**Interfaces:**
- Consumes: 无
- Produces: `StreamEvent::RunQueued { run_id: String, session_key: String, ahead: u16 }`；`GatewayEventFrame::RunQueued { .. }`（同字段）；wire method `stream.run_queued`

**⚠️ 本任务的陷阱**：`frame.rs::stream_method` 结尾是 `_ => None`（`frame.rs:768`）。**一个没有对应臂的新变体不会编译错，它会静默地拿不到 method、从此永远到不了任何客户端。** 所以 Step 1 的测试必须先钉住这一点。

`redacting.rs` 相反——它**穷尽无通配**（`redacting.rs:81` 的注释逐字说明这是有意的），所以不加臂就是编译错误。

- [ ] **Step 1: 写失败测试**

在 `src/gateway/events/frame.rs` 的 `#[cfg(test)] mod tests` 里追加：

```rust
    /// A new variant that nobody gives a `stream_method` arm falls into the
    /// catch-all `_ => None` and is silently unroutable — it is built,
    /// converted, and then dropped before any client sees it. That failure is
    /// not a compile error, so it needs this.
    #[test]
    fn run_queued_has_a_wire_method_and_survives_conversion() {
        let frame = GatewayEventFrame::from(StreamEvent::RunQueued {
            run_id: "run-1".to_string(),
            session_key: "agent/main".to_string(),
            ahead: 2,
        });
        assert_eq!(frame.stream_method(), Some("stream.run_queued"));

        let GatewayEventFrame::RunQueued {
            run_id,
            session_key,
            ahead,
        } = frame
        else {
            panic!("conversion dropped the variant");
        };
        assert_eq!(run_id, "run-1");
        assert_eq!(ahead, 2);
        assert_eq!(
            session_key, "agent/main",
            "the frame must name its session: it is the FIRST frame of a run, \
             so nothing has seeded the run→session index yet"
        );
    }
```

- [ ] **Step 2: 跑测试确认它红**

```
cargo test -p alephcore --lib events::frame::tests::run_queued_has_a_wire_method
```
Expected: **BUILD-ERROR**（`no variant named RunQueued`）。

- [ ] **Step 3: 实现（四处）**

**3a.** `src/gateway/event_emitter/types.rs`，紧接 `RunAccepted { .. }` 变体之后：

```rust
    /// The run joined its session's wait lane and has not been admitted yet.
    ///
    /// The first frame of a run that has to wait, and the only representation
    /// waiting has ever had on the wire. Before it, a queued run emitted
    /// nothing at all between `chat.send` returning its id and
    /// `RunAccepted` — so every client painted "thinking" for a run the
    /// engine had never heard of.
    ///
    /// Not emitted at all on the common path: an idle session takes the front
    /// of the lane immediately and runs, so nothing new appears on a
    /// conversation that was not already waiting.
    ///
    /// # Why it names its session
    ///
    /// `EventVisibilityIndex`'s run→session seed came only from
    /// `RunAccepted`, which is emitted *post-admission*, so nothing during the
    /// wait could be resolved. Carrying `session_key` moves that seed from
    /// admission to **arrival** — the moment the run actually became
    /// addressable, since `chat.send` has already handed the id to the caller.
    /// `note_frame` runs ahead of the filter, so the frame seeds itself.
    ///
    /// Deliberately has no `seq`: its sibling `RunAccepted` has none either,
    /// and nothing reads `seq` for ordering or de-duplication.
    RunQueued {
        run_id: String,
        /// The session the run was **addressed to** — not the derived
        /// execution lane a `/btw` side question runs on. A client is attached
        /// to the former; the latter may have no row at all yet. Same
        /// reasoning as the `session_key` on the never-ran `RunError` in
        /// `busy_queue::spawn`.
        session_key: String,
        /// How many messages ahead of this one may still run.
        /// `0` means "nobody ahead, but not started yet".
        ahead: u16,
    },
```

**3b.** `src/gateway/event_emitter/redacting.rs`，把 `RunQueued` 加进 line 241 的 pass-through 臂：

```rust
            other @ (StreamEvent::RunAccepted { .. }
            | StreamEvent::RunQueued { .. }
            | StreamEvent::AgentTrace { .. }
```

（排队帧不含模型正文，没有可脱敏的东西。）

**3c.** `src/gateway/events/frame.rs`，`enum GatewayEventFrame` 里紧接 `RunAccepted` 之后：

```rust
    RunQueued {
        run_id: String,
        session_key: String,
        ahead: u16,
    },
```

**3d.** 同文件 `impl From<StreamEvent> for GatewayEventFrame`，紧接 `RunAccepted` 臂之后：

```rust
            StreamEvent::RunQueued {
                run_id,
                session_key,
                ahead,
            } => Self::RunQueued {
                run_id,
                session_key,
                ahead,
            },
```

**3e.** 同文件 `stream_method`，加在 `Self::RunError` 臂附近（`_ => None` **之前**）：

```rust
            Self::RunQueued { .. } => Some("stream.run_queued"),
```

**3f.（控制器裁定 R7）** `shared/protocol/src/events.rs`，`pub enum StreamEvent` 里紧接 `RunAccepted` 之后：

```rust
    /// The run joined its session's wait lane and has not been admitted yet.
    ///
    /// Terminal clients decode this enum, so a `stream.*` method without a
    /// variant here is a silent drop at `shared/client/src/connection.rs` for
    /// every one of them — the census in `gateway::events::frame_census`
    /// exists because that happened before.
    RunQueued {
        run_id: String,
        session_key: String,
        /// How many messages ahead of this one may still run. `0` means
        /// "nobody ahead, but not started yet".
        ahead: u16,
    },
```

同文件 `run_id()` 的 match —— `RunQueued` 是 run-keyed，加进那一串或运算的模式里（`run_id()` **穷尽无通配**，不加就是编译错误，这正是它该有的样子）：

```rust
            Self::RunAccepted { run_id, .. }
            | Self::RunQueued { run_id, .. }
```

- [ ] **Step 4: 补一条「索引真的被播种了」的断言**

`note_frame`（`event_visibility.rs:649`）是**完全通用**的——它对任意 `stream.*` 帧读 `session_key` 与 `run_id` 两个 JSON 字段，不逐变体匹配。所以 `RunQueued` **零改动自动播种**。

正因为它是隐式的（没有任何一行代码提到 `RunQueued`），它欠一条断言：下一个人给帧改字段名或把 `session_key` 挪进嵌套对象，这条线会静默断掉，而症状是所有排队帧对**所有人**（含机主）fail-closed。

**同文件 `event_visibility.rs:1063` 起已有一条形状完全相同的测试**（never-admitted run 的 `stream.run_error` 带 `session_key`，断言机主收得到、别人收不到、去掉字段则机主也收不到）。照它写，只换 topic 与 payload：

```rust
    /// `RunQueued` is a run's FIRST frame — the first chance to seed the
    /// run→session index, and the reason the seed moved from admission to
    /// arrival. `note_frame` is generic over "names both", so this works with
    /// no code mentioning the variant, which is exactly why it needs a test:
    /// renaming the field or nesting it breaks the seed silently and every
    /// queued-run frame then fails closed for everyone, its owner included.
    #[tokio::test]
    async fn a_queued_frame_reaches_its_session_and_nobody_else() {
        // Reuse the `store` / `key` / owner setup from
        // `…_never_admitted_run_error_…` directly above.
        let run_queued = serde_json::json!({
            "run_id": "r-still-waiting",
            "session_key": key.to_key_string(),
            "ahead": 1,
        });

        let index = EventVisibilityIndex::new();
        index
            .note_frame("stream.run_queued", Some(&run_queued))
            .await;
        assert!(
            index
                .event_admits(
                    "stream.run_queued",
                    Some(&run_queued),
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await,
            "the owner must see their own message waiting"
        );
        assert!(
            !index
                .event_admits(
                    "stream.run_queued",
                    Some(&run_queued),
                    Some("bob"),
                    false,
                    &store,
                    None
                )
                .await,
            "naming the session must not widen the audience beyond it"
        );
    }
```

> ⚠️ `key` / `store` / `"alice"` / `"bob"` 来自那条既有测试的 setup。**把它的 setup 段照抄进来**（或把两条测试放进同一个 fixture 函数）；不要新造。

- [ ] **Step 5: 跑测试确认它绿**

```
cargo test -p alephcore --lib events::frame::tests::run_queued_has_a_wire_method
cargo test -p alephcore --lib event_visibility::
cargo test -p alephcore --lib events::frame_census
cargo test -p aleph-tui -p aleph-cli --no-run
```
Expected: 前三条都 `test result: ok`，第四条构建通过。

`frame_census` 的两半都必须绿：`every_stream_method_has_a_typed_twin_in_the_protocol_enum`（3f 的枚举变体满足它）与 `every_protocol_stream_variant_has_a_gateway_producer`（3e 的 `stream_method` 臂满足它）。**不要**把 `run_queued` 加进 `PANEL_ONLY_STREAM_METHODS`。

第四条是因为本任务动了协议 crate：那个枚举被 TUI / CLI 解码，加变体可能点名它们的穷尽 match。

- [ ] **Step 6: 变异证伪（两次）**

1. 删掉 3e 那一行（让它落回 `_ => None`），重跑第一条命令 → Expected **RED**，断言 `Some("stream.run_queued")` 失败。
2. 把 `RunQueued` 的 `session_key` 字段临时改名成 `session`，重跑 `event_visibility::` → Expected **RED**，`a_queued_frame_reaches_its_session_and_nobody_else`。

两次都确认后改回。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/event_emitter/types.rs src/gateway/event_emitter/redacting.rs src/gateway/events/frame.rs src/gateway/event_visibility.rs shared/protocol/src/events.rs
git commit -m "gateway: add the RunQueued frame, waiting's first wire representation"
```

---

### Task 3: 等待者在已有唤醒点自报位置

**Files:**
- Modify: `src/gateway/busy_queue/deliver.rs`（`deliver_with_ticket` 签名 + 循环 + 既有测试的调用点）
- Modify: `src/gateway/busy_queue/spawn.rs`（Panel/CLI 到达路径）
- Modify: `src/gateway/inbound_router/executor.rs:497`（channel 到达路径）

**Interfaces:**
- Consumes: `TicketGuard::ahead()`（Task 1）· `StreamEvent::RunQueued`（Task 2）
- Produces: `deliver_with_ticket(ticket, cfg, attempt, report)` —— 第 4 参 `report: &mut R`，`R: FnMut(u16) -> RFut`, `RFut: Future<Output = ()>`

**⚠️ 两条设计约束，写代码前先读**：

1. **`report` 必须是 async 且内联 await**，不能是 `FnMut(u16)` + `tokio::spawn`。`ahead` 单调下降；spawn 出去就放弃了顺序，乱序到达 = 界面数字闪回。内联 await 按构造保序，且与既有的 `attempt: &mut F, F: FnMut() -> Fut` 是同一个惯用法。
2. **上报点在「决定 park 之前」，不在 `else`（不是队首）臂里**。队首被 steering 背压推回（`attempt()` 回 `AgentBusy`）同样是等待，且它今天完全不可见——那正是 §4.8 Round-9 处理的那类等待。放在 `else` 里会漏掉它。

- [ ] **Step 1: 写失败测试**

该模块的测试**不直接调 `deliver_with_ticket`**，走一个 `deliver(session, run_id, cfg(n), attempt)` 包装器（`deliver.rs:178`），`cfg(max_per_session)` 是**带参**夹具（`deliver.rs:166`，`wake_fallback_secs: 3600`，好让通过的测试证明它靠的是真唤醒而不是兜底 tick）。新测试沿用同一套。

在 `#[cfg(test)] mod tests` 里，先加一个**并列**的报告版包装器（既有的 `deliver` 保持不变——报告不是那些测试要说的事）：

```rust
    /// Mirrors [`deliver`], but threads the position reporter the wait loop
    /// hands `RunQueued` through. A separate helper so the tests above stay
    /// exactly as they were.
    async fn deliver_reporting<F, Fut>(
        session_key: &str,
        run_id: &str,
        cfg: BusyQueueConfig,
        mut attempt: F,
        sink: Arc<tokio::sync::Mutex<Vec<u16>>>,
    ) -> DeliveryOutcome
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<(), ExecutionError>>,
    {
        let mut report = move |ahead: u16| {
            let sink = Arc::clone(&sink);
            async move {
                sink.lock().await.push(ahead);
            }
        };
        match register(session_key, cfg.max_per_session, run_id) {
            None => DeliveryOutcome::Rejected,
            Some(ticket) => deliver_with_ticket(ticket, cfg, &mut attempt, &mut report).await,
        }
    }

    /// The common path must stay silent: an idle session takes the front of
    /// the lane and runs, so a conversation that never waited gains no new
    /// frames at all. This is the ceiling on this feature's regression risk,
    /// so it is asserted directly rather than inferred.
    #[tokio::test]
    async fn an_idle_session_reports_no_position_at_all() {
        let sink = Arc::new(tokio::sync::Mutex::new(Vec::<u16>::new()));
        let outcome = deliver_reporting(
            "bqd-report-idle",
            "run-idle",
            cfg(4),
            || async { Ok(()) },
            Arc::clone(&sink),
        )
        .await;

        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
        assert!(
            sink.lock().await.is_empty(),
            "a conversation that never waited must gain no RunQueued frames"
        );
    }

    /// A front ticket refused for steering backpressure IS waiting, and it is
    /// the one wait with no representation anywhere before this. Reporting
    /// only from the "not front" branch would miss it entirely.
    #[tokio::test]
    async fn a_front_ticket_deferred_for_backpressure_still_reports() {
        let s = "bqd-report-deferred";
        let sink = Arc::new(tokio::sync::Mutex::new(Vec::<u16>::new()));
        let attempts = Arc::new(AtomicUsize::new(0));

        let waker = {
            let attempts = Arc::clone(&attempts);
            tokio::spawn(async move {
                while attempts.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                // Let the loop reach its park, then wake it for the retry.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                notify_slot_free(s);
            })
        };

        let seen = Arc::clone(&attempts);
        let outcome = deliver_reporting(
            s,
            "run-a",
            cfg(4),
            move || {
                let seen = Arc::clone(&seen);
                async move {
                    // Refuse the first attempt (backpressure), pass the retry.
                    if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err(ExecutionError::AgentBusy("a".to_string()))
                    } else {
                        Ok(())
                    }
                }
            },
            Arc::clone(&sink),
        )
        .await;
        waker.await.expect("waker task");

        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
        assert_eq!(
            *sink.lock().await,
            vec![0u16],
            "front-but-deferred reports ahead=0 — nobody ahead, but not started"
        );
    }

    /// Position is news only when it changes. The fallback tick re-runs this
    /// loop on a cadence, so re-sending an unchanged number would turn a
    /// bounded signal into a per-session heartbeat.
    #[tokio::test]
    async fn an_unchanged_position_is_not_reported_twice() {
        let s = "bqd-report-dedup";
        let front = register(s, 8, "run-front").expect("lane accepts the first message");
        let sink = Arc::new(tokio::sync::Mutex::new(Vec::<u16>::new()));

        let waker = tokio::spawn(async move {
            // Two spurious wakes with no lane movement between them.
            for _ in 0..2 {
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                notify_slot_free(s);
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            // Now the position really changes — and the waiter becomes front,
            // so it runs instead of reporting 0.
            drop(front);
        });

        let outcome =
            deliver_reporting(s, "run-b", cfg(8), || async { Ok(()) }, Arc::clone(&sink)).await;
        waker.await.expect("waker task");

        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
        assert_eq!(
            *sink.lock().await,
            vec![1u16],
            "ahead=1 is news once; repeating it on every wake makes this a heartbeat"
        );
    }
```

- [ ] **Step 2: 跑测试确认它红**

```
cargo test -p alephcore --lib busy_queue::deliver::tests::
```
Expected: **BUILD-ERROR**（`deliver_with_ticket` takes 3 arguments but 4 were supplied）。

- [ ] **Step 3: 实现**

**3a.** `src/gateway/busy_queue/deliver.rs` —— 签名：

```rust
/// …（保留既有 doc，在末尾追加下面这段）
///
/// # Reporting position
///
/// `report` is called with `ahead` **only when it changes**, from the point
/// just before this loop parks. That point — not the "not front" branch — is
/// deliberate: a front ticket refused for steering backpressure is waiting
/// too, and it is the one wait that had no representation anywhere.
///
/// It is `async` and awaited inline rather than a sync callback that spawns:
/// `ahead` decreases monotonically, and a spawn abandons ordering, so two
/// rapid updates could land inverted and flicker the number a user is
/// reading. Awaiting inline is in-order by construction and mirrors how
/// `attempt` is already threaded through.
///
/// An idle session never reaches the report point: it is front, `attempt`
/// does not refuse, and the function returns first. Conversations that never
/// wait therefore gain no frames at all.
pub async fn deliver_with_ticket<F, Fut, R, RFut>(
    ticket: TicketGuard,
    cfg: BusyQueueConfig,
    attempt: &mut F,
    report: &mut R,
) -> DeliveryOutcome
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), ExecutionError>>,
    R: FnMut(u16) -> RFut,
    RFut: Future<Output = ()>,
{
```

在 `let mut announced_busy = false;` 旁边加：

```rust
    let mut last_reported: Option<u16> = None;
```

在**既有的 park 段之前**（即 `// Park until the lane says something changed.` 那行注释**之前**）插入：

```rust
        // Still waiting — either behind someone, or front and refused for
        // backpressure. Both are waits; only the number changing is news.
        let ahead = ticket.ahead();
        if last_reported != Some(ahead) {
            last_reported = Some(ahead);
            report(ahead).await;
        }
```

同文件把既有的测试包装器 `deliver`（`deliver.rs:178`）里那一处调用补上 no-op 报告器——**只有这一处**，六条既有测试全都经它：

```rust
            Some(ticket) => {
                let mut noop = |_: u16| async {};
                deliver_with_ticket(ticket, cfg, &mut attempt, &mut noop).await
            }
```

**3b.** `src/gateway/busy_queue/spawn.rs` —— 在 `deliver_with_ticket` 调用处（`Some(ticket) => { … }` 臂内）：

```rust
                Some(ticket) => {
                    let mut attempt =
                        || engine.execute(request.clone(), agent.clone(), emitter.clone());
                    // The lane's own surface for "still waiting". `session_key`
                    // is the ADDRESSED session, not the derived execution lane
                    // — same reason the never-ran `RunError` below names it:
                    // the client resolving this frame is attached to the
                    // former, and this is the run's first frame, so nothing
                    // has seeded the run→session index yet.
                    let queued_emitter = emitter.clone();
                    let queued_run_id = run_id.clone();
                    let queued_session = session_key.clone();
                    let mut report = move |ahead: u16| {
                        let emitter = queued_emitter.clone();
                        let run_id = queued_run_id.clone();
                        let session_key = queued_session.clone();
                        async move {
                            if let Err(e) = emitter
                                .emit(StreamEvent::RunQueued {
                                    run_id,
                                    session_key,
                                    ahead,
                                })
                                .await
                            {
                                // Best-effort mirror: `chat.history.pending`
                                // is the authoritative half, so a dropped
                                // frame costs liveness, never correctness.
                                tracing::debug!("failed to emit RunQueued: {e}");
                            }
                        }
                    };
                    deliver_with_ticket(ticket, cfg, &mut attempt, &mut report).await
                }
```

**3c.** `src/gateway/inbound_router/executor.rs:497` —— 同样的闭包（该处的 emitter 变量名与 session key 变量名以文件内实际为准；**必须用被寻址的 session key**）：

```rust
                Some(ticket) => {
                    let mut attempt = || {
                        execution_adapter.execute(request.clone(), agent.clone(), emitter.clone())
                    };
                    // Same frame from the channel arrival path. It goes on the
                    // WS bus, NOT back to Telegram/Slack: a Panel watching the
                    // shared session should see a queued message from a
                    // channel, while the channel itself gets no "you are in
                    // line" chatter. `OriginFanoutEmitter` only fans out final
                    // answers, so skeleton events never reach a channel.
                    let queued_emitter = emitter.clone();
                    let queued_run_id = run_id.clone();
                    let queued_session = addressed_session_key.clone();
                    let mut report = move |ahead: u16| {
                        let emitter = queued_emitter.clone();
                        let run_id = queued_run_id.clone();
                        let session_key = queued_session.clone();
                        async move {
                            if let Err(e) = emitter
                                .emit(crate::gateway::StreamEvent::RunQueued {
                                    run_id,
                                    session_key,
                                    ahead,
                                })
                                .await
                            {
                                tracing::debug!("failed to emit RunQueued: {e}");
                            }
                        }
                    };
                    deliver_with_ticket(ticket, busy_cfg, &mut attempt, &mut report).await
                }
```

> ⚠️ `addressed_session_key` 是占位名。**打开 `executor.rs` 找到它 spawn 前已经算好的、被寻址的 session key 变量并用那个**；如果只有 `RunRequest` 在手，用 `request.session_key.to_key_string()`（在 spawn **之前**克隆出来）。**不要**用 `btw::execution_session` 派生的车道键。

- [ ] **Step 4: 跑测试确认它绿**

```
cargo test -p alephcore --lib busy_queue::
cargo test -p alephcore --lib --no-run
```
Expected: 第一条 `test result: ok`；第二条构建通过。

- [ ] **Step 5: 变异证伪（两次）**

1. 把 `if last_reported != Some(ahead)` 改成 `if true`，重跑 → Expected **RED**：`an_unchanged_position_is_not_reported_twice`。
2. 把上报块整体移进一个 `if !ticket.is_front() { … }`，重跑 → Expected **RED**：`a_front_ticket_deferred_for_backpressure_still_reports`。

两次都确认后改回。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/busy_queue/deliver.rs src/gateway/busy_queue/spawn.rs src/gateway/inbound_router/executor.rs
git commit -m "gateway: report lane position from the waiter's existing wake point"
```

---

### Task 4: `chat.history` 带上 `pending[]`（权威那一半）

**Files:**
- Create: `shared/protocol/src/queue.rs`
- Modify: `shared/protocol/src/lib.rs`（模块声明，按字母序插在 `providers` 与 `receipt` 之间——`receipt` 由任务 5 加，两条不冲突）
- Modify: `src/gateway/busy_queue/mod.rs`（新增 `pending_for`）
- Modify: `src/gateway/handlers/chat.rs`（history 响应）

**Interfaces:**
- Consumes: `Lane` 内部结构
- Produces: `aleph_protocol::queue::PendingRun { pub run_id: String, pub ahead: u16 }`（`Serialize + Deserialize`）；`pub fn busy_queue::pending_for(session_key: &str) -> Vec<PendingRun>`；`chat.history` 响应新键 `pending`

**⚠️ 控制器裁定 R4（覆盖计划初稿）**：`PendingRun` **住在协议 crate**，不在 `busy_queue` 里。任务 9 的 Panel 侧读的是**同一个类型**，不是自己再定义一个 `PendingRunDto`。理由：这是本仓记录在案、复发过四次的缺陷——「跨 crate 的 wire 契约要么共用一个类型（重命名 ⇒ 编译错），要么在依赖两边的那一侧留一条对账测试」。`alephcore` 与 `aleph-panel` 互不依赖，所以**共用类型是唯一合规的形状**，而任务 5 在本轮已经为另一条 wire 契约立了同一个先例。

**为什么挂在 `chat.history` 上而不是新开 RPC**：与 `active_run` / `plan` 逐字同一个论证——它们是**一个**快照，分两次调用就开出一个「拿着 transcript 却拿着另一份状态」的窗口。

- [ ] **Step 1: 写失败测试**

`src/gateway/busy_queue/mod.rs` 的 tests 里：

```rust
/// The durable half of `RunQueued`. A client that attaches mid-wait never saw
/// the frame — it fired before the socket existed — so the snapshot it
/// already fetches has to carry the same fact.
#[test]
fn pending_for_lists_live_waiters_in_lane_order() {
    let s = "sess-pending";
    let a = register(s, CAP, "run-a").expect("a");
    let b = register(s, CAP, "run-b").expect("b");
    let c = register(s, CAP, "run-c").expect("c");

    let pending = pending_for(s);
    assert_eq!(
        pending
            .iter()
            .map(|p| (p.run_id.as_str(), p.ahead))
            .collect::<Vec<_>>(),
        vec![("run-a", 0), ("run-b", 1), ("run-c", 2)]
    );

    assert!(cancel_queued_run("run-b"));
    let pending = pending_for(s);
    assert_eq!(
        pending
            .iter()
            .map(|p| (p.run_id.as_str(), p.ahead))
            .collect::<Vec<_>>(),
        vec![("run-a", 0), ("run-c", 1)],
        "a cancelled waiter is neither listed nor counted"
    );

    assert!(pending_for("sess-that-never-existed").is_empty());
    drop((a, b, c));
}
```

- [ ] **Step 2: 跑测试确认它红**

```
cargo test -p alephcore --lib busy_queue::tests::pending_for_lists_live_waiters
```
Expected: **BUILD-ERROR**（`cannot find function pending_for`）。

- [ ] **Step 3: 实现**

**3a.** 新建 `shared/protocol/src/queue.rs`：

```rust
//! The shape of a session's server-side wait lane, as it crosses the wire.
//!
//! Lives here rather than in either side because `alephcore` and
//! `aleph-panel` do not depend on each other: a type they both derive their
//! serde from is the only way a field rename can be a compile error instead
//! of a client that silently reads nothing. Same reasoning as
//! [`crate::receipt`], for the other wire contract in this round.

use serde::{Deserialize, Serialize};

/// One message still waiting on a session's lane.
///
/// Serialized onto `chat.history`'s `pending` array — the authoritative half
/// of the best-effort `StreamEvent::RunQueued` frame, in exactly the split
/// `agent_trace` (lossy mirror) and `RunSummary` (authority) already use. A
/// client that attaches mid-wait never received the frame, so the snapshot it
/// already fetches has to answer the same question.
///
/// Deliberately carries no message text: the lane does not hold the payload
/// (the full `RunRequest` lives only in the two spawn closures), and giving it
/// one is the same change as making the queue crash-durable — a separate,
/// recorded piece of work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRun {
    pub run_id: String,
    /// How many messages ahead of this one may still run.
    pub ahead: u16,
}
```

`shared/protocol/src/lib.rs`，在 `pub mod providers;` 之后加：

```rust
pub mod queue;
```

**3b.** `src/gateway/busy_queue/mod.rs`，紧接 `snapshot` 之后：

```rust
pub use aleph_protocol::queue::PendingRun;

/// Messages still waiting on `session_key`, in lane order.
///
/// Cancelled tickets are neither listed nor counted, matching [`snapshot`]'s
/// `total_waiting` contract and [`TicketGuard::ahead`].
#[must_use]
pub fn pending_for(session_key: &str) -> Vec<PendingRun> {
    let map = lock();
    let Some(lane) = map.get(session_key) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut ahead = 0u16;
    for t in &lane.tickets {
        if t.cancelled {
            continue;
        }
        out.push(PendingRun {
            run_id: t.run_id.clone(),
            ahead,
        });
        ahead = ahead.saturating_add(1);
    }
    out
}
```

**3c.** `src/gateway/handlers/chat.rs`，在 `let session_snapshot = …;` 之后、`JsonRpcResponse::success` 之前：

```rust
            // The lane's waiting messages, by the SAME canonical key and at
            // the same post-gate position as `active_run` and `plan` above,
            // and for the same reason: they are one snapshot with the
            // transcript. A client that attached mid-wait never saw the
            // `RunQueued` frames — they fired before its socket existed — so
            // without this it paints "thinking" over a queue it cannot see.
            let pending = crate::gateway::busy_queue::pending_for(&canonical);
```

并在 `json!({ … })` 里加一行（放在 `"active_run"` 之后）：

```rust
                    "pending": pending,
```

- [ ] **Step 4: 跑测试确认它绿**

```
cargo test -p aleph-protocol queue::
cargo test -p alephcore --lib busy_queue::tests::pending_for_lists_live_waiters
cargo test -p alephcore --lib handlers::chat
```
Expected: 三条都 `test result: ok`（第一条可能是 `running 0 tests`——`queue.rs` 本身不带测试，形状由任务 9 的 Panel 侧对账覆盖）。

- [ ] **Step 5: 变异证伪**

把 `if t.cancelled { continue; }` 删掉，重跑第一条。
Expected: **RED**（`a cancelled waiter is neither listed nor counted`）。确认后加回。

- [ ] **Step 6: 提交**

```bash
git add shared/protocol/src/queue.rs shared/protocol/src/lib.rs src/gateway/busy_queue/mod.rs src/gateway/handlers/chat.rs
git commit -m "gateway: carry the wait lane on chat.history, the attach-time authority"
```

---

### Task 5: `aleph_protocol::receipt` —— 错误码的单一源

**Files:**
- Create: `shared/protocol/src/receipt.rs`
- Modify: `shared/protocol/src/lib.rs`（模块声明，按字母序插在 `providers` 与 `session_thread` 之间）
- Modify: `src/gateway/i18n.rs`（`ReceiptKind::code()` 改为读协议表 + 对账测试）

**Interfaces:**
- Consumes: 无
- Produces:
  - `aleph_protocol::receipt::ReceiptCode`（枚举，8 个变体：`Timeout` `Cancelled` `AgentBusy` `RateLimited` `Auth` `ProvidersUnreachable` `Failed` `SpendExhausted`）
  - `ReceiptCode::as_wire(self) -> &'static str`
  - `ReceiptCode::from_wire(s: &str) -> Option<Self>`
  - `ReceiptCode::ALL: &'static [ReceiptCode]`

- [ ] **Step 1: 写失败测试**

新建 `shared/protocol/src/receipt.rs`，先只写测试模块（实现留到 Step 3）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The wire strings are API — a client switches on them. Renaming one
    /// silently reclassifies every error of that kind on every surface that
    /// has already shipped.
    #[test]
    fn wire_spellings_are_frozen() {
        assert_eq!(ReceiptCode::Timeout.as_wire(), "TIMEOUT");
        assert_eq!(ReceiptCode::Cancelled.as_wire(), "CANCELLED");
        assert_eq!(ReceiptCode::AgentBusy.as_wire(), "AGENT_BUSY");
        assert_eq!(ReceiptCode::RateLimited.as_wire(), "RATE_LIMITED");
        assert_eq!(ReceiptCode::Auth.as_wire(), "AUTH");
        // NOTE the asymmetry: the variant is `ProvidersUnreachable` and the
        // wire code is `PROVIDERS_UNREACHABLE`, but the server-side variant it
        // mirrors is named `Unreachable`. The wire string is the contract.
        assert_eq!(
            ReceiptCode::ProvidersUnreachable.as_wire(),
            "PROVIDERS_UNREACHABLE"
        );
        assert_eq!(ReceiptCode::Failed.as_wire(), "FAILED");
        assert_eq!(ReceiptCode::SpendExhausted.as_wire(), "SPEND_EXHAUSTED");
    }

    /// `ALL` is what both sides derive their expectations from, so a variant
    /// missing from it is a variant no reconciliation test can see.
    #[test]
    fn all_covers_every_variant_and_round_trips() {
        assert_eq!(ReceiptCode::ALL.len(), 8);
        for code in ReceiptCode::ALL {
            assert_eq!(
                ReceiptCode::from_wire(code.as_wire()),
                Some(*code),
                "{} does not round-trip",
                code.as_wire()
            );
        }
        assert_eq!(ReceiptCode::from_wire("NOT_A_CODE"), None);
    }
}
```

- [ ] **Step 2: 跑测试确认它红**

```
cargo test -p aleph-protocol receipt::
```
Expected: **BUILD-ERROR**（模块未声明 / 类型不存在）。

- [ ] **Step 3: 实现**

`shared/protocol/src/receipt.rs` 的**开头**（测试模块之前）写：

```rust
//! Stable wire codes for user-facing failure receipts.
//!
//! One source for the classification both halves of the system already need:
//! the server picks a bucket (`gateway::i18n::ReceiptKind`) and puts its code
//! on `StreamEvent::RunError.error_code`; the Panel has to render that bucket.
//!
//! Before this module existed the Panel did not read the code at all — it
//! lower-cased the message and keyword-matched its way to a *second*, smaller
//! taxonomy with no bucket for `CANCELLED`, `AGENT_BUSY`, `AUTH`, or
//! `SPEND_EXHAUSTED`. That is the same defect the server deleted from
//! `inbound_router::executor` (re-classifying an already-typed error from its
//! string), one crate over.
//!
//! Living here rather than in either side is what makes a rename a compile
//! error instead of a silent reclassification: `aleph-protocol` is depended on
//! by both `alephcore` and `aleph-panel`.

use serde::{Deserialize, Serialize};

/// A user-facing failure bucket, as carried on the wire.
///
/// The `as_wire` strings are API. New variants may be added; **existing
/// spellings may never change** — a shipped client switches on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReceiptCode {
    /// The run exceeded its wall-clock budget.
    Timeout,
    /// The user (or an `Interrupt`-mode message) stopped the run.
    Cancelled,
    /// The session was busy and the message could not be steered or queued —
    /// including a queued message rejected at the lane cap or timed out
    /// waiting.
    AgentBusy,
    /// Every provider in the chain reported rate limiting.
    RateLimited,
    /// Credential rejected (401 / invalid API key). Retrying will not help,
    /// so a surface that renders this as "try again" is actively misleading.
    Auth,
    /// Network / upstream outage across the whole provider chain.
    ProvidersUnreachable,
    /// Anything else. Deliberately opaque — the raw chain stays in the server
    /// log and never reaches the wire.
    Failed,
    /// A spend ceiling was reached.
    SpendExhausted,
}

impl ReceiptCode {
    /// Every variant. Both sides derive their reconciliation expectations from
    /// this rather than restating a literal list, so a new bucket cannot be
    /// added on one side only.
    pub const ALL: &'static [Self] = &[
        Self::Timeout,
        Self::Cancelled,
        Self::AgentBusy,
        Self::RateLimited,
        Self::Auth,
        Self::ProvidersUnreachable,
        Self::Failed,
        Self::SpendExhausted,
    ];

    /// The stable wire spelling. **Never rename an existing one.**
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Timeout => "TIMEOUT",
            Self::Cancelled => "CANCELLED",
            Self::AgentBusy => "AGENT_BUSY",
            Self::RateLimited => "RATE_LIMITED",
            Self::Auth => "AUTH",
            Self::ProvidersUnreachable => "PROVIDERS_UNREACHABLE",
            Self::Failed => "FAILED",
            Self::SpendExhausted => "SPEND_EXHAUSTED",
        }
    }

    /// Parse a wire code. `None` for anything this build does not know —
    /// a newer core may send a bucket an older client has never heard of, and
    /// guessing is what this module exists to stop.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.as_wire() == s)
    }
}
```

`shared/protocol/src/lib.rs`，在 `pub mod providers;` 之后加：

```rust
pub mod receipt;
```

`src/gateway/i18n.rs`，把 `ReceiptKind::code()` 的函数体改为委托（**保留原 doc，追加一句说明单一源**）：

```rust
    /// Stable wire code carried on `StreamEvent::RunError.error_code`.
    /// Clients may switch on it, so these strings are API — do not rename.
    ///
    /// The spellings live in `aleph_protocol::receipt::ReceiptCode`, which the
    /// Panel also reads, so the two sides cannot drift into two taxonomies.
    #[must_use]
    pub const fn code(self) -> &'static str {
        self.protocol_code().as_wire()
    }

    /// This bucket as the shared protocol type.
    #[must_use]
    pub const fn protocol_code(self) -> aleph_protocol::receipt::ReceiptCode {
        use aleph_protocol::receipt::ReceiptCode as C;
        match self {
            Self::Timeout => C::Timeout,
            Self::Cancelled => C::Cancelled,
            Self::AgentBusy => C::AgentBusy,
            Self::RateLimited => C::RateLimited,
            Self::Auth => C::Auth,
            Self::Unreachable => C::ProvidersUnreachable,
            Self::Failed => C::Failed,
            Self::SpendExhausted { .. } => C::SpendExhausted,
        }
    }
```

并在 `src/gateway/i18n.rs` 的 `#[cfg(test)] mod tests` 里加对账（**期望值从协议类型派生，不写字面量清单**）：

```rust
    /// Server-side reconciliation: every protocol bucket must be reachable
    /// from some `ReceiptKind`, and every `ReceiptKind` must map to one. A
    /// literal list here would be the same enumeration bug one level up, so
    /// the expectation is derived from `ReceiptCode::ALL`.
    #[test]
    fn every_protocol_receipt_code_is_produced_by_some_kind() {
        use aleph_protocol::receipt::ReceiptCode;
        use std::collections::HashSet;

        let produced: HashSet<ReceiptCode> = [
            ReceiptKind::Timeout,
            ReceiptKind::Cancelled,
            ReceiptKind::AgentBusy,
            ReceiptKind::RateLimited,
            ReceiptKind::Auth,
            ReceiptKind::Unreachable,
            ReceiptKind::Failed,
            ReceiptKind::SpendExhausted {
                limit: Limit::Total,
                reset_ms: 0,
            },
        ]
        .into_iter()
        .map(ReceiptKind::protocol_code)
        .collect();

        let expected: HashSet<ReceiptCode> = ReceiptCode::ALL.iter().copied().collect();
        assert_eq!(
            produced, expected,
            "a protocol bucket with no producing ReceiptKind is unreachable; \
             a ReceiptKind with no bucket cannot be rendered by any client"
        );
    }
```

> `Limit` 来自 `crate::spend::Limit`（`src/spend/mod.rs:148`），i18n.rs 顶部已 `use` 它。`Limit::Total` 是**无字段**变体，正合此处——这条测试只关心 `SpendExhausted` 这一支映射得出去，携带值无关。

- [ ] **Step 4: 跑测试确认它绿**

```
cargo test -p aleph-protocol receipt::
cargo test -p alephcore --lib i18n::
```
Expected: 两条都 `test result: ok`。

- [ ] **Step 5: 变异证伪**

从 `ReceiptCode::ALL` 里删掉 `Self::Auth`，重跑两条命令。
Expected: **RED** —— `all_covers_every_variant_and_round_trips`（长度 8）与 `every_protocol_receipt_code_is_produced_by_some_kind`（集合不等）**各红一条**。确认后加回。

- [ ] **Step 6: 提交**

```bash
git add shared/protocol/src/receipt.rs shared/protocol/src/lib.rs src/gateway/i18n.rs
git commit -m "protocol: own the receipt wire codes both sides classify by"
```

---

### Task 6: Panel 改读 `error_code`（含 `state.rs` 拆分）

**Files:**
- Rename: `interfaces/webchat/src/platform/wide/views/chat/state.rs` → `.../chat/state/mod.rs`（`git mv`）
- Create: `.../chat/state/send_error.rs`
- Modify: `.../chat/state/mod.rs`（删掉搬走的块，加 `mod` + `pub use`）

**Interfaces:**
- Consumes: `aleph_protocol::receipt::ReceiptCode`（Task 5）
- Produces: `ChatSendErrorCode`（新增变体 `Cancelled` `AgentBusy` `Auth` `SpendExhausted`）· `ChatSendError::from_wire_code(code: Option<&str>, message: impl Into<String>) -> Self` · `ChatSendError::classify`（保留，降级为兜底）

- [ ] **Step 1: 搬文件（无行为改动，先单独提交）**

```bash
mkdir -p interfaces/webchat/src/platform/wide/views/chat/state
git mv interfaces/webchat/src/platform/wide/views/chat/state.rs \
       interfaces/webchat/src/platform/wide/views/chat/state/mod.rs
cargo test -p aleph-panel --lib --no-run
```
Expected: 构建通过、零改动（`chat/mod.rs` 的 `pub mod state;` 对目录模块同样成立）。

```bash
git commit -am "panel: move chat state.rs to state/mod.rs ahead of splitting it"
```

- [ ] **Step 2: 写失败测试**

新建 `.../chat/state/send_error.rs`，先只写测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The server already classified this failure and put a stable code on the
    /// wire. Re-deriving it from the message is how the Panel ended up with a
    /// taxonomy that had no bucket for Stop, for a rejected queued message, or
    /// for an expired API key — all three rendered as an UNKNOWN banner.
    #[test]
    fn the_wire_code_wins_over_the_message_text() {
        // A message whose text would keyword-match CloudSendFailed.
        let e = ChatSendError::from_wire_code(Some("CANCELLED"), "http provider stopped");
        assert_eq!(e.code, ChatSendErrorCode::Cancelled);

        let e = ChatSendError::from_wire_code(Some("AGENT_BUSY"), "session is occupied");
        assert_eq!(e.code, ChatSendErrorCode::AgentBusy);

        let e = ChatSendError::from_wire_code(Some("AUTH"), "http 401 from provider");
        assert_eq!(
            e.code,
            ChatSendErrorCode::Auth,
            "an expired key must never render as a retryable cloud failure"
        );
    }

    /// Every bucket the server can send must land somewhere real. Derived from
    /// `ReceiptCode::ALL` rather than a literal list, so a bucket added
    /// server-side fails here instead of silently becoming Unknown.
    #[test]
    fn every_server_bucket_maps_to_something_other_than_unknown() {
        for code in aleph_protocol::receipt::ReceiptCode::ALL {
            let mapped = ChatSendError::from_wire_code(Some(code.as_wire()), "msg").code;
            assert_ne!(
                mapped,
                ChatSendErrorCode::Unknown,
                "{} has no Panel bucket — it would render as an UNKNOWN banner",
                code.as_wire()
            );
        }
    }

    /// `classify` is the fallback, not the classifier: a core that predates
    /// `error_code`, and the transport-layer errors `ChatApi::send` raises
    /// before any run exists, both arrive without one.
    #[test]
    fn a_missing_code_falls_back_to_the_keyword_classifier() {
        let e = ChatSendError::from_wire_code(None, "websocket disconnected");
        assert_eq!(e.code, ChatSendErrorCode::SocketDisconnected);
        assert_eq!(e.message, "websocket disconnected");
    }

    /// An unknown spelling is not a licence to guess. A newer core sending a
    /// bucket this build has never heard of must not be re-derived from prose.
    #[test]
    fn an_unrecognized_code_is_unknown_not_a_keyword_guess() {
        let e = ChatSendError::from_wire_code(Some("BRAND_NEW_BUCKET"), "rate limit exceeded");
        assert_eq!(
            e.code,
            ChatSendErrorCode::Unknown,
            "the server named a bucket; guessing a different one from the text \
             is exactly the defect this replaces"
        );
    }
}
```

- [ ] **Step 3: 跑测试确认它红**

```
cargo test -p aleph-panel --lib send_error::
```
Expected: **BUILD-ERROR**（模块未声明）。

- [ ] **Step 4: 实现**

把 `state/mod.rs` 里 `ChatSendErrorCode` / `ChatSendError` 两个块（含 `impl` 与既有 doc）**整体剪切**到 `send_error.rs` 的测试模块**之前**，并在文件顶部加：

```rust
//! The Panel's user-facing failure taxonomy.
//!
//! Populated from the server's `error_code` (`aleph_protocol::receipt`), with
//! the keyword classifier kept only for inputs that genuinely carry no code.

use serde::{Deserialize, Serialize};
```

在 `ChatSendErrorCode` 里追加四个变体（**只加不改**，注释说明它们的来源）：

```rust
    /// The user stopped the run — `CANCELLED`. Not a failure; surfaces should
    /// not raise an error banner for it (the stopped bubble already says so).
    Cancelled,
    /// The session was busy and the message never ran — `AGENT_BUSY`.
    /// Includes a queued message rejected at the lane cap or timed out.
    AgentBusy,
    /// Credential rejected — `AUTH`. Retrying will not help; the user must fix
    /// the key, so this must never be worded as "try again".
    Auth,
    /// A spend ceiling was reached — `SPEND_EXHAUSTED`.
    SpendExhausted,
```

在 `impl ChatSendError` 里追加：

```rust
    /// Build from the server's classification, falling back to the keyword
    /// classifier only when there is genuinely no code to read.
    ///
    /// # Why the code wins
    ///
    /// The server already picked a bucket (`gateway::i18n::ReceiptKind`) from
    /// a *typed* error and put its stable spelling on the wire. Re-deriving a
    /// bucket from the rendered message is the same defect the server deleted
    /// from `inbound_router::executor`, one crate over — and it is why Stop, a
    /// rejected queued message, and an expired API key all rendered as an
    /// UNKNOWN banner: those buckets had no keyword branch at all.
    ///
    /// An unrecognized spelling maps to [`ChatSendErrorCode::Unknown`] rather
    /// than falling through to the classifier. A newer core naming a bucket
    /// this build has not heard of has still *answered*; guessing a different
    /// answer from its prose is the behaviour being removed.
    #[must_use]
    pub fn from_wire_code(code: Option<&str>, message: impl Into<String>) -> Self {
        let message = message.into();
        let Some(code) = code else {
            return Self::classify(message);
        };
        let mapped = match aleph_protocol::receipt::ReceiptCode::from_wire(code) {
            Some(c) => Self::from_receipt_code(c),
            None => ChatSendErrorCode::Unknown,
        };
        Self {
            code: mapped,
            message,
        }
    }

    /// Total map from the shared protocol bucket. Exhaustive on purpose: a
    /// bucket added to `ReceiptCode` is a compile error here, not a silent
    /// `Unknown`.
    #[must_use]
    const fn from_receipt_code(code: aleph_protocol::receipt::ReceiptCode) -> ChatSendErrorCode {
        use aleph_protocol::receipt::ReceiptCode as C;
        match code {
            C::Timeout => ChatSendErrorCode::SafetyTimeout,
            C::Cancelled => ChatSendErrorCode::Cancelled,
            C::AgentBusy => ChatSendErrorCode::AgentBusy,
            C::RateLimited => ChatSendErrorCode::UsageLimitReached,
            C::Auth => ChatSendErrorCode::Auth,
            C::ProvidersUnreachable => ChatSendErrorCode::CloudSendFailed,
            C::Failed => ChatSendErrorCode::CloudSendFailed,
            C::SpendExhausted => ChatSendErrorCode::SpendExhausted,
        }
    }
```

在 `classify` 的 doc 上追加一句：

```rust
    /// **Fallback only** — prefer [`Self::from_wire_code`]. Reachable for a
    /// core that predates `error_code` and for transport-layer failures raised
    /// before any run exists. Its keyword table cannot see `CANCELLED`,
    /// `AGENT_BUSY`, `AUTH`, or `SPEND_EXHAUSTED`; that is why it is not the
    /// first classifier any more.
```

在 `state/mod.rs` 顶部（`use` 之后）加：

```rust
mod send_error;
pub use send_error::{ChatSendError, ChatSendErrorCode};
```

- [ ] **Step 5: 跑测试确认它绿**

```
cargo test -p aleph-panel --lib send_error::
cargo test -p aleph-panel --lib --no-run
```
Expected: 第一条 `test result: ok. 4 passed`；第二条构建通过。

- [ ] **Step 6: 变异证伪**

把 `from_wire_code` 的函数体第一行改成 `return Self::classify(message);`（即退回旧行为），重跑。
Expected: **RED** —— `the_wire_code_wins_over_the_message_text` 与 `every_server_bucket_maps_to_something_other_than_unknown` **各红一条**。确认后改回。

- [ ] **Step 7: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/state/
git commit -m "panel: classify failures by the server's error_code, not by keyword"
```

---

### Task 7: `ChatPhase::Queued` —— 让编译器点名每个读者

**Files:**
- Create: `.../chat/state/run_phase.rs`
- Modify: `.../chat/state/mod.rs`
- Modify（编译器点名的 5 个读者）：`.../chat/messages.rs` · `.../chat/reasoning.rs` · `.../chat/composer/mod.rs` · `interfaces/webchat/src/platform/phone/chat/composer.rs` · `.../chat/team_events.rs`

**Interfaces:**
- Consumes: 无
- Produces: `ChatPhase`（新增 `Queued { ahead: u16 }`）· `ChatPhase::is_busy(self) -> bool` · `ChatState::mark_queued(&self, run_id: &str, ahead: u16)` · `ChatState::mark_admitted(&self, run_id: &str)`

**⚠️ 控制器裁定 R3（补计划初稿的一个缺口）**：还要产出 `mark_admitted`，并且这两个方法都欠一条**断言效果到达**的测试（初稿只测了 `is_busy` 这个纯函数）。

理由：`start_assistant_message`（`state.rs:1175`）在气泡**已存在**时**提前返回**，而 `active_run_id` / `phase` 的写入排在那道早返回**之后**（`state.rs:1210-1211`）。任务 8 的 `run_queued` 臂先建气泡，于是随后到达的 `run_accepted` 变成 no-op，相位**停在 `Queued`**，直到第一个 `turn_started` / 第一个 token 才被别的路径改掉——那段时间正好是模型延迟，用户读到的是「排队中」而模型其实在思考。spec §3.6 逐字写着这两种 `ahead==0` 的来源「接下来都会被 `RunAccepted` 或 `RunError` 覆盖」，初稿没有让那句话成真。

`mark_admitted` **只碰 `Queued`**（其余相位原样返回），所以对今天工作正常的每一条转移逐字节 no-op。

- [ ] **Step 1: 写失败测试**

新建 `.../chat/state/run_phase.rs`，先只写测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Queued is a busy phase. Every surface that gates on "is a turn in
    /// flight" must treat a waiting run as in flight — the composer must not
    /// offer a fresh send, and Stop must stay reachable.
    #[test]
    fn queued_counts_as_busy() {
        assert!(ChatPhase::Queued { ahead: 0 }.is_busy());
        assert!(ChatPhase::Queued { ahead: 3 }.is_busy());
        assert!(ChatPhase::Thinking.is_busy());
        assert!(ChatPhase::Streaming.is_busy());
        assert!(!ChatPhase::Idle.is_busy());
        assert!(!ChatPhase::Error.is_busy());
    }

    /// `ahead = 0` means "nobody ahead of me, but not started" — it reaches
    /// the wire from a front ticket refused for steering backpressure and from
    /// the lane's fail-open read. Both are true and both render the same.
    #[test]
    fn zero_ahead_is_still_a_queued_phase() {
        assert_eq!(
            ChatPhase::Queued { ahead: 0 },
            ChatPhase::Queued { ahead: 0 }
        );
        assert_ne!(ChatPhase::Queued { ahead: 0 }, ChatPhase::Thinking);
    }
}
```

并在 `state/mod.rs` 的 `#[cfg(test)] mod tests` 里追加**效果到达**的三条（`run_phase.rs` 只测了纯函数；这三条测的是 `ChatState` 上的转移，所以住在 state 的测试模块里，沿用该模块既有的 `Owner::new(); owner.set(); ChatState::new()` 写法）：

```rust
    /// Asserting the effect arrives, not that the call happened. `mark_queued`
    /// is guarded on `active_run_id`, and `start_assistant_message` sets that
    /// only on the branch where it does not early-return — so this is the one
    /// path the whole queued phase depends on.
    #[test]
    fn marking_a_run_queued_moves_the_phase() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("run-a");
        chat.mark_queued("run-a", 2);
        assert_eq!(chat.phase.get_untracked(), ChatPhase::Queued { ahead: 2 });
    }

    /// A lane frame for a sibling run — another tab, a channel, cron — must not
    /// repaint this conversation.
    #[test]
    fn a_sibling_runs_lane_frame_repaints_nothing() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("run-a");
        chat.mark_queued("run-b", 5);
        assert_eq!(chat.phase.get_untracked(), ChatPhase::Thinking);
    }

    /// Admission is the edge that ends the wait. It cannot ride on
    /// `start_assistant_message`: that early-returns once the bubble exists,
    /// and by admission time the queued frame has already created it — so
    /// without this the phase reads "queued" for the whole of model latency.
    #[test]
    fn admission_clears_the_queued_phase_and_touches_nothing_else() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("run-a");
        chat.mark_queued("run-a", 1);
        chat.mark_admitted("run-a");
        assert_eq!(chat.phase.get_untracked(), ChatPhase::Thinking);

        // Every other phase is left exactly as it was: admission answers only
        // "the wait is over", never "what is happening now".
        chat.phase.set(ChatPhase::Streaming);
        chat.mark_admitted("run-a");
        assert_eq!(chat.phase.get_untracked(), ChatPhase::Streaming);

        // And a sibling run's admission is not this conversation's news.
        chat.mark_queued("run-a", 1);
        chat.mark_admitted("run-b");
        assert_eq!(chat.phase.get_untracked(), ChatPhase::Queued { ahead: 1 });
    }
```

- [ ] **Step 2: 跑测试确认它红**

```
cargo test -p aleph-panel --lib run_phase::
```
Expected: **BUILD-ERROR**（模块未声明）。

- [ ] **Step 3: 实现**

把 `state/mod.rs` 里的 `ChatPhase` 定义**整体剪切**到 `run_phase.rs` 的测试模块之前，加变体与 `is_busy`：

```rust
//! The chat surface's top-level phase.

/// Top-level Chat UI phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatPhase {
    #[default]
    Idle,
    /// Waiting in the session's server-side lane — the run exists and has an
    /// id, but the engine has not admitted it yet.
    ///
    /// Before this variant there was no way to say that, so every client
    /// painted `Thinking` over a run the engine had never heard of, for as
    /// long as `max_wait_secs`.
    ///
    /// `ahead` is how many messages ahead of this one may still run. `0` means
    /// "nobody ahead, but not started" — it reaches the wire both from a front
    /// ticket refused for steering backpressure and from the lane's fail-open
    /// read, and both are true.
    Queued {
        ahead: u16,
    },
    Thinking,
    Streaming,
    Error,
}

impl ChatPhase {
    /// Whether a turn is in flight from the user's point of view.
    ///
    /// The single predicate every surface gates on, so a new phase cannot be
    /// classified one way in the composer and the other way in the message
    /// list. Waiting counts: the composer must not offer a fresh send, and
    /// Stop must stay reachable.
    #[must_use]
    pub const fn is_busy(self) -> bool {
        matches!(self, Self::Queued { .. } | Self::Thinking | Self::Streaming)
    }
}
```

`state/mod.rs` 顶部加：

```rust
mod run_phase;
pub use run_phase::ChatPhase;
```

在 `impl ChatState` 里加（放在 `start_assistant_message` 附近）：

```rust
    /// The run joined its session's wait lane. Idempotent — the lane reports
    /// only when `ahead` changes, but a re-attach can replay the same value.
    ///
    /// Scoped to the run this conversation is actually following: a lane frame
    /// for a sibling run (another tab, a channel, cron) must not repaint this
    /// conversation's phase.
    pub fn mark_queued(&self, run_id: &str, ahead: u16) {
        if self.active_run_id.get_untracked().as_deref() != Some(run_id) {
            return;
        }
        self.phase.set(ChatPhase::Queued { ahead });
    }

    /// The run was admitted to the engine — the wait is over.
    ///
    /// Only ever moves `Queued` to `Thinking`; every other phase is left
    /// exactly as it was, because admission answers "the wait ended", never
    /// "what is happening now" (a later frame may already have moved this
    /// conversation to `Streaming`).
    ///
    /// This cannot ride on `start_assistant_message`, which is what
    /// `run_accepted` already calls: that early-returns once the run's bubble
    /// exists, and by admission time the queued frame has created it. Without
    /// this method the phase would read "queued" until the first
    /// `turn_started` or token — i.e. for the whole of model latency, exactly
    /// while the model is in fact thinking.
    pub fn mark_admitted(&self, run_id: &str) {
        if self.active_run_id.get_untracked().as_deref() != Some(run_id) {
            return;
        }
        if matches!(self.phase.get_untracked(), ChatPhase::Queued { .. }) {
            self.phase.set(ChatPhase::Thinking);
        }
    }
```

然后跑构建，**让编译器点名每个 `ChatPhase` 读者**，逐个改：

- `phone/chat/composer.rs:66` 的 `matches!(chat.phase.get(), ChatPhase::Thinking | ChatPhase::Streaming)` → `chat.phase.get().is_busy()`
- 其余 4 个文件的 `== ChatPhase::Thinking` 等比较**保持语义不变**（它们问的是"正在思考"而不是"忙"，`Queued` 对它们应为 false）；只有被 `matches!` 当成"忙"用的那些改成 `is_busy()`。**逐个读上下文判断，不要一律替换。**

在 `messages.rs` 的占位气泡处渲染排队文案（紧邻既有的 `<Show when=move || chat.phase.get() == ChatPhase::Thinking>` 块）：

```rust
                            <Show when=move || matches!(chat.phase.get(), ChatPhase::Queued { .. })>
                                {move || match chat.phase.get() {
                                    ChatPhase::Queued { ahead: 0 } => t_string!(i18n, chat.queued_next).to_string(),
                                    ChatPhase::Queued { ahead } => {
                                        t_string!(i18n, chat.queued_behind, count = ahead as i64).to_string()
                                    }
                                    _ => String::new(),
                                }}
                            </Show>
```

> ⚠️ i18n key 名与 `t_string!` 的确切写法**以该文件既有用法为准**；两条新文案需要在 locale 文件里加（en + zh）。**记忆里有一条坑**：`leptos_i18n` 0.6 的复数键，单 form 的 locale 必须声明用不到的 `_one`（zh 也要写）。

- [ ] **Step 4: 跑测试确认它绿**

```
cargo test -p aleph-panel --lib run_phase::
cargo test -p aleph-panel --lib --no-run
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
```
Expected: 三条全过。**第三条是唯一编译出厂形态的命令，必须跑。**

- [ ] **Step 5: 变异证伪**

1. 把 `is_busy` 里的 `Self::Queued { .. } |` 删掉，重跑第一条命令。
   Expected: **RED** —— `queued_counts_as_busy`。
2. 把 `mark_admitted` 的函数体整体换成 `{}`（即让「准入」什么都不做，这正是初稿的行为），跑 `cargo test -p aleph-panel --lib state::`。
   Expected: **RED** —— `admission_clears_the_queued_phase_and_touches_nothing_else`。

两次都确认后改回。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/
git commit -m "panel: give the waiting phase a name so every surface must classify it"
```

---

### Task 8: Panel 消费 `run_queued` 与 `error_code`

**Files:**
- Modify: `.../chat/events.rs`（`resolve_target` 的路由臂 + `bind_run` 条件 + `match event_type` 新臂 + `run_error` 臂）
- Modify: `.../chat/state/mod.rs`（`fail_run` 签名）

**Interfaces:**
- Consumes: `ChatState::mark_queued`（Task 7）· `ChatSendError::from_wire_code`（Task 6）· `stream.run_queued`（Task 2）
- Produces: `ChatState::fail_run(&self, run_id: &str, error: &str, error_code: Option<&str>)`

**⚠️ 路由是本任务的重点**：`resolve_target` 的三步解析写在一个**字面量** `"run_accepted"` 臂上（`events.rs:683`），`bind_run` 的条件也是字面量（`events.rs:709`）。`run_queued` 现在是一个 run 的**第一帧**，它和 `run_accepted` 一样可能在本客户端还没有 route 时到达，所以**必须同臂**。

- [ ] **Step 1: 写失败测试**

该文件的 `resolve_target` 测试用的是这套夹具（`events.rs:1485` 起，逐字沿用，**不要新造**）：

```rust
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let a = sessions.open_conversation("agent-a", "A");
        sessions.activate(singleton, a);
```

追加三条：

```rust
    /// `run_queued` is now a run's FIRST frame, so it needs the same
    /// three-step resolution `run_accepted` has: route, then the session key
    /// the frame carries, then the foreground only when nothing proves it
    /// belongs elsewhere. Without this it falls through to `route_lookup`,
    /// finds nothing, and a queued run is invisible even in the tab that
    /// started it.
    #[test]
    fn run_queued_resolves_like_run_accepted() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let a = sessions.open_conversation("agent-a", "A");
        sessions.activate(singleton, a);

        assert!(
            resolve_target(&sessions, singleton, "run_queued", "run-a", Some("sk-a")).is_some(),
            "a queued run must reach the conversation that started it"
        );
    }

    /// A queued frame for a session this client can prove belongs elsewhere is
    /// dropped, not painted into whatever the viewer happens to be reading —
    /// the same defect the unconditional `run_accepted` fallback caused.
    ///
    /// Model the "foreground is a DIFFERENT session" setup on the existing
    /// test `the defect the run_accepted fallback used to cause`
    /// (`events.rs` ~1735) and change only the event type.
    #[test]
    fn run_queued_for_a_foreign_session_is_dropped() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let a = sessions.open_conversation("agent-a", "A");
        sessions.activate(singleton, a);
        // Give the foreground a key, so the incoming one can be proved foreign.
        sessions.bind_run("run-mine", a, Some("sk-mine"));

        assert!(
            resolve_target(&sessions, singleton, "run_queued", "run-x", Some("sk-somebody-else"))
                .is_none(),
            "a queued run whose session is open in no tab must be dropped"
        );
    }

    /// The server already classified this failure and named the bucket.
    /// Passing only the prose is what left Stop rendering as an UNKNOWN error
    /// banner.
    #[test]
    fn run_error_forwards_the_servers_error_code() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("run-a");
        chat.fail_run("run-a", "task cancelled", Some("CANCELLED"));

        assert_eq!(
            chat.send_error.get_untracked().map(|e| e.code),
            Some(ChatSendErrorCode::Cancelled)
        );
    }
```

> ⚠️ `sessions.bind_run(..)` 的第三参与 `chat.send_error` 的字段名**以文件实际为准**。第二条测试若用 `bind_run` 搭不出「前台已有一个不同 key」的状态，直接照 `events.rs` ~1735 那条既有测试的搭法复制——它就是为这个场景写的。

- [ ] **Step 2: 跑测试确认它红**

```
cargo test -p aleph-panel --lib events::
```
Expected: **RED** 或 BUILD-ERROR（`fail_run` 参数数量不符 / 路由返回 `None`）。

- [ ] **Step 3: 实现**

**3a.** `events.rs:683` 的臂头改为两个字面量，并在既有注释末尾追加理由：

```rust
        // `run_queued` joins this arm because it is now a run's FIRST frame:
        // it can arrive before this client has any route for the run, and it
        // carries the same `session_key`. Every LATER frame still routes by
        // `route_lookup` alone — only the first one has nothing to look up.
        "run_accepted" | "run_queued" => sessions
```

**3b.** `events.rs:709` 的 bind 条件：

```rust
    if matches!(event_type, "run_accepted" | "run_queued") && sessions.route_lookup(run_id).is_none()
    {
        sessions.bind_run(run_id, conv, session_key);
    }
```

（既有的 `route_lookup(run_id).is_none()` 守卫已经防了重复绑定：`run_queued` 先绑，随后的 `run_accepted` 看到 route 已在就不再绑。）

**3c.** `match event_type` 里，在 `"run_accepted"` 臂**之前**加：

```rust
            "run_queued" => {
                // Backfill the key exactly like `run_accepted` does, for the
                // same reason: a brand-new conversation learns its
                // server-assigned key from whichever of the two arrives first,
                // and for a queued run that is this one.
                if let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) {
                    if chat.session_key.get_untracked().is_none() {
                        chat.session_key.set(Some(sk.to_string()));
                    }
                }
                let ahead = data
                    .get("ahead")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                chat.start_assistant_message(run_id);
                chat.mark_queued(run_id, u16::try_from(ahead).unwrap_or(u16::MAX));
            }
```

**3d.** `run_error` 臂改为转发 code：

```rust
            "run_error" => {
                let error = data
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("Unknown error");
                // The server already classified this failure and named the
                // bucket. Dropping the code here is what made Stop, a rejected
                // queued message, and an expired key all render as UNKNOWN.
                let error_code = data.get("error_code").and_then(|c| c.as_str());
                chat.fail_run(run_id, error, error_code);
```

**3e.** `state/mod.rs` 的 `fail_run` 加第三参，内部把 `ChatSendError::classify(error)` 换成 `ChatSendError::from_wire_code(error_code, error)`。**全仓搜 `fail_run(` 补齐其余调用点**（编译器会点名）。

**3f.（控制器裁定 R3）** 既有的 `"run_accepted"` 臂（`events.rs:926`）末尾，在 `chat.start_assistant_message(run_id);` **之后**加一行：

```rust
                // Admission is the edge that ends the wait, and it cannot ride
                // on `start_assistant_message`: that early-returns once the
                // run's bubble exists, and the queued frame has already
                // created it. Without this the phase reads "queued" until the
                // first `turn_started` or token — the whole of model latency.
                chat.mark_admitted(run_id);
```

（`mark_admitted` 只碰 `Queued`，所以没有排过队的 run 逐字节不受影响。）

- [ ] **Step 4: 跑测试确认它绿**

```
cargo test -p aleph-panel --lib events::
cargo test -p aleph-panel --lib --no-run
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
```
Expected: 三条全过。

- [ ] **Step 5: 变异证伪**

把 3a 的臂头改回单个 `"run_accepted"`，重跑第一条。
Expected: **RED** —— `run_queued_resolves_like_run_accepted`。确认后改回。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/
git commit -m "panel: render the queued phase and stop guessing at failure codes"
```

---

### Task 9: attach 时从 `pending[]` 重建排队相位

**Files:**
- Modify: `interfaces/webchat/src/api/chat.rs`（`SessionHistory` + 解析）
- Modify: `.../chat/state/mod.rs`（`hydrate_and_follow` 或其等价的水化路径）

**Interfaces:**
- Consumes: `chat.history` 的 `pending`（Task 4）· `aleph_protocol::queue::PendingRun`（Task 4）· `ChatState::mark_queued`（Task 7）
- Produces: `SessionHistory.pending: Vec<PendingRun>`；`pub fn parse_history_pending(result: &Value) -> Vec<PendingRun>`

**⚠️ 控制器裁定 R4（覆盖计划初稿）**：**不要**定义 `PendingRunDto`。读的是任务 4 放进 `aleph_protocol::queue` 的**同一个** `PendingRun`，且逐项**用 serde 反序列化**而不是手抄 `"run_id"` / `"ahead"` 两个字面量——手抄的话服务端改字段名不会在这一侧变成编译错误，共用类型就白共用了。外层的宽容守卫保留（`as_array` 拿不到 ⇒ 空），逐项失败 `filter_map` 掉。

- [ ] **Step 1: 写失败测试**

在 `interfaces/webchat/src/api/chat.rs` 的测试模块里（沿用 `parse_history_plan` 已有测试的写法）：

```rust
    /// A client that attaches mid-wait never received the `RunQueued` frames —
    /// they fired before its socket existed. Without the snapshot it paints
    /// "thinking" over a queue it cannot see.
    #[test]
    fn pending_is_read_off_the_history_response() {
        let raw = serde_json::json!({
            "messages": [],
            "active_run": null,
            "pending": [
                {"run_id": "run-a", "ahead": 0},
                {"run_id": "run-b", "ahead": 1},
            ]
        });
        let pending = parse_history_pending(&raw);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[1].run_id, "run-b");
        assert_eq!(pending[1].ahead, 1);
    }

    /// The shape is `aleph_protocol::queue::PendingRun`, the same type the
    /// server serializes — not a Panel-local copy of its field names. That is
    /// what makes a server-side rename a compile error here instead of a
    /// client that silently reads an empty queue. Asserting it by round-trip
    /// keeps the check on the type rather than on a literal key list.
    #[test]
    fn the_shape_is_the_shared_protocol_type() {
        let one = aleph_protocol::queue::PendingRun {
            run_id: "run-a".to_string(),
            ahead: 2,
        };
        let raw = serde_json::json!({ "pending": [serde_json::to_value(&one).unwrap()] });
        assert_eq!(parse_history_pending(&raw), vec![one]);
    }

    /// Absent against a core that predates the field, and absent when nothing
    /// is waiting. Both mean "no queue to show", so neither may error.
    #[test]
    fn a_missing_or_malformed_pending_array_reads_as_empty() {
        assert!(parse_history_pending(&serde_json::json!({"messages": []})).is_empty());
        assert!(parse_history_pending(&serde_json::json!({"pending": "nonsense"})).is_empty());
    }
```

- [ ] **Step 2: 跑测试确认它红**

```
cargo test -p aleph-panel --lib api::chat::
```
Expected: **BUILD-ERROR**（`parse_history_pending` 不存在）。

- [ ] **Step 3: 实现**

`interfaces/webchat/src/api/chat.rs`：

```rust
use aleph_protocol::queue::PendingRun;

/// Read the wait lane off a `chat.history` response.
///
/// Free function so the skew and malformed cases are testable without a live
/// socket — same shape as `parse_history_plan` next door. Absent reads as
/// empty: a core that predates the field and an idle session are
/// indistinguishable here, and both mean "no queue to show".
///
/// Each item is deserialized into the shared protocol type rather than
/// hand-read key by key. That is the whole point of the type being shared: a
/// field renamed on the server changes this side's parse at the same time,
/// instead of leaving a client that reads an empty queue and says nothing.
/// An item that fails to deserialize is dropped, not fatal — one malformed
/// entry must not hide the rest of the lane.
#[must_use]
pub fn parse_history_pending(result: &Value) -> Vec<PendingRun> {
    let Some(items) = result.get("pending").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| serde_json::from_value::<PendingRun>(v.clone()).ok())
        .collect()
}
```

`SessionHistory` 加字段（并在其 doc 末尾追加一段说明，与 `active_run` / `plan` 并列）：

```rust
    pub pending: Vec<PendingRun>,
```

解析处加：

```rust
            pending: parse_history_pending(&result),
```

在水化路径（`hydrate_and_follow` 或等价处，即今天消费 `history.active_run` 的那一段）之后加：

```rust
        // Restore the queued phase for the run this client is following. Live
        // clients got here via `RunQueued`; a client that attached mid-wait
        // has only this. `mark_queued` is already scoped to `active_run_id`,
        // so a lane entry for a sibling run repaints nothing.
        if let Some(run) = history.active_run.as_deref() {
            if let Some(entry) = history.pending.iter().find(|p| p.run_id == run) {
                chat.mark_queued(run, entry.ahead);
            }
        }
```

> ⚠️ 变量名 `history` / `chat` 与插入位置**以该文件实际水化代码为准**。找到今天读 `active_run` 的那几行，紧跟其后。

- [ ] **Step 4: 跑测试确认它绿**

```
cargo test -p aleph-panel --lib api::chat::
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
```
Expected: 两条都过。

- [ ] **Step 5: 变异证伪**

把 `parse_history_pending` 的第一行改成 `return Vec::new();`，重跑第一条。
Expected: **RED** —— `pending_is_read_off_the_history_response`。确认后改回。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/
git commit -m "panel: rebuild the queued phase from the attach-time snapshot"
```

---

### Task 10: CUT `EventEmitter::emit_run_error`

**Files:**
- Modify: `src/gateway/event_emitter/mod.rs`（删 `emit_run_error`，约 178 行起）

**Interfaces:**
- Consumes: 无
- Produces: 无（纯删除）

**依据**：全仓零生产者——所有 `RunError` 都直接构造 `StreamEvent`。P6「删除优于注释」。

- [ ] **Step 1: 确认它仍然零生产者**

```
grep -rn "emit_run_error" src/ interfaces/ shared/ --include='*.rs' | grep -v "^src/gateway/event_emitter/mod.rs"
```
Expected: **无输出**。

⚠️ **若有输出，停止本任务并报告**——记录已过期，那就不是 CUT 而是别的东西。判据：「扫断线前先剥掉注释行；把 bug 藏起来的注释正是它唯一的搜索命中」，所以命中若只是注释，仍算零生产者，但要在提交信息里说明。

- [ ] **Step 2: 删除**

删掉 `src/gateway/event_emitter/mod.rs` 里整个 `async fn emit_run_error` 默认方法及其 doc comment。

- [ ] **Step 3: 验证**

```
cargo test -p alephcore --lib --no-run
cargo clippy --all-targets
```
Expected: 构建通过，clippy 无新警告。

- [ ] **Step 4: 提交**

```bash
git add src/gateway/event_emitter/mod.rs
git commit -m "gateway: cut EventEmitter::emit_run_error, a trait method with no producers"
```

---

### Task 11: 文档

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§4.7 · §4.8 · §6.1 · §6.9）
- Modify: `docs/reference/GATEWAY.md`

- [ ] **Step 1: FEATURE_LOCATOR 三条已过期登记项改正**

按 spec §4.2 表逐条改：
1. §4.7 的 `[memory.assembler] render_style` 「无人读（DECIDE）」→ 改为已 CONNECT（`thinker/memory_context_provider/memory.rs:115` 读它；`DISCARD_TAG_PAIRS[1]` 已加 `<memory>` 围栏 + 漂移守卫）。
2. §4.8 的 `render_user_session_text` CUT 候选 → 删除该条（符号已不存在）。
3. §6.9「已知边界」里「对端发的那条用户消息没有实时回显」→ 删除（`stream.session_user_message` 已在 wire 上）。

- [ ] **Step 2: 三节各加本轮条目**

- **§4.7**：新增 Round-8 条目，记 `RunQueued` 帧、错误码跨端收口、`emit_run_error` CUT。**必须写明**：`RunError.session_key`（Round-8 ②）现已冗余但**刻意保留**，理由是「帧自解析比索引更强」——否则下一个读者会把它当残留清掉。
- **§4.8**：新增 Round-11 条目，记「等待者自报位置」这个产地选择与它否决的两个替代方案（车道广播 / 客户端轮询），以及 owed backlog 的接缝确认（四臂与墓碑四臂同源）。
- **§6.1**：新增条目，记 `ChatPhase::Queued`、`pending[]` 水化、以及「加变体是编译错误」这个形状。

- [ ] **Step 3: GATEWAY.md**

`busy_queue` 一节补两段：「等待态的 wire 表示」与「为什么位置由等待者自报」。

- [ ] **Step 4: 提交**

```bash
git add docs/
git commit -m "docs: record the queue-visibility round and correct three stale entries"
```

---

## 收尾验证（全部任务完成后）

- [ ] **最小验证集五条全绿**

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
cargo clippy --all-targets
```

- [ ] **判据 §10 补充：本轮动了协议 crate，两个客户端 crate 必须一起跑**

```
cargo test -p aleph-tui -p aleph-cli --no-run
```

- [ ] **全量 lib 测试**

```
cargo test -p alephcore --lib
cargo test -p aleph-panel --lib
```
⚠️ **一次跑没有跑到 `test result:` 行就不算跑过**。尾巴上的 "has been running for over 60 seconds" 是唯一的症状。

- [ ] **真机 QA（红/绿双向）**

装置：隔离 `HOME` + `ALEPH_HOME`、loopback 上一个慢速确定性 mock provider、Chrome。

**GREEN**：一个会话连发两条 → 第二条显示「排队中 · 前面还有 1 条」→ **刷新页面仍在**（走 `pending[]`）→ 第一条完成 → 第二条转 Thinking → 完成。
**RED**（同场景，只把 Task 3 的 `report(ahead).await` 剪断的对照二进制）：第二条全程「思考中」，刷新后回到空白。

**终局面**：把 `max_wait_secs` 调到 5s 让第二条超时 → Panel 显示 `AGENT_BUSY` 对应文案，**不是** `UNKNOWN` 红横幅。

⚠️ QA 的时钟必须是**会话日志**，不是墙钟。
