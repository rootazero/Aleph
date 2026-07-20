# 多会话平行 · P0+P1 地基 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Aleph 的 run 并发闸从 per-agent（`AgentState`）改为 per-session（新 `SessionRunRegistry`），让同一 agent 的多个会话真并行，同时守住 transcript 不交错、agent 记忆隔离不回归、seq 不再永久卡死。

**Architecture:** 双闸——**会话互斥锁**（`SessionRunRegistry`，键=SessionKey，保一会话一 run）+ **并发上限许可**（`ConcurrencyLimiter` 信号量，持有到 run 结束）。前置去险：修 `SessionActor` seq 自愈 + 拆 `execute.rs` 巨石。全部落在 `src/gateway/`（编排层），不进 `src/harness/`（R10）。

**Tech Stack:** Rust · tokio（`Semaphore`/`OwnedSemaphorePermit`）· `crate::sync_primitives`（poison-safe `Mutex`）· thiserror · async-trait。

## Global Constraints

- **INV-ISO（隔离红线）**：并发只改"何时开跑"，绝不改"run 看见什么身份/记忆/config"。每 run 用自己的 `SessionKey` 经 task-local `TURN_CONTEXT` 携带身份；记忆按 `agent_id` 物理分区。禁止 agent 级共享可变的"当前会话/agent"态被 run 环境式读取。
- **INV-SEQ（单写者红线）**：每会话同一时刻只有一个逻辑写者 + 一会话一 run。
- **R10**：所有新机件在 `src/gateway/execution_engine/`，**不进 `src/harness/`**。
- **锁安全（P7）**：`crate::sync_primitives::Mutex` 一律 `.lock().unwrap_or_else(|e| e.into_inner())`。
- **不可变优先**：`let` 默认，返回新值优于原地改。
- **cargo 极度节制**：至多在 Task 6（闸合并）后跑一次 `cargo check --lib`。其余任务只跑本任务定向单测（`cargo test -p alephcore <test_name>`）。
- **commit 规范**：English，`<scope>: <description>`（如 `session: self-heal SessionActor seq on append collision`）。单分支 main。
- 默认并发值：per-session **1**（硬编码，即 registry 本身）· per-agent **3** · global **8**（可配）。

---

## 文件结构（本轮创建/修改）

| 文件 | 职责 | 动作 |
|---|---|---|
| `src/session/actor.rs` | SessionActor `EmitEvent` seq 分配 | 修改（自愈） |
| `src/gateway/execution_engine/session_run_registry.rs` | per-session 会话互斥锁 | **新建** |
| `src/gateway/execution_engine/concurrency.rs` | 并发上限信号量 + RAII 许可 | **新建** |
| `src/gateway/execution_engine/gate.rs` | 准入闸（从 execute.rs 抽出） | **新建** |
| `src/gateway/execution_engine/spawn.rs` | run task spawn（从 execute.rs 抽出） | **新建** |
| `src/gateway/execution_engine/post_run.rs` | 收尾续跑（从 execute.rs 抽出） | **新建** |
| `src/gateway/execution_engine/execute.rs` | 瘦编排器 | 修改（拆分 + 闸重写） |
| `src/gateway/execution_engine/engine.rs` | ExecutionEngine 结构体 | 修改（加 registry+limiter 字段） |
| `src/gateway/execution_engine/mod.rs` | 配置 + 模块声明 | 修改 |
| `src/gateway/agent_instance.rs` | AgentInstance | 修改（退休 gate 语义） |
| `src/config/types/*` (execution) | `[execution]` 配置 | 修改（加两个上限字段） |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:729` | engine_config 构建 | 修改（连线上限） |
| `src/gateway/handlers/gateway_metrics.rs` | metrics 快照 | 修改（透出 N/M 槽） |

---

## Task 1（P0-b）: SessionActor seq 自愈

**Files:**
- Modify: `src/session/actor.rs:93-115`（`ActorCommand::EmitEvent` 分支）
- Test: `src/session/actor.rs`（同文件 `#[cfg(test)]`）

**Interfaces:**
- Consumes: `SessionEventStore::append(session_id, seq, event, created_at_ms) -> Result<(), SessionError>`（`store.rs:39`）；`SessionEventStore::load_head_seq(session_id) -> Result<EventSeq, SessionError>`（`store.rs:62`）。
- Produces: 行为——append 撞键/失败后 actor **resync `head_seq` + 重试一次**，不再永久卡死。

**背景**：`actor.rs:112-114` 的 `Err(e) => { let _ = reply.send(Err(e)); }` 不 resync `head_seq`，撞键后永远算出同一 seq → 该会话写入永久卡死（审计 4.1）。`SessionError` 无专用冲突变体（`service.rs:14`：只有 `Storage(String)`），故用"失败→resync→重试一次"的鲁棒策略（幂等、防死循环、不靠字符串匹配）。

