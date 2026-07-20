# Panel 玻璃二轮收口 + WASM 体积 + 连线修复 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把剩余 10 个瞬时表面迁入统一玻璃材质、把 19.5MB WASM 缩 ≥25%、兑现 routing_rules 事件连线 TODO、删 3 处死代码、纯移动拆分 1625 行 cron.rs。

**Architecture:** 全部改动限于 `interfaces/webchat/` + `Cargo.toml` profile + `justfile`。玻璃迁移是纯 class 字符串替换（卡片 `glass bg-surface-overlay/85`、遮罩加 `aleph-scrim`）；连线复用 context.rs 既有 `subscribe_events`（全局已订 `config.**`，topic=`config.changed`+`data.section`）；cron 拆分纯移动 + `pub(super)`。

**Tech Stack:** Leptos 0.7 WASM / Tailwind v4 / wasm-bindgen / wasm-opt(可选)。

**Spec:** `docs/superpowers/specs/2026-06-10-panel-glass2-perf-wiring-design.md`

**验证基线（动手前在 worktree 跑一次记录）:** `cargo test -p aleph-panel --lib` 当前应为 341 passed；`ls -la interfaces/webchat/dist/aleph_panel_bg.wasm` = 19,504,325 bytes。

**统一玻璃模式（上轮定稿，所有任务遵守）:**
- 弹出层/modal 卡片：在 class 串最前加 `glass `，背景换 `bg-surface-overlay/85`（替换原 `bg-surface`/`bg-surface-raised`/`bg-surface-base`），保留原有 border/rounded/布局类不动。
- 全屏 dim 遮罩：在 class 串里加 `aleph-scrim `（保留各自 `bg-black/NN`）。
- 不新增常驻 backdrop-filter、不加 will-change、不动 shadow 之外的布局类。

---

### Task 1: 玻璃迁移 — 菜单弹出层（chat_sidebar dropdown + project_menu）

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:795`
- Modify: `interfaces/webchat/src/views/chat/project_menu.rs:181`

- [ ] **Step 1: chat_sidebar 会话菜单加玻璃**

旧（:795，注意是多行字符串字面量内的 class）：
```
<div class="absolute right-0 top-full mt-1 z-50 min-w-[120px]
            bg-surface-raised border border-border rounded-lg shadow-lg
            py-1 text-xs">
```
新：
```
<div class="glass absolute right-0 top-full mt-1 z-50 min-w-[120px]
            bg-surface-overlay/85 border border-border rounded-lg shadow-xl
            py-1 text-xs">
```

- [ ] **Step 2: project_menu 加玻璃**

旧（:181）：
```
class="absolute z-10 left-0 bottom-full mb-1 w-64 rounded-lg border border-border-subtle bg-surface-base shadow-lg py-1"
```
新：
```
class="glass absolute z-10 left-0 bottom-full mb-1 w-64 rounded-lg border border-border bg-surface-overlay/85 shadow-xl py-1"
```

- [ ] **Step 3: 编译验证** — Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown` Expected: clean
- [ ] **Step 4: Commit** — `git add -A && git commit -m "panel: glass material for chat session menu + project menu"`

### Task 2: 玻璃迁移 — 设置页 modal 群（7 个）

**Files:**
- Modify: `interfaces/webchat/src/views/settings/skills.rs:546,559,1012-1013`
- Modify: `interfaces/webchat/src/views/settings/plugins.rs:366-367`
- Modify: `interfaces/webchat/src/views/settings/mcp.rs:377-378`
- Modify: `interfaces/webchat/src/views/settings/network/cluster.rs:144-145`
- Modify: `interfaces/webchat/src/views/settings/network/connection.rs:114-115`
- Modify: `interfaces/webchat/src/views/pairing_modal.rs:234-236`

每处两个编辑：遮罩 div 加 `aleph-scrim`；卡片 div 加 `glass` + 背景换 `bg-surface-overlay/85`。

- [ ] **Step 1: skills.rs 详情 modal（:546 遮罩 + :559 卡片）**

遮罩旧 `class="fixed inset-0 bg-black/50 flex items-center justify-center z-50"` → 新 `class="aleph-scrim fixed inset-0 bg-black/50 flex items-center justify-center z-50"`（注意 :546 这处遮罩有 on:click 关闭逻辑用 `contains("fixed")` 判定，加类不影响）。
卡片旧 `class="bg-surface border border-border rounded-lg w-full max-w-lg mx-4 max-h-[85vh] flex flex-col overflow-hidden"` → 新 `class="glass bg-surface-overlay/85 border border-border rounded-lg w-full max-w-lg mx-4 max-h-[85vh] flex flex-col overflow-hidden"`

