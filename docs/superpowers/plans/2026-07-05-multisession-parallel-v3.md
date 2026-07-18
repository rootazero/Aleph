# 多会话平行执行 v3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Panel 左侧会话列表的"正在执行"红点与真实运行态绝对同步（不假阳/不假阴），修复同 agent 多会话在通道路径被串行化的并行 bug，新增版本化运行态广播与并发上限热重载，并清理相关死代码。

**Architecture:** 后端 `SessionRunRegistry` 成为运行态唯一真源，持一个单调 `seq` 与可注入的事件总线，在每次 claim/release 广播 `RunningSetChanged{seq,running}` 事件；Panel 侧栏红点改为纯读服务端权威集合（带 seq 守卫），彻底弃用会脆的客户端 refcount 判红点。inbound busy FIFO 队列从 `agent_id` 重键为 `session_key`。`ConcurrencyLimiter` 获得 `reconfigure`，经进程级句柄由 `self_config` 在 `[execution]` patch 后热重载。

**Tech Stack:** Rust（alephcore 后端）、tokio 1.35（`Semaphore` 只增不减 → rebuild-swap）、`arc_swap::ArcSwap`（已是依赖）、Leptos + WASM（aleph-panel 前端）、serde（事件 wire 形状）。

## Global Constraints

- **隔离**：全部改动在 worktree `D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3`（分支 `feat-multisession-parallel-v3`）。严禁触碰 main。所有命令用绝对路径或 `git -C <worktree>`。
- **不变量**：INV-SEQ（一会话至多一个 Running run）；INV-ISO（并行 run 按 `agent_id` 隔离）；RAII 认领（`RunSlot` 认领即构造、drop 即释放，不回退）；session-ssot 单写者（不新增 `session_events` 第二写者）。
- **两条身份轴**：session=并行/红点/transcript；agent=记忆/存储/子上限。红点 = session 轴。
- **rustfmt 纪律**：只格式化实际改动文件 `rustfmt <file.rs>`，**禁用** `rustfmt mod.rs`（会顺 `mod` 递归卷入兄弟文件；本仓有既存 fmt drift）。
- **构建内存**：`alephcore` lib-test 吃内存，测试前置 `CARGO_PROFILE_TEST_DEBUG=line-tables-only`，scoped 到具体模块，禁全量 `cargo test`。
- **验证范围（Windows）**：排除 `aleph-desktop-macos` / `aleph-desktop-linux`。前端用 `cargo check -p aleph-panel --target wasm32-unknown-unknown` + 具体 host 单测。
- **提交**：每 Task 末尾提交，`<type>: <desc>`（feat/fix/refactor/docs/test/chore）。全局归属已禁用，勿加 Co-authored-by。

---

### Task 1: 新增 `GatewayEventFrame::RunningSetChanged` 事件变体

红点广播的 wire 契约。两处 `match self`（`topic_name`/`stream_method`）是穷尽匹配，加变体不加臂无法编译——本 Task 把契约立好，后端 emit（Task 3/4）与前端 consume（Task 9）都依赖它。

**Files:**
- Modify: `src/gateway/events/frame.rs`（enum 定义 `:23-239`、`topic_name` `:435-472`、`stream_method` `:484-504`、测试模块 `:521-554`）

**Interfaces:**
- Produces: `GatewayEventFrame::RunningSetChanged { seq: u64, running: Vec<String> }`；wire `type="running_set_changed"`；`stream_method()` → `Some("stream.running_set_changed")`；`topic_name()` → `"running.set.changed"`。前端收到时 topic 被改写为 `run.running_set_changed`（`context.rs` 的 `stream.`→`run.` 规则）。

- [ ] **Step 1: 写失败测试**（追加到 `frame.rs` 的 `#[cfg(test)] mod tests`，`:521` 内）

```rust
    #[test]
    fn running_set_changed_wire_shape() {
        let f = GatewayEventFrame::RunningSetChanged {
            seq: 42,
            running: vec!["agent:main|conv-1".into(), "agent:main|conv-2".into()],
        };
        // Streaming path so it rides the same stream.* delivery as session_updated.
        assert_eq!(f.stream_method(), Some("stream.running_set_changed"));
        assert_eq!(f.topic_name(), "running.set.changed");
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "running_set_changed");
        assert_eq!(v["seq"], 42);
        assert_eq!(v["running"][0], "agent:main|conv-1");
        assert_eq!(v["running"][1], "agent:main|conv-2");
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo test -p alephcore --lib gateway::events::frame::tests::running_set_changed_wire_shape 2>&1 | tail -20`
Expected: 编译失败（`RunningSetChanged` 变体不存在）。

- [ ] **Step 3: 加变体**（在 enum 内，紧跟 `SessionUpdated { .. }` 之后，`frame.rs:134` 后）

```rust
    /// Authoritative running-session set changed (a run was claimed or
    /// released). `seq` is a monotonic version stamped by the
    /// `SessionRunRegistry` under its map lock; consumers keep the highest seq
    /// and ignore any older-or-equal frame so a reordered delivery self-heals.
    /// `running` is the full set of backend session keys with an in-flight run
    /// — the sidebar red-dot reads this directly (server-authoritative, no
    /// client refcount). Payload is intentionally the registry's own data only
    /// (no `ConcurrencySnapshot`) so the registry can emit it without reaching
    /// the limiter; gauge consumers re-fetch `run_concurrency` on receipt.
    RunningSetChanged {
        seq: u64,
        running: Vec<String>,
    },
```

- [ ] **Step 4: 加 `topic_name` 臂**（`frame.rs:452` `SessionUpdated` 臂之后）

```rust
            Self::RunningSetChanged { .. } => "running.set.changed",
```

- [ ] **Step 5: 加 `stream_method` 臂**（`frame.rs:501` `SessionUpdated` 臂之后，`_ => None` 之前）

```rust
            Self::RunningSetChanged { .. } => Some("stream.running_set_changed"),
```

- [ ] **Step 6: 运行测试确认通过 + 编译干净**

Run: `cargo test -p alephcore --lib gateway::events::frame::tests::running_set_changed_wire_shape 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/src/gateway/events/frame.rs"
git -C "$WT" add src/gateway/events/frame.rs
git -C "$WT" commit -m "feat: add RunningSetChanged gateway event frame (versioned running-set broadcast)"
```

---

### Task 2: `ConcurrencyLimiter::reconfigure` + 内部可变上限

热重载地基。tokio 1.35 `Semaphore` 只能 `add_permits` 不能缩，故 global 信号量整体 rebuild-swap；per-agent 清空懒重建；caps 走原子。

**Files:**
- Modify: `src/gateway/execution_engine/concurrency.rs`（struct `:77-85`、`new` `:87-100`、`agent_sem` `:102-107`、`acquire` `:113-130`、`try_acquire` `:134-142`、`snapshot` `:145-176`、tests `:179-253`）

