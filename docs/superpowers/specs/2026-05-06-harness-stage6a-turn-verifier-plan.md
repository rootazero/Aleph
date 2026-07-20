# Stage 6a Plan — TurnVerifier Trait + StopHookVerifier 迁移 + ToolLoopVerifier

**Status**: 📝 Draft (2026-05-06)
**Master spec**: [`2026-05-05-harness-12-module-roadmap-design.md`](2026-05-05-harness-12-module-roadmap-design.md) § Stage 6
**Module**: #10 Verification & Feedback Loop（前半）
**Risk class**: high（按 master spec 分级；6a 取 6 中较低风险的子集）
**Depends on**: Stage 1（ErrorClass）、Stage 4（ChainContext，仅 deps 字段相邻）、Stage 5（callsite 模板）

---

## 1. 目标

收口 master roadmap 表 1.4 中标注的 P1 修复：

| Fix | 原 priority | 收纳 stage |
|-----|-------------|-----------|
| Stop hook 仅在模型停手触发（tool_use 死循环不覆盖） | P1 | Stage 6（行为扩展） |

通过引入 `TurnVerifier` trait 把"仅 Done 前生效"的 stop hook 升级为"每轮 think→act 之间生效"的统一回路，并新增 `ToolLoopVerifier` 覆盖 tool_use 死循环（无 thinking 文本、纯重复 tool call）这一现状盲区。

**6a 范围内**:
1. 引入 `TurnVerifier` / `VerifierVerdict` / `VerifierChain` 三类抽象。
2. 把现有 stop hook 包装成 `StopHookVerifier`，行为完全保持。
3. 新增 `ToolLoopVerifier`：连续 N 轮（默认 5）相同 tool 调用且无 thinking 文本时 Veto。
4. agent.rs 主循环 callsite 由 `evaluate_stop_hooks` 替换为 `run_verifiers`，**单一 callsite 同时覆盖 mid-turn 与 pre-stop**。
5. `HarnessDeps.stop_hooks` 字段更名为 `verifier_chain`，全部构造点同步迁移。

**6a 不在范围**（明文 defer 至独立 spec）:
- `JudgeVerifier`（subagent 二次评估）
- `ComputationalVerifier`（say-do mismatch trace 检测）
- 6b 仍处于待审：与 `src/verification/mod.rs` 顶部 R8/R10 redline 注释（"no Rust-level verifier, judge, or critic is introduced"）存在张力；任何 6b 的实施必须先在 verification/mod.rs 显式撤销该 redline。

---

## 2. 架构

```text
┌──────────────────────────────────────────────────────────────────┐
│ src/verification/                                                │
│   ├─ mod.rs                       (导出新增 trait + 类型)         │
│   ├─ stop_hooks.rs                (保留 — ShellStopHook 等)       │
│   ├─ turn_verifier.rs       NEW   (trait / Verdict / Chain / Ctx) │
│   ├─ stop_hook_verifier.rs  NEW   (StopHookVerifier impl)         │
│   ├─ tool_loop_verifier.rs  NEW   (ToolLoopVerifier impl)         │
│   └─ tests/                 NEW                                   │
│       ├─ turn_verifier.rs         (Chain semantics)               │
│       ├─ stop_hook_verifier.rs    (迁移行为不退化)                │
│       └─ tool_loop_verifier.rs    (死循环检测正负样本)            │
│                                                                  │
│ src/harness/                                                     │
│   ├─ deps.rs                      (字段 stop_hooks → verifier_chain)│
│   ├─ agent.rs                     (callsite 替换 + 死循环缓冲区)   │
│   └─ tests/verifier.rs       NEW  (端到端 harness 集成)           │
└──────────────────────────────────────────────────────────────────┘
```

### 2.1 Trait surface

```rust
// src/verification/turn_verifier.rs
#[async_trait]
pub trait TurnVerifier: Send + Sync {
    fn name(&self) -> &str;
    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        cancel: &CancellationToken,
    ) -> VerifierVerdict;
}

pub enum VerifierVerdict {
    Continue,
    Veto { reason: String, class: ErrorClass },
}

pub struct TurnVerifyContext<'a> {
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub final_text: Option<&'a str>,
    pub recent_tool_calls: &'a [ToolCallSummary],   // 最近 N 次 attempted call（含本轮）
    pub stop_reason: Option<&'a str>,               // None = mid-turn, Some = pre-stop
}

#[derive(Clone)]
pub struct ToolCallSummary {
    pub name: String,
    pub args_hash: u64,
}

pub struct VerifierChain { /* Vec<Arc<dyn TurnVerifier>> + AtomicBool kill-switch */ }
```

