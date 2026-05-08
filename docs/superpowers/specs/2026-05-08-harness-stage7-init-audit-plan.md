# Stage 7 Plan — Initialization Audit (#12)

**Status**: 🟢 Shipped on 2026-05-08 (commits T1-T6: `f13f355c6 → fae84fe9c → 83b26848c → 319bc4572 → ca6bc5f9b → docs-ship`)
**Master spec**: [`2026-05-05-harness-12-module-roadmap-design.md`](2026-05-05-harness-12-module-roadmap-design.md) § Stage 7
**Module**: #12 Initialization Audit
**Risk class**: medium（按 master spec 分级）
**Depends on**: Stage 1-6 全部已 ship（Stage 6b 永久 defer，不阻塞）

---

## 1. 目标

收口 master roadmap 路线图：在 Stage 1-6 引入大量 trait / Option seam 后，**审计冷启动装配链**，确保：

1. **每个 seam 都有真实生产者**（boot wiring）— 不存在"trait 定义了，但启动路径上没人塞 impl 进去"
2. **每个 seam 都有真实消费者**（runtime 路径）— 不存在"字段塞了 Some(impl)，但 agent.rs 永远走不到那条 match 臂"
3. **冷启动 trace 端到端可观测**— TraceSink 在每个关键 wiring 点 emit init 事件，使运维 / 验收侧能机械验证装配链
4. **启动时间不退化**— `< 1.05 × baseline`（baseline = bf0de41cc commit 处冷启动 timing；本 stage 实施时锁定具体 ms 数值）

**Stage 7 范围内**:
1. 审计现有 init 路径（`init_unified/` / `bin/aleph-server/commands/start/builder/` / `orchestrator_init.rs` / `harness_bridge.rs`）
2. 修补 production wiring gap（已发现：`guardrails / fallback_llm / stall_config / consecutive_failure_cap / turn_timeout` 在 `harness_bridge.rs` 硬编码 `None`）
3. 加 init trace events（每个 stage 1-6 seam 在 wiring 点 emit 一次）
4. ≥1 端到端启动 trace assertion + ≥1 init 时序完整性测试

**Stage 7 不在范围**（明文 defer）:
- ❌ Stage 6b（**永久 deferred**，见 master spec § Stage 6 + `src/verification/mod.rs` 红线注释）
- ❌ 新增任何 trait / struct / 抽象（master spec: "Allowed seams: 无（纯 wiring + trace 字段补充）"）
- ❌ first-time install path（`init_unified/coordinator.rs` 创建 dirs / config / sqlite / runtime ledger / built-in skills）— 这是另一个独立关切，与 harness wiring 审计不混合
- ❌ Subsystem boot order 重排 / 新增并发引导
- ❌ 任何 R10 / R7 / R8 红线变更

---

## 2. 架构

```text
┌──────────────────────────────────────────────────────────────────┐
│ 审计目标文件（只读 + 小补丁）                                    │
│   src/init_unified/coordinator.rs       (first-install — 不动)   │
│   src/bin/aleph-server/commands/start/                           │
│     ├─ orchestrator_init.rs             (143 行 — 已审 6a)        │
│     ├─ builder/agent_init.rs            (2132 行 — 主要补丁面)    │
│     ├─ builder/handlers.rs              (1955 行)                 │
│     └─ builder/subsystems.rs            (599 行)                  │
│   src/orchestrator/harness_bridge.rs    (975 行 — gap 主战场)     │
│                                                                  │
│ 新增（最小化）：                                                 │
│   src/harness/trace_sink.rs              (扩 InitEvent variant)  │
│   src/harness/tests/init_audit.rs   NEW (≥2 测试)                │
│   docs/superpowers/specs/                                        │
│     2026-05-08-harness-stage7-audit-report.md   NEW (审计快照)   │
└──────────────────────────────────────────────────────────────────┘
```

### 2.1 Audit Matrix（初版，实施 Task 2 时填充终版）