- [ ] **Step 2: skills.rs 添加 modal（:1012 遮罩 + :1013 卡片）**

遮罩同 Step 1 模式。卡片旧 `class="bg-surface border border-border rounded-lg p-6 max-w-md w-full mx-4"` → 新 `class="glass bg-surface-overlay/85 border border-border rounded-lg p-6 max-w-md w-full mx-4"`

- [ ] **Step 3: plugins.rs（:366 遮罩 + :367 卡片）** — 同 Step 2 卡片模式（class 串完全相同）。
- [ ] **Step 4: mcp.rs（:377 遮罩 + :378 卡片）** — 同 Step 2 卡片模式。
- [ ] **Step 5: cluster.rs（:144 遮罩 bg-black/40 + :145 卡片）**

卡片旧 `class="bg-surface-raised rounded-lg border border-border p-6 max-w-md w-full space-y-4"` → 新 `class="glass bg-surface-overlay/85 rounded-lg border border-border p-6 max-w-md w-full space-y-4"`

- [ ] **Step 6: connection.rs（:114 遮罩 bg-black/40 + :115 卡片）**

卡片旧 `class="bg-surface-raised rounded-lg border border-border p-6 max-w-md space-y-4"` → 新 `class="glass bg-surface-overlay/85 rounded-lg border border-border p-6 max-w-md space-y-4"`

- [ ] **Step 7: pairing_modal.rs（:234 遮罩 bg-black/60 + :236 卡片）**

卡片旧 `class="bg-surface border border-border rounded-xl p-8 max-w-md w-full mx-4 shadow-2xl"` → 新 `class="glass bg-surface-overlay/85 border border-border rounded-xl p-8 max-w-md w-full mx-4 shadow-2xl"`

- [ ] **Step 8: 编译验证** — `cargo check -p aleph-panel --target wasm32-unknown-unknown` Expected: clean
- [ ] **Step 9: Commit** — `git commit -am "panel: glass material + scrim blur for settings/pairing modals"`

### Task 3: 玻璃迁移 — Teams（task_drawer + create_form）

**Files:**
- Modify: `interfaces/webchat/src/views/teams/components/task_drawer.rs:191-192`
- Modify: `interfaces/webchat/src/views/teams/components/create_form.rs:118-119`

- [ ] **Step 1: task_drawer**

遮罩旧 `<div class="absolute inset-0 bg-black/30" on:click=close></div>` → 新 `<div class="aleph-scrim absolute inset-0 bg-black/30" on:click=close></div>`
抽屉旧 `class="relative w-96 h-full bg-surface border-l border-border shadow-xl flex flex-col"` → 新 `class="glass relative w-96 h-full bg-surface-overlay/85 border-l border-border shadow-xl flex flex-col"`

- [ ] **Step 2: create_form**

遮罩旧 `<div class="absolute inset-0 bg-black/40" on:click=move |_| close()></div>` → 新 `<div class="aleph-scrim absolute inset-0 bg-black/40" on:click=move |_| close()></div>`
卡片旧 `class="relative w-[26rem] max-w-[92vw] max-h-[90vh] bg-surface border border-border rounded-xl shadow-xl flex flex-col"` → 新 `class="glass relative w-[26rem] max-w-[92vw] max-h-[90vh] bg-surface-overlay/85 border border-border rounded-xl shadow-xl flex flex-col"`

- [ ] **Step 3: 编译验证** — `cargo check -p aleph-panel --target wasm32-unknown-unknown`
- [ ] **Step 4: Commit** — `git commit -am "panel: glass material for teams task drawer + create form"`

### Task 4: 死代码删除（agent_run.rs / tooltip.rs / 死 CSS）

**Files:**
- Delete: `interfaces/webchat/src/api/agent_run.rs`
- Modify: `interfaces/webchat/src/api.rs:19,46`（删 `pub mod agent_run;` 与 `pub use agent_run::*;`）
- Delete: `interfaces/webchat/src/components/ui/tooltip.rs`
- Modify: `interfaces/webchat/src/components/ui/mod.rs`（删 `mod tooltip;` 声明与 `pub use tooltip::Tooltip;`）
- Modify: `interfaces/webchat/styles/tailwind.css:637-656`（删 `.skeleton/.skeleton-line/.skeleton-line-sm/.skeleton-block/.skeleton-avatar` 块）、`:1246-1248`（删 `.aleph-chrome-row-h`）

