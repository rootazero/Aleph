# MoA 持续咨询第二轮优化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 完成 MoA 持续咨询第二轮：修 8 个缺陷（B1-B8）、接 3 处线（W1-W3）、补 3 项增强（E1/E3/E4）、重构打磨（R1-R3）与测试补齐（R4），收尾运行时 QA。

**Architecture:** MoA 架构本质不变（虚拟 `AiProvider` 门面 `MoaProvider`，runner Step 3-MoA 接线）。P1 先拆 `provider.rs::process()` 出 `fan_out.rs`，P2 修缺陷（事件 schema 自由改——第一轮未推送未发版，零线上消费者），P3 接线（TUI/回放/core tools），P4 增强（prompt-cache 断点 / `[image]` 占位 / 选择器集成走既有 `providers.catalog` + `chat.send model_override` 管道，零新 RPC），P5 测试文档 QA。

**Tech Stack:** Rust (tokio, serde, schemars) · Leptos/WASM panel · ratatui TUI · sqlite (rusqlite)

**Spec:** [docs/superpowers/specs/2026-07-05-moa-round2-optimization-design.md](../specs/2026-07-05-moa-round2-optimization-design.md)

## Global Constraints

- **R10**：`src/harness/` 只允许动 `trace.rs` 单文件，且只做既有 4 个 MoA 变体的**字段级**调整；不新增文件、不新增变体。
- **事件 schema 自由改**：第一轮 15 commits 只在本地 main（未推送未发版），MoA 事件 wire 字段无线上消费者，加字段不需要兼容层。
- **验证纪律**（用户系统负担约束，极度节制 cargo）：每任务只跑**定向**测试（`cargo test -p alephcore --lib <模块过滤器>`）；不跑全量测试套件；整计划收口时跑一次 `cargo check --lib`。
- **提交规范**：`<scope>: <description>`，英文，一任务一提交（或一任务两提交：test + impl 亦可，见各任务）。
- **锁卫生**：一切 `Mutex`/`RwLock` 用 `.lock()/.read()/.write().unwrap_or_else(|e| e.into_inner())`（项目 P7 规范）。
- **UTF-8 安全**：字符串切片只用 `char_indices()` 字节偏移或 `chars()`，禁止 `&s[..n]` 直切。
- **注释英文，回复中文**；无新第三方依赖；全栈 serde。
- **单分支开发**：直接在 main 工作（项目分支策略）。若使用 worktree 执行，遵守「会话内只合并不删除」铁律。

## 任务依赖图

```
Task 1 (R1 拆分) ──► Task 3 (B1/B2/B4 schema+发射) ──► Task 4 (B3 turn-trace)
                └──► Task 12 (E1 cache 断点)          └──► Task 10 (呈现层) ──► Task 11 (TUI)
Task 2 (R2/R3) 独立（在 Task 1 后做，避免冲突）
Task 5 (B5) / 6 (B6) / 7 (B7) / 8 (B8) / 9 (W1) / 13 (E4) 相互独立
Task 14 (E3-core) ──► Task 15 (E3 catalog+gateway)
Task 16 (CRUD happy-path 测试) 独立
Task 17 (docs + 收口门 + 运行时 QA) 最后
```

---

### Task 1: R1 — 拆分 `process()`：提炼 `fan_out.rs`（纯重构，行为零变化）

**Files:**
- Create: `src/providers/moa/fan_out.rs`
- Modify: `src/providers/moa/provider.rs`（`process()` 缩为编排骨架；`spend_event`/fan-out 闭包/事件发射块迁出）
- Modify: `src/providers/moa/mod.rs`（+`pub(crate) mod fan_out;`）

**Interfaces:**
- Consumes: `AdvisorSlot`（provider.rs，改为 `pub(crate)` 字段可见或移入 fan_out.rs）、`AdvisorOutcome`（prompts.rs）、`LoopTraceEvent`、`TraceSink`
- Produces（后续任务依赖的确切签名）:
  - `pub(crate) struct AdvisorResult { pub outcome: AdvisorOutcome, pub usage: Option<TokenUsage>, pub error: Option<String> }`
  - `pub(crate) async fn run_fan_out(advisors: &[AdvisorSlot], view: &[UnifiedMessage], timeout: Duration, temperature: Option<f32>, max_tokens: Option<u32>) -> Vec<AdvisorResult>`
  - `pub(crate) fn emit_fanout_events(sink: &Option<Arc<dyn TraceSink>>, advisors: &[AdvisorSlot], results: &[AdvisorResult], aggregator_label: &str)`（Task 3 将改此函数签名与内部）

**决策记录（写进代码注释）**：`display_name` 字段**保留不删**——`fn name(&self) -> &str` 要求返回借用，派生形式无法返回 `&str`；spec §7 的该项清理经评估否决。

- [ ] **Step 1: 创建 `src/providers/moa/fan_out.rs`，把 fan-out 闭包与发射块原样搬入**

```rust
//! Advisor fan-out: parallel consultation with per-advisor timeout,
//! fail-soft degradation, and trace-event emission. Extracted from
//! `provider.rs::process()` (round-2 refactor R1) so the facade stays an
//! orchestration skeleton.

use std::time::Duration;

use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::providers::adapter::{RequestPayload, TokenUsage};
use crate::providers::message::UnifiedMessage;
use crate::sync_primitives::Arc;

use super::prompts::{AdvisorOutcome, ADVISOR_SYSTEM_PROMPT};
use super::provider::AdvisorSlot;

/// One advisor's full fan-out result: display outcome + accounting + the
/// structural error (None on success). The error channel feeds the
/// `MoaAdvisor.error` trace field (round-2 B2).
pub(crate) struct AdvisorResult {
    pub outcome: AdvisorOutcome,
    pub usage: Option<TokenUsage>,
    pub error: Option<String>,
}

/// Parallel fan-out, per-advisor timeout, fail-soft. Result order is stable
/// (preset slot order). Never fails the turn: an advisor error/timeout
/// degrades to a labelled note in `outcome.text`.
pub(crate) async fn run_fan_out(
    advisors: &[AdvisorSlot],
    view: &[UnifiedMessage],
    timeout: Duration,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> Vec<AdvisorResult> {
    let futures = advisors.iter().map(|slot| async move {
        let advisor_payload = RequestPayload::new(view)
            .with_system(Some(ADVISOR_SYSTEM_PROMPT))
            .with_temperature(temperature)
            .with_max_tokens(max_tokens);
        match tokio::time::timeout(timeout, slot.chain.process(advisor_payload)).await {
            Ok(Ok(resp)) => {
                let text = resp
                    .text
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "(empty response)".to_string());
                (text, resp.usage, None::<String>)
            }
            Ok(Err(e)) => (format!("[failed: {e}]"), None, Some(e.to_string())),
            Err(_) => (
                format!("[timeout after {}s]", timeout.as_secs()),
                None,
                Some(format!("timeout after {}s", timeout.as_secs())),
            ),
        }
    });
    let results = futures::future::join_all(futures).await;

    results
        .into_iter()
        .enumerate()
        .map(|(idx, (text, usage, error))| AdvisorResult {
            outcome: AdvisorOutcome {
                label: advisors[idx].label.clone(),
                text,
            },
            usage,
            error,
        })
        .collect()
}
```

注意与原代码的两处刻意差异（均为 Task 3 铺路，行为不变）：(a) timeout 错误串从 `"timeout"` 改为 `"timeout after {N}s"`（更精确，该串此前被丢弃从未消费）；(b) 错误值不再被 `_err` 弃置而是存进 `AdvisorResult.error`（本任务还没有消费者，Task 3 接上）。

- [ ] **Step 2: 在 provider.rs 中改造 `process()` 为编排骨架**

`AdvisorSlot` 加 `pub(crate)` 字段可见性（fan_out.rs 需要 `label`/`chain`）：

```rust
/// One resolved advisor: label + provider chain + identity for pricing.
pub(crate) struct AdvisorSlot {
    pub(crate) label: String,
    pub(crate) provider_key: String,
    pub(crate) model: String,
    pub(crate) chain: Arc<dyn AiProvider>,
}
```

`process()` 的 MISS 分支替换为（保持事件顺序与内容逐字节一致——advisor 事件 → aggregating → spend → turn-trace → 写缓存）：

```rust
            let outcomes: Vec<AdvisorOutcome> = if let Some(hit) = cached {
                hit
            } else if self.advisors.is_empty() {
                Vec::new()
            } else {
                // 3. Parallel fan-out (extracted: fan_out.rs).
                let results = super::fan_out::run_fan_out(
                    &self.advisors,
                    &view,
                    self.advisor_timeout,
                    self.advisor_temperature,
                    self.advisor_max_tokens,
                )
                .await;

                let usages: Vec<(usize, TokenUsage)> = results
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, r)| r.usage.clone().map(|u| (idx, u)))
                    .collect();
                let outcomes: Vec<AdvisorOutcome> =
                    results.iter().map(|r| r.outcome.clone()).collect();

                // 4. Display + accounting + heavy trace events (MISS only).
                let count = outcomes.len();
                for (idx, o) in outcomes.iter().enumerate() {
                    self.emit(LoopTraceEvent::MoaAdvisor {
                        index: idx + 1,
                        count,
                        label: o.label.clone(),
                        text: o.text.clone(),
                    });
                }
                self.emit(LoopTraceEvent::MoaAggregating {
                    aggregator: self.aggregator_label.clone(),
                    advisor_count: count,
                });
                if !usages.is_empty() {
                    let spend = self.spend_event(&usages);
                    self.emit(spend);
                }
                if self.save_traces {
                    self.emit(LoopTraceEvent::MoaTurnTrace {
                        preset: self.preset_name.clone(),
                        payload: json!({
                            "aggregator": self.aggregator_label,
                            "view_signature": sig,
                            "advisors": outcomes
                                .iter()
                                .map(|o| json!({ "label": o.label, "output": o.text }))
                                .collect::<Vec<_>>(),
                        }),
                    });
                }

                *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(AdvisorCache {
                    signature: sig,
                    outcomes: outcomes.clone(),
                });
                outcomes
            };
```

`TokenUsage` 若无 `Clone`，改用 `results.into_iter()` 消费式拆解（先收集 outcomes 克隆，再取 usage move）——以实际编译为准，两种写法都保持语义。删除 provider.rs 中不再使用的 `ADVISOR_SYSTEM_PROMPT` import（fan_out.rs 接管）。

- [ ] **Step 3: mod.rs 注册模块**

```rust
pub(crate) mod advisory_view;
pub mod config_handle;
pub(crate) mod fan_out;
pub(crate) mod prompts;
pub mod provider;
```

- [ ] **Step 4: 跑既有定向测试确认行为零变化**

Run: `cargo test -p alephcore --lib providers::moa`
Expected: 既有 8 个 provider 测试 + advisory_view/prompts 测试全 PASS（`advisors_run_in_parallel_and_aggregator_answers`、`advisor_failure_and_timeout_degrade_to_notes`、`per_iteration_cache_dedupes_identical_state`、`user_turn_cache_survives_state_growth`、`no_advisors_means_bare_aggregator` 等）。

注意：`advisor_failure_and_timeout_degrade_to_notes` 断言的是 outcome 文本 `[timeout after 1s]`——该文本未变，仅内部 error 串变化，测试应仍绿。

- [ ] **Step 5: Commit**

```bash
git add src/providers/moa/
git commit -m "providers: extract MoA fan-out into fan_out.rs (pure refactor, R1)"
```

---

### Task 2: R2+R3 — 热路径单遍化 + 缓存不变量守卫

**Files:**
- Modify: `src/providers/moa/advisory_view.rs`（`view_signature` 免中间分配；`truncate_tool_result` 免 String collect）
- Modify: `src/providers/moa/provider.rs`（缓存不变量文档注释 + debug_assert）

**Interfaces:**
- Produces: `view_signature(&[UnifiedMessage]) -> u64` 签名不变但**哈希值允许改变**（内部哈希顺序变化）——它只做进程内缓存去重，从不持久化，改变哈希值无兼容影响。

- [ ] **Step 1: 先加守护测试（锁"签名稳定性 + 变化敏感性"语义，不锁具体值）**

在 `advisory_view.rs` tests 模块追加：

