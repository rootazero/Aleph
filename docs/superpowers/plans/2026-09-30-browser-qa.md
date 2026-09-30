# browser_qa 单调用诊断裁决（C3）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 第 28 个工具 `browser_qa`：attached 模式单调用诊断裁决（文本/选择器断言 + console/错误/网络健康检查 + 基线减法 + 失败分级 + 三态 verdict），顺带交付 tab 死亡检测器（关 C2 残余 + TabGone 即时裁决）。

**Architecture:** 方案 A（已裁定）——QA 住工具层，编排现有 trait 动词（`wait_for`/`console_messages`/`network_log`/screenshot，backend.rs:163/168）+ 纯本地逻辑（基线减法计数对齐 + 分级器数据表）；backend 唯一增量是死亡检测器（事件泵 arm `Target.targetDestroyed`，events.rs:254 `set_discover_targets(true)` 已在调——只差 destroyed arm）+ `WaitCondition::SelectorGone` 枚举变体（types.rs:76-90，复用同一 evaluate 轮询机械）。

**Tech Stack:** Rust / tokio / aleph-cdp（Target 域 5 方法已齐全，零新 CDP 代码）/ FakeCdpServer（`push_event` 已有）。

**Spec:** `docs/superpowers/specs/2026-09-30-browser-qa-design.md`

## Global Constraints

- **诚实三角**：skip→unverified（注明原因）、证据不足→unverified、实测不符→fail。`passed=true` ⟺ failed_checks 空 **且** unverified 空。绝不让 skip 冒充 pass。
- **断言失败 ≠ 工具错误**：期望超时/不符进 `failed_checks`，工具返回**成功** + verdict。只有引擎级故障走 `backend_error_text` 咽喉。
- **基线减法按计数对齐**：规范化行文本为指纹，基线中 N 次 → 终读前 N 次不计新增；匹配额度内的行进 `unverified`（诚实点：同指纹并发新行会漏报——这是设计已承认的取舍，unverified 数组是泄压阀）。
- **零新引擎原语**：QA 不动 backend.rs（无新 trait 方法——自然避开 `async fn ` census 陷阱）；唯一类型变更是 `WaitCondition::SelectorGone`（两后端的 wait_for 实现都要接新臂，穷尽 match 会强制）。
- **九处注册**（C2 教训：不是七处）：① `browser_tools/mod.rs` 导出 **及其内的 `approval_wiring_census::SOURCES`（mod.rs:1526）** ② `definitions.rs`（条目+census 断言+session-less fall-through）③ `groups.rs` ④ `builder/constructor/mod.rs` ⑤ `registry/struct_def.rs` ⑥ `registry/tool_registry_impl.rs` ⑦ `builtin_tools/mod.rs` 顶层 re-export ⑧ `tabs.rs::BROWSER_TOOL_SOURCES`（tabs.rs:443）⑨ dialog-gate census 放置（actions.rs:3012——browser_qa 纯读 **ungated**，须登记为 ungated）。browser_qa 不过 approval gate → **不需要** `approval/types.rs` ActionType 新变体（与 record 不同，注意区分）。
- **R9 字节纪律**：DESCRIPTION ≤80B（spec 已实测顶格 80B 的版本，别改文案）；ceiling 棘轮现 116,150B，超限走三问流程记账。
- **TDD + 每条守卫先证伪**（变异→红→恢复并记录）；英文 commit `<scope>: <description>`；内存守卫（cargo 前查 MemAvailable≥4G，`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`）；rustfmt 只碰目标文件（提交前 `git diff --stat` 确认）。
- 既存红基线 6 条 + pty flaky（memory 有清单：skill_doc_drift×2、extension validation、btw_wire、a2a bearer、harness 预算）不算回归。
- 工作区：`/home/zou/data/workspace/Aleph-browser-r3`，分支 `browser-native-r3`。禁碰其他检出；真机 QA 前 `pgrep -f aleph-server` 确认无残留。

## Review Focus

1. **skip 冒充 pass**：driver 缺某维度（playwright-cli 的不可观测面）→ 该检查必须进 unverified 且 passed=false。→ T3 钉。
2. **断言失败被抛成工具错误**：模型拿到裸 error 丢掉三数组结构。→ T3 钉（failed_checks 非空时工具返回成功）。
3. **基线计数对齐的边界**：同指纹行基线 N 次、终读 N+1 次 → 第 N+1 次算新增 fail；终读恰 N 次 → 全进 unverified。→ T2 钉。
4. **destroyed arm 误伤 popup 收养**：targetCreated/targetDestroyed 同泵共存；targetCreated 的收养逻辑不得被新 arm 影响；且死亡事件须触发录制自动收尾（C2 残余关门）。→ T1 钉。
5. **SelectorGone 与 TextGone 语义混淆**：选择器存在性 vs 文本内容——两个变体的轮询 JS 不同，串了会静默误判。→ T1/T3 各钉一层。