- [ ] **Step 1: 删前复核零消费者**（红线纪律，逐条跑）：

```bash
rg -n 'AgentApi|agent_run' interfaces/webchat/src/ --type rust   # 仅 api.rs 两行 + 文件自身
rg -n 'Tooltip' interfaces/webchat/src/ --type rust               # 仅 ui/mod.rs export + doc 注释
rg -n 'skeleton|aleph-chrome-row-h' interfaces/webchat/src/       # 零命中
```
- [ ] **Step 2: 执行删除**（`git rm` 两文件 + Edit 三处）。注意 tailwind.css 删 CSS 块时连同其上的注释行与 `@keyframes aleph-skeleton-shimmer`（若 grep 确认零其它引用）。
- [ ] **Step 3: 验证** — `cargo check -p aleph-panel --target wasm32-unknown-unknown` clean；`rg 'aleph-skeleton-shimmer' interfaces/webchat/` 零命中
- [ ] **Step 4: Commit** — `git commit -am "panel: delete dead AgentApi, Tooltip component, skeleton CSS"`

### Task 5: routing_rules 事件连线 + context.rs unwrap 修复

**Files:**
- Modify: `interfaces/webchat/src/views/settings/routing_rules.rs:41-47`
- Modify: `interfaces/webchat/src/context.rs:1037`

- [ ] **Step 1: routing_rules 订阅 config.changed**

把现有空 Effect（含 TODO 注释）整块替换为（镜像 runtimes.rs:40 模式；`config.**` 已在 context.rs:628 全局订阅，无需 subscribe_topic）：

```rust
    // Reload when routing rules change elsewhere (another client / CLI).
    // The panel's connection already subscribes to `config.**` globally
    // (context.rs), so we only register a local event handler here.
    {
        let state = state.clone();
        let handler_id = state.subscribe_events(move |ev| {
            if ev.topic != "config.changed" {
                return;
            }
            let section = ev.data.get("section").and_then(|s| s.as_str());
            if section != Some("routing_rules") {
                return;
            }
            let state = state.clone();
            spawn_local(async move {
                if let Ok(list) = RoutingRulesApi::list(&state).await {
                    rules.set(list);
                }
            });
        });
        on_cleanup(move || {
            let state = expect_context::<DashboardState>();
            state.unsubscribe_events(handler_id);
        });
    }
```

实现注意：`DashboardState` 是 `Clone`（看 runtimes.rs 用法）；若 `subscribe_events` 回调签名/捕获方式编译报错，以 runtimes.rs:36-76 实际写法为准对齐（包括 on_cleanup 里拿 state 的方式——若 runtimes.rs 是直接 move 一个 clone 进 on_cleanup，照抄那种）。文件头 doc 注释 "Real-time updates via config events" 由此兑现。

- [ ] **Step 2: context.rs:1037 unwrap 修复**

旧：
```rust
let _ = web_sys::window().unwrap().location().reload();
```
新：
```rust
if let Some(w) = web_sys::window() {
    let _ = w.location().reload();
}
```

- [ ] **Step 3: 验证** — `cargo check -p aleph-panel --target wasm32-unknown-unknown` clean
- [ ] **Step 4: Commit** — `git commit -am "panel: wire routing-rules live reload via config.changed; drop window unwrap"`

### Task 6: WASM 体积优化（profile + wasm-opt）

**Files:**
- Modify: `Cargo.toml:497-499`（`[profile.wasm-release]`）
- Modify: `justfile`（`wasm` recipe，wasm-bindgen 之后）

- [ ] **Step 1: profile 加尺寸优化**

旧：
```toml
[profile.wasm-release]
inherits = "release"
strip = false
```
新：
```toml
[profile.wasm-release]
inherits = "release"
strip = false
opt-level = "z"
lto = true
codegen-units = 1
```

- [ ] **Step 2: justfile wasm recipe 加条件 wasm-opt**

在 `# 3. Generate JS bindings` 的 wasm-bindgen 命令之后、`# 4. Runtime index.html` 之前插入：

```bash
    # 3.5 Shrink wasm (optional; -g keeps the name section for crash diagnostics)
    if command -v wasm-opt >/dev/null 2>&1; then
        wasm-opt -Oz -g {{panel_dist}}/aleph_panel_bg.wasm -o {{panel_dist}}/aleph_panel_bg.wasm
        echo "✓ wasm-opt applied"
    else
        echo "⚠ wasm-opt not found; skipping (brew install binaryen)"
    fi
```