```rust
    #[test]
    fn signature_ignores_cache_control_marks() {
        let mut a = vec![UnifiedMessage::user("hello")];
        let sig_before = view_signature(&a);
        // Simulate a cache_control mark on the text block (E1 will do this
        // in place) — the signature must not change.
        if let Some(UnifiedMessage::User { content }) = a.last_mut() {
            if let Some(ContentBlock::Text { cache_control, .. }) = content.last_mut() {
                *cache_control =
                    Some(crate::providers::message::CacheControl::Ephemeral { ttl: None });
            }
        }
        assert_eq!(sig_before, view_signature(&a));
    }
```

Run: `cargo test -p alephcore --lib providers::moa::advisory_view`
Expected: PASS（现实现哈希 `text_of` 输出，本就忽略 cache_control——测试先锁住该语义，E1 依赖它）。

- [ ] **Step 2: 重写 `view_signature` 免中间 String**

```rust
/// Stable signature of the advisory view — the fan-out cache key. Uses the
/// std hasher (cache dedup only, not security). Hashes text parts directly
/// (no intermediate join allocation); deliberately ignores cache_control
/// marks so E1's in-place breakpoint marking never perturbs the cache key.
pub(crate) fn view_signature(view: &[UnifiedMessage]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for msg in view {
        let (role, content) = match msg {
            UnifiedMessage::User { content } => ("user", content),
            UnifiedMessage::Assistant { content } => ("assistant", content),
            UnifiedMessage::ToolResult { content, .. } => ("tool", content),
        };
        role.hash(&mut hasher);
        for block in content {
            match block {
                ContentBlock::Text { text, .. } => {
                    if !text.is_empty() {
                        text.hash(&mut hasher);
                    }
                }
                ContentBlock::Json { value } => value.to_string().hash(&mut hasher),
                _ => {}
            }
        }
    }
    hasher.finish()
}
```

注意：`signature_changes_with_new_tool_result_and_is_stable` 测试断言相对性质（同输入同值/不同输入不同值），不锁具体 u64 值——重写后仍绿。

- [ ] **Step 3: 重写 `truncate_tool_result` 借用切片替代双 String collect**

```rust
/// Head+tail preview with a `[... N chars omitted ...]` marker. UTF-8 safe:
/// slices at char boundaries found via char_indices (no per-char String
/// collection; one full count pass + two partial boundary scans).
pub(crate) fn truncate_tool_result(text: &str, budget: usize) -> String {
    let total = text.chars().count();
    if total <= budget {
        return text.to_string();
    }
    let half = budget / 2;
    // Byte offset AFTER the half-th char (head end boundary).
    let head_end = text
        .char_indices()
        .nth(half)
        .map_or(text.len(), |(i, _)| i);
    // Byte offset of the (total-half)-th char (tail start boundary).
    let tail_start = text
        .char_indices()
        .nth(total - half)
        .map_or(text.len(), |(i, _)| i);
    let omitted = total - 2 * half;
    format!(
        "{}\n[... {omitted} chars omitted ...]\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}
```

- [ ] **Step 4: provider.rs 缓存不变量文档 + debug_assert**

在 `MoaProvider` 的 `cache` 字段上方补注释，并在 `process()` 缓存写回前加守卫：

```rust
    /// Fan-out cache. INVARIANT: a MoaProvider is run-scoped and the Think
    /// loop drives `process()` strictly sequentially, so the read (cache
    /// decision) and write (post-fan-out) never race. If an instance were
    /// ever shared across concurrent calls, two MISSes could both fan out
    /// (duplicate advisor spend, last-writer-wins) — the check-then-act gap
    /// is deliberate, not an oversight (round-2 R3).
    cache: Mutex<Option<AdvisorCache>>,
```

- [ ] **Step 5: 跑定向测试**

Run: `cargo test -p alephcore --lib providers::moa`
Expected: 全 PASS（含新 `signature_ignores_cache_control_marks`、既有 `truncation_is_head_tail_and_utf8_safe`——汉字 5000 重复用例正是多字节边界回归网）。

- [ ] **Step 6: Commit**

```bash
git add src/providers/moa/
git commit -m "providers: single-pass MoA signature/truncation + cache invariant guard (R2/R3)"
```

---

### Task 3: B1+B2+B4 — 事件 schema 定稿 + 发射修正 + RecordingSink 锁定测试

**Files:**
- Modify: `src/harness/trace.rs:118-154`（MoaAdvisor +`error`；MoaAggregating +`cached`；MoaAdvisorSpend +`billed_count`）与 `:353-397`（From 镜像 arms）
- Modify: `shared/protocol/src/events.rs:378-445`（协议侧同步）
- Modify: `src/providers/moa/provider.rs`（发射逻辑：spend 双计数、HIT 补发 cached 聚合事件）
- Modify: `src/providers/moa/fan_out.rs`（若发射块在此——按 Task 1 实况）
- Test: `src/providers/moa/provider.rs` tests 模块（RecordingSink）+ `shared/protocol/src/events.rs` tests（wire 字段名锁）

**Interfaces:**
- Produces（定稿的事件 schema，后续任务与面板/TUI 消费）:
  - `MoaAdvisor { index: usize, count: usize, label: String, text: String, error: Option<String> }` — `count == 0` 保留语义：**MoA 激活失败警告**（Task 5 使用；index 亦为 0）
  - `MoaAggregating { aggregator: String, advisor_count: usize, cached: bool }`
  - `MoaAdvisorSpend { advisor_count: usize, billed_count: usize, input_tokens: u32, output_tokens: u32, cost_usd: Option<f64> }` — `advisor_count`=本轮咨询数，`billed_count`=返回用量数
  - `spend_event` 新签名：`fn spend_event(&self, consulted: usize, usages: &[(usize, TokenUsage)]) -> LoopTraceEvent`

- [ ] **Step 1: 写失败测试——RecordingSink 捕获 + wire 语义断言**

在 `provider.rs` tests 模块加（helper 置于 `make_provider` 旁）：

```rust
    struct RecordingSink(std::sync::Mutex<Vec<LoopTraceEvent>>);
    impl RecordingSink {
        fn new() -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(Vec::new())))
        }
        fn events(&self) -> Vec<LoopTraceEvent> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }
    impl crate::harness::TraceSink for RecordingSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(event.clone());
        }
    }
```

（若 `TraceSink` 还有 `flush`/`on_init_seam` 等**必需**方法——以编译报错为准——补空实现 `fn flush(&self) {}` / `fn on_init_seam(&self, _: &'static str, _: &'static str, _: bool) {}`。）

`make_provider` 增加 sink 参数版本：

```rust
    fn make_provider_sinked(
        advisors: Vec<(Arc<dyn AiProvider>, &str)>,
        aggregator: Arc<dyn AiProvider>,
        fanout: MoaFanout,
        sink: Arc<RecordingSink>,
    ) -> MoaProvider {
        let mut p = make_provider(advisors, aggregator, fanout, 5);
        p.sink = Some(sink);
        p
    }
```

三个测试（先写，先失败）：

```rust
    #[tokio::test]
    async fn events_carry_error_cached_and_billed_count() {
        let sink = RecordingSink::new();
        let ok = Arc::new(CountingProvider::new("advice"));
        let bad: Arc<dyn AiProvider> = Arc::new(
            crate::providers::mock::MockProvider::new()
                .with_error(crate::providers::mock::MockError::Network("boom".into())),
        );
        let agg = Arc::new(CountingProvider::new("final"));
        let p = make_provider_sinked(
            vec![(ok, "mock:ok"), (bad, "mock:bad")],
            agg,
            MoaFanout::PerIteration,
            sink.clone(),
        );
        p.process(RequestPayload::new(&user_msgs("go"))).await.unwrap();

        let events = sink.events();
        let advisors: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, LoopTraceEvent::MoaAdvisor { .. }))
            .collect();
        assert_eq!(advisors.len(), 2);
        // B2: success carries error=None, failure carries the structural reason.
        let LoopTraceEvent::MoaAdvisor { error: e0, .. } = advisors[0] else { panic!() };
        let LoopTraceEvent::MoaAdvisor { error: e1, .. } = advisors[1] else { panic!() };
        assert!(e0.is_none());
        assert!(e1.as_deref().is_some_and(|e| e.contains("boom")));
        // B4: MISS aggregating is cached=false.
        assert!(events.iter().any(|e| matches!(
            e,
            LoopTraceEvent::MoaAggregating { cached: false, .. }
        )));
        // B1: spend advisor_count = consulted (2), billed_count = with-usage (1).
        let spend = events
            .iter()
            .find(|e| matches!(e, LoopTraceEvent::MoaAdvisorSpend { .. }))
            .expect("spend event");
        let LoopTraceEvent::MoaAdvisorSpend { advisor_count, billed_count, .. } = spend else {
            panic!()
        };
        assert_eq!(*advisor_count, 2);
        assert_eq!(*billed_count, 1);
    }

    #[tokio::test]
    async fn cache_hit_emits_cached_aggregating_only() {
        let sink = RecordingSink::new();
        let adv = Arc::new(CountingProvider::new("advice"));
        let agg = Arc::new(CountingProvider::new("final"));
        let p = make_provider_sinked(
            vec![(adv, "mock:a")],
            agg,
            MoaFanout::UserTurn,
            sink.clone(),
        );
        let msgs = user_msgs("go");
        p.process(RequestPayload::new(&msgs)).await.unwrap();
        let miss_events = sink.events().len();
        p.process(RequestPayload::new(&msgs)).await.unwrap();
        let all = sink.events();
        let hit_events = &all[miss_events..];
        // HIT: exactly one new event — MoaAggregating { cached: true }; no
        // advisor re-emission, no spend re-emission.
        assert_eq!(hit_events.len(), 1);
        assert!(matches!(
            hit_events[0],
            LoopTraceEvent::MoaAggregating { cached: true, .. }
        ));
    }

    #[tokio::test]
    async fn save_traces_gate_controls_turn_trace() {
        let sink = RecordingSink::new();
        let adv = Arc::new(CountingProvider::new("advice"));
        let agg = Arc::new(CountingProvider::new("final"));
        let mut p = make_provider_sinked(
            vec![(adv, "mock:a")],
            agg,
            MoaFanout::PerIteration,
            sink.clone(),
        );
        p.save_traces = false;
        p.process(RequestPayload::new(&user_msgs("go"))).await.unwrap();
        assert!(!sink
            .events()
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::MoaTurnTrace { .. })));
        // Flip the gate on a fresh provider: trace fires.
        let sink2 = RecordingSink::new();
        let adv2 = Arc::new(CountingProvider::new("advice"));
        let agg2 = Arc::new(CountingProvider::new("final"));
        let mut p2 = make_provider_sinked(
            vec![(adv2, "mock:a")],
            agg2,
            MoaFanout::PerIteration,
            sink2.clone(),
        );
        p2.save_traces = true;
        p2.process(RequestPayload::new(&user_msgs("go"))).await.unwrap();
        assert!(sink2
            .events()
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::MoaTurnTrace { .. })));
    }
```

（`CountingProvider::new` 的实际构造签名以测试模块现状为准——它已存在，带 fixed text + 可选 delay + call counter。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib providers::moa::provider`
Expected: FAIL——`error`/`cached`/`billed_count` 字段不存在（编译错误即失败形态）。

- [ ] **Step 3: trace.rs 加字段（唯一允许触碰的 harness 文件）**

`src/harness/trace.rs` 三个变体改为：

```rust
    /// MoA advisor consultation result — one per advisor per fan-out
    /// (cache-MISS iterations only). Emitted by the `MoaProvider` facade
    /// through the run's TraceSink (MeteringProvider pattern — zero harness
    /// logic; this enum is the carrier, not the brain).
    /// `count == 0` is the activation-failure form: MoA could not engage for
    /// this run (preset missing / slot unresolvable) and `error` says why.
    MoaAdvisor {
        index: usize,
        count: usize,
        /// `provider:model` of the advisor slot.
        label: String,
        text: String,
        /// Structural failure reason (timeout / provider error); `None` on
        /// success. The guidance-block `[failed:]` note is the display twin.
        error: Option<String>,
    },
    /// MoA fan-out complete (or reused); the aggregator (acting model) is
    /// being called. `cached: true` = this iteration reuses the previous
    /// fan-out's advice (no advisor re-run, no new spend).
    MoaAggregating {
        aggregator: String,
        advisor_count: usize,
        cached: bool,
    },
    /// Summed advisor spend for one fan-out. Priced per-advisor at each
    /// advisor's OWN model rate; kept out of `ProviderResponse.usage` so the
    /// context gauge stays honest (spec §8). `advisor_count` = advisors
    /// consulted this fan-out; `billed_count` = advisors that returned usage
    /// (denominator for cost-per-advisor math).
    MoaAdvisorSpend {
        advisor_count: usize,
        billed_count: usize,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: Option<f64>,
    },