---

### Task 1: tab 死亡检测器 + WaitCondition::SelectorGone

**Files:**
- Modify: `src/browser/cdp_backend/events.rs`（destroyed arm，:254/:279 先例旁）
- Modify: `src/browser/tab_registry.rs` 或 TabTable 所在处（死亡折叠；`resolve_identity` :246-265 的 live_target_ids 主动枚举升级为事件推送）
- Modify: `src/browser/cdp_backend/recording.rs`（tab 死亡事件 → 录制自动收尾——C2 残余关门；录制 supervisor 现只靠 `conn.closed()` watch 的 socket 级死亡臂）
- Modify: `src/browser/types.rs`（`WaitCondition::SelectorGone` 变体）+ 两后端 wait_for 实现的新臂（cdp `wait_probe.rs`、playwright-cli 侧——穷尽 match 指路）
- Test: 各文件 `#[cfg(test)]` + FakeCdpServer 集成

**Interfaces:**
- Consumes: `aleph_cdp::methods::target::*`（5 方法已齐全）；事件泵 loop（events.rs:254 起）；TabTable
- Produces: 死 tab 上任何后续动词 → `BrowserError::TabGone` **立即**（不等命令超时）；录制中 tab 死亡 → 自动 finalize 截断回执（`stop_by_id` 可取，complete=false）；`WaitCondition::SelectorGone(String)`

- [ ] **Step 1: 失败测试**

```rust
#[tokio::test]
async fn a_destroyed_target_folds_the_tab_and_later_calls_answer_tab_gone() {
    // FakeCdpServer push_event Target.targetDestroyed →
    // browser_state(tab) 立即 TabGone（不等超时）——钉「事件推送」而非「下次枚举发现」
}

#[tokio::test]
async fn tab_death_event_auto_finalizes_an_active_recording() {
    // 录制中 → targetDestroyed → stop_by_id 取回截断回执 complete=false
    // （与 C2 的 socket 级死亡臂并存——事件臂先到先用）
}

#[tokio::test]
async fn popup_adoption_still_works_with_the_destroyed_arm_installed() {
    // Review Focus #4：targetCreated 收养回归钉——arm 共存零干扰
}

#[tokio::test]
async fn selector_gone_waits_for_absence_not_text() {
    // Review Focus #5：页面有元素 → 等；元素移除 → 解；文本变化但元素在 → 不解
}
```

- [ ] **Step 2: 跑确认红 → Step 3: 实现**（destroyed arm 解析 targetId → TabTable 折叠 + 通知录制 registry；SelectorGone 轮询 JS = `!document.querySelector(sel)`，与 Selector 臂同机械反向）
- [ ] **Step 4: 绿 + 证伪**（删 destroyed arm → Step 1 前两条红；SelectorGone 实现错接 TextGone 逻辑 → 第四条红）+ commit `browser: targetDestroyed arm folds dead tabs; WaitCondition::SelectorGone`

---

### Task 2: QA 纯本地逻辑（基线减法 + 失败分级器）

**Files:**
- Create: `src/builtin_tools/browser_tools/qa_verdict.rs`（纯函数模块——与 qa.rs 工具分离，单测零引擎依赖）
- Test: 同文件 `#[cfg(test)]`

**Interfaces:**
- Produces（T3 消费）:
  - `pub struct Baseline { /* console/network 环尾部的规范化指纹计数 */ }`
  - `pub fn subtract(baseline: &Baseline, final_lines: &[String]) -> Subtraction { new: Vec<String>, matched: Vec<String> }`（matched = 额度内行 → unverified）
  - `pub enum Impact { Actionable, Benign }` + `pub fn classify_network_failure(url: &str, resource_hint: &str) -> Impact`（常量表 substring 匹配：favicon.ico、analytics/telemetry 域、.map sourcemap → Benign；其余 Actionable——表是数据，注释写清每行依据）
  - `pub enum CheckOutcome { Pass, Fail{detail}, Warn{detail}, Unverified{reason} }` + `pub struct Verdict { passed, failed_checks, warnings, unverified, summary }` + `pub fn assemble(...) -> Verdict`（passed 充要条件 + summary 必须报「通过 X 项、Y 项无法证实」）