| # | Seam | 来源 stage | 生产者（boot） | 消费者（runtime） | Trace event | 当前状态 |
|---|------|-----------|---------------|-------------------|-------------|---------|
| 1 | `ErrorClass` enum | Stage 1 | n/a（编译期） | `agent.rs` race_llm_call / act 错误回流 / fallback retry | n/a | ✅ |
| 2 | `ToolService` trait | Stage 2 | `builder/agent_init.rs` | `agent.rs::act` | TBD | ⚠️ trace 缺 |
| 3 | `PromptBuilder` trait | Stage 3 | `harness_bridge.rs:156` `DefaultPromptBuilder`（gateway）/ `subagent_spawner.rs`（custom subagent） | `agent.rs::run_turn_internal` | TBD | ⚠️ trace 缺 |
| 4 | `ChainContext` | Stage 4 | `harness_bridge.rs:157` `default()`（gateway root） / `subagent_spawner.rs` `parent.chain.child()` | `agent.rs::chain_context()` + subagent 路径 | TBD | ⚠️ trace 缺 |
| 5 | `GuardrailRegistry` | Stage 5a/5b | **❌ `harness_bridge.rs:158` hardcoded `None`** | `agent.rs` input/output/tool-call 三 callsite | TBD | **❌ wiring 缺** |
| 6 | `fallback_llm` | Stage 5b | **❌ `harness_bridge.rs:159` hardcoded `None`** | `agent.rs::race_llm_call` | TBD | **❌ wiring 缺** |
| 7 | `VerifierChain` | Stage 6a | `orchestrator_init.rs:86-93` ✅ | `agent.rs::run_verifiers` ✅ | TBD | ✅ wiring，⚠️ trace 缺 |
| 8 | `TraceSink` | P0 rescue | `harness_bridge.rs` `trace_sink.clone()` | agent.rs 多处 fire 点 | n/a（自身） | ✅ |
| 9 | `StallConfig` | P0 rescue | **❌ `harness_bridge.rs:162` hardcoded `None`** | agent.rs stall watchdog | TBD | **❌ wiring 缺** |
| 10 | `consecutive_failure_cap` | P0 rescue | **❌ `harness_bridge.rs:163` hardcoded `None`** | agent.rs 连续失败兜底 | TBD | **❌ wiring 缺** |
| 11 | `turn_timeout` | P0 rescue | **❌ `harness_bridge.rs:164` hardcoded `None`** | agent.rs per-turn timeout | TBD | **❌ wiring 缺** |
| 12 | `SkillPrefetcher` | 既有 | `harness_bridge.rs:153` clone | agent.rs Think 入口 | TBD | ⚠️ trace 缺 |

**关键发现**：5/12 seams 在 production gateway 路径上 hardcoded `None`。这些 seam 在测试（`task10_wiring.rs` / `guardrails.rs` / `think.rs`）里有覆盖，但 production 走 `harness_bridge.rs::AgentHarnessRunner.build()` 那条路 — boot path 没有把 config → runner 字段 → HarnessDeps 的链路打通。

### 2.2 修补策略（最小化 surgical wiring）

**对每个"❌ wiring 缺"行**：

1. 在 `AgentHarnessRunner` 结构体加一个对应字段（pub，与现有 `verifier_chain` 同形式）
2. 在 `harness_bridge.rs::build()` 用 `self.<field>.clone()` 代替 hardcoded `None`
3. 在 `orchestrator_init.rs::initialize_orchestrator(...)` 从 config 装配该字段（参考第 86-93 行 `verifier_chain` 范式）
4. 不引入新 trait / 不调整既有 trait 签名 / 不改 `HarnessDeps` 字段顺序 / 不改 callsite 语义

**对每个"⚠️ trace 缺"行**：

`TraceSink` 扩一个 `InitEvent { stage: &'static str, seam: &'static str }` variant，在 `harness_bridge.rs::build()` 装配每个 seam 后调用一次。Production `GatewayTraceSink` 持久化；测试 `RecordingTraceSink` 收集断言。

### 2.3 trace event schema 草案

```rust
// src/harness/trace_sink.rs（追加，非破坏）
pub enum LoopTrace {
    // ...existing variants...
    InitSeam {
        stage: &'static str,    // "stage5a-guardrails", "stage6a-verifier" 等
        seam: &'static str,     // "GuardrailRegistry", "VerifierChain" 等
        configured: bool,       // true = Some(impl) 装配；false = None（跳过）
    },
}
```

**为什么是字符串字面量**：避免引入 enum 防止 trait 抖动；枚举值用编译期 `&'static str` 锁定，跨进程消费侧只读取字符串。