```

From 镜像 arms（`:353-397`）同步新字段（逐字段透传，模式与现有 arms 一致）。

- [ ] **Step 4: shared/protocol/src/events.rs 协议侧同步**

同样的三个变体加同样的字段（保持 doc comment 同步更新）。`kind()` 不变。

在 events.rs（或其既有 tests 模块）加 wire 字段名锁定测试：

```rust
    #[test]
    fn moa_event_wire_field_names_locked() {
        let advisor = AgentTraceEvent::MoaAdvisor {
            index: 1,
            count: 2,
            label: "p:m".into(),
            text: "t".into(),
            error: Some("boom".into()),
        };
        let v = serde_json::to_value(&advisor).unwrap();
        assert_eq!(v["kind"], "moa_advisor");
        for k in ["index", "count", "label", "text", "error"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }

        let agg = AgentTraceEvent::MoaAggregating {
            aggregator: "p:m".into(),
            advisor_count: 2,
            cached: true,
        };
        let v = serde_json::to_value(&agg).unwrap();
        assert_eq!(v["kind"], "moa_aggregating");
        for k in ["aggregator", "advisor_count", "cached"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }

        let spend = AgentTraceEvent::MoaAdvisorSpend {
            advisor_count: 2,
            billed_count: 1,
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: Some(0.01),
        };
        let v = serde_json::to_value(&spend).unwrap();
        assert_eq!(v["kind"], "moa_advisor_spend");
        for k in ["advisor_count", "billed_count", "input_tokens", "output_tokens", "cost_usd"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
    }
```

- [ ] **Step 5: 发射端修正（provider.rs / fan_out.rs）**

`spend_event` 新签名与实现（新增 `consulted` 参数 + `billed_count`）：

```rust
    /// Sum advisor usages + per-advisor own-rate pricing for the spend event.
    /// `consulted` = advisors fanned out; `usages` = those that returned usage.
    fn spend_event(&self, consulted: usize, usages: &[(usize, TokenUsage)]) -> LoopTraceEvent {
        // ... 累加循环不变 ...
        LoopTraceEvent::MoaAdvisorSpend {
            advisor_count: consulted,
            billed_count: usages.len(),
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
        }
    }
```

MISS 分支发射块：advisor 事件带 `error`（从 `AdvisorResult.error` 透传——B2 接活）、aggregating 带 `cached: false`、spend 调 `self.spend_event(count, &usages)`。

HIT 分支补发（B4）——`process()` 中 `let outcomes = if let Some(hit) = cached { ... }` 处改为：

```rust
            let outcomes: Vec<AdvisorOutcome> = if let Some(hit) = cached {
                // Cache HIT: the aggregator still runs on the reused advice —
                // emit the lightweight aggregating moment so multi-iteration
                // user_turn runs don't go dark on the panel (round-2 B4).
                if !hit.is_empty() {
                    self.emit(LoopTraceEvent::MoaAggregating {
                        aggregator: self.aggregator_label.clone(),
                        advisor_count: hit.len(),
                        cached: true,
                    });
                }
                hit
            } else if ...
```

同时修正 Task 1 引入的 `AdvisorResult` 消费：发射循环改为遍历 `results`（含 error），outcomes 单独收集。

- [ ] **Step 6: 修编译涟漪**

以下位置模式匹配需适配新字段（全部用 `..` 已兼容的不动，显式解构的补字段）：
- `src/harness/trace.rs` From 镜像 arms（Step 3 已做）
- `shared/protocol/src/trace_presentation.rs:451-484`——三个 arms 均用 `..`，编译无涟漪（呈现内容更新留给 Task 10）
- `interfaces/tui/src/tui/app/mod.rs:640-644`——`{ .. }` 通配，无涟漪
- 面板 events.rs 读 JSON，无编译涟漪

- [ ] **Step 7: 跑测试确认通过**

Run: `cargo test -p alephcore --lib providers::moa`
Expected: 全 PASS（3 个新测试 + 既有全部）。

Run: `cargo test --manifest-path shared/protocol/Cargo.toml moa_event_wire_field_names_locked`
Expected: PASS。

- [ ] **Step 8: Commit**

```bash
git add src/harness/trace.rs shared/protocol/src/events.rs src/providers/moa/
git commit -m "providers: finalize MoA event schema — advisor error, cached aggregating, billed_count (B1/B2/B4)"
```

---

### Task 4: B3 — `MoaTurnTrace` 移到聚合器返回后，补聚合器输出与状态

**Files:**
- Modify: `src/providers/moa/provider.rs`（`process()` 尾部重排）
- Test: 同文件 tests 模块

**Interfaces:**
- Produces: `MoaTurnTrace.payload` JSON 新键：`aggregator_output: String`、`aggregator_status: "ok" | "error: <摘要>"`（payload 是 opaque `Value`——**枚举零改动**，harness 不动）
- 语义：turn trace 仅在 **cache-MISS 且 save_traces=true** 的迭代发射（与 hermes 一致——HIT 迭代无新顾问 I/O）；聚合器失败时仍发射（顾问已实际计费，如实记录）；`process()` future 被取消时无 trace（可接受，per-advisor Metering 已记 spend）。

- [ ] **Step 1: 写失败测试**

```rust
    #[tokio::test]
    async fn turn_trace_carries_aggregator_output_and_fires_after_it() {
        let sink = RecordingSink::new();
        let adv = Arc::new(CountingProvider::new("advice"));
        let agg = Arc::new(CountingProvider::new("final answer"));
        let mut p = make_provider_sinked(
            vec![(adv, "mock:a")],
            agg,
            MoaFanout::PerIteration,
            sink.clone(),
        );
        p.save_traces = true;
        p.process(RequestPayload::new(&user_msgs("go"))).await.unwrap();
        let events = sink.events();
        let trace = events
            .iter()
            .find_map(|e| match e {
                LoopTraceEvent::MoaTurnTrace { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .expect("turn trace");
        assert_eq!(trace["aggregator_status"], "ok");
        assert_eq!(trace["aggregator_output"], "final answer");
        // Ordering: the turn trace must be the LAST event (after aggregating).
        assert!(matches!(
            events.last().unwrap(),
            LoopTraceEvent::MoaTurnTrace { .. }
        ));
    }

    #[tokio::test]
    async fn turn_trace_fires_with_error_status_when_aggregator_fails() {
        let sink = RecordingSink::new();
        let adv = Arc::new(CountingProvider::new("advice"));
        let agg: Arc<dyn AiProvider> = Arc::new(
            crate::providers::mock::MockProvider::new()
                .with_error(crate::providers::mock::MockError::Network("agg down".into())),
        );
        let mut p = make_provider_sinked(
            vec![(adv, "mock:a")],
            agg,
            MoaFanout::PerIteration,
            sink.clone(),
        );
        p.save_traces = true;
        let result = p.process(RequestPayload::new(&user_msgs("go"))).await;
        assert!(result.is_err());
        let events = sink.events();
        let trace = events
            .iter()
            .find_map(|e| match e {
                LoopTraceEvent::MoaTurnTrace { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .expect("turn trace fires even on aggregator error — advisors were billed");
        assert!(trace["aggregator_status"]
            .as_str()
            .unwrap()
            .starts_with("error:"));
    }
```

Run: `cargo test -p alephcore --lib providers::moa::provider::tests::turn_trace`
Expected: FAIL（trace 目前在聚合器调用前发射，无 aggregator_output/status 键）。

- [ ] **Step 2: 重排 `process()` 尾部**

MISS 分支不再直接发射 MoaTurnTrace，改为构造挂起上下文（在 `let outcomes = ...` 之前声明 `let mut pending_trace: Option<serde_json::Value> = None;`）：

```rust
                if self.save_traces {
                    pending_trace = Some(json!({
                        "aggregator": self.aggregator_label,
                        "view_signature": sig,
                        "advisors": outcomes
                            .iter()
                            .map(|o| json!({ "label": o.label, "output": o.text }))
                            .collect::<Vec<_>>(),
                    }));
                }
```

`process()` 最后（聚合器调用处）改为：

```rust
            let agg_result = self.aggregator.process(agg_payload).await;

            // Round-2 B3: the heavy turn trace fires AFTER the aggregator so
            // it records the full turn (hermes parity: advisor I/O + the
            // aggregator's actual output). Fires on error too — advisors ran
            // and were billed, the audit record must say so. A cancelled
            // future drops the pending trace (advisor spend is already on the
            // per-advisor MeteringProvider events).
            if let Some(mut payload) = pending_trace {
                let (output, status) = match &agg_result {
                    Ok(resp) => (
                        resp.text.clone().unwrap_or_default(),
                        "ok".to_string(),
                    ),
                    Err(e) => (String::new(), format!("error: {e}")),
                };
                payload["aggregator_output"] = json!(output);
                payload["aggregator_status"] = json!(status);
                self.emit(LoopTraceEvent::MoaTurnTrace {
                    preset: self.preset_name.clone(),
                    payload,
                });
            }
            agg_result
```

- [ ] **Step 3: 跑测试**

Run: `cargo test -p alephcore --lib providers::moa`
Expected: 全 PASS（含 Task 3 的 `save_traces_gate_controls_turn_trace`——其断言只查存在性，仍然成立）。

- [ ] **Step 4: Commit**

```bash
git add src/providers/moa/provider.rs
git commit -m "providers: emit MoaTurnTrace after aggregator with output+status (B3)"
```

---

### Task 5: B5 — one-shot 构建失败原子回填 + 激活失败警告事件

**Files:**
- Modify: `src/providers/session_moa_handle.rs`（+`restore_one_shot`）
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs:123-155`（Err 分支回填 + 发警告事件）
- Test: `src/providers/session_moa_handle.rs` tests

**Interfaces:**
- Consumes: Task 3 定稿的 `MoaAdvisor { count: 0, error: Some(..) }` 激活失败形态
- Produces: `pub fn restore_one_shot(session_key: &str, pref: SessionMoaPref)`——仅在槽位为空时插回（entry().or_insert，期间用户新写入的激活优先）

- [ ] **Step 1: 写失败测试（session_moa_handle.rs tests 模块追加）**

```rust
    #[test]
    fn restore_one_shot_refills_empty_slot_only() {
        let key = "test:moa:restore";
        set_session_moa(key, Some("deep".to_string()), true);
        let pref = take_for_run(key).unwrap();
        assert!(get_session_moa(key).is_none());
        // Build failed → restore: the one-shot survives for the next turn.
        restore_one_shot(key, pref.clone());
        assert_eq!(get_session_moa(key).unwrap().preset.as_deref(), Some("deep"));
        assert!(get_session_moa(key).unwrap().one_shot);
        // A newer activation written meanwhile must NOT be clobbered.
        set_session_moa(key, Some("newer".to_string()), false);
        restore_one_shot(key, pref);
        assert_eq!(get_session_moa(key).unwrap().preset.as_deref(), Some("newer"));
        clear_session_moa(key);
    }
```

Run: `cargo test -p alephcore --lib providers::session_moa_handle`
Expected: FAIL（`restore_one_shot` 不存在）。

- [ ] **Step 2: 实现 `restore_one_shot`**

```rust
/// Refill a consumed one-shot when run construction failed BEFORE MoA could
/// engage (preset deleted / slot unresolvable) — the user's single activation
/// must not burn on a run that never used it. Only fills an EMPTY slot: an
/// activation written meanwhile wins. Post-construction failures/cancels do
/// NOT restore (structural zero-leak, round-1 surpass item ④ unchanged).
pub fn restore_one_shot(session_key: &str, pref: SessionMoaPref) {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .entry(session_key.to_string())
        .or_insert(pref);
}
```

- [ ] **Step 3: runner_impl.rs Err 分支接线**

`runner_impl.rs` Step 3-MoA 的 `match ... take_for_run` 中 `Some(pref)` arm 的 `Err(reason)` 分支替换为：

```rust
                        Err(reason) => {
                            // Round-2 B5: a one-shot consumed by a build that
                            // never engaged MoA is refilled (empty-slot-only),
                            // and the failure is surfaced to the panel via the
                            // activation-failure advisor event (count == 0).
                            if pref.one_shot {
                                crate::providers::session_moa_handle::restore_one_shot(
                                    &session_pref_key,
                                    pref.clone(),
                                );
                            }
                            if let Some(sink) = &trace_sink {
                                sink.on_trace(
                                    &crate::harness::trace::LoopTraceEvent::MoaAdvisor {
                                        index: 0,
                                        count: 0,
                                        label: format!(
                                            "moa:{}",
                                            pref.preset.as_deref().unwrap_or("<default>")
                                        ),
                                        text: String::new(),
                                        error: Some(format!(
                                            "MoA not activated: {reason}; run continues on the normal model"
                                        )),
                                    },
                                );
                            }
                            tracing::warn!(
                                reason = %reason,
                                "MoA activation unusable; run proceeds on the normal provider chain"
                            );
                            llm
                        }
```

（`pref` 在 arm 内被 clone 使用——若所有权冲突，将 arm 头改为 `Some(pref)` 后先 `let pref_for_restore = pref.clone();`。`trace_sink` 在此处是 `Option<Arc<dyn TraceSink>>`——以现场类型为准，`.as_ref()` 适配。）

- [ ] **Step 4: 跑测试 + 编译**

Run: `cargo test -p alephcore --lib providers::session_moa_handle`
Expected: 4 测试全 PASS。

- [ ] **Step 5: Commit**

```bash
git add src/providers/session_moa_handle.rs src/orchestrator/harness_bridge/runner_impl.rs
git commit -m "orchestrator: refill one-shot MoA pref on build failure + surface activation-failure event (B5)"
```

---

### Task 6: B6 — 顾问开销汇总桶（消灭"不可见/幽灵"两难）

**背景修正**（审计再核实）：`aggregate_usage_by_agents` 用 `agent_id IN (...)` 过滤，`moa:*` 合成 id 根本不在调用方的 agent 列表里——所以现状不是"幽灵 agent 出现"而是"**顾问开销在汇总里不可见**"。修法维持 spec 决策方向：汇总层归类为单一「MoA advisors」桶。

**Files:**
- Modify: `src/resilience/database/traces.rs`（+`aggregate_moa_advisor_usage`）
- Modify: `src/gateway/handlers/teams/snapshot.rs:189`（`handle_usage` 响应 +`moa_advisors` 字段）
- Modify: `src/builtin_tools/team/usage.rs:148`（team 工具 usage action 同步）
- Test: `src/resilience/database/traces.rs` tests

**Interfaces:**
- Produces: `pub async fn aggregate_moa_advisor_usage(&self, since: Option<i64>, until: Option<i64>) -> Result<Option<AgentUsageTotal>, AlephError>`——`agent_id` 固定 `"moa-advisors"`，无数据返回 `Ok(None)`
- `teams.usage` 响应新增可选键 `moa_advisors`（`AgentUsageTotal` 或省略）；`per_agent` 列表不变

- [ ] **Step 1: 找到 traces.rs 既有测试的 StateDatabase 构造模式**

Run: `grep -n "fn.*test\|StateDatabase::" src/resilience/database/traces.rs | head -20` 与 `grep -rn "in_memory\|memory()" src/resilience/database/*.rs | head`
Expected: 找到测试用内存库构造器（如 `StateDatabase::in_memory()` 或等价 helper）与插入 ProviderUsage trace 行的既有测试模式。**以下测试代码按该现场模式适配**（插入行需 `event_kind='provider_usage'`、`event_json` 含 `agent_id`/token 字段）。

- [ ] **Step 2: 写失败测试（按 Step 1 发现的模式落地，断言语义如下）**

```rust
    #[tokio::test]
    async fn moa_advisor_usage_rolls_into_single_bucket() {
        let db = /* Step 1 发现的内存构造器 */;
        // Two advisor usage rows + one real-agent row.
        /* 用现场的 insert_trace/TaskTrace::new 模式插入:
           agent_id="moa:0:openai:gpt-5"  input=100 output=10
           agent_id="moa:1:deepseek:v4"   input=200 output=20
           agent_id="main"                input=999 output=99  */
        let bucket = db.aggregate_moa_advisor_usage(None, None).await.unwrap().unwrap();
        assert_eq!(bucket.agent_id, "moa-advisors");
        assert_eq!(bucket.call_count, 2);
        assert_eq!(bucket.input_tokens, 300);
        assert_eq!(bucket.output_tokens, 30);
        // Real agents are untouched by the bucket query.
        let empty = db.aggregate_moa_advisor_usage(Some(i64::MAX - 1), None).await.unwrap();
        assert!(empty.is_none());
    }
```

Run: `cargo test -p alephcore --lib resilience::database::traces`
Expected: FAIL（方法不存在）。

- [ ] **Step 3: 实现 `aggregate_moa_advisor_usage`（镜像 `aggregate_usage_by_agents` 的 SQL 模式）**

```rust
    /// Roll every per-advisor MoA `ProviderUsage` row (synthetic agent_id
    /// `moa:<idx>:<provider>:<model>`, written by the per-advisor
    /// MeteringProvider) into ONE `"moa-advisors"` bucket. Keeps advisor
    /// spend visible in usage rollups without materializing phantom agents
    /// (round-2 B6). `None` when no advisor usage exists in the window.
    pub async fn aggregate_moa_advisor_usage(
        &self,
        since: Option<i64>,
        until: Option<i64>,
    ) -> Result<Option<AgentUsageTotal>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut where_extras = String::new();
        let mut binds: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(ts) = since {
            where_extras.push_str(" AND timestamp >= ?1");
            binds.push(rusqlite::types::Value::Integer(ts));
        }
        if let Some(ts) = until {
            where_extras.push_str(&format!(" AND timestamp <= ?{}", binds.len() + 1));
            binds.push(rusqlite::types::Value::Integer(ts));
        }
        let sql = format!(
            r#"
            SELECT
                COUNT(*),
                SUM(COALESCE(CAST(json_extract(event_json, '$.input_tokens') AS INTEGER), 0)),
                SUM(COALESCE(CAST(json_extract(event_json, '$.output_tokens') AS INTEGER), 0)),
                SUM(COALESCE(CAST(json_extract(event_json, '$.cache_read_tokens') AS INTEGER), 0)),
                SUM(COALESCE(CAST(json_extract(event_json, '$.cache_creation_tokens') AS INTEGER), 0)),
                SUM(COALESCE(CAST(json_extract(event_json, '$.thinking_tokens') AS INTEGER), 0))
            FROM task_traces
            WHERE event_kind = 'provider_usage'
              AND json_extract(event_json, '$.agent_id') LIKE 'moa:%'{where_extras}
            "#
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("Failed to prepare moa usage query: {e}")))?;
        let row = stmt
            .query_row(rusqlite::params_from_iter(binds), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| AlephError::config(format!("Failed to query moa usage: {e}")))?;
        if row.0 == 0 {
            return Ok(None);
        }
        let to_u64 = |v: i64| u64::try_from(v).unwrap_or(0);
        Ok(Some(AgentUsageTotal {
            agent_id: "moa-advisors".to_string(),
            call_count: to_u64(row.0),
            input_tokens: to_u64(row.1),
            output_tokens: to_u64(row.2),
            cache_read_tokens: to_u64(row.3),
            cache_creation_tokens: to_u64(row.4),
            reasoning_tokens: to_u64(row.5),
        }))
    }
```

（SUM over 0 行返回 NULL——`row.get::<_, i64>` 会因此失败；若现场如此，把 SUM 列包 `COALESCE(SUM(...), 0)`。以测试驱动修正。）

- [ ] **Step 4: `handle_usage`（teams/snapshot.rs:189）响应加桶**

在组装响应 JSON 处（`total {...}` + `per_agent` 旁）追加：

```rust
        let moa_advisors = state_db
            .aggregate_moa_advisor_usage(since, until)
            .await
            .ok()
            .flatten();
```

响应 json! 里加一键：`"moa_advisors": moa_advisors,`（`Option` serde 序列化为 null——面板/调用方按可选处理）。`src/builtin_tools/team/usage.rs:148` 的 usage action 同样在输出结构中追加 `moa_advisors` 可选字段（同一调用模式）。

- [ ] **Step 5: 跑测试**

Run: `cargo test -p alephcore --lib resilience::database::traces`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add src/resilience/database/traces.rs src/gateway/handlers/teams/snapshot.rs src/builtin_tools/team/usage.rs
git commit -m "gateway: roll MoA advisor spend into a dedicated moa-advisors usage bucket (B6)"
```

---

### Task 7: B7 — channel 路径 `/moa` 前缀残留剥离

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs:408-420`（Fallthrough 捕获处 rewrite input）
- Test: `src/providers/moa/mod.rs` 已有 `parse_one_shot_command` 测试覆盖解析；本任务补 execute 层语义注释 + 复用解析器

**Interfaces:**
- Consumes: `crate::providers::moa::parse_one_shot_command(&str) -> Option<&str>`（已存在）
- 语义：仅当 Fallthrough `reason == "moa one-shot"` 时 rewrite——嵌套 slash 守卫已在 slash_command.rs 拦截 arm 保证不会 arm MoA，但 `/moa /help` 类输入在该 arm 也 Fallthrough 且**不该**剥前缀吗？——该 arm 只在 `args` 非 slash 时 arm MoA，但 Fallthrough 无条件返回。剥离必须与 arm 条件一致：只有 `parse_one_shot_command` 返回 Some 且结果非 slash 开头时剥（与 slash_command.rs:157-163 的守卫完全同构）。

- [ ] **Step 1: 修改 Fallthrough 捕获点**

`execute.rs:408` 的 arm 内（`run.state = RunState::Running;` 块之后、`warn!` 之前）插入：

```rust
                    // Round-2 B7: a `/moa <prompt>` arriving through a channel
                    // reaches this fallthrough with the RAW "/moa ..." text
                    // still in request.input (the channel-path intercept can't
                    // mutate its borrow). Strip the prefix here — mirroring
                    // the Panel/CLI intercept — so the LLM sees the prompt,
                    // not the command. The nested-slash guard stays aligned
                    // with the arming site: a slash remainder was never armed,
                    // strip only the "/moa " wrapper and let it resolve.
                    if reason == "moa one-shot" {
                        if let Some(prompt) =
                            crate::providers::moa::parse_one_shot_command(&request.input)
                        {
                            request.input = prompt.to_string();
                        }
                    }
```

（`request` 在 execute() 里是 `mut`——Panel 路径已有 `request.input = prompt.to_string()` 先例。若此处 request 已被借出，把 rewrite 移到 match 之后、agent loop 构造之前，用局部 flag 传递。）

- [ ] **Step 2: 编译检查（该文件在 alephcore）**

Run: `cargo test -p alephcore --lib providers::moa::tests::parse_one_shot`（若过滤器无命中则跑 `cargo test -p alephcore --lib providers::moa`）
Expected: PASS；execute.rs 无编译错误（测试编译会带动整 crate check）。

- [ ] **Step 3: Commit**

```bash
git add src/gateway/execution_engine/execute.rs
git commit -m "gateway: strip /moa prefix from channel-path fallthrough input (B7)"
```

---

### Task 8: B8 — VESR 归因记录实际 serving 模型（聚合器）

**Files:**
- Modify: `src/providers/moa/provider.rs`（+`aggregator_identity` getter）
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs`（Step 3-MoA 捕获 identity → OutcomeObserver 用它）
- Test: `src/providers/moa/provider.rs` tests

**Interfaces:**
- Produces: `impl MoaProvider { pub fn aggregator_identity(&self) -> (String, String) }` — `(provider, model)`，由 `aggregator_label`（`"provider:model"`）`split_once(':')` 拆出

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn aggregator_identity_splits_label() {
        let agg = Arc::new(CountingProvider::new("x"));
        let p = make_provider(vec![], agg, MoaFanout::PerIteration, 5);
        // make_provider sets aggregator_label = "mock:agg".
        assert_eq!(
            p.aggregator_identity(),
            ("mock".to_string(), "agg".to_string())
        );
    }
```

Run: `cargo test -p alephcore --lib providers::moa::provider::tests::aggregator_identity`
Expected: FAIL（方法不存在）。

- [ ] **Step 2: 实现 getter**

```rust
    /// `(provider, model)` of the aggregator slot — run-level attribution
    /// (VESR must record the ACTING model, not the pre-MoA directive; the
    /// gauge fold at runner_impl.rs already does the equivalent via
    /// serving_model_hint). Split at the FIRST ':' — provider keys never
    /// contain one; model ids may.
    #[must_use]
    pub fn aggregator_identity(&self) -> (String, String) {
        match self.aggregator_label.split_once(':') {
            Some((p, m)) => (p.to_string(), m.to_string()),
            None => (String::new(), self.aggregator_label.clone()),
        }
    }
```

- [ ] **Step 3: runner_impl.rs 接线**

Step 3-MoA 块外声明捕获变量，Ok arm 里填充：

```rust
        let mut moa_active = false;
        let mut moa_aggregator_identity: Option<(String, String)> = None;
        let llm: Arc<dyn crate::providers::AiProvider> =
            match crate::providers::session_moa_handle::take_for_run(&session_pref_key) {
                Some(pref) => {
                    ...
                        Ok(moa) => {
                            moa_active = true;
                            moa_aggregator_identity = Some(moa.aggregator_identity());
                            Arc::new(moa)
                        }
                    ...
```

OutcomeObserver 构造处（`:374-387`）改为：

```rust
        // Round-2 B8: when MoA is active the run's acting model is the
        // preset's aggregator — record THAT into routing experience, not the
        // pre-MoA directive/pin (which never served a token this run).
        let (vesr_model_id, vesr_provider_id): (String, String) =
            match &moa_aggregator_identity {
                Some((p, m)) => (m.clone(), p.clone()),
                None => (
                    routing_model_id.clone(),
                    routing_provider_id.clone().unwrap_or_default(),
                ),
            };
        let trace_sink = match (trace_sink, self.routing_store.as_ref()) {
            (Some(parent), Some(store)) => {
                Some(std::sync::Arc::new(crate::routing::OutcomeObserver::new(
                    parent,
                    store.clone(),
                    routing_attribution.clone(),
                    vesr_model_id,
                    vesr_provider_id,
                    spec.agent.clone(),
                ))
                    as std::sync::Arc<dyn crate::harness::TraceSink>)
            }
            (other, _) => other,
        };
```

（`routing_provider_id` 原为 `Option<String>` 且原代码 `unwrap_or_default()`——保持等价。`routing_model_id`/`routing_provider_id` 若在此后还有其他消费处则保留原变量不动，本处只换 observer 入参。）

- [ ] **Step 4: 跑测试**

Run: `cargo test -p alephcore --lib providers::moa::provider`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add src/providers/moa/provider.rs src/orchestrator/harness_bridge/runner_impl.rs
git commit -m "orchestrator: VESR records MoA aggregator as the acting model (B8)"
```

---

### Task 9: W1 — `moa` 进渐进披露核心工具集

**Files:**
- Modify: `src/config/types/tools.rs:162-173`（`default_core_tools` 列表 + doc）
- Test: 既有 `snapshot_exempt_tools_must_stay_core` 应仍绿（它只断言 subagent/get_tool_schema 在集内，加法安全）

- [ ] **Step 1: 加入列表**

```rust
pub fn default_core_tools() -> Vec<String> {
    [
        "ask_user", "subagent", "bash", "code_exec", "code_check",
        "file_read", "file_write", "file_edit", "file_ops",
        "search", "web_fetch", "memory_search", "remember",
        "skill_read", "skill_list", "scratchpad", "note_manage",
        "system", "get_tool_schema", "moa",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}
```

doc comment 补一行：

```rust
/// `moa` is core so a user's "turn on MoA for this session" engages in one
/// step — the activation toggle must not hide behind a get_tool_schema
/// round-trip (R8 conversational management; round-2 W1).
```

- [ ] **Step 2: 跑定向测试**

Run: `cargo test -p alephcore --lib config::types::tools`
Expected: 全 PASS（含 `snapshot_exempt_tools_must_stay_core`；若有断言核心集**确切内容**的测试，把 `"moa"` 加进期望值）。

- [ ] **Step 3: Commit**

```bash
git add src/config/types/tools.rs
git commit -m "config: add moa to progressive-disclosure core tool set (W1)"
```

---

### Task 10: W3(a) — trace_presentation 呈现更新（error/cached/turn-trace 摘要行）

**Files:**
- Modify: `shared/protocol/src/trace_presentation.rs:451-484`
- Test: 同文件既有 tests 模块（或 events.rs tests——以现场为准）

**Interfaces:**
- Produces（TUI/CLI 消费的呈现语义）:
  - `MoaAdvisor`：`count == 0` → status `Failed`, content `"MoA not activated — {error}"`；`error.is_some()` → status `Failed`, content `"Advisor {i}/{n} — {label} [failed: {error截断}]"`；正常 → 现状 Info
  - `MoaAggregating`：`cached` → content `"MoA aggregating ({agg}, cached advice)"`；否则现状
  - `MoaTurnTrace`：从 `None` 改为 `Some(Info, "MoA turn trace — preset {p} ({n} advisors)")`（n 从 payload `advisors` 数组长度取；单行摘要——`AgentTracePresentation` 是扁平结构，重内容的展开呈现在面板 events.rs（Task 11））

- [ ] **Step 1: 写失败测试（trace_presentation.rs tests 模块，按既有测试形态）**

```rust
    #[test]
    fn moa_presentation_reflects_error_cached_and_turn_trace() {
        let opts = AgentTracePresentationPreset::PanelTrace.options();
        let labels = AgentTracePresentationLabels::default();

        let failed = AgentTraceEvent::MoaAdvisor {
            index: 2, count: 3, label: "p:m".into(), text: String::new(),
            error: Some("timeout after 120s".into()),
        };
        let p = present_agent_trace_event(&failed, &opts, &labels).unwrap();
        assert!(matches!(p.status, AgentTracePresentationStatus::Failed));
        assert!(p.content.contains("failed"));

        let not_activated = AgentTraceEvent::MoaAdvisor {
            index: 0, count: 0, label: "moa:deep".into(), text: String::new(),
            error: Some("preset not found".into()),
        };
        let p = present_agent_trace_event(&not_activated, &opts, &labels).unwrap();
        assert!(p.content.contains("not activated"));

        let cached = AgentTraceEvent::MoaAggregating {
            aggregator: "p:m".into(), advisor_count: 2, cached: true,
        };
        let p = present_agent_trace_event(&cached, &opts, &labels).unwrap();
        assert!(p.content.contains("cached"));

        let trace = AgentTraceEvent::MoaTurnTrace {
            preset: "deep".into(),
            payload: serde_json::json!({ "advisors": [{"label": "a", "output": "x"}] }),
        };
        let p = present_agent_trace_event(&trace, &opts, &labels).unwrap();
        assert!(p.content.contains("deep"));
        assert!(p.content.contains("1 advisor"));
    }
```

（`options()`/`labels` 的实际构造以文件内既有测试为准适配。）

Run: `cargo test --manifest-path shared/protocol/Cargo.toml moa_presentation`
Expected: FAIL。

- [ ] **Step 2: 更新四个 arms**

```rust
        AgentTraceEvent::MoaAdvisor {
            index,
            count,
            label,
            error,
            ..
        } => Some(if *count == 0 {
            // Activation-failure form (runner Step 3-MoA build failure).
            AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Failed,
                content: format!(
                    "MoA not activated — {}",
                    truncate(error.as_deref().unwrap_or("unknown"), options.content_limit)
                ),
                duration_ms: None,
            }
        } else if let Some(err) = error {
            AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Failed,
                content: format!(
                    "Advisor {index}/{count} — {label} [failed: {}]",
                    truncate(err, options.content_limit)
                ),
                duration_ms: None,
            }
        } else {
            AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Info,
                content: format!("Advisor {index}/{count} — {label}"),
                duration_ms: None,
            }
        }),

        AgentTraceEvent::MoaAggregating {
            aggregator, cached, ..
        } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::InProgress,
            content: if *cached {
                format!("MoA aggregating ({aggregator}, cached advice)")
            } else {
                format!("MoA aggregating ({aggregator})")
            },
            duration_ms: None,
        }),

        AgentTraceEvent::MoaAdvisorSpend {
            input_tokens,
            output_tokens,
            billed_count,
            advisor_count,
            ..
        } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::Info,
            content: format!(
                "MoA advisors spent {input_tokens}+{output_tokens} tok ({billed_count}/{advisor_count} billed)"
            ),
            duration_ms: None,
        }),

        AgentTraceEvent::MoaTurnTrace { preset, payload } => {
            // Persisted-only on the wire; in REPLAY the one-line summary makes
            // the audit record discoverable (round-2 W3). Full advisor I/O
            // renders panel-side (events.rs "moa_turn_trace" arm).
            let n = payload
                .get("advisors")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len);
            Some(AgentTracePresentation {
                kind: event.kind().into(),
                status: AgentTracePresentationStatus::Info,
                content: format!(
                    "MoA turn trace — preset {preset} ({n} advisor{})",
                    if n == 1 { "" } else { "s" }
                ),
                duration_ms: None,
            })
        }