- [ ] **Step 1: 失败测试**（计数对齐边界=Review Focus #3 两例；分级器表每行至少一钉；assemble 的 passed 充要条件双向钉：unverified 非空 → passed=false 且 summary 含「无法证实」）
- [ ] **Step 2: 跑确认红 → Step 3: 实现 → Step 4: 绿 + 证伪**（减法改布尔去重 → 边界红；passed 判据砍掉 unverified → 红）+ commit `browser: qa verdict logic — counted baseline subtraction and failure classifier`

---

### Task 3: browser_qa 工具

**Files:**
- Create: `src/builtin_tools/browser_tools/qa.rs`
- Modify: 九处注册（Global Constraints 清单逐处）+ `capability.rs`（`error_events` 行：CHROMIUM=Supported 同泵推断 / OBSCURA=Unsupported NOT_PROBED 注释点名 T4 探针；裁清单/NOT_PROBED/矩阵引用五处同步——C2 T3 实测是五处不是三处）
- Test: qa.rs `#[cfg(test)]`

**Interfaces:**
- Consumes: T1 的 TabGone 即时裁决与 `WaitCondition::SelectorGone`；T2 的 `Baseline/subtract/classify_network_failure/assemble/Verdict`；trait 动词 `wait_for`/`console_messages`/`network_log`/screenshot
- Produces: `browser_qa` 工具——输入字段与 verdict 形状以 spec §3 为准（expected_text/expected_selector/gone_selector/check_console/check_errors/check_network/screenshot/timeout_ms → `{passed, failed_checks, warnings, unverified, summary, evidence}`），DESCRIPTION 用 spec 实测 80B 版本原文

- [ ] **Step 1: 失败测试**

```rust
#[tokio::test]
async fn failed_expectations_return_a_successful_tool_result_with_passed_false() {
    // Review Focus #2：expected_text 不出现 → 工具 Ok + passed=false + failed_checks 含详情
}

#[tokio::test]
async fn a_dimension_the_driver_cannot_serve_lands_in_unverified_not_pass() {
    // Review Focus #1：playwright-cli profile 上跑 qa → 不可观测维度 unverified 注明 driver、
    // passed=false、summary 报「Y 项无法证实」
}

#[tokio::test]
async fn qa_on_a_dead_tab_answers_tab_gone_immediately() { /* T1 事件的端到端受益者钉 */ }

#[tokio::test]
async fn description_stays_within_the_80_byte_discipline() { /* ≤80 钉 */ }
```

- [ ] **Step 2: 跑确认红 → Step 3: 实现**（管线序按 spec §2①；等待委托 wait_for；150ms 沉淀；减法/分级调 T2 纯函数；screenshot 证据复用既有动词）→ **Step 4: 绿 + 证伪**（摘一处注册 → 对应 census 红；DESCRIPTION 超限 → ratchet 红）+ commit `browser: browser_qa tool — one-call diagnostic verdict with honest tri-state`

---

### Task 4: 探针 + 文档 + 全量验证

**Files:**
- Modify: `qa/browser_dual/caps.py`（`error_events` 行：触发页面异常 → 断言 `[error]` 行到达—— obscura 有静默缺失前科，到达才算）
- Modify: `src/browser/engine/capability.rs`（obscura 行按实测翻转或维持）
- Modify: `docs/reference/FEATURE_LOCATOR.md` §3.12（C3 段 + ①②③挂号项关账）+ `qa/README.md`

- [ ] **Step 1: caps 行 + 真机跑 + 证伪**（翻转表值 → diff 红 → 恢复）
- [ ] **Step 2: 文档**（C3 段：落地清单、三态语义、与参考的 tool_result 翻转分歧登记、死亡检测器关 C2 残余、刻意不做；挂号项关账：①关闭 ②③降级路径交付）
- [ ] **Step 3: 七条全量验证集**（基线 20,324 passed / 6 既存红——C3 新增测试 passed 变大正常，实测数字进 commit body）+ commit `docs: browser_qa — locator entry, caps probe, plan ledger`

---

## 合并与验收

单线单分支（`browser-native-r3`），T1 ∥ T2 可并行（backend vs 纯函数零交集），T3 汇合，T4 收尾。检查点：T3 后 `--lib --no-run` + `--lib browser`；T4 后七条全量。合并前：无新增红、clippy 净（2 条既存告警不算）、caps 真机绿。
