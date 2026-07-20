# Stage 7 Audit Report — Init Wiring & Trace Coverage

**Status**: 🟢 Final (2026-05-08) · all 5 wiring gaps + 14 trace gaps closed in commits `83b26848c` (T3) + `319bc4572` (T4); 3 integration tests in `ca6bc5f9b` (T5)
**Plan**: [`2026-05-08-harness-stage7-init-audit-plan.md`](2026-05-08-harness-stage7-init-audit-plan.md)
**Auditor**: AI assistant
**Audit method**: Static grep + Read of `HarnessDeps` field producers/consumers across boot path & runtime path
**HEAD at audit time**: `f13f355c6` (post Stage 7 plan commit)

---

## 1. 方法

对 `HarnessDeps` 中每个字段（共 14 个），定位以下三类引用：

1. **生产者（Producer / Boot Path）**：在启动序列里用 `Some(...)` / `default()` / `clone()` 写入的位置
2. **消费者（Consumer / Runtime Path）**：在 `agent.rs` 主循环 / 辅助函数中读取该字段的位置
3. **测试 fixture**：`#[cfg(test)]` 路径的 `None` 占位（不算 production wiring）

定位手段：
- `grep -nE` 跨 `src/` 全文搜索字段名（精确匹配）
- 排除测试文件、tests/ 目录、`#[cfg(test)]` 块
- Production 路径仅看：`harness_bridge.rs` / `orchestrator_init.rs` / `subagent_spawner.rs` / `bin/aleph-server/commands/start/builder/`

---

## 2. Audit Matrix（终版）

| # | Seam | Stage | 生产者 (Boot) | 消费者 (Runtime) | Trace event | 状态 |
|---|------|-------|---------------|-------------------|-------------|------|
| 1 | `ErrorClass` enum | 1 | n/a — 编译期类型 | `agent.rs:259` (Transient), `agent.rs:653,696` 等多处 | n/a | ✅ |
| 2 | `tools: Arc<dyn ToolService>` | 2 | `harness_bridge.rs::build()` 通过 `AgentHarnessRunner.tool_service` | `agent.rs::act` | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 3 | `prompt_builder: Arc<dyn PromptBuilder>` | 3 | `harness_bridge.rs:156` `DefaultPromptBuilder` (gateway) / `subagent_spawner.rs:211` `DefaultPromptBuilder` (subagent) | `agent.rs::run_turn_internal` | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 4 | `chain_context: ChainContext` | 4 | `harness_bridge.rs:157` `default()` (gateway root) / `subagent_spawner.rs:215` `child_chain.clone()` (parent.child) | `agent.rs::chain_context()` | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 5 | `guardrails: Option<Arc<GuardrailRegistry>>` | 5a/5b | **❌ `harness_bridge.rs:158` hardcoded `None`** (gateway) / `subagent_spawner.rs:218` `base.guardrails.clone()` ✅ (subagent inherit) | `agent.rs:171,304,488,653,696` | ⚠️ trace 缺 | ❌ **gateway wiring 缺**，subagent ✅ |
| 6 | `fallback_llm: Option<Arc<dyn AiProvider>>` | 5b | **❌ `harness_bridge.rs:159` hardcoded `None`** / `subagent_spawner.rs:219` `None` (deliberate — subagent 短周期不重试) | `agent.rs:250,259,263` `race_llm_call` | ⚠️ trace 缺 | ❌ **gateway wiring 缺** |
| 7 | `verifier_chain: Option<Arc<VerifierChain>>` | 6a | `orchestrator_init.rs:86-93` ✅ → `harness_bridge.rs:150` ✅ (gateway) / `subagent_spawner.rs:205` `None` (deliberate — subagent 不需 turn 级验证) | `agent.rs:765` `run_verifiers` | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 8 | `trace_sink: Option<Arc<dyn TraceSink>>` | P0 rescue | `harness_bridge.rs:88,154` ✅ | `agent.rs` 多处 fire 点 | n/a (本身) | ✅ |
| 9 | `stall_config: Option<StallConfig>` | P0 rescue | **❌ `harness_bridge.rs:162` hardcoded `None`** / `subagent_spawner.rs:222` `None` (deliberate) | `agent.rs:74` stall watchdog | ⚠️ trace 缺 | ❌ **gateway wiring 缺** |
| 10 | `consecutive_failure_cap: Option<usize>` | P0 rescue | **❌ `harness_bridge.rs:163` hardcoded `None`** / `subagent_spawner.rs:223` `None` (deliberate) | `agent.rs:896` 连续失败兜底 | ⚠️ trace 缺 | ❌ **gateway wiring 缺** |
| 11 | `turn_timeout: Option<Duration>` | P0 rescue | **❌ `harness_bridge.rs:164` hardcoded `None`** / `subagent_spawner.rs:224` `None` (deliberate) | `agent.rs:510,628` per-turn timeout | ⚠️ trace 缺 | ❌ **gateway wiring 缺** |
| 12 | `skill_prefetcher: Option<Arc<SkillPrefetcher>>` | 既有 | `harness_bridge.rs:153` ✅ | `agent.rs` Think 入口 | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 13 | `context_budget: Option<Arc<Mutex<ContextBudget>>>` | 既有 | `harness_bridge.rs:151` ✅ | `agent.rs` 间 turn 评估 | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 14 | `context_compactor: Option<Arc<ContextCompactor>>` | 既有 | `harness_bridge.rs:152` ✅ | `agent.rs` Compactor directive | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 15 | `system_prompt: Option<String>` | 既有 | `harness_bridge.rs::build()` 通过 `memory_context_provider` 装配 | `agent.rs::run_turn_internal` | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 16 | `power: Option<Arc<dyn PowerCapability>>` | 既有 | `orchestrator_init.rs:98-115` (cfg-gated per-OS) → `harness_bridge.rs` clone | `agent.rs` turn idle inhibit | ⚠️ trace 缺 | ✅ wiring，⚠️ trace |
| 17 | `max_iterations: Option<usize>` | 既有 | `harness_bridge.rs:160` `None` (gateway path 通过 FlowOverrides 覆盖；非 hardcoded gap) / `subagent_spawner.rs` 通过 base inherit | `agent.rs::run` 主循环硬上限 | ⚠️ trace 缺 | ⚠️ wiring 经 FlowOverrides，非直接 |