- [ ] **Step 1: 写失败测试**

在 `actor.rs` 的 `#[cfg(test)] mod tests` 加（若无测试模块则新建，`use super::*;`）。测试用一个**受控 store**：首次 `append` 对 seq=1 返回 `Err(Storage("UNIQUE"))`（模拟直写者已占 seq=1），`load_head_seq` 返回 1，第二次 `append`（seq=2）成功。断言 actor 的 `emit_event` 最终返回 `Ok(2)` 而非 `Err`。

```rust
#[tokio::test]
async fn actor_self_heals_seq_after_append_collision() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // 受控 store：第一次 append 撞键，load_head_seq=1，第二次 append 成功。
    struct CollideOnceStore { appends: AtomicUsize, head: AtomicUsize }
    #[async_trait::async_trait]
    impl SessionEventStore for CollideOnceStore {
        async fn append(&self, _id: &SessionId, seq: EventSeq, _e: &SessionEvent, _at: i64)
            -> Result<(), SessionError> {
            let n = self.appends.fetch_add(1, Ordering::SeqCst);
            if n == 0 { // 首次：seq=1 撞键
                assert_eq!(seq, 1);
                self.head.store(1, Ordering::SeqCst); // 直写者已落 seq=1
                Err(SessionError::Storage("UNIQUE constraint failed".into()))
            } else { // 重试：应为 seq=2
                assert_eq!(seq, 2);
                self.head.store(2, Ordering::SeqCst);
                Ok(())
            }
        }
        async fn load_all_events(&self, _id: &SessionId) -> Result<Vec<SessionEventRecord>, SessionError> { Ok(vec![]) }
        async fn load_events_range(&self, _id: &SessionId, _f: Option<EventSeq>, _t: Option<EventSeq>) -> Result<Vec<SessionEventRecord>, SessionError> { Ok(vec![]) }
        async fn load_head_seq(&self, _id: &SessionId) -> Result<EventSeq, SessionError> {
            Ok(self.head.load(Ordering::SeqCst) as EventSeq)
        }
        // 其余 trait 方法用 unimplemented!() 或最小桩，按 SessionEventStore 实际签名补全。
    }
    // 构造 actor（复用现有测试构造助手），emit 一个事件，断言 Ok(2)。
    // 具体构造见本文件既有测试的 actor 起法（load_head_seq 初值 0 → head_seq=0 → 首 seq=1）。
}
```