---

## 3. Acceptance Criteria

### 功能
- [ ] **Audit Matrix 全 12 行状态** = ✅（无 ❌ / ⚠️）
- [ ] 冷启动后 `RecordingTraceSink` 至少捕获以下 InitSeam 事件：`PromptBuilder` / `ChainContext` / `GuardrailRegistry` / `FallbackLLM` / `VerifierChain` / `TraceSink` / `StallConfig` / `ConsecutiveFailureCap` / `TurnTimeout`（≥9 个事件）
- [ ] `harness_bridge.rs` 中 5 处 `hardcoded None`（`guardrails / fallback_llm / stall_config / consecutive_failure_cap / turn_timeout`）全部改为 `self.<field>.clone()`
- [ ] `orchestrator_init.rs` 增加对应 5 个字段的 config-driven 装配（最简：先全部默认 `None`，但代码路径上从 config 读取）

### 不破坏
- [ ] `cargo test -p alephcore --lib` 全绿（baseline = main HEAD）
- [ ] `harness_run_e2e` 集成测试通过
- [ ] 启动时间 `< 1.05 × baseline`（baseline 在 Task 2 时通过 `cargo bench --bench startup` 或 hyperfine 锁定）
- [ ] `verifier_chain` 6a 行为完全不变

### 测试
- [ ] ≥1 端到端启动 trace assertion 测试（`init_audit::cold_start_emits_all_seam_events`）
- [ ] ≥1 init 时序完整性测试（`init_audit::init_events_precede_first_turn`）
- [ ] R10 self-check：agent.rs 行数 ≤ 1500（**注意：当前 main HEAD = 1520 行，已超 R10 cap 20 行**；Stage 7 不主动减 — 由后续 R10 修复负责，但 Stage 7 不能继续加）

---

## 4. Tasks（每个独立 commit）

### T1 — Plan doc commit
- 落地本 plan
- master spec § Stage 6 已在 Task 1 同 commit 链中标记 6b "Permanently Deferred"

### T2 — Audit pass + report
- 读 6 个 init 关键文件（agent_init.rs / handlers.rs / subsystems.rs / orchestrator_init.rs / harness_bridge.rs / coordinator.rs）
- 产出 `docs/superpowers/specs/2026-05-08-harness-stage7-audit-report.md`：
  - 终版 Audit Matrix（每行附文件:行号引用）
  - 启动 timing baseline 数值（hyperfine 测三次取中位数）
  - 每个 ❌/⚠️ 的具体修补建议
- **commit 内容**：仅文档。不动代码。

### T3 — wiring 修补：补 `AgentHarnessRunner` 5 字段 + `harness_bridge.rs::build()`
- `AgentHarnessRunner` 加 5 字段（`guardrails / fallback_llm / stall_config / consecutive_failure_cap / turn_timeout`），全部 `pub`、默认 `None`、与 `verifier_chain` 字段顺序对齐
- `harness_bridge.rs::build()` 把 5 处 `hardcoded None` 改成 `self.<field>.clone()`
- `orchestrator_init.rs::initialize_orchestrator(...)` 在构造 runner 时显式传入 5 个 `None`（保持 Phase-6 占位语义；不破坏现有行为）
- 验证：`cargo test -p alephcore --lib` 全绿；`harness_run_e2e` 通过
- **不改 production 默认行为**：5 字段仍是 `None`，但路径已打通；后续 Phase-6 工作可以从 config 装配

### T4 — TraceSink::InitSeam variant + harness_bridge.rs emit
- `trace_sink.rs` 加 `InitSeam` variant（追加式 enum，非破坏）
- `harness_bridge.rs::build()` 在每个 seam clone 后调用 `trace_sink.emit(LoopTrace::InitSeam { stage, seam, configured })`
- 验证：单元测试 + 编译

### T5 — 集成测试：cold-start trace assertion + 时序完整性
- `src/harness/tests/init_audit.rs`（新文件，注册到 `tests/mod.rs`）：
  - `cold_start_emits_all_seam_events`：用 `RecordingTraceSink` 装配一个最小 harness，运行一次空 turn，断言至少 9 个 InitSeam 事件按指定 seam 集合出现（顺序无要求）
  - `init_events_precede_first_turn`：断言所有 InitSeam 事件的序号 < 第一个 LoopTraceTurn 事件序号