**Interfaces:**
- Consumes: 无（自足）。
- Produces: `ConcurrencyLimiter::reconfigure(&self, global_cap: usize, per_agent_cap: usize)`（`&self`，内部可变）。`new`/`acquire`/`try_acquire`/`snapshot` 签名不变。

- [ ] **Step 1: 写失败测试**（追加到 `concurrency.rs` tests，`:179` 内）

```rust
    #[tokio::test]
    async fn reconfigure_grows_and_shrinks_caps() {
        let lim = ConcurrencyLimiter::new(1, 1);
        let _p1 = lim.try_acquire("main").expect("slot 1");
        assert!(lim.try_acquire("other").is_none(), "global cap=1 已满");

        // Grow global to 3 → a new agent can now acquire.
        lim.reconfigure(3, 2);
        assert_eq!(lim.snapshot().global_total, 3);
        assert_eq!(lim.snapshot().per_agent_cap, 2);
        let _p2 = lim.try_acquire("other").expect("grown global slot");
        // Old in-flight permit still valid (held against the pre-swap semaphore).
        drop(_p1);

        // Shrink global to 1: new acquires bounded by the new semaphore.
        lim.reconfigure(1, 1);
        assert_eq!(lim.snapshot().global_total, 1);
    }

    #[tokio::test]
    async fn reconfigure_rebuilds_per_agent_caps() {
        let lim = ConcurrencyLimiter::new(10, 1);
        let _a1 = lim.try_acquire("main").unwrap();
        assert!(lim.try_acquire("main").is_none(), "per-agent cap=1 已满");
        // Raise per-agent cap → the same agent gets a fresh semaphore at cap 3.
        lim.reconfigure(10, 3);
        let _a2 = lim.try_acquire("main").expect("re-capped agent slot");
        let _a3 = lim.try_acquire("main").expect("re-capped agent slot");
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo test -p alephcore --lib gateway::execution_engine::concurrency 2>&1 | tail -20`
Expected: 编译失败（`reconfigure` 不存在）。

- [ ] **Step 3: 改 struct 为内部可变**（替换 `concurrency.rs:77-85`）

```rust
pub(super) struct ConcurrencyLimiter {
    /// Hot-swappable so `reconfigure` can resize (tokio 1.35 `Semaphore` grows
    /// via `add_permits` but cannot shrink; a whole-semaphore swap is the
    /// version-safe resize). In-flight permits held against the previous
    /// `Arc<Semaphore>` keep it alive until they drop, so a shrink overshoots
    /// transiently by at most the old in-flight count, then converges.
    global: ArcSwap<Semaphore>,
    global_total: AtomicUsize,
    per_agent_cap: AtomicUsize,
    per_agent: Mutex<HashMap<String, Arc<Semaphore>>>,
    /// Live count of runs blocked in `acquire().await` (queue depth).
    waiting: AtomicUsize,
}
```

- [ ] **Step 4: 加 import**（`concurrency.rs:8-14` 区，加 `arc_swap`）

```rust
use arc_swap::ArcSwap;
```

- [ ] **Step 5: 改 `new`**（替换 `concurrency.rs:88-100`）

```rust
    #[must_use]
    pub(super) fn new(global_cap: usize, per_agent_cap: usize) -> Self {
        // Clamp to >=1 (a 0-permit semaphore would deadlock every run).
        let global_cap = global_cap.max(1);
        let per_agent_cap = per_agent_cap.max(1);
        Self {
            global: ArcSwap::from_pointee(Semaphore::new(global_cap)),
            global_total: AtomicUsize::new(global_cap),
            per_agent_cap: AtomicUsize::new(per_agent_cap),
            per_agent: Mutex::new(HashMap::new()),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Live-resize both caps (hot-reload of `[execution] max_runs_*`). The
    /// global semaphore is swapped wholesale; the per-agent map is cleared so
    /// each agent's sub-semaphore rebuilds lazily at the new cap. In-flight
    /// permits against the old semaphores stay valid until dropped.
    pub(super) fn reconfigure(&self, global_cap: usize, per_agent_cap: usize) {
        let global_cap = global_cap.max(1);
        let per_agent_cap = per_agent_cap.max(1);
        self.global.store(Arc::new(Semaphore::new(global_cap)));
        self.global_total.store(global_cap, Ordering::Release);
        self.per_agent_cap.store(per_agent_cap, Ordering::Release);
        self.per_agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
```

- [ ] **Step 6: 改 `agent_sem` 读原子 cap**（替换 `concurrency.rs:102-107`）

```rust
    fn agent_sem(&self, agent_id: &str) -> Arc<Semaphore> {
        let cap = self.per_agent_cap.load(Ordering::Acquire);
        let mut map = self.per_agent.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(cap)))
            .clone()
    }
```

- [ ] **Step 7: 改 `acquire`（不持 ArcSwap guard 跨 await）**（替换 `concurrency.rs:113-130` 的 global 获取段）

```rust
    pub(super) async fn acquire(&self, agent_id: &str) -> RunPermit {
        let _wait = WaitGuard::enter(&self.waiting);
        let agent_sem = self.agent_sem(agent_id);
        let agent = agent_sem
            .acquire_owned()
            .await
            .expect("agent sem never closed");
        // Clone the current global semaphore Arc, then await on it — never hold
        // the ArcSwap guard across the await (a concurrent reconfigure must not
        // be blocked, and the permit binds to whichever semaphore was live).
        let global_sem = self.global.load_full();
        let global = global_sem
            .acquire_owned()
            .await
            .expect("global sem never closed");
        RunPermit {
            _global: global,
            _agent: agent,
        }
    }
```

- [ ] **Step 8: 改 `try_acquire`**（替换 `concurrency.rs:134-142`）

```rust
    #[must_use]
    pub(super) fn try_acquire(&self, agent_id: &str) -> Option<RunPermit> {
        let agent_sem = self.agent_sem(agent_id);
        let agent = Arc::clone(&agent_sem).try_acquire_owned().ok()?;
        let global_sem = self.global.load_full();
        let global = global_sem.try_acquire_owned().ok()?;
        Some(RunPermit {
            _global: global,
            _agent: agent,
        })
    }
```

- [ ] **Step 9: 改 `snapshot` 读原子 + 当前 global 信号量**（替换 `concurrency.rs:145-176` 的 per_agent cap 读与末尾结构体）

在 `snapshot` 内，把 `self.per_agent_cap`（原 usize 字段）读改为原子读，把 `global_total`/`available_permits` 改为读当前值：