> 注：`SessionEventStore` 的完整方法集见 `store.rs:37-` — 桩实现须覆盖全部方法（未用到的 `unimplemented!()`）。若文件已有 mock store，直接扩展它而非新建。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore actor_self_heals_seq_after_append_collision`
Expected: FAIL（当前 Err 直接上抛，返回 `Err(Storage(...))` 非 `Ok(2)`）。

- [ ] **Step 3: 实现自愈**

把 `actor.rs:93-115` 的 `EmitEvent` 分支改为（保留成功路径逻辑，仅在 `Err` 分支加 resync+重试一次）：

```rust
Some(ActorCommand::EmitEvent { event, reply }) => {
    let at = now_ms();
    let mut seq = self.head_seq + 1;
    let mut append_result = self.store.append(&self.id, seq, &event, at).await;

    // Self-heal: an append failure (typically a `(session_id, seq)` UNIQUE
    // collision from a direct-store writer racing the actor — audit 4.1)
    // must not permanently wedge this session's writes. Resync `head_seq`
    // from the store and retry ONCE. Bounded: no loop, propagate on second
    // failure. `append` is a single atomic INSERT, so a failed attempt wrote
    // nothing and the retry cannot double-write.
    if append_result.is_err() {
        if let Ok(stored_head) = self.store.load_head_seq(&self.id).await {
            self.head_seq = stored_head;
            seq = self.head_seq + 1;
            append_result = self.store.append(&self.id, seq, &event, at).await;
        }
    }

    match append_result {
        Ok(()) => {
            let record = SessionEventRecord { seq, event, created_at_ms: at };
            self.state.apply(&record.event);
            self.head_seq = seq;
            if let Some(obs) = &self.observer {
                obs.on_appended(&self.id, &record);
            }
            let _ = self.broadcaster.send(record);
            let _ = reply.send(Ok(seq));
            idle_deadline = Instant::now() + self.idle_timeout;
        }
        Err(e) => {
            let _ = reply.send(Err(e));
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore actor_self_heals_seq_after_append_collision`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/session/actor.rs
git commit -m "session: self-heal SessionActor seq on append collision (audit 4.1)"
```

---

## Task 2（P0-a）: 拆分 `execute.rs` 巨石（纯行为保持）

**Files:**
- Create: `src/gateway/execution_engine/gate.rs`、`spawn.rs`、`post_run.rs`
- Modify: `src/gateway/execution_engine/execute.rs`、`mod.rs`（加 `mod` 声明）

**Interfaces:**
- Produces: 三个私有子模块，`execute()` 瘦成薄编排器。**无公共签名变化，无行为变化**——现有 `execution_engine` 测试全绿即验收。

**说明**：`execute.rs` = 1657 行，`execute()`（`:108-1250`）内联了准入闸、busy 分支、run spawn、收尾续跑（topic-gen/压缩/goal-loop/strategy）。本任务**纯机械搬移**，按职责切三块，为 Task 6 的闸重写隔离出小落点。这是搬移不是重写——保持被搬函数体逐字不变。

> P0-a 与 P0-b（Task 1）文件不相交（`execute.rs` vs `actor.rs`），先后无所谓；两者都须在 Task 3+ 之前。

- [ ] **Step 1: 建三个空子模块并声明**

`mod.rs` 在既有子模块声明处（`execute` 附近）加：

```rust
mod gate;
mod spawn;
mod post_run;
```

创建 `gate.rs` / `spawn.rs` / `post_run.rs`，各写文件头 doc 注释 + `use super::*;`（按需）。

- [ ] **Step 2: 搬移准入闸到 `gate.rs`**

把 `execute.rs:123-231`（`try_start_run` 门 + 三态 busy 分支 + 死 count 检查）整段抽成 `gate.rs` 的一个 `impl` 方法或自由函数，签名保留其消费的 `&self`/`request`/`agent`/`run_id`/`active_runs`。**逐字搬移，不改逻辑**。`execute()` 原位置改为调用它。

> 具体切法：执行者先 Read `execute.rs:108-260` 确认闭包捕获，再抽出返回 `Result<(), ExecutionError>` 的门函数；`Ok(())` 表示"已放行、继续 run"，`Err`/早退表示 busy/拒绝。

- [ ] **Step 3: 搬移 run spawn 到 `spawn.rs`、收尾到 `post_run.rs`**

把 run task 的 `tokio::spawn` 组装段移到 `spawn.rs`；把收尾续跑段（topic-gen / 压缩触发 / goal-loop / strategy，`execute.rs` 后半 ~`:1150-1250`）移到 `post_run.rs`。同样逐字搬移。

- [ ] **Step 4: 跑现有测试确认零回归**

Run: `cargo test -p alephcore --lib execution_engine`
Expected: 全绿（行为保持）。若有失败＝搬移引入了差异，逐一比对回滚。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/
git commit -m "gateway: split execute.rs monolith into gate/spawn/post_run (behavior-preserving)"
```

---

## Task 3（P1）: `SessionRunRegistry` 会话互斥锁（新代码）

**Files:**
- Create: `src/gateway/execution_engine/session_run_registry.rs`
- Modify: `src/gateway/execution_engine/mod.rs`（`mod session_run_registry; pub(crate) use ...`）
- Test: 同文件 `#[cfg(test)]`

**Interfaces:**
- Consumes: `crate::routing::session_key::SessionKey`（`.to_key_string() -> String`、`.agent_id() -> &str`）。
- Produces:
  - `SessionRunRegistry::default() -> Self`
  - `fn try_claim(&self, session_key: &SessionKey, run_id: &str) -> bool`
  - `fn release(&self, session_key: &SessionKey, run_id: &str)`
  - `fn is_running(&self, session_key: &SessionKey) -> bool`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    fn sk(agent: &str, conv: &str) -> SessionKey {
        // 用 SessionKey 的实际构造 API 建一个 (agent, conversation) 键。
        // 见 src/routing/session_key.rs 的构造函数。
        SessionKey::new(agent, conv) // 按实际签名调整
    }

    #[test]
    fn claim_is_exclusive_per_session_but_free_across_sessions() {
        let reg = SessionRunRegistry::default();
        let a1 = sk("main", "conv-1");
        let a2 = sk("main", "conv-2"); // 同 agent 不同会话

        assert!(reg.try_claim(&a1, "run-1"));
        assert!(!reg.try_claim(&a1, "run-1b"), "同会话二次 claim 必须被拒");
        assert!(reg.try_claim(&a2, "run-2"), "同 agent 不同会话必须放行（真并行）");

        reg.release(&a1, "run-1");
        assert!(reg.try_claim(&a1, "run-3"), "release 后可再 claim");
    }

    #[test]
    fn release_only_matching_run_id() {
        let reg = SessionRunRegistry::default();
        let s = sk("main", "conv-1");
        assert!(reg.try_claim(&s, "run-A"));
        reg.release(&s, "run-STALE"); // 陈旧 run 的迟到 release 不得释放当前 claim
        assert!(reg.is_running(&s), "不匹配的 release 不生效");
        reg.release(&s, "run-A");
        assert!(!reg.is_running(&s));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore session_run_registry`
Expected: FAIL（`SessionRunRegistry` 未定义）。

- [ ] **Step 3: 实现 registry**

```rust
//! Per-session run mutual-exclusion registry.
//!
//! Replaces the per-agent `AgentState` gate (`agent_instance.rs::try_start_run`).
//! Exactly one run may be Running per `SessionKey` at a time (INV-SEQ / audit
//! 4.2: prevents two runs interleaving one session's `session_events`, which
//! would corrupt the transcript). Sessions of the *same* agent no longer
//! contend — they run in parallel (bounded by `ConcurrencyLimiter`).

use std::collections::HashMap;

use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Mutex;

/// Tracks the single in-flight run per session. `session_key_string -> run_id`.
#[derive(Default)]
pub struct SessionRunRegistry {
    running: Mutex<HashMap<String, String>>,
}

impl SessionRunRegistry {
    /// Atomically claim this session's single run slot. `true` = claimed,
    /// `false` = a run is already active on this session (caller routes the
    /// message to the per-session `BusyInputMode` steer/interrupt/queue path).
    #[must_use]
    pub fn try_claim(&self, session_key: &SessionKey, run_id: &str) -> bool {
        let key = session_key.to_key_string();
        let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if map.contains_key(&key) {
            return false;
        }
        map.insert(key, run_id.to_string());
        true
    }

    /// Release this session's run slot. Idempotent, and only releases when the
    /// stored `run_id` matches — a superseded run's late release can't free a
    /// newer run's claim.
    pub fn release(&self, session_key: &SessionKey, run_id: &str) {
        let key = session_key.to_key_string();
        let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if map.get(&key).map(String::as_str) == Some(run_id) {
            map.remove(&key);
        }
    }

    /// Is a run currently active on this session?
    #[must_use]
    pub fn is_running(&self, session_key: &SessionKey) -> bool {
        let key = session_key.to_key_string();
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key)
    }
}
```

> 执行者先 Read `src/routing/session_key.rs` 确认 `SessionKey::new` / 构造与 `to_key_string`/`agent_id` 的确切签名，据此微调测试构造与 import。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore session_run_registry`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/session_run_registry.rs src/gateway/execution_engine/mod.rs
git commit -m "gateway: SessionRunRegistry per-session run mutual-exclusion (audit 4.2)"
```

---

## Task 4（P1）: `ConcurrencyLimiter` 并发上限许可（新代码）

**Files:**
- Create: `src/gateway/execution_engine/concurrency.rs`
- Modify: `src/gateway/execution_engine/mod.rs`（`mod concurrency;`）
- Test: 同文件 `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `ConcurrencyLimiter::new(global_cap: usize, per_agent_cap: usize) -> Self`
  - `async fn acquire(&self, agent_id: &str) -> RunPermit`（global + per-agent 双许可，满则 await）
  - `fn try_acquire(&self, agent_id: &str) -> Option<RunPermit>`（不阻塞，供"先试后排队并发背压"）
  - `fn snapshot(&self) -> ConcurrencySnapshot { global_in_use, global_total }`
  - `struct RunPermit`（持有两个 `OwnedSemaphorePermit`，drop 即释放）

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn global_cap_bounds_total_and_releases_on_drop() {
        let lim = ConcurrencyLimiter::new(2, 5);
        let p1 = lim.try_acquire("main").expect("slot 1");
        let p2 = lim.try_acquire("other").expect("slot 2");
        assert!(lim.try_acquire("third").is_none(), "global cap=2 已满");
        assert_eq!(lim.snapshot().global_in_use, 2);
        drop(p1);
        assert!(lim.try_acquire("third").is_some(), "drop 释放全局槽");
        drop(p2);
    }

    #[tokio::test]
    async fn per_agent_cap_bounds_one_agent_without_starving_others() {
        let lim = ConcurrencyLimiter::new(10, 2);
        let _a1 = lim.try_acquire("main").unwrap();
        let _a2 = lim.try_acquire("main").unwrap();
        assert!(lim.try_acquire("main").is_none(), "per-agent cap=2 已满");
        assert!(lim.try_acquire("other").is_some(), "别的 agent 不受 main 影响");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore concurrency::tests`
Expected: FAIL（未定义）。

- [ ] **Step 3: 实现 limiter**

```rust
//! Run-lifetime concurrency limiter.
//!
//! Unlike the RPC lane permit (`lane.rs`, released at dispatch — audit 1.4),
//! a `RunPermit` is held for the whole run (acquired at the gate, dropped when
//! `execute()` returns). Two caps stack: a `global` semaphore and a per-agent
//! sub-cap so one busy agent can't monopolize all global slots (audit C4).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::sync_primitives::Mutex;

pub struct ConcurrencySnapshot {
    pub global_in_use: usize,
    pub global_total: usize,
}

pub struct RunPermit {
    _global: OwnedSemaphorePermit,
    _agent: OwnedSemaphorePermit,
}

pub struct ConcurrencyLimiter {
    global: Arc<Semaphore>,
    global_total: usize,
    per_agent_cap: usize,
    per_agent: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl ConcurrencyLimiter {
    #[must_use]
    pub fn new(global_cap: usize, per_agent_cap: usize) -> Self {
        // Clamp to >=1 (a 0-permit semaphore would deadlock every run).
        let global_cap = global_cap.max(1);
        let per_agent_cap = per_agent_cap.max(1);
        Self {
            global: Arc::new(Semaphore::new(global_cap)),
            global_total: global_cap,
            per_agent_cap,
            per_agent: Mutex::new(HashMap::new()),
        }
    }

    fn agent_sem(&self, agent_id: &str) -> Arc<Semaphore> {
        let mut map = self.per_agent.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.per_agent_cap)))
            .clone()
    }

    /// Acquire both a global and a per-agent permit, awaiting if either cap is
    /// full. The per-agent permit is taken first so a saturated agent waits on
    /// its own sub-cap without consuming a scarce global slot.
    pub async fn acquire(&self, agent_id: &str) -> RunPermit {
        let agent_sem = self.agent_sem(agent_id);
        let agent = agent_sem.acquire_owned().await.expect("agent sem never closed");
        let global = self.global.clone().acquire_owned().await.expect("global sem never closed");
        RunPermit { _global: global, _agent: agent }
    }

    /// Non-blocking variant. Returns `None` if either cap is currently full.
    #[must_use]
    pub fn try_acquire(&self, agent_id: &str) -> Option<RunPermit> {
        let agent_sem = self.agent_sem(agent_id);
        let agent = Arc::clone(&agent_sem).try_acquire_owned().ok()?;
        let global = self.global.clone().try_acquire_owned().ok()?;
        Some(RunPermit { _global: global, _agent: agent })
    }

    #[must_use]
    pub fn snapshot(&self) -> ConcurrencySnapshot {
        ConcurrencySnapshot {
            global_in_use: self.global_total - self.global.available_permits(),
            global_total: self.global_total,
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore concurrency::tests`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/concurrency.rs src/gateway/execution_engine/mod.rs
git commit -m "gateway: ConcurrencyLimiter run-lifetime global+per-agent caps (audit 1.4/C4)"
```

---

## Task 5（P1）: 上限配置连线（`[execution]` + engine 构建）

**Files:**
- Modify: `src/config/types/`（`ExecutionConfig` — 执行者 grep `mid_turn_steering` 找到其结构体）
- Modify: `src/gateway/execution_engine/mod.rs:76,102`（`ExecutionEngineConfig`）
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:729`
- Test: config 默认值 + clamp 单测

**Interfaces:**
- Consumes: 无（配置层）。
- Produces: `ExecutionEngineConfig` 新增 `max_runs_global: usize`（默认 8）、`max_runs_per_agent: usize`（默认 3）；`ExecutionConfig` 新增对应 TOML 字段（缺省回退默认，向后兼容）。

> **偏离 spec 说明**：spec 写"连进 `[gateway]`"；但 `mid_turn_steering`/`default_timeout_secs` 这些直接兄弟旋钮都在 `[execution]`，并发上限是执行引擎旋钮，放 `[execution]` 与兄弟一致更内聚。

- [ ] **Step 1: 写失败测试（config 默认 + clamp）**

在 `mod.rs` 的 `#[cfg(test)]` 加：

```rust
#[test]
fn engine_config_default_concurrency_caps() {
    let c = ExecutionEngineConfig::default();
    assert_eq!(c.max_runs_global, 8);
    assert_eq!(c.max_runs_per_agent, 3);
}
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p alephcore engine_config_default_concurrency_caps`
Expected: FAIL（字段不存在，编译错）。

- [ ] **Step 3: 加字段 + 默认 + 退休死旋钮**

`mod.rs`：删 `max_concurrent_runs`（死旋钮，审计 1.3），加两新字段。

```rust
// 结构体内（替换 max_concurrent_runs）：
    /// Global cap on concurrently-executing runs across all sessions/agents.
    /// Held for the run's lifetime by `ConcurrencyLimiter` (audit 1.4).
    pub max_runs_global: usize,
    /// Per-agent sub-cap so one busy agent can't monopolize all global slots
    /// (audit C4). per-session is hard-capped at 1 by `SessionRunRegistry`.
    pub max_runs_per_agent: usize,
```

```rust
// Default 内（替换 max_concurrent_runs: 5）：
            max_runs_global: 8,
            max_runs_per_agent: 3,
```

`ExecutionConfig`（config types）加两个 `#[serde(default = "...")]` 字段 `max_runs_global` / `max_runs_per_agent`，默认 fn 返回 8 / 3。`agent_init/mod.rs:729` 的 `ExecutionEngineConfig { ... }` 加：

```rust
            max_runs_global: app_config.execution.max_runs_global.max(1),
            max_runs_per_agent: app_config.execution.max_runs_per_agent.max(1),
```

删除 execute.rs:210-231 引用 `max_concurrent_runs` 的死 count 检查（Task 6 会一并处理其区域；此处先让编译通过——若 Task 6 尚未做，暂把该块的 `self.config.max_concurrent_runs` 换成 `self.config.max_runs_per_agent` 保持编译，Task 6 再整体删除）。

- [ ] **Step 4: 跑确认通过**

Run: `cargo test -p alephcore engine_config_default_concurrency_caps`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/config/ src/gateway/execution_engine/mod.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "config: wire [execution] max_runs_global/per_agent, retire dead max_concurrent_runs (audit 1.3)"
```

---

## Task 6（P1，核心）: 闸重写 — per-session claim + run-lifetime 许可

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs:42`（加字段）+ 其构造函数
- Modify: `src/gateway/execution_engine/gate.rs`（Task 2 抽出的门）
- Modify: `src/gateway/execution_engine/execute.rs`（RAII 守卫贯穿 run）
- Modify: `src/gateway/agent_instance.rs`（退休 `try_start_run` gate 语义）
- Test: 集成测试（两会话同 agent 并行 / 同会话 steer / cap 满排队）

**Interfaces:**
- Consumes: Task 3 `SessionRunRegistry`、Task 4 `ConcurrencyLimiter`、Task 5 配置。
- Produces: 门谓词 `session_run_registry.try_claim(&request.session_key, &run_id)` 取代 `agent.try_start_run`；RAII `RunSlot` 守卫持有会话 claim + `RunPermit`，run 结束（含早退/panic）自动释放。

- [ ] **Step 1: engine 加字段 + 构造**

`engine.rs:42` 结构体加：

```rust
    /// Per-session run mutual-exclusion (replaces per-agent AgentState gate).
    pub(super) session_run_registry: Arc<crate::gateway::execution_engine::session_run_registry::SessionRunRegistry>,
    /// Run-lifetime concurrency caps (global + per-agent).
    pub(super) concurrency: Arc<crate::gateway::execution_engine::concurrency::ConcurrencyLimiter>,
```

在 engine 的构造处（`ExecutionEngine::new` / builder — 执行者 grep `active_runs: Arc::new` 找到）初始化：

```rust
    session_run_registry: Arc::new(Default::default()),
    concurrency: Arc::new(crate::gateway::execution_engine::concurrency::ConcurrencyLimiter::new(
        config.max_runs_global, config.max_runs_per_agent,
    )),
```

（`SimpleExecutionEngine`/测试构造同样补上，用默认值。）

- [ ] **Step 2: 写集成失败测试**

在 `execution_engine/tests.rs` 加（复用其既有 harness 构造两个 session 同 agent 的 run）：

```rust
#[tokio::test]
async fn two_sessions_same_agent_run_in_parallel() {
    // 用同一 agent、两个不同 conversation 的 RunRequest。
    // 断言：第二个 run 不再返回 AgentBusy/Failed，两者都进入 Running。
    // （对照旧行为：per-agent 门会拒第二个。）
}

#[tokio::test]
async fn second_message_same_session_takes_busy_path() {
    // 同一 session 已有 run 时，第二条消息仍走 BusyInputMode（默认 Steer 注入）。
    // 断言：try_claim 对同 session 第二次返回 false → 进 steer 分支。
}
```

- [ ] **Step 3: 跑确认失败**

Run: `cargo test -p alephcore two_sessions_same_agent_run_in_parallel`
Expected: FAIL（当前 per-agent 门拒第二个 run）。

- [ ] **Step 4: 重写门谓词 + 加 RAII 守卫**

`gate.rs`（Task 2 抽出的门）：把 `if !agent.try_start_run(&run_id).await {` 改为

```rust
    if !self.session_run_registry.try_claim(&request.session_key, &run_id) {
        // 已有 run 在**本会话**上 → 走 per-session BusyInputMode（原三态分支不变，
        // 它们本就按 request.session_key 判定 steering target，见 execute.rs 原 :154-206）。
        // ...（Steer / Interrupt / Queue 分支逐字保留）...
    }
```

删除 `:210-231` 的死 count 检查整块。改为在门放行后获取 run-lifetime 许可并建守卫：

```rust
    // 门放行：本会话已 claim。取 run-lifetime 并发许可（满则等 = 准入队列）。
    // 先非阻塞试；拿不到则发一个类型化背压事件再 await（UI 显示"已排队"）。
    let agent_id = request.session_key.agent_id().to_string();
    let permit = match self.concurrency.try_acquire(&agent_id) {
        Some(p) => p,
        None => {
            // TODO(P2): emit typed RunQueued{position} to UI here. 本轮先只等。
            self.concurrency.acquire(&agent_id).await
        }
    };
    let _run_slot = RunSlot {
        registry: Arc::clone(&self.session_run_registry),
        session_key: request.session_key.clone(),
        run_id: run_id.clone(),
        _permit: permit,
    };
    // `_run_slot` 存活到 execute() 返回 → 释放会话 claim + 并发许可（含早退/panic）。
```

> ⚠️ 注：上面的 `TODO(P2)` **不是占位空实现**——它标注的是 spec §3.4 明确划归 P2 的"类型化背压事件到 UI"。本轮 P1 只保证 cap 满时**排队等待**（`acquire().await`），不 Fail（已修审计 1.2）；把"queued/pos N"事件推给前端属 P2 事件路由 SSOT。此处 await 即正确行为。

在 `execute.rs` 定义 `RunSlot`（或放 `session_run_registry.rs`）：

```rust
pub(super) struct RunSlot {
    pub(super) registry: Arc<super::session_run_registry::SessionRunRegistry>,
    pub(super) session_key: crate::routing::session_key::SessionKey,
    pub(super) run_id: String,
    pub(super) _permit: super::concurrency::RunPermit,
}
impl Drop for RunSlot {
    fn drop(&mut self) {
        self.registry.release(&self.session_key, &self.run_id);
    }
}
```

`_run_slot` 必须绑定在 `execute()` 主体作用域（不是门函数内），让它活到 run 结束。若门已抽成子函数，改为门函数**返回** `RunSlot`，由 `execute()` 持有。

- [ ] **Step 5: 退休 per-agent gate 释放点**

grep `set_state(AgentState::Idle)`、`set_idle`、`try_start_run` 在 `src/gateway/execution_engine/` 与 `agent_instance.rs`：
- 删除 run-completion 处把 agent 置 Idle 的"释放并发闸"调用（`RunSlot` Drop 现在负责释放）。
- `agent_instance.rs::try_start_run` 若无其它消费者则删除；`AgentState`/`set_state` 若仅剩生命周期**展示**用途（如 session_manager 状态显示）则保留但不再作并发闸。执行者 grep 确认消费者后决定删/留，**不留死代码**（R10 YAGNI）。

- [ ] **Step 6: 跑集成 + 定向单测**

Run: `cargo test -p alephcore two_sessions_same_agent_run_in_parallel second_message_same_session_takes_busy_path`
Expected: PASS。
然后本轮唯一一次全库编译检查：
Run: `cargo check --lib`
Expected: 干净（无 warning 阻断）。

- [ ] **Step 7: Commit**

```bash
git add src/gateway/execution_engine/ src/gateway/agent_instance.rs
git commit -m "gateway: per-session run gate replaces per-agent AgentState gate (audit 1.1/1.2)"
```

---

## Task 7（INV-ISO）: 隔离回归测试钉死

**Files:**
- Test: `src/gateway/execution_engine/tests.rs`（或新 `tests/multi_session_isolation.rs`）

**Interfaces:**
- Consumes: Task 6 后的并行 run 能力 + 记忆写入路径（`note_manage` / raw_memory）。
- Produces: 两条永久回归护栏，钉死 INV-ISO。

- [ ] **Step 1: 写隔离测试**

```rust
#[tokio::test]
async fn concurrent_runs_different_agents_do_not_cross_write_memory() {
    // 起两个并发 run：agent "A" 会话 与 agent "B" 会话，各写一条 note。
    // 断言：A 的 note 落在 note/A 分区、B 的落在 note/B 分区，零串写。
    // （验 INV-ISO：TURN_CONTEXT task-local + agent_id 物理分区在并发下不串。）
}

#[tokio::test]
async fn concurrent_runs_same_agent_do_not_interleave_transcript() {
    // 起两个并发 run：同 agent "A" 的会话 conv-1 与 conv-2。
    // 断言：conv-1 的 session_events seq 单调且只含 conv-1 的事件、conv-2 同理，
    //       两会话事件不交错；两条记忆写入都落 agent A 分区不丢不死锁。
}
```

- [ ] **Step 2: 跑确认（此时应通过——Task 6 已使能并行且隔离本就成立）**

Run: `cargo test -p alephcore concurrent_runs_different_agents concurrent_runs_same_agent`
Expected: PASS。若 FAIL＝Task 6 引入了隔离回归，立即回到 Task 6 修（这正是这两条测试的价值）。

- [ ] **Step 3: Commit**

```bash
git add src/gateway/execution_engine/tests.rs
git commit -m "test: INV-ISO regression — concurrent runs preserve agent memory isolation + non-interleaved transcripts"
```

---

## Task 8（P1）: 透出 "N/M 槽在用" 到 Panel metrics

**Files:**
- Modify: `src/gateway/handlers/gateway_metrics.rs`（执行者先 Read 全文 + `lane.rs:379-395` 的 lane 快照模式）
- Modify: engine 暴露 `concurrency.snapshot()` 的访问器

**Interfaces:**
- Consumes: Task 4 `ConcurrencyLimiter::snapshot() -> ConcurrencySnapshot`。
- Produces: `gateway.metrics` RPC 输出新增 `run_concurrency: { global_in_use, global_total }` 字段（R4 纯 I/O，向后兼容超集）。

- [ ] **Step 1: 加访问器 + 测试**

engine 加 `pub fn concurrency_snapshot(&self) -> ConcurrencySnapshot { self.concurrency.snapshot() }`。在 `gateway_metrics.rs` 按 `lane.rs` lane-snapshot 的同款模式，把 `run_concurrency` 字段并入 metrics 响应 JSON。写一个断言该字段存在且 `global_total == 8`（默认）的单测。

- [ ] **Step 2: 跑确认失败 → 实现 → 通过**

Run: `cargo test -p alephcore gateway_metrics`（红 → 实现 → 绿）。

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/gateway_metrics.rs src/gateway/execution_engine/
git commit -m "gateway: surface run_concurrency (N/M slots) in gateway.metrics (audit 3.4)"
```

---

## Self-Review（对照 spec）

**Spec coverage**：
- P0-a 拆 execute.rs → Task 2 ✓
- P0-b seq 自愈 → Task 1 ✓
- P1 会话互斥锁 → Task 3 ✓
- P1 并发上限持有到结束 → Task 4 + Task 6（RunSlot 守卫）✓
- P1 退休 per-agent 闸 → Task 6 ✓
- P1 busy 判定 per-session 化 → Task 6（门谓词换 session claim，busy 分支本已 session-aware）✓
- P1 cap 满排队非 Fail → Task 6（`acquire().await`）✓；类型化背压事件明列 P2（§3.4）
- 三层上限 config 连线 → Task 5 ✓
- INV-ISO 回归 → Task 7 ✓
- N/M 槽透出 → Task 8 ✓
- R10 全在 gateway → 所有 Task ✓

**已知边界（非占位）**：
- Task 6 的 `TODO(P2)` = spec §3.4 明划 P2 的类型化背压事件，非空实现。
- Task 5/8 的部分字段名（`ExecutionConfig` 结构体、`gateway_metrics` 响应形状）需执行者 Read 对应文件确认——已给精确锚点 + 参照模式（delivery_queue / lane snapshot），非"自行发挥"。
- Task 6 的 per-agent gate 释放点删除靠 grep 定位——已给精确 grep 词（`set_state(AgentState::Idle)`/`set_idle`/`try_start_run`）+ 删/留判据（有无其它消费者）。

**Type consistency**：`try_claim(&SessionKey, &str)->bool` / `release(&SessionKey,&str)` / `acquire(&str)->RunPermit` / `snapshot()->ConcurrencySnapshot` 在 Task 3/4 定义、Task 6/8 消费，签名一致。`RunSlot{registry,session_key,run_id,_permit}` Task 6 内自洽。

---

## Execution Handoff

计划覆盖 P0+P1；每个 Task 产出可独立测试的交付物。建议 **subagent-driven** 执行（每 Task 一个新 subagent + 任务间评审），Task 1→8 顺序（Task 1/2 可互换，均须先于 3+；Task 6 是核心闸合并点，唯一 `cargo check --lib` 处）。