设计点：
- **首个 Veto 短路**（与 GuardrailRegistry 一致）。
- **`Arc` 共享**（subagent 路径透传，类似 guardrails）。
- **AtomicBool kill-switch**（运维兜底，与 guardrails 同语义）。
- **`ErrorClass` 复用 Stage 1**：Veto 携带分类便于上层（未来 cap 改为按 class 计数）。

### 2.2 callsite 拓扑

`agent.rs::run_turn_internal` 当前形态（after Think 完成）：

```
if response.tool_calls.is_empty() {
    evaluate_stop_hooks(...) → Some(reason): Veto path / None: Done
} else {
    proceed to act()
}
```

替换后：

```
let stop_reason = if response.tool_calls.is_empty() { Some("end_turn") } else { None };
push_to_history(&response.tool_calls);                  // 死循环缓冲
let verdict = run_verifiers(stop_reason, &history).await;
match verdict {
    VerifierVerdict::Veto { reason, .. } => {
        inject [verifier veto] feedback; return (Continue, 0, true);
    }
    VerifierVerdict::Continue => {
        if tool_calls.is_empty() { Done } else { act() }
    }
}
```

**单一 callsite 同时覆盖**：
- 模型欲停手（`tool_calls.is_empty()`）→ `stop_reason: Some("end_turn")` → StopHookVerifier 触发。
- 模型死循环工具（`tool_calls.is_empty() == false`）→ `stop_reason: None` → ToolLoopVerifier 触发。

### 2.3 `HarnessDeps` 字段迁移

```diff
-    pub stop_hooks: Option<Arc<Vec<Arc<dyn StopHookHandler>>>>,
+    pub verifier_chain: Option<Arc<VerifierChain>>,
```

`StopHookConfig` → `Arc<Vec<Arc<dyn StopHookHandler>>>`（`build_from_config`，保留）→ wiring 层包成 `StopHookVerifier` 并加入 `VerifierChain`。

### 2.4 死循环缓冲

`agent.rs::run` 持有 `VecDeque<ToolCallSummary>` cap=8（足够 threshold=5 + 余量）。每轮 Think 完成后 `push_back`，超出则 `pop_front`。`ToolLoopVerifier::repeat_threshold` 默认 5，可在生产 wiring 处覆盖。

`args_hash`：`DefaultHasher` 喂入 `serde_json::to_vec(&args).unwrap_or_default()`，O(arg_size) 一次性、零分配（hash 状态 8 字节）。

---

## 3. Acceptance Criteria

### 功能
- ✅ 现有 stop hook 行为完全保留（`MAX_STOP_HOOK_VETOS=10` cap、shell hook exit code 语义、并发执行、cancel 传递）。
- ✅ 模型连续 N 次（N=5 默认）相同 tool 调用 + 无 thinking 文本 → ToolLoopVerifier 在第 N 轮产生 Veto，注入 `[verifier veto] tool '<name>' invoked 5 consecutive times with no thinking text`。
- ✅ `VerifierChain::empty()` 行为等价 P0 rescue baseline（无 verifier 即跳过整个 callsite）—— 满足 master spec rollback 要求。

### 不破坏
- ✅ P0 rescue 引入的 `consecutive_failure_cap`、watchdog timeout、turn_timeout 全部保持。
- ✅ Stage 5b ToolCallGuardrail callsite 不受影响（Veto 在 act() 之前触发，guardrail 在 act() 之内）。
- ✅ Stage 4 ChainContext 通过 subagent_spawner 透传 `verifier_chain`（默认 `None`，与 guardrails 同模式）。

### 测试
- ≥3 个 unit：Chain 短路 / Chain 空 / kill-switch
- ≥2 个 unit：StopHookVerifier 在 stop_reason=None 时跳过 / 在 Some 时复现 P0 行为
- ≥2 个 unit：ToolLoopVerifier 阈值边界 / "有 thinking 文本则不触发"
- ≥2 个 harness 集成：模拟死循环 → Veto 注入 / 模拟正常 stop hook → 现有行为
- ≥1 个并发 hammer（与 guardrails 测一致风格）：Chain disable_all/enable_all 切换下不 panic

### 性能
- VerifierChain dispatch 开销 ≤ 1 个 `Vec::iter()` + Arc deref + AtomicBool::load（与 guardrails 同量级）。
- `args_hash` 仅在每轮 Think 后计算一次（amortized O(arg_size)）。
- 无 verifier 注册时整路径 short-circuit（`Option::is_none()` 早退）。