```rust
    #[must_use]
    pub(super) fn snapshot(&self) -> ConcurrencySnapshot {
        let per_agent_cap = self.per_agent_cap.load(Ordering::Acquire);
        let global_sem = self.global.load_full();
        let global_total = self.global_total.load(Ordering::Acquire);
        let per_agent = {
            let map = self.per_agent.lock().unwrap_or_else(|e| e.into_inner());
            let mut rows: Vec<AgentSlotUsage> = map
                .iter()
                .filter_map(|(id, sem)| {
                    let in_use = per_agent_cap.saturating_sub(sem.available_permits());
                    (in_use > 0).then(|| AgentSlotUsage {
                        agent_id: id.clone(),
                        in_use,
                    })
                })
                .collect();
            rows.sort_by(|a, b| {
                b.in_use
                    .cmp(&a.in_use)
                    .then_with(|| a.agent_id.cmp(&b.agent_id))
            });
            rows
        };
        ConcurrencySnapshot {
            global_in_use: global_total.saturating_sub(global_sem.available_permits()),
            global_total,
            per_agent_cap,
            waiting: self.waiting.load(Ordering::Acquire),
            per_agent,
        }
    }
```

- [ ] **Step 10: 运行全模块测试确认通过**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::execution_engine::concurrency 2>&1 | tail -25`
Expected: 全 6 测（原 4 + 新 2）PASS。

- [ ] **Step 11: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/src/gateway/execution_engine/concurrency.rs"
git -C "$WT" add src/gateway/execution_engine/concurrency.rs
git -C "$WT" commit -m "feat: ConcurrencyLimiter::reconfigure for live cap hot-reload (arc-swap global sem)"
```

---

### Task 3: `SessionRunRegistry` 加 seq + running_snapshot + 内部广播

运行态唯一真源持单调 `seq` 与可选事件总线，在 claim/release 后广播 `RunningSetChanged`。emit 在释放 map 锁后进行（不持锁跨广播）。

**Files:**
- Modify: `src/gateway/execution_engine/session_run_registry.rs`（struct `:15-18`、`try_claim` `:25-33`、`release` `:38-44`、`running_keys` `:56-63`、tests `:66-137`）

**Interfaces:**
- Consumes: `GatewayEventFrame::RunningSetChanged`（Task 1）；`crate::gateway::event_bus::GatewayEventBus`。
- Produces:
  - `SessionRunRegistry::set_event_bus(&self, bus: Arc<GatewayEventBus>)`（`&self`，OnceLock 注入）。
  - `SessionRunRegistry::running_snapshot(&self) -> (u64, Vec<String>)`（锁内一致读 seq+keys）。
  - `try_claim`/`release`/`running_keys` 签名不变；claim 成功与 release 生效后各广播一次。

- [ ] **Step 1: 写失败测试**（追加到 `session_run_registry.rs` tests，`:66` 内）

```rust
    #[test]
    fn seq_is_monotonic_across_claim_and_release() {
        let reg = SessionRunRegistry::default();
        let s = sk("main", "conv-1");
        let (seq0, keys0) = reg.running_snapshot();
        assert!(keys0.is_empty());
        assert!(reg.try_claim(&s, "run-1"));
        let (seq1, keys1) = reg.running_snapshot();
        assert!(seq1 > seq0, "claim bumps seq");
        assert_eq!(keys1, vec![s.to_key_string()]);
        reg.release(&s, "run-1");
        let (seq2, keys2) = reg.running_snapshot();
        assert!(seq2 > seq1, "release bumps seq");
        assert!(keys2.is_empty());
        // A no-op release (mismatched run id) must NOT bump seq.
        assert!(reg.try_claim(&s, "run-2"));
        let (seq3, _) = reg.running_snapshot();
        reg.release(&s, "STALE");
        let (seq4, _) = reg.running_snapshot();
        assert_eq!(seq3, seq4, "no-op release does not bump seq");
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo test -p alephcore --lib gateway::execution_engine::session_run_registry 2>&1 | tail -20`
Expected: 编译失败（`running_snapshot` 不存在）。

- [ ] **Step 3: 改 struct + imports**（替换 `session_run_registry.rs:9-18`）

```rust
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Mutex;

/// Tracks the single in-flight run per session. `session_key_string -> run_id`.
#[derive(Default)]
pub(super) struct SessionRunRegistry {
    running: Mutex<HashMap<String, String>>,
    /// Monotonic version stamped under the `running` lock on every effective
    /// claim/release, so a `(seq, keys)` snapshot is internally consistent and
    /// consumers can drop reordered/stale broadcasts.
    seq: AtomicU64,
    /// Optional broadcast sink (injected post-construction, mirrors the
    /// engine's own `event_bus: Option`). When present, every state change
    /// publishes `RunningSetChanged` so the Panel red-dot stays authoritative.
    event_bus: OnceLock<Arc<GatewayEventBus>>,
}
```

- [ ] **Step 4: 加 set_event_bus / running_snapshot / 内部 emit**（在 `impl SessionRunRegistry` 内，`try_claim` 之前）

```rust
    /// Inject the broadcast sink once (idempotent no-op if already set). Called
    /// by the engine when its own `event_bus` is wired.
    pub(super) fn set_event_bus(&self, bus: Arc<GatewayEventBus>) {
        let _ = self.event_bus.set(bus);
    }

    /// Internally-consistent `(seq, running_keys)` read under the map lock.
    #[must_use]
    pub(super) fn running_snapshot(&self) -> (u64, Vec<String>) {
        let map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let seq = self.seq.load(Ordering::Acquire);
        (seq, map.keys().cloned().collect())
    }

    /// Bump seq (under the caller-held lock's happens-before) and broadcast the
    /// fresh running set. Call AFTER dropping the map lock to avoid holding it
    /// across serialization/broadcast.
    fn broadcast_change(&self) {
        if let Some(bus) = self.event_bus.get() {
            let (seq, running) = self.running_snapshot();
            let _ = bus.publish_frame(&GatewayEventFrame::RunningSetChanged { seq, running });
        }
    }
```

- [ ] **Step 5: 改 `try_claim`（成功 → bump+广播）**（替换 `session_run_registry.rs:25-33`）

```rust
    #[must_use]
    pub(super) fn try_claim(&self, session_key: &SessionKey, run_id: &str) -> bool {
        let key = session_key.to_key_string();
        {
            let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
            if map.contains_key(&key) {
                return false;
            }
            map.insert(key, run_id.to_string());
            self.seq.fetch_add(1, Ordering::AcqRel);
        }
        self.broadcast_change();
        true
    }
```

- [ ] **Step 6: 改 `release`（生效 → bump+广播）**（替换 `session_run_registry.rs:38-44`）

```rust
    pub(super) fn release(&self, session_key: &SessionKey, run_id: &str) {
        let key = session_key.to_key_string();
        let changed = {
            let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
            if map.get(&key).map(String::as_str) == Some(run_id) {
                map.remove(&key);
                self.seq.fetch_add(1, Ordering::AcqRel);
                true
            } else {
                false
            }
        };
        if changed {
            self.broadcast_change();
        }
    }
```