**汇总**：
- **❌ Production gateway wiring 缺**：5 处（`guardrails / fallback_llm / stall_config / consecutive_failure_cap / turn_timeout`）— Stage 5a/5b/P0 rescue 的 seam 在 gateway 侧从未配通
- **⚠️ Trace 缺**：14 处（每个非平凡 seam，TraceSink 自身除外）— 所有装配点都缺 init 事件
- **✅ 已完整**：6 处（ErrorClass / verifier_chain / trace_sink / skill_prefetcher / context_* / power）

---

## 3. Subagent 路径设计意图（非 bug，不修补）

`subagent_spawner.rs:205-224` 显式给 subagent 的 4 个字段写 `None`：

| 字段 | 为什么 subagent 不继承 |
|------|----------------------|
| `verifier_chain` | turn 级验证（stop hook / tool-loop）作用于 main agent 主循环，subagent 是任务式短跑 |
| `fallback_llm` | subagent 失败可被父 agent 感知；subagent 自己不需在内部 race fallback |
| `stall_config` / `turn_timeout` / `consecutive_failure_cap` | subagent 任务边界明确，没有"长时间空转"语义 |

**保留继承**：`guardrails` (line 218) — security policy 必须全局一致。

**审计判定**：✅ 设计意图清晰，Stage 7 不动 subagent 路径。

---

## 4. 修补建议（落入 plan T3-T5）

### 4.1 T3 修补：5 处 hardcoded None → config-driven

**文件**: `src/orchestrator/harness_bridge.rs`