```

- [ ] **Step 3: 跑测试**

Run: `cargo test --manifest-path shared/protocol/Cargo.toml`
Expected: 全 PASS。

- [ ] **Step 4: Commit**

```bash
git add shared/protocol/src/trace_presentation.rs
git commit -m "protocol: MoA presentations — error/cached/billed forms + turn-trace summary (W3a)"
```

---

### Task 11: W2 — TUI 渲染三个 MoA 事件（吃掉 "Task 9" 遗留 stub）

**Files:**
- Modify: `interfaces/tui/src/tui/app/mod.rs:640-644`

**Interfaces:**
- Consumes: Task 10 的呈现内容（`presentation.content` 已包含 error/cached/billed 形态）
- 设计：三个 live 事件走 `append_reasoning_entry(presentation.content.clone())`（与 VerifierVeto 同路——TUI reasoning 块 `┊` 暗色 gutter，verbose 门控）；`MoaTurnTrace` 保持显式 no-op（TUI 无回放面）。

- [ ] **Step 1: 改 match arms**

`append_trace_debug_entry` 中把四个 Moa 变体从 no-op arm 拆出：

```rust
            AgentTraceEvent::ToolCallStarted { .. }
            | AgentTraceEvent::ToolCallCompleted { .. }
            | AgentTraceEvent::WorktreeCreated { .. }
            | AgentTraceEvent::WorktreeCleanedUp { .. }
            | AgentTraceEvent::McpScopeAttached { .. }
            | AgentTraceEvent::McpScopeCleaned { .. }
            | AgentTraceEvent::ProviderUsage { .. }
            | AgentTraceEvent::ReactiveCompactionAttempted { .. }
            // MoaTurnTrace is persisted-only (no live wire, no TUI replay).
            | AgentTraceEvent::MoaTurnTrace { .. } => {}
            // MoA fan-out moments render as reasoning entries — presentation
            // already carries the error/cached/billed forms (round-2 W2).
            AgentTraceEvent::MoaAdvisor { .. }
            | AgentTraceEvent::MoaAggregating { .. }
            | AgentTraceEvent::MoaAdvisorSpend { .. } => {
                self.append_reasoning_entry(presentation.content.clone());
            }