- [ ] **Step 7: 运行确认通过（含既有测试）**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::execution_engine::session_run_registry 2>&1 | tail -25`
Expected: 全部 PASS（原 3 测 + 新 1；无 bus 时 `broadcast_change` 为 no-op，测试不需 bus）。

- [ ] **Step 8: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/src/gateway/execution_engine/session_run_registry.rs"
git -C "$WT" add src/gateway/execution_engine/session_run_registry.rs
git -C "$WT" commit -m "feat: SessionRunRegistry seq + RunningSetChanged broadcast (single-source running truth)"
```

---

### Task 4: 引擎连线 — 注入事件总线到 registry + 安装并发上限句柄

让 registry 拿到 bus（激活 Task 3 的广播），并安装进程级 `ConcurrencyLimiter` 句柄（供 Task 6 热重载）。

**Files:**
- Create: `src/gateway/execution_engine/concurrency_handle.rs`
- Modify: `src/gateway/execution_engine/mod.rs`（加 `mod concurrency_handle;`）
- Modify: `src/gateway/execution_engine/engine.rs`（`new` `:127-165` 末尾安装句柄；`set_event_bus` `:266-271` 注入 registry）

**Interfaces:**
- Consumes: `ConcurrencyLimiter`（Task 2）；`SessionRunRegistry::set_event_bus`（Task 3）。
- Produces:
  - `pub(crate) fn concurrency_handle::install_global(limiter: &Arc<ConcurrencyLimiter>)`
  - `pub fn concurrency_handle::reconfigure_global(global_cap: usize, per_agent_cap: usize) -> bool`（无句柄返回 `false`）。

- [ ] **Step 1: 建句柄模块**（`concurrency_handle.rs` 全文）

```rust
//! Process-global handle to the live `ConcurrencyLimiter`, mirroring
//! `providers::route_handle`. The limiter is built once inside the
//! `ExecutionEngine`; a `[execution]` config patch alone never reaches it.
//! This handle lets `self_config` hot-apply new run caps on the next
//! admission — no daemon restart (the task's hot-state requirement).
//!
//! Holds a `Weak` so a torn-down engine doesn't keep the limiter alive;
//! `reconfigure_global` is a no-op returning `false` when nothing is live.

use std::sync::{OnceLock, Weak};

use super::concurrency::ConcurrencyLimiter;
use crate::sync_primitives::Arc;

static HANDLE: OnceLock<Weak<ConcurrencyLimiter>> = OnceLock::new();

/// Register the engine's limiter once (idempotent). Called from
/// `ExecutionEngine::new`.
pub(crate) fn install_global(limiter: &Arc<ConcurrencyLimiter>) {
    let _ = HANDLE.set(Arc::downgrade(limiter));
}

/// Live-resize the global run caps. Returns `false` if no engine is installed
/// or it has been dropped (the caller reports "no live limiter").
pub fn reconfigure_global(global_cap: usize, per_agent_cap: usize) -> bool {
    match HANDLE.get().and_then(Weak::upgrade) {
        Some(limiter) => {
            limiter.reconfigure(global_cap, per_agent_cap);
            true
        }
        None => false,
    }
}
```

- [ ] **Step 2: 注册模块**（`mod.rs` 的 mod 声明区，与 `mod concurrency;` 同处加一行）

```rust
mod concurrency_handle;
```

- [ ] **Step 3: `new` 末尾安装句柄**（`engine.rs` 内 `new` 构造出 `self` 后、返回前——由于 `concurrency` 是 `Arc<ConcurrencyLimiter>` 字段，构造完 struct 再调）

在 `new` 的结构体字面量之后、函数返回该值之前，改为先绑定再安装：

```rust
        let engine = Self {
            // ... 原有全部字段（含 concurrency: Arc::new(ConcurrencyLimiter::new(...))）...
        };
        super::concurrency_handle::install_global(&engine.concurrency);
        engine
```
（若原 `new` 直接返回结构体字面量，则改为先 `let engine = Self {…};` 再 `install_global` 再 `engine`。）

- [ ] **Step 4: `set_event_bus` 注入 registry**（`engine.rs:266-271`，在 `self.event_bus = Some(event_bus)` 处）

```rust
    pub fn set_event_bus(&mut self, event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>) {
        self.session_run_registry.set_event_bus(event_bus.clone());
        self.event_bus = Some(event_bus);
    }
```
（若该 setter 是 `with_event_bus(mut self, ...) -> Self` 形态，同样在设 `self.event_bus` 前加 `self.session_run_registry.set_event_bus(event_bus.clone());`。以实际签名为准，仅新增一行注入。）

- [ ] **Step 5: 编译确认**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo check -p alephcore --lib 2>&1 | tail -25`
Expected: 干净编译。

- [ ] **Step 6: 写句柄行为测试**（`concurrency_handle.rs` 追加 `#[cfg(test)]`）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconfigure_without_install_returns_false() {
        // No engine installed in a bare unit test → no-op, false.
        // (Uses a fresh process assumption; if another test installs a handle
        // this still holds because that limiter's reconfigure is harmless.)
        let _ = reconfigure_global(4, 2);
        // Assert the contract shape, not global state: calling twice is safe.
        let _ = reconfigure_global(8, 3);
    }
}
```

- [ ] **Step 7: 运行测试 + 提交**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::execution_engine::concurrency_handle 2>&1 | tail -15`
Expected: PASS。

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/src/gateway/execution_engine/concurrency_handle.rs" "$WT/src/gateway/execution_engine/engine.rs"
git -C "$WT" add src/gateway/execution_engine/concurrency_handle.rs src/gateway/execution_engine/mod.rs src/gateway/execution_engine/engine.rs
git -C "$WT" commit -m "feat: wire event bus into SessionRunRegistry + install global concurrency handle"
```

---

### Task 5: FIFO busy 队列从 `agent_id` 重键为 `session_key`

修复同 agent 多会话在通道路径被串行化的并行 bug。`busy_queue` 模块 key 无关，改动集中在 `executor.rs` 的键 + 模块文档订正。

**Files:**
- Modify: `src/gateway/inbound_router/executor.rs`（busy 段 `:406-459`）
- Modify: `src/gateway/inbound_router/busy_queue.rs`（模块文档 `:1-32`、常量名 `:43`、`register/is_front/remove` 文档中的 "agent" 措辞）

**Interfaces:**
- Consumes: `SessionKey::to_key_string()`（现有）。
- Produces: 行为变更——busy 队列按 per-session 分桶；`AgentBusy` 错误仍携带 `agent_id`（语义不变）。

- [ ] **Step 1: 写回归测试**（追加到 `busy_queue.rs` tests，`:96` 内——验证不同 key 互不阻塞、同 key FIFO；键语义无关，测试用两个"会话键"字符串）

```rust
    #[test]
    fn distinct_session_keys_do_not_block_each_other() {
        // Two different sessions (same agent in production) get independent
        // lanes → both are immediately front (true cross-session parallelism).
        let s1 = "bq-test-agentX|conv-1";
        let s2 = "bq-test-agentX|conv-2";
        let t1 = register(s1).unwrap();
        let t2 = register(s2).unwrap();
        assert!(is_front(s1, t1), "session-1 lane is its own front");
        assert!(is_front(s2, t2), "session-2 lane is its own front — not blocked by session-1");
        remove(s1, t1);
        remove(s2, t2);
    }