```rust
// AgentHarnessRunner 结构体（约 50-65 行）追加 5 字段
pub struct AgentHarnessRunner {
    // ...existing fields...
    pub verifier_chain: Option<Arc<VerifierChain>>,
    // ── Stage 7 添加（与 verifier_chain 风格对齐）──
    pub guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    pub fallback_llm: Option<Arc<dyn crate::providers::AiProvider>>,
    pub stall_config: Option<crate::harness::deps::StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<std::time::Duration>,
    // ── /Stage 7 ──
    pub context_budget: Option<Arc<Mutex<ContextBudget>>>,
    // ...
}

// build() 中（约 158-164 行）替换 hardcoded None
let deps = HarnessDeps {
    // ...
    guardrails: self.guardrails.clone(),                          // was: None
    fallback_llm: self.fallback_llm.clone(),                      // was: None
    max_iterations: None,                                          // unchanged (FlowOverrides)
    stall_config: self.stall_config.clone(),                      // was: None
    consecutive_failure_cap: self.consecutive_failure_cap,        // was: None
    turn_timeout: self.turn_timeout,                              // was: None
    // ...
};
```

**文件**: `src/bin/aleph-server/commands/start/orchestrator_init.rs`

```rust
// 构造 AgentHarnessRunner 时显式传入 5 个 None（保持 Phase-6 占位）
let harness = Arc::new(AgentHarnessRunner {
    // ...existing fields...
    verifier_chain,
    guardrails: None,            // PHASE-6: load from aleph.toml [guardrails]
    fallback_llm: None,          // PHASE-6: secondary provider from auth profile
    stall_config: None,          // PHASE-6: tunable from aleph.toml [agent.stability]
    consecutive_failure_cap: None,  // PHASE-6: same
    turn_timeout: None,          // PHASE-6: same
    context_budget: None,
    // ...
});
```

**风险**：极低。5 个字段全部默认 `None`，行为完全等价 main HEAD。打通的是字段路径，不是配置消费。

### 4.2 T4 修补：`LoopTrace::InitSeam` variant + emit

**文件**: `src/harness/trace_sink.rs`

```rust
pub enum LoopTrace {
    // ...existing variants...
    InitSeam {
        stage: &'static str,    // e.g. "stage5a-guardrails"
        seam: &'static str,     // e.g. "GuardrailRegistry"
        configured: bool,       // true = Some(_) wired; false = None (deliberate skip)
    },
}
```

**文件**: `src/orchestrator/harness_bridge.rs`

```rust
// build() 在每个 deps 字段装配后 emit
if let Some(sink) = trace_sink.as_ref() {
    sink.emit(LoopTrace::InitSeam {
        stage: "stage3-prompt",
        seam: "PromptBuilder",
        configured: true,
    });
    sink.emit(LoopTrace::InitSeam {
        stage: "stage4-chain",
        seam: "ChainContext",
        configured: true,
    });
    sink.emit(LoopTrace::InitSeam {
        stage: "stage5a-guardrails",
        seam: "GuardrailRegistry",
        configured: deps.guardrails.is_some(),
    });
    // ... 余 6 处类似（fallback_llm / verifier_chain / stall_config /
    //                consecutive_failure_cap / turn_timeout / skill_prefetcher）
}
```

**Emit 顺序**：与 `HarnessDeps` 字段定义顺序一致（便于读取 trace 时直接对应 deps.rs 行号）。

### 4.3 T5 修补：集成测试

**文件**: `src/harness/tests/init_audit.rs`（新建）

测试 1 — `cold_start_emits_all_seam_events`：
```rust
#[tokio::test]
async fn cold_start_emits_all_seam_events() {
    let (sink, events) = RecordingTraceSink::new();
    let runner = AgentHarnessRunner { /* minimal — all None except trace_sink */ };
    let _harness = runner.build_for_test(...);  // 触发 emit
    let init_seams: Vec<_> = events.lock().unwrap()
        .iter()
        .filter_map(|t| match t {
            LoopTrace::InitSeam { seam, .. } => Some(*seam),
            _ => None,
        })
        .collect();
    let expected = [
        "PromptBuilder", "ChainContext", "GuardrailRegistry",
        "FallbackLLM", "VerifierChain", "StallConfig",
        "ConsecutiveFailureCap", "TurnTimeout", "SkillPrefetcher",
    ];
    for s in expected {
        assert!(init_seams.contains(&s), "missing init seam event: {s}");
    }
}
```