```

- [ ] **Step 2: 编译检查**

Run: `cargo check --manifest-path interfaces/tui/Cargo.toml`
Expected: 0 错误 0 警告。

- [ ] **Step 3: Commit**

```bash
git add interfaces/tui/src/tui/app/mod.rs
git commit -m "tui: render MoA advisor/aggregating/spend as reasoning entries (W2)"
```

---

### Task 12: W3(b) + 面板字段更新 — wide 面板渲染 error/cached/billed + turn-trace 回放块

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs:196-231`（三 arm 更新 + 新 `moa_turn_trace` arm）

**Interfaces:**
- Consumes: Task 3/4 wire 字段（`error`/`cached`/`billed_count`/payload `aggregator_output`/`aggregator_status`）
- 语义：`moa_turn_trace` 只经 `trace.by_runs` 回放到达（非 wire 白名单）——本 arm 兑现 spec「回放可见完整审计视图」。

- [ ] **Step 1: 更新三个既有 arm + 新增一个 arm**

```rust
        "moa_advisor" => {
            let index = trace_event.get("index").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let count = trace_event.get("count").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let label = trace_event.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let text = trace_event.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let error = trace_event.get("error").and_then(|v| v.as_str());
            if count == 0 {
                // Activation failure (runner build error): MoA didn't engage.
                append_reasoning(
                    chat,
                    &format!("⚠ MoA 未生效：{}", error.unwrap_or("unknown")),
                );
            } else if let Some(err) = error {
                append_reasoning(
                    chat,
                    &format!("◇ 顾问 {index}/{count} — {label}\n⚠ {err}"),
                );
            } else {
                append_reasoning(chat, &format!("◇ 顾问 {index}/{count} — {label}\n{text}"));
            }
            workspace.note_activity();
        }
        "moa_aggregating" => {
            let aggregator = trace_event.get("aggregator").and_then(|v| v.as_str()).unwrap_or("");
            let n = trace_event
                .get("advisor_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let cached = trace_event
                .get("cached")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if cached {
                append_reasoning(chat, &format!("◆ MoA 聚合中（{aggregator}，沿用缓存顾问意见）"));
            } else {
                append_reasoning(chat, &format!("◆ MoA 聚合中（{aggregator}，{n} 位顾问）"));
            }
        }
        "moa_advisor_spend" => {
            let input = trace_event
                .get("input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let output = trace_event
                .get("output_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let billed = trace_event
                .get("billed_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let n = trace_event
                .get("advisor_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let cost = trace_event.get("cost_usd").and_then(serde_json::Value::as_f64);
            let cost_str = cost.map_or(String::new(), |c| format!("，约 ${c:.4}"));
            append_reasoning(
                chat,
                &format!("▫ 顾问开销：{input}+{output} tokens（{billed}/{n} 位计费）{cost_str}"),
            );
        }
        // Heavy audit record — arrives only via trace.by_runs REPLAY (never
        // wire-whitelisted). Renders the full "why did MoA advise this" view
        // into the reasoning panel (round-2 W3b).
        "moa_turn_trace" => {
            let preset = trace_event.get("preset").and_then(|v| v.as_str()).unwrap_or("");
            let payload = trace_event.get("payload").cloned().unwrap_or_default();
            let mut block = format!("📋 MoA turn trace — preset {preset}");
            if let Some(advisors) = payload.get("advisors").and_then(serde_json::Value::as_array) {
                for (i, a) in advisors.iter().enumerate() {
                    let label = a.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    let output = a.get("output").and_then(|v| v.as_str()).unwrap_or("");
                    block.push_str(&format!("\n─── 顾问 {} — {label} ───\n{output}", i + 1));
                }
            }
            if let Some(out) = payload.get("aggregator_output").and_then(|v| v.as_str()) {
                let status = payload
                    .get("aggregator_status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ok");
                block.push_str(&format!("\n─── 聚合器（{status}）───\n{out}"));
            }
            append_reasoning(chat, &block);
        }
```