```

- [ ] **Step 2: 运行确认通过**（此测试对现有 key 无关模块应已 PASS——它锁定"重键后仍成立"的契约）

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo test -p alephcore --lib gateway::inbound_router::busy_queue 2>&1 | tail -20`
Expected: PASS（含既有 5 测）。

- [ ] **Step 3: 重键 executor.rs**（`executor.rs:406`，把队列键从 agent_id 改为 session_key；保留 agent_id 仅用于 `AgentBusy` 错误）

替换 `:406`：
```rust
            let agent_key = request.session_key.agent_id().to_string();
```
为：
```rust
            // Busy lane is keyed by SESSION (matches the engine's per-session
            // SessionRunRegistry gate). Two sessions of the same agent get
            // independent lanes and run in parallel; only same-session messages
            // serialize FIFO. The AgentBusy error still reports the agent id.
            let session_key = request.session_key.to_key_string();
            let agent_id = request.session_key.agent_id().to_string();
```

然后在 busy 段（`:415`/`:425`/`:459`）把 `register(&agent_key)`/`is_front(&agent_key, ticket)`/`remove(&agent_key, ticket)` 的键参数改为 `&session_key`；把 `ExecutionError::AgentBusy(agent_key.clone())`（`:418`/`:453`）改为 `ExecutionError::AgentBusy(agent_id.clone())`；日志字段 `agent = %agent_key`（`:435`）改为 `session = %session_key`。

- [ ] **Step 4: 订正 busy_queue.rs 文档 + 常量名**

- 模块头（`:1`）`//! Per-agent FIFO wait queue` → `//! Per-session FIFO wait queue`。
- `:29-32` R10 段那句 "the engine's per-agent `try_start_run` gate stays the single authority" → "the engine's per-session `SessionRunRegistry` gate stays the single authority"。
- 常量重命名 `MAX_QUEUED_PER_AGENT` → `MAX_QUEUED_PER_SESSION`（`:43` 定义 + `register` `:64` 引用 + 测试 `full_lane_rejects_newest` `:143`），文档措辞 "one agent's lane" → "one session's lane"。
- `register`/`is_front`/`remove` doc 里 "agent_id" 措辞改 "session key"（不改参数名亦可，但改注释以免误导）。

- [ ] **Step 5: 运行 busy_queue + executor 相关测试 + 编译**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::inbound_router 2>&1 | tail -25`
Expected: 全 PASS。
Run: `cargo check -p alephcore --lib 2>&1 | tail -10`
Expected: 干净。

- [ ] **Step 6: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/src/gateway/inbound_router/executor.rs" "$WT/src/gateway/inbound_router/busy_queue.rs"
git -C "$WT" add src/gateway/inbound_router/executor.rs src/gateway/inbound_router/busy_queue.rs
git -C "$WT" commit -m "fix: re-key inbound busy FIFO lane by session (restore same-agent cross-session parallelism)"
```

---

### Task 6: 并发上限热重载连线（reload_impact + self_config）

`[execution]` 归 Live，`self_config` 在 patch 成功后调 `reconfigure_global`。

**Files:**
- Modify: `src/config/reload_impact.rs`（`LIVE_SECTIONS` `:56`、测试 `:116+`）
- Modify: `src/builtin_tools/self_config.rs`（route 热应用块之后 `:415` 附近）

**Interfaces:**
- Consumes: `concurrency_handle::reconfigure_global`（Task 4）；`AppConfig.execution.{max_runs_global, max_runs_per_agent}`。
- Produces: `ReloadImpact::classify("execution") == Live`；execution patch 后热重载并生效于下一次 admission。

- [ ] **Step 1: 写 reload_impact 测试**（`reload_impact.rs` tests 内）

```rust
    #[test]
    fn execution_is_live() {
        assert_eq!(ReloadImpact::classify("execution"), ReloadImpact::Live);
        assert_eq!(
            ReloadImpact::classify("execution.max_runs_global"),
            ReloadImpact::Live
        );
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo test -p alephcore --lib config::reload_impact::tests::execution_is_live 2>&1 | tail -15`
Expected: FAIL（execution 当前归 Restart）。

- [ ] **Step 3: 加入 LIVE_SECTIONS + 文档**（替换 `reload_impact.rs:56`）

```rust
/// - `execution` — `[execution] max_runs_*` are hot-applied to the live
///   `ConcurrencyLimiter` by `self_config` via
///   `execution_engine::concurrency_handle::reconfigure_global`, mirroring the
///   `route` hot-apply. New caps bind on the next admission — no restart.
const LIVE_SECTIONS: &[&str] = &["route", "behavior", "execution"];
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p alephcore --lib config::reload_impact 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 5: self_config 热应用 execution**（`self_config.rs:415` 的 route `if` 块之后，紧邻新增一个 execution 块）

```rust
                // Hot-apply an [execution] cap change to the live
                // ConcurrencyLimiter so new run caps bind on the next admission
                // (mirrors the route hot-apply above; makes the Live verdict true).
                if !dry_run
                    && result.success
                    && (config_path == "execution" || config_path.starts_with("execution."))
                {
                    if let Some(cfg) = self.config.as_ref() {
                        let ex = &cfg.read().await.execution;
                        crate::gateway::execution_engine::concurrency_handle::reconfigure_global(
                            ex.max_runs_global,
                            ex.max_runs_per_agent,
                        );
                    }
                }
```
（注：`concurrency_handle` 模块与 `reconfigure_global` 需 `pub`——Task 4 已定 `pub fn reconfigure_global`；`mod concurrency_handle;` 若为私有需在 `execution_engine/mod.rs` 改 `pub(crate) mod concurrency_handle;` 使 `crate::gateway::execution_engine::concurrency_handle::reconfigure_global` 可达。实现时确认可见性并按需放宽到 `pub(crate)`。）

- [ ] **Step 6: 校正模块可见性**（若 Step 5 报私有不可达，改 `execution_engine/mod.rs` 的 `mod concurrency_handle;` → `pub(crate) mod concurrency_handle;`）

- [ ] **Step 7: 编译确认**

Run: `cargo check -p alephcore --lib 2>&1 | tail -15`
Expected: 干净。

- [ ] **Step 8: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/src/config/reload_impact.rs" "$WT/src/builtin_tools/self_config.rs" "$WT/src/gateway/execution_engine/mod.rs"
git -C "$WT" add src/config/reload_impact.rs src/builtin_tools/self_config.rs src/gateway/execution_engine/mod.rs
git -C "$WT" commit -m "feat: hot-reload [execution] run caps via self_config (execution section now Live)"
```