### R10 行号红线
- agent.rs **必须 ≤ 1500 行**（当前正好 1500，零余量）。
  - 删除 `evaluate_stop_hooks` helper（约 40 行）。
  - 新增 `run_verifiers` helper + 死循环缓冲（合计 ≤ 35 行）。
  - 净结果：≤ 1495 行。
- `harness/` 目录总行数变化 ≤ +50 行（master spec A1 redline 宽限）。

---

## 4. Tasks（按提交粒度分解，1 plan + 3 feat/test + 1 docs ship）

### Commit 1 — `docs: Stage 6a plan`（本文档）
- 写入 `docs/superpowers/specs/2026-05-06-harness-stage6a-turn-verifier-plan.md`。

### Commit 2 — `feat(verification): TurnVerifier trait + StopHookVerifier + ToolLoopVerifier skeletons`
- 新建 `src/verification/turn_verifier.rs`（trait + Verdict + Chain + Context + ToolCallSummary）。
- 新建 `src/verification/stop_hook_verifier.rs`（包装 stop_hooks，stop_reason guard）。
- 新建 `src/verification/tool_loop_verifier.rs`（阈值检测）。
- `src/verification/mod.rs` 导出新类型，**保留** stop_hooks 模块（`ShellStopHook` / `build_from_config` 不动）。
- `src/lib.rs`（如需）re-export。
- 不动 harness。
- ✅ 验收：`cargo check -p alephcore` 通过。

### Commit 3 — `feat(harness): wire VerifierChain into HarnessDeps + agent.rs`
- `HarnessDeps.stop_hooks` → `verifier_chain: Option<Arc<VerifierChain>>`。
- 全部 ~21 个 `HarnessDeps { ... }` 构造点更新（perl 批量 + 手工核对）：
  - 生产：`src/agents/subagent_spawner.rs`、`src/orchestrator/harness_bridge.rs`、`src/agents/runtime.rs`
  - 测试：`src/harness/agent.rs`（3 处）、`src/harness/tests/{act,chain,driver,guardrails,stability,task10_wiring,think}.rs`、`tests/harness_run_e2e.rs`
- `agent.rs`：删除 `evaluate_stop_hooks`，新增 `run_verifiers` + ring buffer。
- `agent.rs`：callsite 替换（line ~366-410）。
- 重命名 `MAX_STOP_HOOK_VETOS` → `MAX_VERIFIER_VETOS`、`stop_hook_veto_count` → `verifier_veto_count`、 `[stop-hook veto]` → `[verifier veto]`。
- 生产 wiring（subagent_spawner / runtime / harness_bridge）：从 `StopHookConfig` 构造 `StopHookVerifier`，附 `ToolLoopVerifier::default()`，组成 `VerifierChain`。
- ✅ 验收：`cargo check -p alephcore`、`cargo test -p alephcore --lib harness::` 全绿。

### Commit 4 — `test(verification,harness): TurnVerifier unit + harness integration + concurrency hammer`
- `src/verification/tests/{turn_verifier,stop_hook_verifier,tool_loop_verifier}.rs`：单元测试集（≥7 个）。
- `src/harness/tests/verifier.rs`：harness 端到端集成（≥2 个：模拟死循环 / stop hook 行为不退化）。
- 并发 hammer（沿 5b `concurrent_evaluate_vs_disable_all_is_consistent` 模板）。
- ✅ 验收：`cargo test -p alephcore --lib verification:: harness::` 全绿。

### Commit 5 — `docs: Stage 6a shipped — flip master spec status + CHANGELOG`
- master spec § Stage 6 状态 `🟡 Pending` → `🟡 6a Shipped on 2026-05-06 · 6b Pending`。
- CHANGELOG `[Unreleased] § Added` 顶部追加 6a 条目。
- 模块 #10 在 master spec § 0.4 进度表标 `🟡 部分（6a）`。

---

## 5. CHANGELOG 草案（中文 entry 由 release 时改 English；plan 内保持中文方便审阅）

```markdown
### Added
- **Harness Stage 6a** — Verification turn-level seam: `TurnVerifier` trait
  + `VerifierChain` registry land in `src/verification/`. `StopHookVerifier`
  migrates the existing pre-stop hook behavior 1:1; `ToolLoopVerifier`
  closes the master roadmap P1 gap by detecting N consecutive identical
  tool calls with no thinking text (default threshold = 5). Single
  callsite in `agent.rs::run_turn_internal` now covers both pre-stop
  and mid-turn checks, replacing the legacy `evaluate_stop_hooks`
  helper. `HarnessDeps.stop_hooks` retired in favour of
  `HarnessDeps.verifier_chain`. `JudgeVerifier` /
  `ComputationalVerifier` (Stage 6b) deferred pending explicit redline
  waiver in `src/verification/mod.rs`.
```