- 验证：新测试绿；既有 71 harness 测试不退化

### T6 — docs ship
- master spec § Stage 7 状态：`🟡 Pending` → `🟢 Shipped on YYYY-MM-DD · plan: ...`
- CHANGELOG.md 加 Stage 7 条目（[Unreleased] / Added）
- 本 plan Status 改 `🟢 Shipped on YYYY-MM-DD`
- audit-report.md 锁定终版

---

## 5. CHANGELOG draft

```markdown
### Added
- **Harness Stage 7 (Init Audit)**: Cold-start initialization audit
  closes the master roadmap § 1.4 wiring gap. Adds `LoopTrace::InitSeam`
  trace events emitted from `harness_bridge.rs::build()` for each
  Stage 1-6 seam (PromptBuilder, ChainContext, GuardrailRegistry,
  fallback_llm, VerifierChain, TraceSink, StallConfig,
  consecutive_failure_cap, turn_timeout). Patches 5 production
  hardcoded-`None` wiring gaps in the gateway path: `AgentHarnessRunner`
  now plumbs guardrails / fallback_llm / stall_config /
  consecutive_failure_cap / turn_timeout from config to HarnessDeps
  (defaults still `None`; Phase-6 will wire from `aleph.toml`). Two
  new integration tests in `harness/tests/init_audit.rs` lock the
  cold-start trace contract.
```

---

## 6. Verification

```bash
# 单元 + 集成
cargo check -p alephcore
cargo test -p alephcore --lib --no-fail-fast
cargo test -p alephcore --test harness_run_e2e

# R10 self-check
wc -l src/harness/agent.rs src/harness/deps.rs
ls src/harness/*.rs | wc -l    # 期望 ≤ 9

# 启动 timing（baseline 锁定 + 回归）
hyperfine --warmup 3 'cargo run --release --bin aleph-server -- start --dry-run'
# 期望：< 1.05 × baseline_ms
```

---

## 7. Out of Scope（明文）

| 项 | 原因 |
|----|------|
| Stage 6b（JudgeVerifier / ComputationalVerifier） | 永久 deferred — 违 R7+R8+R10 |
| 新 trait / 新 struct | master spec § Stage 7 "Allowed seams: 无" |
| `init_unified/coordinator.rs` 改造 | 是 first-install 关切，与 harness wiring 审计正交 |
| Subsystem boot 顺序 / 并发改造 | 出范围；本 stage 是审，不是重构 |
| `agent.rs` 行数压缩 | 当前 1520 超 R10 cap 20 行 — 由独立 R10 修复负责，不与本 stage 绑 |
| Phase-6 config 加载（`aleph.toml [guardrails]` 等） | 留给 Phase-6 — 本 stage 只打通路径 |

---

## 8. Rollback Plan

每个 commit 独立可 revert：

| Commit | 风险 | revert 影响 |
|--------|------|-------------|
| T1 plan doc | 0 | docs only，无影响 |
| T2 audit report | 0 | docs only |
| T3 5 字段补 wiring | 低 | 字段默认 `None`，行为完全等价 main HEAD |
| T4 InitSeam variant | 低 | enum 追加（非破坏），revert = 删 variant + emit 调用 |
| T5 集成测试 | 0 | 测试 only |
| T6 docs ship | 0 | docs only |

**最坏路径**：T3-T5 全部回滚 = 仅留 audit-report.md 作为一次性快照，0 行代码影响。

---

## 9. R10 Self-Check

| 维度 | 限值 | 当前 | Stage 7 之后预期 | 通过 |
|------|------|------|------------------|------|
| `harness/agent.rs` 行数 | ≤ 1500 | 1520（已超） | 1520（不动） | ⚠️ 已偏，但本 stage 不增 |
| `harness/` 文件数 | ≤ 9 | 9 | 9（不增） | ✅ |
| 新 trait | 0 | n/a | 0 | ✅ |
| 笨循环 5 个不 | 全过 | 已过 | 已过 | ✅ |
| Future-Proof Test | 模型升级仍有用 | yes | yes（init wiring 与模型无关） | ✅ |