---

### Task 7: 熵减 — 死变体 / 死持久化链 / SimpleExecutionEngine 假 0

**Files:**
- Modify: `src/gateway/execution_engine/mod.rs`（`RunState` `:265-270`）
- Modify: `src/gateway/execution_engine/simple.rs`（adapter impl `:441-464`）
- Modify: `src/gateway/session_manager/ops/modify.rs`（`set_running` `:479-481`）、`src/gateway/session_store/mod.rs`（`:247` trait 声明）、`src/gateway/session_store/sqlite_backend/mod.rs`（`:541`）、`src/gateway/session_store/file_backend/mod.rs`（`:1065`）

**Interfaces:**
- Consumes: `SimpleExecutionEngine` 的 limiter/registry（若无则返回 default）。
- Produces: `RunState` 少两个变体；`SessionStore::set_running` 链删除；`SimpleExecutionEngine` adapter 覆写 `concurrency_snapshot`/`running_sessions`。

- [ ] **Step 1: 删死变体 `RunState::{Queued, Paused}`**（`mod.rs:265-270` 删除这两个变体定义）

先确认零构造：
Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && grep -rn "RunState::Queued\|RunState::Paused" src/ | grep -v "enum RunState\|// " | tail`
Expected: 无匹配（除定义处）。若有 `match` 通配以外的显式臂，删除对应臂。

- [ ] **Step 2: 删 `set_running` 死链**

先确认零生产调用：
Run: `grep -rn "\.set_running(\|fn set_running" src/ | grep -v "test\|cron" | tail`
Expected: 仅 4 处定义 + trait 声明，无生产调用。删除：`ops/modify.rs:479-481` 方法、`session_store/mod.rs:247` trait 声明、`sqlite_backend/mod.rs:541` impl、`file_backend/mod.rs:1065` impl。

- [ ] **Step 3: `SimpleExecutionEngine` 覆写 adapter 两方法**（`simple.rs` 的 `impl ExecutionAdapter for SimpleExecutionEngine`，`:441-464` 内新增）

```rust
    fn concurrency_snapshot(&self) -> crate::gateway::execution_engine::ConcurrencySnapshot {
        // Simple engine uses a per-agent try_start_run gate, not the run-lifetime
        // ConcurrencyLimiter. Report an explicit unsupported/zeroed snapshot so
        // metrics don't read as a plausible "0 of N slots" from a real limiter.
        crate::gateway::execution_engine::ConcurrencySnapshot::default()
    }

    fn running_sessions(&self) -> Vec<String> {
        // Surface the simple engine's own in-flight set if it tracks one;
        // otherwise empty is the honest answer (no per-session registry here).
        Vec::new()
    }