---

## 6. Verification（自验脚本）

```bash
# Trait 表面
cargo test -p alephcore --lib verification::tests::turn_verifier
cargo test -p alephcore --lib verification::tests::stop_hook_verifier
cargo test -p alephcore --lib verification::tests::tool_loop_verifier

# 集成
cargo test -p alephcore --lib harness::tests::verifier

# 行数红线（手工或 just）
wc -l src/harness/agent.rs   # 必须 ≤ 1500
wc -l src/harness/*.rs       # 总和不超过基线 + 50

# 全量回归
cargo test -p alephcore --lib
```

---

## 7. Out of Scope（明文 defer）

1. **`JudgeVerifier`**（subagent 二次评估）— 与 `src/verification/mod.rs` 顶部 R8/R10 redline 直接冲突。开 6b plan 前必须先在 verification/mod.rs 撤销该注释，并取得用户显式 sign-off。
2. **`ComputationalVerifier`**（say-do mismatch trace 自动检测）— 同上 redline 张力；交由 6b plan 决定是否实施。
3. `MAX_VERIFIER_VETOS` 由按 count 改为按 ErrorClass 加权计数 — Stage 7 init audit 中评估。
4. `recent_tool_calls` ring buffer 持久化（跨 turn 的 trace 保存）— Stage 7。

---

## 8. Rollback Plan（master spec 要求 high-risk 必填）

**触发条件**：6a 上线后任一发生：
- 回归 P0 rescue 行为（watchdog / consecutive_failure_cap / turn_timeout 任一退化）
- ToolLoopVerifier 在合法重复调用（如 read_file 同路径多次合理重读）误触发率 > 1%

**回滚步骤**（4 commits 单笔 revert 即可）：
1. `git revert <commit-5> <commit-4> <commit-3> <commit-2>`（plan commit 保留作为历史档案）
2. `cargo test -p alephcore --lib` 应自然回到 5b shipped 基线。

`VerifierChain::empty()` 的存在保证：即使 commit 3 wiring 部分回滚失败，运行时把 `verifier_chain` 设为 `None` 即可短路全部新代码路径。

---

## 9. R10 合规自检（薄 Harness 哲学）

| 5 个"不"检查 | 6a 状态 |
|----------|--------|
| ❌ 不判断意图分类 | ✅ ToolLoopVerifier 是结构性看门狗（重复检测），非意图判断 |
| ❌ 不做工具过滤 / 相关性评分 | ✅ 不涉及 |
| ❌ 不做完成度判断（除模型显式 stop） | ⚠️ ToolLoopVerifier 在 mid-turn 中断 — 但属于 master spec line 104 明文授权的 P1"行为扩展"（看门狗式安全网，与 watchdog timer 同性质） |
| ❌ 不做内容审查 / 安全打分 | ✅ Stage 5 已覆盖；6a 不涉及 |
| ❌ 不做错误恢复策略选择 | ✅ Veto 仅注入反馈消息，恢复策略仍由模型自行决定 |

**Future-proof 测试**：模型升级（更强、更不易死循环）后，ToolLoopVerifier 自然 Continue（never trigger），StopHookVerifier 仍按用户配置触发。无需改 agent.rs。✅ 通过。

---

## 10. 提交链落地命令（参考 5b 节奏）

```text
git add docs/superpowers/specs/2026-05-06-harness-stage6a-turn-verifier-plan.md
git commit -m "docs: Stage 6a plan — TurnVerifier trait + StopHookVerifier + ToolLoopVerifier"

# Commit 2 — verification 模块新文件
git add src/verification/
git commit -m "feat(verification): TurnVerifier trait + StopHookVerifier + ToolLoopVerifier"

# Commit 3 — harness 接入
git add src/harness/ src/agents/ src/orchestrator/ tests/harness_run_e2e.rs
git commit -m "feat(harness): wire VerifierChain into HarnessDeps + agent.rs"

# Commit 4 — 测试
git add src/verification/tests/ src/harness/tests/verifier.rs
git commit -m "test(verification,harness): TurnVerifier unit + integration + concurrency hammer"

# Commit 5 — docs ship
git add CHANGELOG.md docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md
git commit -m "docs: Stage 6a shipped — TurnVerifier seam wired, 6b deferred"
```

---

**结束语**：6a 是 master spec § Stage 6 的安全子集，落地后即关闭路线图表 1.4 的 P1 gap。6b 留待显式 redline 决策。