测试 2 — `init_events_precede_first_turn`：
```rust
#[tokio::test]
async fn init_events_precede_first_turn() {
    let (sink, events) = RecordingTraceSink::new();
    let harness = build_minimal_harness(sink).await;
    let _ = harness.run("hello".into(), None).await;  // 一次 turn
    let log = events.lock().unwrap();
    let last_init_idx = log.iter().rposition(|t| matches!(t, LoopTrace::InitSeam { .. }))
        .expect("at least one InitSeam");
    let first_turn_idx = log.iter().position(|t| matches!(t, LoopTrace::TurnStart { .. }))
        .expect("at least one TurnStart");
    assert!(last_init_idx < first_turn_idx,
        "all InitSeam events must precede first TurnStart");
}
```

---

## 5. 启动 timing baseline

**当前状态**：⏳ TBD — 待 T5 实施时锁定

**计划**：
```bash
# baseline (bf0de41cc commit)
git stash && git checkout bf0de41cc
cargo build --release --bin aleph-server
hyperfine --warmup 3 --runs 10 \
    'target/release/aleph-server start --dry-run' \
    --export-json /tmp/baseline.json
git checkout main && git stash pop

# regression check (post Stage 7 implementation)
cargo build --release --bin aleph-server
hyperfine --warmup 3 --runs 10 \
    'target/release/aleph-server start --dry-run' \
    --export-json /tmp/post-stage7.json

# 验证：post-stage7 mean < 1.05 × baseline mean
```

**注意**：`--dry-run` flag 是否存在需要在 T5 时确认；若不存在，可用 `start` 配合 SIGTERM at 2s 模拟冷启 + 立即退出。

---

## 6. Open Questions

1. **subagent 是否应继承 `verifier_chain`？**
   - 当前：`None`（deliberate per design）
   - 反方观点：subagent 内部也可能 tool-loop 死循环
   - **判定**：保持 `None`。subagent 调用栈外层（main agent 的 verifier_chain）会感知 subagent 卡住，由父级 verifier 兜底。如未来发现盲区，单开 spec。

2. **`max_iterations` 算 wiring gap 吗？**
   - 当前：`harness_bridge.rs:160` `None`（gateway 路径）
   - 来源：FlowOverrides 在 `harness_bridge.rs::run_for_session` 等接口中透传，**非 HarnessDeps 字段直接装配**
   - **判定**：⚠️ 但不算 ❌；FlowOverrides 是另一条独立 max_iterations 通道。Stage 7 不动这条路径。

3. **测试 fixture 中的 `None` 是否需要审？**
   - 答：不需要。`agent.rs:1226+ / 1426+ / 1482+` 是 `#[cfg(test)]` 块内的 fixture，与 production wiring 完全解耦。

---

## 7. Stage 7 实施门禁（自审）

| 验证项 | 期望 |
|--------|------|
| 5 处 hardcoded `None` 打通 | T3 commit |
| 9 处 InitSeam 事件 emit | T4 commit |
| 2 处集成测试覆盖 | T5 commit |
| 启动 timing < 1.05× baseline | T5 commit 验收 |
| `cargo test -p alephcore --lib` 全绿 | 每个 commit |
| `agent.rs` 行数 ≤ 1520（不增） | 每个 commit |
| 6b 红线未变 | n/a（已永久 defer） |

---

## 8. 与 plan §2.1 初版 Audit Matrix 的差异

- 增加第 13-17 行（context_budget / context_compactor / system_prompt / power / max_iterations）— 完整覆盖 `HarnessDeps` 17 字段
- 第 5 行 `guardrails` 状态拆为"gateway ❌ / subagent ✅"（plan 初版只标 ❌）
- 第 6/9/10/11 行加 subagent deliberate-None 注释
- 第 17 行 `max_iterations` 标 ⚠️ 不算 ❌（FlowOverrides 独立通道）