- [ ] **Step 2: WASM 编译**

Run: `just wasm`
Expected: 构建成功 0 警告。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/events.rs
git commit -m "panel: MoA error/cached/billed rendering + turn-trace replay block (W3b)"
```

---

### Task 13: E1 — advisor prompt-cache 断点标记

**Files:**
- Modify: `src/providers/moa/advisory_view.rs`（+`mark_cache_breakpoints`）
- Modify: `src/providers/moa/provider.rs`（签名计算后就地打标）
- Test: advisory_view.rs tests

**Interfaces:**
- Produces: `pub(crate) fn mark_cache_breakpoints(view: &mut [UnifiedMessage])` — 视图**尾部最后 3 条消息**各自最后一个 `Text` 块打 `cache_control: Some(CacheControl::Ephemeral { ttl: None })`（镜像 hermes `system_and_3` 布局；Anthropic 协议适配器 `proto_impl.rs:73` 已映射标记为 ephemeral，非 Anthropic 适配器天然忽略——无条件打标零分支）
- 前置保障：Task 2 的 `signature_ignores_cache_control_marks` 已锁「标记不扰动缓存键」——就地打标安全（签名先算，标记后打）。

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn cache_breakpoints_mark_last_three_messages() {
        let mut view = vec![
            UnifiedMessage::user("one"),
            UnifiedMessage::assistant("two"),
            UnifiedMessage::user("three"),
            UnifiedMessage::assistant("four"),
            UnifiedMessage::user("five"),
        ];
        mark_cache_breakpoints(&mut view);
        let marked: Vec<bool> = view
            .iter()
            .map(|m| {
                let content = match m {
                    UnifiedMessage::User { content }
                    | UnifiedMessage::Assistant { content } => content,
                    UnifiedMessage::ToolResult { content, .. } => content,
                };
                content.iter().any(|b| {
                    matches!(b, ContentBlock::Text { cache_control: Some(_), .. })
                })
            })
            .collect();
        assert_eq!(marked, vec![false, false, true, true, true]);
    }

    #[test]
    fn cache_breakpoints_short_view_marks_all() {
        let mut view = vec![UnifiedMessage::user("only")];
        mark_cache_breakpoints(&mut view);
        let UnifiedMessage::User { content } = &view[0] else { panic!() };
        assert!(matches!(
            content.last(),
            Some(ContentBlock::Text { cache_control: Some(_), .. })
        ));
    }
```

Run: `cargo test -p alephcore --lib providers::moa::advisory_view`
Expected: FAIL（函数不存在）。

- [ ] **Step 2: 实现**

```rust
/// Mark Anthropic prompt-cache breakpoints on the advisory view: the last
/// Text block of each of the LAST THREE messages gets an ephemeral
/// cache_control (hermes `system_and_3` layout). The view is append-only
/// across iterations, so iteration N+1's prefix replays N's cached segment —
/// without this, per_iteration advisors re-bill the whole prefix every tool
/// step (hermes measured 0/1227 cache reads, 11.5M re-billed tokens).
/// Marking is unconditional: the Anthropic protocol adapter maps the mark to
/// `ephemeral`; every other adapter ignores it (zero per-provider branching).
/// Call AFTER view_signature — the signature deliberately ignores marks.
pub(crate) fn mark_cache_breakpoints(view: &mut [UnifiedMessage]) {
    let len = view.len();
    for msg in view.iter_mut().skip(len.saturating_sub(3)) {
        let content = match msg {
            UnifiedMessage::User { content } | UnifiedMessage::Assistant { content } => content,
            UnifiedMessage::ToolResult { content, .. } => content,
        };
        if let Some(ContentBlock::Text { cache_control, .. }) = content
            .iter_mut()
            .rev()
            .find(|b| matches!(b, ContentBlock::Text { .. }))
        {
            *cache_control = Some(crate::providers::message::CacheControl::Ephemeral {
                ttl: None,
            });
        }
    }
}
```

（`CacheControl::Ephemeral` 的确切变体形状以 `src/providers/message.rs:30-40` 为准：`Ephemeral { ttl: Option<EphemeralTtl> }`。）

- [ ] **Step 3: provider.rs 接线**

`process()` 中签名计算后（`let sig = view_signature(&view);` 之后）就地打标：

```rust
            // 1b. Prompt-cache breakpoints (round-2 E1) — AFTER the signature
            //     (which ignores marks) so the cache key is never perturbed.
            let mut view = view;
            super::advisory_view::mark_cache_breakpoints(&mut view);
```

（`view` 由 `build_advisory_view` 返回的 owned Vec——`let view = build_advisory_view(&messages);` 改 `let mut view = ...` 亦可。）

- [ ] **Step 4: 跑测试**

Run: `cargo test -p alephcore --lib providers::moa`
Expected: 全 PASS（含 Task 2 的 `signature_ignores_cache_control_marks` 回归网）。

- [ ] **Step 5: Commit**

```bash
git add src/providers/moa/
git commit -m "providers: advisor prompt-cache breakpoints on advisory view tail (E1)"
```

---

### Task 14: E4 — 多模态 `[image]` 占位标记 + Json 分支补测试

**Files:**
- Modify: `src/providers/moa/advisory_view.rs`（`text_of` 的 Image arm）
- Test: 同文件 tests

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn image_blocks_render_placeholder_and_json_stringifies() {
        let msgs = vec![UnifiedMessage::User {
            content: vec![
                ContentBlock::Text { text: "look at this".into(), cache_control: None },
                ContentBlock::Image { data: "base64...".into(), mime_type: "image/png".into() },
                ContentBlock::Json { value: json!({"k": 1}) },
            ],
        }];
        let view = build_advisory_view(&msgs);
        let texts = view_texts(&view);
        assert!(texts[0].1.contains("look at this"));
        // E4: advisors learn an image exists (hermes drops it silently).
        assert!(texts[0].1.contains("[image: image/png]"));
        assert!(texts[0].1.contains("{\"k\":1}"));
    }
```

Run: `cargo test -p alephcore --lib providers::moa::advisory_view::tests::image_blocks`
Expected: FAIL（`[image: image/png]` 不存在——Image 现走 `_ => {}`）。

- [ ] **Step 2: 实现 `text_of` Image arm**

```rust
fn text_of(blocks: &[ContentBlock]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                if !text.is_empty() {
                    parts.push(text.clone());
                }
            }
            ContentBlock::Json { value } => parts.push(value.to_string()),
            // Advisors can't see pixels, but they must know an image exists —
            // hermes drops multimodal content silently (its #51 gap); the
            // placeholder keeps them from being blindsided by "the screenshot
            // above" (round-2 E4).
            ContentBlock::Image { mime_type, .. } => {
                parts.push(format!("[image: {mime_type}]"));
            }
            // Thinking is the acting model's private reasoning; ToolCall is
            // rendered separately.
            _ => {}
        }
    }
    parts.join("\n")
}
```

- [ ] **Step 3: 跑测试**

Run: `cargo test -p alephcore --lib providers::moa::advisory_view`
Expected: 全 PASS。注意 `view_signature` 的 Task 2 重写版对 Image 块走 `_ => {}`——`text_of` 现在产出占位文本但签名函数**不含** Image：两者不一致会导致「加图不换签名」。同步修 `view_signature` 的块循环加 Image arm：

```rust
                ContentBlock::Image { mime_type, .. } => {
                    "image".hash(&mut hasher);
                    mime_type.hash(&mut hasher);
                }
```

（补断言进测试：追加一张图后 `view_signature` 必须变化。）

```rust
    #[test]
    fn signature_changes_when_image_added() {
        let base = vec![UnifiedMessage::user("go")];
        let with_image = vec![UnifiedMessage::User {
            content: vec![
                ContentBlock::Text { text: "go".into(), cache_control: None },
                ContentBlock::Image { data: "d".into(), mime_type: "image/png".into() },
            ],
        }];
        assert_ne!(
            view_signature(&build_advisory_view(&base)),
            view_signature(&build_advisory_view(&with_image))
        );
    }