- [ ] **Step 3: 尝试安装 binaryen（可失败，不阻塞）** — Run: `brew install binaryen`（若失败记录原因继续，wasm-opt 步骤是条件化的）
- [ ] **Step 4: 构建并记录尺寸** — Run: `just wasm && ls -la interfaces/webchat/dist/aleph_panel_bg.wasm` Expected: 构建成功；记录新字节数（目标 ≤14.6MB，即 -25%；若仅 profile 生效无 wasm-opt，任何明显缩减都记录）
- [ ] **Step 5: Commit（不含 dist，dist 在 Task 8 统一重建提交）** — `git add Cargo.toml justfile && git commit -m "build: size-optimize wasm-release profile + optional wasm-opt step"`

### Task 7: cron.rs 纯移动拆分

**Files:**
- Delete: `interfaces/webchat/src/views/cron.rs`（1625 行）
- Create: `interfaces/webchat/src/views/cron/mod.rs`（CronView 311-377 段 + 模块声明）
- Create: `interfaces/webchat/src/views/cron/helpers.rs`（行 16-305 全部辅助函数 + 行 1461-1495 `#[cfg(test)] mod tests`）
- Create: `interfaces/webchat/src/views/cron/job_list.rs`（行 378-625：QuickCreatePreset/`cron_quick_create_presets` + `JobList` + `JobListItem`）
- Create: `interfaces/webchat/src/views/cron/job_editor.rs`(行 626-1456：`JobEditor`)
- Create: `interfaces/webchat/src/views/cron/run_history.rs`（行 1496-末尾：`RunHistory`）

规则：**纯移动**——不改任何函数体；`views/mod.rs` 的 `pub mod cron;` 目录模块自动兼容，调用方 `cron::CronView` 不变。

- [ ] **Step 1: 读 cron.rs 全文**，记录每个段落的 `use` 依赖与跨段调用（哪些 helper 被哪个组件用）。
- [ ] **Step 2: 创建 5 个文件**。每个文件顶部补齐该段实际需要的 `use`（从原文件头 use 列表按需挑选）；跨文件引用：helpers 函数与 `QuickCreatePreset` 等改 `pub(super) fn`/`pub(super) struct`，mod.rs 中 `mod helpers; mod job_editor; mod job_list; mod run_history;` + 组件间 `use helpers::*;` 风格按需引入；对外仅 `pub use`/`pub fn CronView`。tests mod 留在 helpers.rs 内（测试只测纯函数）。
- [ ] **Step 3: 删除旧文件** — `git rm interfaces/webchat/src/views/cron.rs`（git 会把新目录识别为新增；纯移动以 Step 4 行为等价验证为准）
- [ ] **Step 4: 验证** — `cargo test -p aleph-panel --lib` Expected: 与基线相同数量 passed（cron tests 随 helpers 迁移不丢）；`cargo check -p aleph-panel --target wasm32-unknown-unknown` clean
- [ ] **Step 5: Commit** — `git add -A && git commit -m "panel: split 1625-line cron.rs into views/cron/ modules (pure move)"`

### Task 8: 终验 + dist 重建

- [ ] **Step 1: 全量验证**

```bash
cargo test -p aleph-panel --lib          # 期望 ≥ 基线 341
cargo clippy -p aleph-panel --target wasm32-unknown-unknown 2>&1 | grep -c warning  # 触及文件零新增
just wasm                                 # i18n 编译校验 + dist 重建（含 wasm-opt 若可用）
ls -la interfaces/webchat/dist/aleph_panel_bg.wasm   # 记录最终尺寸
```

- [ ] **Step 2: 提交 dist** — `git add interfaces/webchat/dist && git commit -m "panel: rebuild dist (glass round-2 + size-optimized wasm)"`
- [ ] **Step 3: 独立 code-review**（feature-dev:code-reviewer agent，对 worktree 分支全 diff）；CRITICAL/HIGH 必修。
- [ ] **Step 4: 合并** — 回主仓 `git merge --no-ff`，合并前 `git log <base>..main -- <触及文件>` 验并发零重叠。

## Self-Review 记录

- Spec 覆盖：A→Task1-3（10 表面全数对应）；B→Task6+Task4(死CSS)；C→Task4-5；D→Task7；验证→Task8。无缺口。
- 占位符扫描：无 TBD/TODO；Task 5 给出完整代码并指明编译兜底参照（runtimes.rs 实写法）。
- 类型一致性：仅 Task 5 引入新代码，类型均来自既有 API（RoutingRulesApi::list / subscribe_events / on_cleanup），与 runtimes.rs 同型。