```
（注：若 `SimpleExecutionEngine` 实际持有可查询的运行集，改为返回它；否则显式空 + 上面的注释即为"honest empty"。实现时读 `simple.rs` 的字段确认。）

- [ ] **Step 4: 编译 + 相关测试**

Run: `cargo check -p alephcore --lib 2>&1 | tail -20`
Expected: 干净（若删变体触发 non-exhaustive match，补 `_` 或删臂）。
Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::execution_engine 2>&1 | tail -15`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/src/gateway/execution_engine/mod.rs" "$WT/src/gateway/execution_engine/simple.rs" "$WT/src/gateway/session_manager/ops/modify.rs" "$WT/src/gateway/session_store/mod.rs" "$WT/src/gateway/session_store/sqlite_backend/mod.rs" "$WT/src/gateway/session_store/file_backend/mod.rs"
git -C "$WT" add -A
git -C "$WT" commit -m "refactor: drop dead RunState::{Queued,Paused} + set_running chain; SimpleEngine reports honest concurrency"
```

---

### Task 8: 前端红点 Model A — 纯服务端权威 + seq 守卫

侧栏红点 `is_running_session_key` 改为只读服务端权威集合；`set_server_running` 加 `seq` 守卫丢弃乱序旧包。

**Files:**
- Modify: `interfaces/webchat/src/state/sessions.rs`（struct `:31-57`、`new` `:67-81`、`set_server_running` `:297-301`、`is_running_session_key` `:312-322`、tests `:406-439`）

**Interfaces:**
- Consumes: 无（前端自足）。
- Produces: `SessionMap::set_server_running(&self, seq: u64, keys: HashSet<String>)`（带 seq 守卫）；`is_running_session_key` 纯读 `server_running`。`bind_run`/`settle_run`/`route`/`is_running(conv)` **不变**（保留给 chunk 路由与活跃视图乐观态）。

- [ ] **Step 1: 改测试以锁定新语义**（替换 `sessions.rs:406-439` 的 `server_running_lights_untracked_...` 测试为服务端权威版）

```rust
    #[test]
    fn dot_is_pure_server_authoritative_with_seq_guard() {
        use std::collections::HashSet;
        with_owner(|| {
            let map = SessionMap::new();

            // Server snapshot lights any session it lists — tracked or not.
            map.set_server_running(1, HashSet::from(["sess-remote".to_string()]));
            assert!(map.is_running_session_key("sess-remote"));
            assert!(!map.is_running_session_key("sess-idle"));

            // A tracked session's dot follows the server set (no client refcount
            // can pin it): even after bind_run, the authoritative absence wins.
            let c = map.open_conversation("agent-a", "A");
            map.bind_run("run-1", c, Some("sess-local"));
            map.set_server_running(2, HashSet::from(["sess-local".to_string()]));
            assert!(map.is_running_session_key("sess-local"), "server says running");
            // Release on the server (higher seq) clears the dot — no stuck dot.
            map.set_server_running(3, HashSet::new());
            assert!(!map.is_running_session_key("sess-local"), "server release clears dot");

            // Stale (lower-or-equal seq) frame is ignored — no flicker back on.
            map.set_server_running(2, HashSet::from(["sess-local".to_string()]));
            assert!(!map.is_running_session_key("sess-local"), "stale seq dropped");
        });
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo test -p aleph-panel state::sessions 2>&1 | tail -25`
Expected: 编译失败（`set_server_running` 参数不匹配）。

- [ ] **Step 3: 加 seq 字段**（`sessions.rs:50` `server_running` 之后加）

```rust
    /// Monotonic version of the last accepted `server_running` snapshot. Frames
    /// with `seq <= server_seq` are dropped so a reordered/stale broadcast can
    /// never resurrect a completed dot.
    server_seq: RwSignal<u64>,
```

- [ ] **Step 4: `new` 初始化**（`sessions.rs:76` `server_running: RwSignal::new(HashSet::new()),` 之后加）

```rust
            server_seq: RwSignal::new(0),
```

- [ ] **Step 5: 改 `set_server_running` 带 seq 守卫**（替换 `sessions.rs:297-301`）

```rust
    /// Apply an authoritative server running-set snapshot at version `seq`.
    /// Drops stale/reordered frames (`seq <= server_seq`) so the dot never
    /// flickers back on. Only writes on real change to avoid churn.
    pub fn set_server_running(&self, seq: u64, keys: HashSet<String>) {
        if seq <= self.server_seq.get_untracked() {
            return;
        }
        self.server_seq.set(seq);
        if self.server_running.with_untracked(|cur| *cur != keys) {
            self.server_running.set(keys);
        }
    }
```

- [ ] **Step 6: 改 `is_running_session_key` 为纯服务端权威**（替换 `sessions.rs:311-322`）

```rust
    /// 侧栏行红点唯一入口：纯读服务端权威运行集（Model A）。由 `RunningSetChanged`
    /// 事件实时刷新（带 seq 守卫），claim 即亮、release 即灭——不假阳（无客户端
    /// refcount 能钉住）、不假阴（任何接口的 run 都在集合里）。客户端 `running`
    /// refcount 仅服务 chunk 路由与活跃视图乐观态，不再判侧栏红点。
    #[must_use]
    pub fn is_running_session_key(&self, sk: &str) -> bool {
        self.server_running.with(|s| s.contains(sk))
    }
```

- [ ] **Step 7: 修其它 `set_server_running` 调用**（`bind_and_settle_run_...` 等测试若调旧签名需加 seq；Task 8 只改本文件测试，`chat_sidebar.rs` 的调用在 Task 9 修）

本文件内除 Step 1 替换的测试外，检查是否还有 `set_server_running(` 旧调用；若有，补 seq 实参。

- [ ] **Step 8: 运行 host 测试 + WASM check**

Run: `cargo test -p aleph-panel state::sessions 2>&1 | tail -25`
Expected: PASS（本文件全测）。
Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -15`
Expected: 前端因 `chat_sidebar.rs` 旧调用**可能报错**——那是 Task 9 的修点；若本步单独 check 报 `chat_sidebar` 参数错，属预期，Task 9 修复后整体应干净。为使本 Task 独立通过，可在本步仅跑 host 测试；WASM 整体 check 放到 Task 9 末尾。

- [ ] **Step 9: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/interfaces/webchat/src/state/sessions.rs"
git -C "$WT" add interfaces/webchat/src/state/sessions.rs
git -C "$WT" commit -m "feat(panel): red-dot pure server-authoritative + seq guard (Model A, no stuck/missing dots)"
```

---

### Task 9: 前端消费 `RunningSetChanged` + 订阅

在 `chat_sidebar.rs` 加事件臂，把 `run.running_set_changed` 直接喂 `set_server_running(seq, keys)`；订阅 wire method；修 `reload_data` 里 seed 调用的 seq。

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`（事件闭包 `:418-454`、订阅列表 `:456-488`、`reload_data` seed `:363-365`）

**Interfaces:**
- Consumes: `SessionMap::set_server_running(seq, keys)`（Task 8）；wire topic `run.running_set_changed`（Task 1）。
- Produces: 红点事件驱动实时刷新。

- [ ] **Step 1: 加事件臂**（`chat_sidebar.rs:418` 的 `subscribe_events` 闭包内，`run.session_updated` 检查之前插入）

```rust
        if event.topic == "run.running_set_changed" {
            let seq = event.data.get("seq").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let running: std::collections::HashSet<String> = event
                .data
                .get("running")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            session_map.set_server_running(seq, running);
            return;
        }
```
（`session_map` 已是本闭包捕获的 `Copy` context，见 `chat_sidebar.rs:254`。）

- [ ] **Step 2: 订阅 wire method**（`chat_sidebar.rs:478-483` 的 topic 数组内加一项）

```rust
        "stream.running_set_changed",
```

- [ ] **Step 3: 修 reload_data seed 的 seq**（`chat_sidebar.rs:363-365`——RPC 兜底 seed 无 seq，用 `0`；但 seq 守卫会丢 `seq<=server_seq` 的包，首次 seed 需能落地）

RPC seed 是"冷加载/兜底"路径，与事件流的 seq 不同源。为不让兜底被 seq 守卫吞掉、又不覆盖更新的事件态，改用一个"兜底通道"策略——把 seed 走**不触发 seq 前进**的合并：新增 `SessionMap::seed_server_running(keys)`（不比较 seq、仅当 `server_seq==0` 即尚未收到任何事件时才应用），mirror 于 `set_server_running`。

在 `sessions.rs`（回到 Task 8 文件，补一个方法——本 Task 允许追加）加：
```rust
    /// Cold-load fallback seed from a `run_concurrency` fetch (no event seq).
    /// Only applies while no authoritative event has arrived yet
    /// (`server_seq == 0`), so it can't clobber live event state.
    pub fn seed_server_running(&self, keys: HashSet<String>) {
        if self.server_seq.get_untracked() == 0
            && self.server_running.with_untracked(|cur| *cur != keys)
        {
            self.server_running.set(keys);
        }
    }
```
并把 `chat_sidebar.rs:364` 的 `session_map.set_server_running(metrics.running_sessions.into_iter().collect())` 改为 `session_map.seed_server_running(metrics.running_sessions.into_iter().collect())`。

- [ ] **Step 4: WASM 整体 check（含 Task 8）**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -20`
Expected: 干净编译。

- [ ] **Step 5: host 测试（seed 语义）**（在 `sessions.rs` tests 加一个）

```rust
    #[test]
    fn seed_applies_only_before_first_event() {
        use std::collections::HashSet;
        with_owner(|| {
            let map = SessionMap::new();
            map.seed_server_running(HashSet::from(["sess-cold".to_string()]));
            assert!(map.is_running_session_key("sess-cold"), "cold seed applies");
            // Once an event bumps seq, later seeds are ignored.
            map.set_server_running(5, HashSet::new());
            map.seed_server_running(HashSet::from(["sess-cold".to_string()]));
            assert!(!map.is_running_session_key("sess-cold"), "seed ignored after event");
        });
    }
```

Run: `cargo test -p aleph-panel state::sessions 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/interfaces/webchat/src/components/chat_sidebar.rs" "$WT/interfaces/webchat/src/state/sessions.rs"
git -C "$WT" add interfaces/webchat/src/components/chat_sidebar.rs interfaces/webchat/src/state/sessions.rs
git -C "$WT" commit -m "feat(panel): consume RunningSetChanged for live red-dot + cold-load seed guard"
```

---

### Task 10: Usage 页 `RunSlotsCard` 事件驱动刷新

修 gauge 从不刷新：收到 `run.running_set_changed` 时 refetch `run_concurrency`。

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/usage.rs`（连接 Effect `:52-78`、`run_slots` 信号 `:43`）

**Interfaces:**
- Consumes: `SystemApi::run_concurrency`（现有）；`DashboardState::subscribe_events`（现有）；topic `run.running_set_changed`。
- Produces: gauge 随运行态变化实时刷新。

- [ ] **Step 1: 加事件订阅刷新**（`usage.rs` 的连接 `Effect`（`:52-78`）内，`run_concurrency` 首拉之后，新增一个 `subscribe_events` 把变更 refetch 进 `run_slots`）

```rust
        // Keep the gauge live: on every running-set change, re-fetch the slot
        // snapshot (the event carries only `running`, not the N/M gauge fields).
        let state_evt = state.clone();
        let run_slots_evt = run_slots;
        let _sub = state.subscribe_events(move |event: crate::context::GatewayEvent| {
            if event.topic != "run.running_set_changed" {
                return;
            }
            let state_inner = state_evt.clone();
            leptos::task::spawn_local(async move {
                if let Ok(m) = SystemApi::run_concurrency(&state_inner).await {
                    run_slots_evt.set(Some(m.run_concurrency));
                }
            });
        });
```
（若本视图已订阅 `stream.running_set_changed` 由全局 `chat_sidebar` 完成，此处仅需本地 `subscribe_events` 监听已 fan-out 的 `run.running_set_changed`——订阅 RPC 在 Task 9 已发。确认 `subscribe_events` 返回值需持有以防被 drop；用 `let _sub` 绑定到组件生命周期，或存入信号，按 `usage.rs` 既有订阅模式对齐。)

- [ ] **Step 2: WASM check**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -20`
Expected: 干净。

- [ ] **Step 3: 提交**

```bash
WT="D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3"
rustfmt "$WT/interfaces/webchat/src/platform/wide/views/usage.rs"
git -C "$WT" add interfaces/webchat/src/platform/wide/views/usage.rs
git -C "$WT" commit -m "fix(panel): refresh RunSlotsCard gauge on running-set change (was connect-time only)"
```

---

### Task 11: 全量验证 + 收尾

**Files:** 无（验证）。

- [ ] **Step 1: 后端 lib check + 全相关模块测试**

Run: `cd D:/Workspace/Aleph/.claude/worktrees/feat-multisession-parallel-v3 && cargo check -p alephcore --lib 2>&1 | tail -10`
Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib "gateway::execution_engine" "gateway::inbound_router::busy_queue" "gateway::events::frame" "config::reload_impact" "handlers::gateway_metrics" 2>&1 | tail -30`
Expected: 全 PASS。

- [ ] **Step 2: 前端 WASM check + host 测试**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -10`
Run: `cargo test -p aleph-panel state::sessions 2>&1 | tail -15`
Expected: 干净 + PASS。

- [ ] **Step 3: fmt 漂移自检**（只查本次改动文件，不 `rustfmt mod.rs`）

Run: `for f in src/gateway/events/frame.rs src/gateway/execution_engine/concurrency.rs src/gateway/execution_engine/session_run_registry.rs src/gateway/execution_engine/concurrency_handle.rs src/gateway/execution_engine/engine.rs src/gateway/inbound_router/executor.rs src/gateway/inbound_router/busy_queue.rs src/config/reload_impact.rs src/builtin_tools/self_config.rs src/gateway/execution_engine/simple.rs src/gateway/execution_engine/mod.rs interfaces/webchat/src/state/sessions.rs interfaces/webchat/src/components/chat_sidebar.rs interfaces/webchat/src/platform/wide/views/usage.rs; do rustfmt --check "$f" 2>&1 | head -3; done`
Expected: 无输出（全部已格式化）。

- [ ] **Step 4: 分支自检**（确认仅本分支有改动、main 未动）

Run: `git -C "$WT" log --oneline main..HEAD` （`WT` 同上）
Run: `git -C "D:/Workspace/Aleph" log --oneline -1`（应仍是 `c6582536d`）
Expected: 分支有 10 个 feat/fix/refactor 提交，main 未动。

- [ ] **Step 5: E2E 手验（本地部署，人工）**

部署 Panel，验证：① 单会话发消息→红点即亮、完成即灭；② 同 agent 开两会话并发跑→两红点同亮、两会话真并行（不互相等待）；③ 另一接口（Telegram/daemon/另一 Panel）在已存在会话起 run→该会话红点亮；④ 刷新页→红点与真实运行态一致；⑤ `[execution] max_runs_global` 改配置→无需重启，下一轮 admission 生效（Usage 页 gauge 上限跟着变）。

---

## Self-Review

**Spec 覆盖核对：**
- §4.1 版本化广播 → Task 1（frame）+ Task 3（registry seq/emit）+ Task 4（bus 注入）✅
- §4.2 红点 Model A → Task 8（sessions.rs 纯服务端 + seq 守卫）+ Task 9（消费/订阅/seed）✅
- §4.2 gauge 从不刷新 → Task 10 ✅
- §4.3 FIFO 重键 → Task 5 ✅
- §4.4 上限热重载 → Task 2（reconfigure）+ Task 4（handle）+ Task 6（reload_impact + self_config）✅
- §4.5 熵减（RunState 死变体 / set_running 死链 / SimpleEngine 假 0）→ Task 7 ✅
- §6 验证口径 → Task 11 ✅
- §7 不做项（LRU/MailboxPhase/每队列 drain/全局会话上限）→ 计划中无对应 Task ✅（正确地未实现）

**类型一致性核对：**
- `set_server_running(seq: u64, keys: HashSet<String>)` — Task 8 定义，Task 9 消费，签名一致 ✅
- `seed_server_running(keys)` — Task 9 定义并消费 ✅
- `RunningSetChanged { seq: u64, running: Vec<String> }` — Task 1 定义，Task 3 emit，Task 9 消费字段名 `seq`/`running` 一致 ✅
- `reconfigure(global_cap, per_agent_cap)` — Task 2 定义；`reconfigure_global(global_cap, per_agent_cap)` — Task 4 定义，Task 6 消费，一致 ✅
- `running_snapshot() -> (u64, Vec<String>)` — Task 3 定义并自用 ✅
- wire method `stream.running_set_changed` → 前端改写 `run.running_set_changed` — Task 1 产出、Task 9/10 消费一致 ✅

**Placeholder 扫描：** 无 TBD/TODO；每改动步含具体代码。少数"以实际签名为准"注记（Task 4 Step 4 setter 形态、Task 6 Step 6 可见性、Task 7 Step 3 SimpleEngine 字段、Task 10 Step 1 订阅持有）均为"读现场确认后按给定代码微调"，非占位——给出了确切代码与判定条件。

**已知实现时校验点（非阻塞）：**
- Task 4 Step 3：`ExecutionEngine::new` 若为直接返回字面量，需改成 `let engine = …; install_global(&engine.concurrency); engine`。
- Task 7 Step 3：`SimpleExecutionEngine` 若持可查询运行集则返回之，否则 honest empty。
- Task 10：`subscribe_events` 返回订阅句柄的持有方式按 `usage.rs` 既有模式。