```

- [ ] **Step 4: Commit**

```bash
git add src/providers/moa/advisory_view.rs
git commit -m "providers: advisory view renders [image] placeholders for multimodal turns (E4)"
```

---

### Task 15: E3-core — `select_model` 认 `moa:` 前缀 + `list_models` 列 preset + 互斥槽位

**Files:**
- Modify: `src/builtin_tools/select_model.rs`（`moa:`/`moa` 前缀分派 + 普通选模清 MoA）
- Modify: `src/builtin_tools/list_models.rs`（输出 +`moa_presets`）
- Modify: `src/builtin_tools/moa_manage.rs`（`activate` 清 session model——互斥对称）
- Test: 各文件 tests

**Interfaces:**
- 语义（选择器唯一槽位，spec §6 E3）：
  - `select_model { model: "moa:<preset>" }` 或 `{ model: "moa" }`（default preset）→ 校验 preset 可解析 → `set_session_moa(key, Some/None, false)` + `clear_session_model(key)`
  - `select_model { model: <普通模型> }` → `set_session_model(...)` + `clear_session_moa(key)`
  - `moa on/once` → `set_session_moa(...)` + `clear_session_model(key)`
  - `moa off` → 只清 MoA（不动模型选择）
- `list_models` 输出新增：`pub moa_presets: Vec<MoaPresetSummary>`，`MoaPresetSummary { name: String, advisors: Vec<String>, aggregator: String, is_default: bool }`（仅 enabled preset；disabled 不列——hermes #47 门控对齐）

- [ ] **Step 1: 写失败测试（select_model.rs tests）**

```rust
    #[tokio::test]
    async fn moa_prefix_activates_preset_and_clears_model_pick() {
        use crate::providers::{session_moa_handle, session_model_handle};
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-moa-test".to_string(),
        };
        let key = sk.to_key_string();
        // Prime a model pick + a resolvable preset.
        session_model_handle::set_session_model(&key, None, "gpt-5".to_string());
        crate::providers::moa::store_moa_config(Some({
            let mut cfg = crate::config::MoaToml::default();
            cfg.presets.insert(
                "deep".to_string(),
                crate::config::MoaPreset {
                    enabled: true,
                    advisors: vec![crate::config::MoaSlot {
                        provider: "openai".into(),
                        model: "gpt-5".into(),
                    }],
                    aggregator: crate::config::MoaSlot {
                        provider: "anthropic".into(),
                        model: "claude-opus-4".into(),
                    },
                    fanout: crate::config::MoaFanout::default(),
                    advisor_timeout_secs: 120,
                    advisor_max_tokens: None,
                    advisor_temperature: None,
                    aggregator_temperature: None,
                },
            );
            cfg
        }));

        let ctx = TurnContext {
            session_key: sk.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                SelectModelTool
                    .call(SelectModelArgs { model: "moa:deep".to_string(), provider: None })
                    .await
            })
            .await
            .unwrap();

        assert!(out.ok);
        // Selector slot is exclusive: MoA armed, model pick cleared.
        let moa = session_moa_handle::get_session_moa(&key).unwrap();
        assert_eq!(moa.preset.as_deref(), Some("deep"));
        assert!(!moa.one_shot);
        assert!(session_model_handle::get_session_model(&key).is_none());
        session_moa_handle::clear_session_moa(&key);
        crate::providers::moa::store_moa_config(None);
    }

    #[tokio::test]
    async fn normal_pick_clears_moa_sticky() {
        use crate::providers::{session_moa_handle, session_model_handle};
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-clears-moa".to_string(),
        };
        let key = sk.to_key_string();
        session_moa_handle::set_session_moa(&key, Some("deep".to_string()), false);
        let ctx = TurnContext {
            session_key: sk,
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                SelectModelTool
                    .call(SelectModelArgs { model: "gpt-5".to_string(), provider: None })
                    .await
            })
            .await
            .unwrap();
        assert!(out.ok);
        assert!(session_moa_handle::get_session_moa(&key).is_none());
        session_model_handle::clear_session_model(&key);
    }

    #[tokio::test]
    async fn moa_prefix_with_unknown_preset_fails_gracefully() {
        crate::providers::moa::store_moa_config(None);
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-moa-unknown".to_string(),
        };
        let ctx = TurnContext {
            session_key: sk,
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                SelectModelTool
                    .call(SelectModelArgs { model: "moa:ghost".to_string(), provider: None })
                    .await
            })
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.message.contains("moa"));
    }
```

注意：这些测试写全局 `store_moa_config`——沿 moa_manage.rs 的 `moa_config_test_lock()` 模式在测试头部拿锁序列化（把该 helper 复制到本测试模块或提到公共测试 util，以现场最小改动为准）。

Run: `cargo test -p alephcore --lib builtin_tools::select_model`
Expected: FAIL（moa: 前缀未处理，走普通路径 ok=true 但 moa handle 为空）。

- [ ] **Step 2: 实现 select_model 分派**

`call()` 中拿到 `key` 之后、`set_session_model` 之前插入：

```rust
        // Round-2 E3: the selector is ONE slot — a "moa:<preset>" (or bare
        // "moa") value arms MoA sticky and clears any model pick; a normal
        // model pick clears MoA. No coexistence confusion (spec §6).
        if args.model == "moa" || args.model.starts_with("moa:") {
            let preset = args.model.strip_prefix("moa:").filter(|s| !s.is_empty());
            let resolved = crate::providers::moa::get_moa_config()
                .as_ref()
                .and_then(|cfg| cfg.resolve_preset(preset))
                .map(|(name, _)| name);
            let Some(name) = resolved else {
                let message = format!(
                    "MoA preset '{}' not found — use the moa tool (action='list') to see presets.",
                    preset.unwrap_or("<default>")
                );
                notify_tool_result(Self::NAME, &message, false);
                return Ok(SelectModelOutput {
                    ok: false,
                    model: args.model,
                    provider: None,
                    message,
                });
            };
            crate::providers::session_moa_handle::set_session_moa(
                &key,
                preset.map(str::to_string),
                false,
            );
            crate::providers::session_model_handle::clear_session_model(&key);
            let message = format!(
                "MoA preset '{name}' activated for this session (sticky); model pick cleared. \
                 Takes effect from the next turn."
            );
            notify_tool_result(Self::NAME, &message, true);
            return Ok(SelectModelOutput {
                ok: true,
                model: args.model,
                provider: None,
                message,
            });
        }

        // Normal pick: selector-slot exclusivity clears any MoA sticky.
        crate::providers::session_moa_handle::clear_session_moa(&key);
```

`SelectModelArgs.model` 的 schemars description 补一句 `Pass "moa:<preset>" (or "moa" for the default preset) to activate Mixture-of-Agents advisory instead of a plain model.`；`SelectModelTool::DESCRIPTION` 尾部补 `Accepts "moa:<preset>" to switch the session onto a MoA advisory preset (advisors + aggregator).`。

- [ ] **Step 3: moa_manage.rs `activate` 对称清模型选择**

`activate()` 中 `set_session_moa(&key, preset, one_shot);` 后加：

```rust
        // Selector-slot exclusivity (round-2 E3): arming MoA supersedes any
        // per-session model pick — one slot, no precedence confusion.
        crate::providers::session_model_handle::clear_session_model(&key);
```

moa_manage 既有测试 `on_with_resolvable_preset_writes_sticky_session_handle` 补断言：`session_model_handle::get_session_model(&key).is_none()`（先 set 一个 pick 再 on）。

- [ ] **Step 4: list_models 输出加 preset 摘要**

`ListModelsOutput` 加字段：

```rust
    /// MoA advisory presets selectable via `select_model` with model
    /// "moa:<name>" (enabled presets only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub moa_presets: Vec<MoaPresetSummary>,