`agent.rs` 1520 行问题在 Stage 6a 落地后被审计到（详见 prior session memory），属于另一支并行 audit-fix 工作的 WIP，**不在 Stage 7 charter 内** — Stage 7 只负责"不继续加"，不负责"主动减"。

---

## 10. Commit Chain

```bash
# T1
git add docs/superpowers/specs/2026-05-08-harness-stage7-init-audit-plan.md \
        docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md \
        src/verification/mod.rs
git commit -m "docs(harness): plan Stage 7 (init audit) + permanently defer 6b"

# T2
git add docs/superpowers/specs/2026-05-08-harness-stage7-audit-report.md
git commit -m "docs(harness): Stage 7 audit report — 12 seams, 5 production wiring gaps"

# T3
git add src/orchestrator/harness_bridge.rs \
        src/bin/aleph-server/commands/start/orchestrator_init.rs
git commit -m "harness: Stage 7 patch 5 hardcoded None gaps in AgentHarnessRunner"

# T4
git add src/harness/trace_sink.rs src/orchestrator/harness_bridge.rs
git commit -m "harness: Stage 7 emit LoopTrace::InitSeam from harness_bridge"

# T5
git add src/harness/tests/init_audit.rs src/harness/tests/mod.rs
git commit -m "harness: Stage 7 integration tests — cold-start trace assertions"

# T6
git add CHANGELOG.md \
        docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md \
        docs/superpowers/specs/2026-05-08-harness-stage7-init-audit-plan.md \
        docs/superpowers/specs/2026-05-08-harness-stage7-audit-report.md
git commit -m "docs(harness): Stage 7 shipped — init audit complete"
```

---

## 11. 与 6a plan 的差异说明

| 维度 | Stage 6a | Stage 7 |
|------|---------|---------|
| 主要交付 | trait 抽象 + 行为扩展 | 审计报告 + wiring 补丁 |
| 新文件数 | 4 prod + 4 test | 0 prod + 1 trace 文件追加 + 1 测试文件 + 2 docs |
| 风险 | high（callsite 替换） | medium（追加式 enum + None→Some 路径） |
| 行为变更 | tool-loop 死循环检测真实生效 | 行为零变化（None 仍然 None） |
| Future-Proof | trait 可扩 | wiring 与模型无关 |

**为什么 Stage 7 行为是零变化**：本 stage 只打通路径（None → 仍是 None，但来源从 hardcoded 变为 config-driven），并加 trace。**真正的 production 行为切换在未来 Phase-6 通过 `aleph.toml` 提供 config**。这是 master spec § Stage 7 "Allowed seams: 无（纯 wiring + trace 字段补充）" 的字面含义。


---

## 12. Phase-6 Closed (2026-05-08)

Stage 7 left five `AgentHarnessRunner` fields hardcoded to `None` with a `PHASE-6` marker. Phase-6 closed those gaps in 6 commits:

| Commit | Subject |
|--------|---------|
| `2969b3ef8` | P6-1 plan doc |
| (P6-2)     | Schema — three toml sections in `src/config/types/phase6_wiring.rs` |
| `a30d16fed` | P6-3 wire `[guardrails]` → `guardrails` |
| `5f02c1480` | P6-4 wire `[fallback_provider]` → `fallback_llm` |
| `95a356aab` | P6-5 wire `[stability]` → `stall_config` + `consecutive_failure_cap` + `turn_timeout` |
| `dbe87fbd7` | P6-5 fixup — StallConfig builder methods (clippy `field_reassign_with_default`) |
| `a3bd091` | `build_fallback_llm` case-insensitive self-reference (`eq_ignore_ascii_case`) |

Three private boot-time builders in `src/bin/aleph-server/commands/start/orchestrator_init.rs` (`build_guardrail_registry`, `build_fallback_llm`, `build_stability_triple`) translate three opt-in `aleph.toml` sections into the five live `Option<T>` fields on `AgentHarnessRunner`. Missing section preserves Stage 7 ship behavior exactly. R10 holds at 1520 lines on `src/harness/agent.rs`.

See `docs/superpowers/specs/2026-05-08-phase6-config-wiring-design.md` for design and `docs/superpowers/specs/2026-05-08-phase6-config-wiring-plan.md` for the task-by-task implementation plan.