```

新结构（同文件）：

```rust
#[derive(Debug, Clone, Serialize)]
pub struct MoaPresetSummary {
    pub name: String,
    /// Advisor slots as "provider:model" labels.
    pub advisors: Vec<String>,
    /// Aggregator (acting model) as "provider:model".
    pub aggregator: String,
    pub is_default: bool,
}
```

`call()` 组装（`models.sort_by` 之后）：

```rust
        // MoA presets ride the same discovery surface (round-2 E3): the model
        // can offer "switch to MoA preset X" with select_model "moa:X".
        let moa_presets: Vec<MoaPresetSummary> = crate::providers::moa::get_moa_config()
            .map(|cfg| {
                let mut names: Vec<&String> = cfg
                    .presets
                    .iter()
                    .filter(|(_, p)| p.enabled)
                    .map(|(n, _)| n)
                    .collect();
                names.sort();
                names
                    .into_iter()
                    .map(|name| {
                        let p = &cfg.presets[name];
                        MoaPresetSummary {
                            name: name.clone(),
                            advisors: p
                                .advisors
                                .iter()
                                .map(|s| format!("{}:{}", s.provider, s.model))
                                .collect(),
                            aggregator: format!(
                                "{}:{}",
                                p.aggregator.provider, p.aggregator.model
                            ),
                            is_default: cfg.default_preset.as_deref() == Some(name.as_str()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
```

`ListModelsOutput` 构造处加 `moa_presets`；message 尾部当非空时追加 `format!(" {} MoA preset(s) available — select with model \"moa:<name>\".", moa_presets.len())`。

- [ ] **Step 5: 跑测试**

Run: `cargo test -p alephcore --lib builtin_tools::select_model builtin_tools::moa_manage builtin_tools::list_models`
Expected: 全 PASS。

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/select_model.rs src/builtin_tools/list_models.rs src/builtin_tools/moa_manage.rs
git commit -m "tools: select_model moa: prefix + list_models preset discovery + exclusive selector slot (E3)"
```

---

### Task 16: E3-panel — `providers.catalog` 列 MoA 伪条目 + `chat.send` 拦截激活

**Files:**
- Modify: `src/gateway/handlers/providers/handlers.rs:866-1011`（`handle_catalog` 追加 moa 条目）
- Modify: `src/gateway/handlers/agent.rs:357-367`（model_override 拦截 provider=="moa"）
- Test: handlers 既有测试模式（若 handle_catalog 有单测则扩展；否则以 agent.rs 拦截逻辑提炼纯函数单测）

**Interfaces:**
- Consumes: `get_moa_config()`、`set_session_moa`/`clear_session_moa`/`clear_session_model`
- 语义：面板选择器（`model_picker.rs`）**零改动**——moa 伪 provider 条目经既有 `providers.catalog` RPC 自然出现在下拉（display_name "Mixture of Agents"，models=preset 名列表）；选中一行 → 既有 `chat.send model_override: Qualified{provider:"moa", model:<preset>}` → gateway 拦截转写 session handle 并吞掉 override。**零新 RPC**。
- 互斥语义：chat.send 带非 moa override → 清 MoA sticky（显式选普通模型=覆盖槽位）；不带 override → 不动（Default 行为交给 `moa off`/工具管理，注释写明）。

- [ ] **Step 1: 拦截逻辑提炼为可测纯函数（agent.rs 或就近模块）**

```rust
/// Round-2 E3: a picker selection of the "moa" pseudo-provider arms session
/// MoA (sticky) instead of riding as a per-turn model override. Returns the
/// override that should continue down the run path (None when consumed).
/// An explicit NON-moa override clears any MoA sticky — the selector is one
/// exclusive slot. No override touches nothing (bare sends must not disturb
/// an armed MoA session).
fn apply_moa_selector_semantics(
    session_key: &str,
    model_override: Option<crate::gateway::model_override::ModelOverride>,
) -> Option<crate::gateway::model_override::ModelOverride> {
    use crate::gateway::model_override::ModelOverride;
    match model_override {
        Some(ModelOverride::Qualified { provider, model }) if provider == "moa" => {
            crate::providers::session_moa_handle::set_session_moa(
                session_key,
                Some(model),
                false,
            );
            crate::providers::session_model_handle::clear_session_model(session_key);
            None
        }
        Some(other) => {
            crate::providers::session_moa_handle::clear_session_moa(session_key);
            Some(other)
        }
        None => None,
    }
}
```

单测（agent.rs tests 模块或就近）：

```rust
    #[test]
    fn moa_override_arms_session_and_is_consumed() {
        use crate::gateway::model_override::ModelOverride;
        let key = "test:moa:selector";
        let out = apply_moa_selector_semantics(
            key,
            Some(ModelOverride::Qualified { provider: "moa".into(), model: "deep".into() }),
        );
        assert!(out.is_none());
        let pref = crate::providers::session_moa_handle::get_session_moa(key).unwrap();
        assert_eq!(pref.preset.as_deref(), Some("deep"));
        assert!(!pref.one_shot);
        // Non-moa override clears the sticky.
        let out = apply_moa_selector_semantics(
            key,
            Some(ModelOverride::Qualified { provider: "openai".into(), model: "gpt-5".into() }),
        );
        assert!(out.is_some());
        assert!(crate::providers::session_moa_handle::get_session_moa(key).is_none());
        // No override leaves state alone.
        crate::providers::session_moa_handle::set_session_moa(key, None, false);
        assert!(apply_moa_selector_semantics(key, None).is_none());
        assert!(crate::providers::session_moa_handle::get_session_moa(key).is_some());
        crate::providers::session_moa_handle::clear_session_moa(key);
    }
```

- [ ] **Step 2: agent.rs 接线**

`agent.rs:357` 的 model_override 解析后（`let model_override = match (params.model_override, ...)` 块之后、`RunRequest` 构造之前）：

```rust
        let model_override =
            apply_moa_selector_semantics(&session_key.to_key_string(), model_override);
```

（`session_key` 类型以现场为准——RunRequest 用 `session_key.clone()`，其 `to_key_string()` 在 execute.rs 有先例。）

- [ ] **Step 3: handle_catalog 追加 moa 伪条目**

先读现场确认 `CatalogEntryView` 字段（Run: `grep -n "struct CatalogEntryView" -A 20 src/gateway/handlers/providers/handlers.rs`——预期与 webchat `CatalogEntry` 镜像：id/display_name/default_model/base_url/protocol/color/homepage/notes/modalities/models/has_api_key/verified/enabled/is_default）。在 `items.retain(...)` 之后、`JsonRpcResponse::success` 之前插入：

```rust
    // Round-2 E3: MoA presets ride the picker as a pseudo-provider row.
    // Selecting one sends model_override {provider:"moa", model:<preset>},
    // which the chat.send handler converts into a session MoA activation.
    // Appended AFTER the view retain: this synthetic row is always shown
    // when presets exist (it has no credential of its own).
    if let Some(moa_cfg) = crate::providers::moa::get_moa_config() {
        let mut names: Vec<String> = moa_cfg
            .presets
            .iter()
            .filter(|(_, p)| p.enabled)
            .map(|(n, _)| n.clone())
            .collect();
        names.sort();
        if !names.is_empty() {
            let default_model = moa_cfg
                .default_preset
                .clone()
                .filter(|d| names.contains(d))
                .unwrap_or_else(|| names[0].clone());
            items.push(CatalogEntryView {
                id: "moa".to_string(),
                display_name: "Mixture of Agents".to_string(),
                default_model,
                base_url: "moa://virtual".to_string(),
                protocol: "moa".to_string(),
                color: "#8b5cf6".to_string(),
                homepage: None,
                notes: Some(
                    "MoA advisory presets — selecting one activates advisor fan-out \
                     for this session"
                        .to_string(),
                ),
                modalities: vec!["chat".to_string()],
                models: names,
                has_api_key: true,
                verified: true,
                enabled: true,
                is_default: false,
            });
        }
    }
```

（字段名与现场 `CatalogEntryView` 精确对齐；若有额外字段用其 `Default`/合理值补齐。）

- [ ] **Step 4: 跑测试**

Run: `cargo test -p alephcore --lib gateway::handlers::agent gateway::handlers::providers`
Expected: 新单测 PASS，既有 handler 测试不回归。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/agent.rs src/gateway/handlers/providers/handlers.rs
git commit -m "gateway: MoA pseudo-provider in catalog + chat.send override-to-activation intercept (E3)"
```

---

### Task 17: R4-4 — preset CRUD happy-path 测试（真实 ConfigPatcher over tempdir）

**Files:**
- Test: `src/builtin_tools/moa_manage.rs` tests 模块

**Interfaces:**
- Consumes: `ConfigPatcher::new(config: Arc<RwLock<Config>>, config_path: PathBuf, backup: ConfigBackup)`（`src/config/patcher.rs:130`）

- [ ] **Step 1: 查 `ConfigBackup` 构造与既有 patcher 测试模式**

Run: `grep -rn "ConfigPatcher::new\|ConfigBackup::" src/ --include="*.rs" | grep -i "test\|tempfile\|tempdir" | head`
Expected: 找到既有测试里 tempdir + ConfigPatcher 的组装先例（self_config 或 patcher 自身测试）。以下测试按该先例适配构造样板。

- [ ] **Step 2: 写 happy-path 测试**

```rust
    #[tokio::test]
    async fn set_preset_then_delete_roundtrip_via_real_patcher() {
        let _guard = moa_config_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Real patcher over a temp config.toml (Step 1 pattern).
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let patcher = Arc::new(ConfigPatcher::new(
            config.clone(),
            config_path.clone(),
            /* ConfigBackup — per the Step-1 discovered pattern */
        ));
        let tool = MoaManageTool::new()
            .with_config(config.clone())
            .with_patcher(patcher);

        // set_preset: writes config + hot-reloads the process-global handle.
        let out = tool
            .call(MoaManageArgs::SetPreset {
                name: "roundtrip".to_string(),
                advisors: vec![MoaSlot { provider: "openai".into(), model: "gpt-5".into() }],
                aggregator: MoaSlot { provider: "anthropic".into(), model: "opus".into() },
                fanout: None,
                advisor_timeout_secs: None,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
                set_default: Some(true),
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        let live = get_moa_config().expect("hot-reloaded");
        assert!(live.presets.contains_key("roundtrip"));
        assert_eq!(live.default_preset.as_deref(), Some("roundtrip"));

        // Second preset so delete has a survivor; then delete the default —
        // default_preset must reassign to the survivor.
        let out = tool
            .call(MoaManageArgs::SetPreset {
                name: "survivor".to_string(),
                advisors: vec![MoaSlot { provider: "openai".into(), model: "gpt-5".into() }],
                aggregator: MoaSlot { provider: "anthropic".into(), model: "opus".into() },
                fanout: None,
                advisor_timeout_secs: None,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
                set_default: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        let out = tool
            .call(MoaManageArgs::DeletePreset { name: "roundtrip".to_string() })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        let live = get_moa_config().expect("hot-reloaded after delete");
        assert!(!live.presets.contains_key("roundtrip"));
        assert_eq!(live.default_preset.as_deref(), Some("survivor"));

        store_moa_config(None);
    }
```

（`tempfile` 已是 dev-dependency 的可能性大——Run: `grep -n "tempfile" Cargo.toml`；若无则用既有测试的临时目录模式替代，**不新增依赖**。`Config::default()` 若无 Default 实现，用既有测试的最小 Config 构造先例。）

- [ ] **Step 3: 跑测试**

Run: `cargo test -p alephcore --lib builtin_tools::moa_manage`
Expected: 全 PASS（含既有 6 个 + 新 roundtrip）。

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/moa_manage.rs
git commit -m "tools: moa preset CRUD happy-path test via real ConfigPatcher (R4)"
```

---

### Task 18: 文档刷新（FEATURE_LOCATOR §4.9 / MULTI_AGENT_SYSTEM / spec 互链）

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§4.9 与 :56 索引行）
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`（MoA 小节补第二轮内容）
- Modify: `docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md`（头部加第二轮修订链接）

- [ ] **Step 1: FEATURE_LOCATOR §4.9 更新**

代码锚点行补：`src/providers/moa/fan_out.rs`（fan-out 提炼）；事件描述改为四事件含 `error`/`cached`/`billed_count` 字段与激活失败形态（`count==0`）；补 `select_model "moa:<preset>"` 入口、`providers.catalog` moa 伪条目、`chat.send model_override provider="moa"` 拦截、`aggregate_moa_advisor_usage` 开销桶、TUI 渲染、`moa_turn_trace` 面板回放块、advisor prompt-cache 断点（`mark_cache_breakpoints`）、`restore_one_shot` 构建失败回填、`moa` 已入 `default_core_tools`。**状态**行补：`✅ 第二轮优化（2026-07-05，spec 2026-07-05-moa-round2-optimization-design.md）`。

- [ ] **Step 2: MULTI_AGENT_SYSTEM.md MoA 小节补段**

「MoA (Mixture of Agents)」小节的持续咨询部分追加一段（中英按该文档现状风格）：第二轮新增——选择器集成（select_model/`providers.catalog` 双入口、互斥槽位语义）、advisor prompt-cache 断点、`[image]` 占位、审计回放（`save_traces` → `trace.by_runs` 完整顾问 I/O + 聚合器输出）、开销桶（`moa-advisors`）。

- [ ] **Step 3: 第一轮 spec 头部加链接**

状态行下加一行：`- **第二轮修订**: [2026-07-05-moa-round2-optimization-design.md](2026-07-05-moa-round2-optimization-design.md)（8 修复 / 3 连线 / 3 增强 / 重构与测试补齐）`。

- [ ] **Step 4: Commit**

```bash
git add -f docs/reference/FEATURE_LOCATOR.md docs/reference/MULTI_AGENT_SYSTEM.md docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md
git commit -m "docs: MoA round-2 — feature locator, multi-agent system, spec cross-links"
```

---

### Task 19: 收口门 + 运行时 QA

**Files:** 无新改动（验证任务；QA 中发现的缺陷按需修复并单独提交）

- [ ] **Step 1: 全量编译门（整计划唯一一次）**

Run: `cargo check --lib 2>&1 | tail -20`
Expected: 0 错误 0 警告。

Run: `just wasm`（若 Task 12 后有改动）与 `cargo check --manifest-path interfaces/tui/Cargo.toml`
Expected: 干净。

- [ ] **Step 2: 定向测试总回归**

Run: `cargo test -p alephcore --lib providers::moa providers::session_moa_handle builtin_tools::moa_manage builtin_tools::select_model builtin_tools::list_models resilience::database::traces config::types::tools && cargo test --manifest-path shared/protocol/Cargo.toml`
Expected: 全 PASS。

- [ ] **Step 3: 重编完整 macOS App 替换 daemon（刷新链见 docs/reference/DESKTOP_SHELL.md）**

Run: `just shell-build`（或按 DESKTOP_SHELL.md 的 dev 替换法——`just wasm` → 重编 server → 替换运行中 binary）

- [ ] **Step 4: 运行时 QA 清单（真实 preset，逐项目视验证）**

前置：经对话让模型 `set_preset` 建一个 2-advisor 真实 preset（走 R8 对话式配置，顺带验证 CRUD 线上路径）。

1. `/moa 解释这段代码的风险` one-shot → 面板出现 ◇ 顾问块（含全文）、◆ 聚合、▫ 开销（billed/consulted 计数）；回合结束后 `moa status` 显示未激活（one-shot 已消费）。
2. `moa on` → 连续两轮多工具对话 → `per_iteration` 每次工具迭代重新咨询；改 preset `fanout="user_turn"` 后迭代 2+ 面板显示「◆ 聚合（沿用缓存顾问意见）」不再变暗（B4）。
3. 删掉 preset 再 `/moa test` → 面板显示「⚠ MoA 未生效：…」且下一回合 one-shot 仍在（`moa status` 可见——B5 回填）。
4. 面板模型选择器出现「Mixture of Agents」分组 → 选中 preset → 下一回合走 MoA；再选普通模型 → MoA 解除（E3 互斥）。
5. `[moa] save_traces = true` 后跑一轮 → 历史会话回放（trace.by_runs 时间线）出现「📋 MoA turn trace」完整块含聚合器输出（B3+W3）。
6. `teams.usage` / team 工具 usage → `moa_advisors` 桶出现且 per_agent 无 `moa:*` 条目（B6）。
7. Telegram（或任一 channel）发 `/moa 你好` → 模型收到的消息不含 `/moa` 前缀（B7——从回复内容判断，或查 transcript）。
8. TUI（verbose 模式）跑一轮 MoA → ┊ 暗色 reasoning 区出现 Advisor/aggregating/spend 三行（W2）。
9. 带一张图的对话开 MoA → advisor 指导块含 `[image: ...]`（E4——查 save_traces 的 turn trace 顾问输入）。
10. Anthropic advisor 连续多迭代 → 第二迭代起 ProviderUsage 的 cache_read_tokens > 0（E1——查 trace 或日志）。

- [ ] **Step 5: QA 缺陷修复（如有）单独提交，格式 `<scope>: fix <issue> found in runtime QA`**

- [ ] **Step 6: 更新记忆（memory 文件按会话规范）标记第二轮 DONE 与 QA 结果**

---

## Self-Review 记录

1. **Spec 覆盖**：B1-B8（Task 3/3/4/3/5/6/7/8）✅ W1-W3（Task 9/11/10+12）✅ E1/E3/E4（Task 13/15+16/14）✅ R1-R4（Task 1/2/2/3+17）✅ 文档+QA（18/19）✅。spec §7 `display_name` 清理→Task 1 记录否决理由（`name() -> &str` 需 owned 存储）。B6 按再核实修正为「不可见→开销桶」，方向与 spec 决策一致。
2. **占位符**：无 TBD；四处「以现场为准」均附带确切发现命令与语义断言（RecordingSink trait 方法、StateDatabase 测试构造、ConfigBackup 构造、CatalogEntryView 字段）——SDD 执行者按命令发现后适配，验收由测试锁定。
3. **类型一致性**：`AdvisorResult`（Task 1 定义，Task 3 消费）；`spend_event(consulted, usages)`（Task 3 定义并自消费）；`restore_one_shot`（Task 5 定义+消费）；`aggregator_identity`（Task 8 定义+消费）；`mark_cache_breakpoints`（Task 13 定义+消费，依赖 Task 2 签名不变式）；事件字段名在 Task 3 Step 4 wire 测试锁定，Task 10/11/12 按同名消费 ✅。
