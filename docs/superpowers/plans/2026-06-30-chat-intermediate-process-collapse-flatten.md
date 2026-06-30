# Chat 中间过程 折叠 + 扁平化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Chat 中间过程改为「左简右详 + 扁平化」：左侧聊天运行中只显示一条会变的状态行、完成后每步一行；点开内联只一层扁平、溢出指向右侧「工具·详情栏」全量详情面；彻底消除多层折叠套娃（删 `JsonViewer` 递归树 + `<details>` 嵌套）。

**Architecture:** 纯 Panel 前端（Leptos/WASM）。`ToolCard` 新增 `ToolSurface{Inline,Detail}` 维度控制封顶；左右两个表面共享同一展开状态（现有 `WorkspaceState::toggle_event`）与聚焦（现有 `focus_step`，自动开 Split），新增幂等 `reveal_tool` 把「溢出 → 详情栏」连成联动。`StepStrip` 三态重做。删 `json_viewer.rs`。

**Tech Stack:** Rust + Leptos 0.7（`view!` 宏 / `RwSignal` / `Memo` / `Show` / `For`）、leptos_i18n（编译期类型化 locale，JSON 源在 `interfaces/webchat/locales/{en,zh}.json`）、`serde_json::Value`、`similar`（diff）。crate 名 `aleph-panel`。

## Global Constraints

逐条来自 spec 与项目宪法，每个 Task 隐含包含：

- **范围仅 wide**：只改 `platform/wide/views/chat/*` 与 `components/*`；不碰 `platform/phone/*`、不碰 Core/native/Server（R1/R2/R4）。
- **纯前端**：消费现有 `ChatState.messages` / `WorkspaceState.tool_payloads`，不改事件流或数据结构。
- **绝不嵌套**：内联详情只一层扁平；禁止 `<details>` 套 `<details>`、禁止递归可折叠树。
- **内联封顶常量** `MAX_INLINE_LINES = 8`（`ToolSurface::Inline`）；`ToolSurface::Detail` 不封顶。
- **推理预览** `PREVIEW_TAIL_LINES` 3 → 2。
- **locale 双写**：leptos_i18n 要求所有 locale key 集一致——每个新 key 必须**同时**加入 `locales/en.json` 与 `locales/zh.json`，否则 build 失败。代码注释英文、UI 文案中英双语。
- **i18n 编译期类型化**：`t!`/`t_string!` 引用的 key 必须先存在于 locale JSON，否则编译失败 → **locale Task 必须最先做**。
- **cargo 极度节制**（项目铁律）：实现期默认不跑全量；宿主机单测用**点名**过滤 `cargo test -p aleph-panel --lib <name>`；编译校验至多一次 `cargo check -p aleph-panel --lib`。多过滤词须放在 `--` 之后。
- **WASM 不在本计划编译**：`view!` 渲染逻辑不做 host 单测，只做 `cargo check` 编译校验 + 留待运行时 QA；纯逻辑函数才写单测（TDD）。
- **提交规范**：English commit，格式 `<scope>: <description>`。单分支 main 开发。
- **不可变优先**：新逻辑返回新值，不就地改 signal 内部结构。

---

## File Structure（决策锁定）

**修改**
- `interfaces/webchat/locales/en.json` / `zh.json` —— 新增 `tool_card.to_detail`、`chat.working`；删除 `json_viewer` 段（Task 6）。
- `interfaces/webchat/src/state/layout.rs` —— 新增 `WorkspaceState::reveal_tool`（幂等确保展开 + 聚焦）。
- `interfaces/webchat/src/components/tool_card.rs` —— 新增 `ToolSurface` 枚举 + `MAX_INLINE_LINES`；纯逻辑 `search_hits`/`flat_kv`；重写所有 `*_body` 为扁平 + 封顶；`ToolCard` 加 `surface`/`iteration` 可选 prop + 溢出行调用 `reveal_tool`；移除 `JsonViewer`/`<details>` 用法。
- `interfaces/webchat/src/components/workspace_panel.rs` —— `StepCard` 的 `ToolCard` 传 `surface=Detail` + `iteration`。
- `interfaces/webchat/src/platform/wide/views/chat/messages.rs` —— `StepStrip` 三态重做 + 纯逻辑 `latest_step_tool`/`step_narration_head`；`ToolCard` 传 `iteration`。
- `interfaces/webchat/src/platform/wide/views/chat/reasoning.rs` —— `PREVIEW_TAIL_LINES` 3→2。

**删除**
- `interfaces/webchat/src/components/json_viewer.rs`（+ `components/mod.rs:13` 的 `pub mod json_viewer;`）。

**任务依赖序**：locale(1) → reveal_tool(2) → tool_card 扁平(3) → workspace_panel(4) → messages StepStrip(5) → 删 json_viewer(6) → reasoning(7)。每个 Task 结束都能 `cargo check -p aleph-panel --lib` 通过。

---

### Task 1: 新增 locale keys（最先做）

**Files:**
- Modify: `interfaces/webchat/locales/en.json`（`tool_card` 段 ~1891；`chat` 段含 `step`/`steps` ~243）
- Modify: `interfaces/webchat/locales/zh.json`（同结构）

**Interfaces:**
- Consumes: 无。
- Produces: i18n key `tool_card.to_detail`、`chat.working`，供 Task 3/5 的 `t_string!` 引用。

- [ ] **Step 1: en.json — 在 `tool_card` 段末加两个 key（`cat_tool` 行后补逗号）**

把 `interfaces/webchat/locales/en.json` 的 tool_card 段：
```json
  "tool_card": {
    "expand_all": "Show all",
    "collapse": "Collapse",
    "cat_edit": "edit",
    "cat_write": "write",
    "cat_patch": "patch",
    "cat_read": "read",
    "cat_run": "run",
    "cat_search": "search",
    "cat_tool": "tool"
  }
```
改为：
```json
  "tool_card": {
    "expand_all": "Show all",
    "collapse": "Collapse",
    "cat_edit": "edit",
    "cat_write": "write",
    "cat_patch": "patch",
    "cat_read": "read",
    "cat_run": "run",
    "cat_search": "search",
    "cat_tool": "tool",
    "to_detail": "detail panel"
  }
```

- [ ] **Step 2: en.json — 在 `chat` 段加 `working`（紧接 `steps` 行后，补逗号）**

`interfaces/webchat/locales/en.json` 的 chat 段里：
```json
    "step": "step",
    "steps": "steps",
```
改为：
```json
    "step": "step",
    "steps": "steps",
    "working": "Working",
```

- [ ] **Step 3: zh.json — 同步 tool_card 段**

`interfaces/webchat/locales/zh.json` 的 `tool_card.cat_tool` 行后补：
```json
    "to_detail": "详情栏"
```
（即 `"cat_tool": "工具",` 之后新增 `"to_detail": "详情栏"`，注意把上一行补成逗号结尾。）

- [ ] **Step 4: zh.json — 同步 chat.working**

`interfaces/webchat/locales/zh.json` 的 `chat` 段 `steps` 行后补：
```json
    "working": "正在工作",
```
（具体中文取值以现有 zh.json 的 step/steps 用词为准；若 steps 文案不同，保持 working 与之协调。）

- [ ] **Step 5: 编译校验 + 提交**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p aleph-panel --lib 2>&1 | tail -5`
Expected: 编译通过（leptos_i18n 生成模块成功，无 "missing key in locale" 报错）。若报某 locale 缺 key，补齐再校验。

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "i18n: add tool_card.to_detail + chat.working keys"
```

---

### Task 2: `WorkspaceState::reveal_tool`（幂等确保展开 + 聚焦）

**Files:**
- Modify: `interfaces/webchat/src/state/layout.rs`（在 `focus_step`/`is_event_toggled` 附近，~245-260）
- Test: 同文件 `#[cfg(test)] mod tests`（已存在，~291 起）

**Interfaces:**
- Consumes: 现有 `WorkspaceState::focus_step(run_id, iteration)`（非 Split 时自动 `set_layout(Split)`）、`is_event_toggled(tool_id) -> bool`、`toggle_event(tool_id)`。
- Produces: `pub fn reveal_tool(&self, run_id: impl Into<String>, iteration: usize, tool_id: &str, default_open: bool)` —— 供 Task 3 的溢出行调用。语义：开右栏 + 聚焦该步 + **幂等**确保该 tool 展开（`expanded = default_open ^ is_event_toggled`；仅当前折叠时 `toggle_event`）。

- [ ] **Step 1: 写失败测试**

在 `interfaces/webchat/src/state/layout.rs` 的 `mod tests` 内追加（沿用现有 `test_ws(LayoutMode)` 辅助）：
```rust
    #[test]
    fn reveal_tool_expands_only_when_collapsed() {
        let ws = test_ws(LayoutMode::Split);
        // default_open=false, 未 toggle → 当前折叠 → reveal 应翻开（toggle_event 置位）
        assert!(!ws.is_event_toggled("t1"));
        ws.reveal_tool("r1", 2, "t1", false);
        assert!(ws.is_event_toggled("t1"), "collapsed-by-default tool should be toggled open");
        // 再次 reveal 幂等：已展开不再翻转（仍为 toggled）
        ws.reveal_tool("r1", 2, "t1", false);
        assert!(ws.is_event_toggled("t1"), "second reveal must not collapse an open tool");
    }

    #[test]
    fn reveal_tool_default_open_stays_open() {
        let ws = test_ws(LayoutMode::Split);
        // default_open=true, 未 toggle → 当前已展开 → reveal 不应 toggle（保持未翻转）
        ws.reveal_tool("r1", 1, "t2", true);
        assert!(!ws.is_event_toggled("t2"), "default-open tool must stay open without toggling");
    }

    #[test]
    fn reveal_tool_focuses_step() {
        let ws = test_ws(LayoutMode::Split);
        ws.reveal_tool("rX", 3, "t3", false);
        assert!(ws.is_step_focused("rX", 3));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p aleph-panel --lib reveal_tool 2>&1 | tail -15`
Expected: FAIL —— `no method named reveal_tool`。

- [ ] **Step 3: 实现 `reveal_tool`**

在 `interfaces/webchat/src/state/layout.rs` 的 `impl WorkspaceState`（紧接 `focus_step` 之后）加入：
```rust
    /// 打开右栏 + 聚焦该步 + 幂等确保该工具展开。
    ///
    /// `default_open` 由调用方按 `ToolKind` 传入（与卡片渲染同源），因为
    /// `toggle_event` 存的是「相对 `default_open` **翻转过**的集合」，
    /// 即 `expanded = default_open ^ is_event_toggled`。仅当前折叠时才
    /// `toggle_event`，所以重复调用是幂等的，绝不误折叠已展开的卡。
    pub fn reveal_tool(
        &self,
        run_id: impl Into<String>,
        iteration: usize,
        tool_id: &str,
        default_open: bool,
    ) {
        self.focus_step(run_id, iteration); // 已会在非 Split 时自动 set_layout(Split)
        let expanded_now = default_open ^ self.is_event_toggled(tool_id);
        if !expanded_now {
            self.toggle_event(tool_id);
        }
    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p aleph-panel --lib reveal_tool 2>&1 | tail -15`
Expected: PASS（3 个测试全绿）。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/state/layout.rs
git commit -m "panel/workspace: add reveal_tool (idempotent expand + focus)"
```

---

### Task 3: `tool_card.rs` 扁平化（枚举 + 纯逻辑 + 扁平 body + 溢出联动）

**Files:**
- Modify: `interfaces/webchat/src/components/tool_card.rs`（整体改卡片体；头部保留）
- Test: 同文件 `#[cfg(test)] mod tests`（已存在，~617 起）

**Interfaces:**
- Consumes: `WorkspaceState::reveal_tool`（Task 2）、`tool_card.to_detail` i18n（Task 1）、现有 `tool_headline`/`tool_icon`/`ToolKind`/`success_output`/`error_message`/`diff_lines`/`split_preview`。
- Produces:
  - `pub enum ToolSurface { Inline, Detail }`（`#[derive(Clone, Copy, PartialEq, Eq, Default)]`，`Inline` 为 `#[default]`）。
  - `pub const MAX_INLINE_LINES: usize = 8;`
  - `pub fn search_hits(result: &serde_json::Value) -> Vec<(String, Option<String>)>`（title, url）。
  - `pub fn flat_kv(value: &serde_json::Value) -> Vec<(String, String)>`（顶层 key → 紧凑单行 value）。
  - `ToolCard` 组件新签名：`pub fn ToolCard(run_id, tool_id, tool_name, #[prop(optional)] surface: ToolSurface, #[prop(optional)] iteration: Option<usize>)`。Task 4/5 据此传参。

- [ ] **Step 1: 写失败测试（纯逻辑 search_hits / flat_kv）**

在 `interfaces/webchat/src/components/tool_card.rs` 的 `mod tests` 内追加：
```rust
    #[test]
    fn search_hits_extracts_title_url() {
        let result = serde_json::json!({
            "Success": { "output": { "results": [
                { "title": "美股暴跌", "url": "https://a.com" },
                { "name": "关税新闻", "link": "https://b.com" },
                { "title": "无链接条目" }
            ] } }
        });
        let hits = search_hits(&result);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0], ("美股暴跌".to_string(), Some("https://a.com".to_string())));
        assert_eq!(hits[1], ("关税新闻".to_string(), Some("https://b.com".to_string())));
        assert_eq!(hits[2], ("无链接条目".to_string(), None));
    }

    #[test]
    fn search_hits_empty_when_no_results() {
        assert!(search_hits(&serde_json::json!({"Success": {"output": {}}})).is_empty());
        assert!(search_hits(&serde_json::json!({"Error": {"error": "x"}})).is_empty());
    }

    #[test]
    fn flat_kv_top_level_only_compact_nested() {
        let v = serde_json::json!({
            "name": "alpha",
            "count": 3,
            "nested": { "a": 1, "b": [2, 3] }
        });
        let kv = flat_kv(&v);
        // 顶层三个键；nested 的值压成紧凑单行 JSON，不展开成子树
        let map: std::collections::HashMap<_, _> = kv.into_iter().collect();
        assert_eq!(map.get("name").map(String::as_str), Some("alpha"));
        assert_eq!(map.get("count").map(String::as_str), Some("3"));
        assert_eq!(map.get("nested").map(String::as_str), Some("{\"a\":1,\"b\":[2,3]}"));
    }

    #[test]
    fn flat_kv_non_object_is_empty() {
        assert!(flat_kv(&serde_json::json!([1, 2, 3])).is_empty());
        assert!(flat_kv(&serde_json::json!("scalar")).is_empty());
    }

    #[test]
    fn tool_surface_defaults_inline() {
        assert_eq!(ToolSurface::default(), ToolSurface::Inline);
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p aleph-panel --lib -- search_hits flat_kv tool_surface 2>&1 | tail -15`
Expected: FAIL —— `cannot find function search_hits` / `flat_kv` / `ToolSurface`。

- [ ] **Step 3: 加枚举 + 常量 + 纯逻辑函数**

在 `interfaces/webchat/src/components/tool_card.rs` 顶部（`ToolKind` 定义附近）加入：
```rust
/// 卡片渲染表面：左侧聊天（封顶）vs 右侧详情栏（全量）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolSurface {
    /// 左侧聊天：内联只一层扁平，封顶 `MAX_INLINE_LINES`，溢出指向详情栏。
    #[default]
    Inline,
    /// 右侧「工具·详情栏」：全量扁平，不封顶。
    Detail,
}

/// 左侧内联详情封顶行数；右侧详情栏不封顶。
pub const MAX_INLINE_LINES: usize = 8;
```

在文件下方（紧邻 `success_output`/`error_message` 等纯逻辑处）加入：
```rust
/// 从搜索结果 `Success.output.results[]` 提取扁平命中列表 `(title, url)`。
/// 字段缺失时 title/url 各自回落（title: `title`→`name`→`"(untitled)"`；
/// url: `url`→`link`→None）。非预期形状返回空。
#[must_use]
pub fn search_hits(result: &Value) -> Vec<(String, Option<String>)> {
    let Some(arr) = success_output(result)
        .and_then(|o| o.get("results"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    arr.iter()
        .map(|item| {
            let title = item
                .get("title")
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("(untitled)")
                .to_string();
            let url = item
                .get("url")
                .or_else(|| item.get("link"))
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string);
            (title, url)
        })
        .collect()
}

/// 把一个 JSON 对象压成顶层 `key: value` 行；嵌套值用紧凑单行 JSON
/// （`serde_json::to_string`，无缩进），不展开成可折叠子树。非对象返回空。
#[must_use]
pub fn flat_kv(value: &Value) -> Vec<(String, String)> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    map.iter()
        .map(|(k, v)| {
            let rendered = match v {
                Value::String(s) => s.clone(),
                Value::Null => "null".to_string(),
                other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
            };
            (k.clone(), rendered)
        })
        .collect()
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p aleph-panel --lib -- search_hits flat_kv tool_surface 2>&1 | tail -15`
Expected: PASS（5 个新测试 + 现有 tool_card 测试全绿）。

- [ ] **Step 5: 重写 `render_body` 接受 surface + 封顶；扁平 search/default body**

把 `render_body` 签名与分发改为带 `surface` + `detail_label`：
```rust
/// 按工具大类渲染卡片体。`surface` 决定封顶：Inline 封顶 `MAX_INLINE_LINES`
/// 并在溢出处显示「→ 详情栏」联动行；Detail 全量。`detail_label` 为已解析的
/// 本地化「详情栏」文案。
fn render_body(
    kind: ToolKind,
    payload: &Option<ToolPayload>,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let Some(p) = payload else {
        return view! { <span class="text-text-tertiary italic text-xs">"…"</span> }.into_any();
    };
    if let Some(res) = p.result.as_ref() {
        if let Some(err) = error_message(res) {
            return view! { <pre class=format!("{MONO_BLOCK} text-danger")>{err}</pre> }.into_any();
        }
    }
    match kind {
        ToolKind::FileEdit => edit_body(p, surface, detail_label, on_overflow),
        ToolKind::FileWrite => write_body(p, surface, detail_label, on_overflow),
        ToolKind::ApplyPatch => patch_body(p, surface, detail_label, on_overflow),
        ToolKind::Bash => shell_body(p, surface, detail_label, on_overflow),
        ToolKind::FileRead => read_body(p, surface, detail_label, on_overflow),
        ToolKind::Search => search_body(p, surface, detail_label, on_overflow),
        ToolKind::Default => default_body(p, surface, detail_label, on_overflow),
    }
}
```

新增一个共用「封顶 + 溢出行」辅助（替代旧 `CollapsibleText` 的内联展开按钮，改为指向详情栏）。**`detail_label` 由 `ToolCard` 在 i18n 上下文确定可用处解析好后以 `String` 透传**——不在这些 plain fn 内调 `use_i18n()`（避免依赖 plain-fn 渲染期 context 时序）：
```rust
/// 把多行文本按 surface 封顶渲染。Inline 超过 `MAX_INLINE_LINES` 时截断并
/// 追加一行「… +N → 详情栏」（点击触发 `on_overflow`）；Detail 全量。
/// 无内层折叠——这是扁平化的核心。
fn capped_block(
    text: &str,
    extra_class: &'static str,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let cap = match surface {
        ToolSurface::Inline => MAX_INLINE_LINES,
        ToolSurface::Detail => usize::MAX,
    };
    let (shown, hidden) = split_preview(text, cap);
    view! {
        <div>
            <pre class=format!("{MONO_BLOCK} {extra_class} overflow-x-auto")>{shown}</pre>
            {(hidden > 0).then(|| overflow_line(hidden, detail_label.clone(), on_overflow.clone()))}
        </div>
    }
    .into_any()
}

/// 统一的「… +N → 详情栏」溢出联动行。`detail_label` 已是解析好的本地化
/// 文案（如 "详情栏" / "detail panel"）。
fn overflow_line(
    hidden: usize,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let label = format!("\u{2026} +{hidden} \u{2192} {detail_label}");
    view! {
        <button
            type="button"
            class="mt-1 text-[10px] text-text-tertiary hover:text-primary"
            on:click=move |ev: web_sys::MouseEvent| { ev.stop_propagation(); on_overflow(); }
        >
            {label}
        </button>
    }
    .into_any()
}
```

> 注：`overflow_line` 用 `web_sys::MouseEvent` + `stop_propagation` 防止冒泡到卡片头部的 toggle。`detail_label` 在 `ToolCard` 组件体（i18n 必然可用，文件已有 `let i18n = use_i18n();` ~246）用 `t_string!(i18n, tool_card.to_detail).to_string()` 解析后透传，故 plain fn 内无需 `use_i18n()`。

- [ ] **Step 6: 把各 `*_body` 改为带 surface；search/default 去递归树**

逐个改写（去掉 `CollapsibleText` 与 `JsonViewer`/`<details>`）。**所有 `*_body` 统一签名 `(p, surface, detail_label, on_overflow)`**：

`edit_body` / `patch_body`（diff 类，保留红绿着色 + 封顶）：
```rust
fn edit_body(p: &ToolPayload, surface: ToolSurface, detail_label: String, on_overflow: impl Fn() + Clone + 'static) -> AnyView {
    let old = arg_str(p, "old_string");
    let new = arg_str(p, "new_string");
    let (lines, _a, _r) = diff_lines(old, new);
    capped_diff(lines, surface, detail_label, on_overflow)
}

fn patch_body(p: &ToolPayload, surface: ToolSurface, detail_label: String, on_overflow: impl Fn() + Clone + 'static) -> AnyView {
    let patch = arg_str(p, "patch");
    let lines: Vec<DiffLine> = patch
        .lines()
        .map(|raw| {
            let sign = match raw.chars().next() {
                Some('+') => '+',
                Some('-') => '-',
                _ => ' ',
            };
            DiffLine { sign, text: raw.to_string() }
        })
        .collect();
    capped_diff(lines, surface, detail_label, on_overflow)
}
```
新增 `capped_diff`（红绿着色 + 封顶 + 溢出行）：
```rust
/// 红删/绿增/中性上下文的 diff 渲染，按 surface 封顶（Inline 超 MAX_INLINE_LINES
/// 截断 + 「→ 详情栏」），扁平无嵌套。
fn capped_diff(
    lines: Vec<DiffLine>,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let cap = match surface {
        ToolSurface::Inline => MAX_INLINE_LINES,
        ToolSurface::Detail => usize::MAX,
    };
    let total = lines.len();
    let hidden = total.saturating_sub(cap);
    let shown: Vec<DiffLine> = lines.into_iter().take(cap).collect();
    view! {
        <div class=format!("{MONO_BLOCK} rounded-md glass-inset overflow-x-auto")>
            {shown.into_iter().map(|l| {
                let cls = match l.sign {
                    '+' => "block px-2 bg-success/10 text-success",
                    '-' => "block px-2 bg-danger/10 text-danger",
                    _ => "block px-2 text-text-secondary",
                };
                let line = format!("{} {}", l.sign, l.text);
                view! { <span class=cls>{line}</span> }
            }).collect_view()}
            {(hidden > 0).then(|| overflow_line(hidden, detail_label.clone(), on_overflow.clone()))}
        </div>
    }
    .into_any()
}
```
删除旧的独立 `diff_view`（被 `capped_diff` 取代）和旧 `CollapsibleText` 组件（被 `capped_block` 取代）。

`write_body` / `read_body`（文本类）改用 `capped_block`：
```rust
fn write_body(p: &ToolPayload, surface: ToolSurface, detail_label: String, on_overflow: impl Fn() + Clone + 'static) -> AnyView {
    let content = arg_str(p, "content").to_string();
    capped_block(&content, "", surface, detail_label, on_overflow)
}

fn read_body(p: &ToolPayload, surface: ToolSurface, detail_label: String, on_overflow: impl Fn() + Clone + 'static) -> AnyView {
    let out = p.result.as_ref().and_then(success_output).cloned();
    let text = match out {
        Some(Value::String(s)) => s,
        Some(ref other) => other
            .get("content")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| other.to_string()),
        None => String::new(),
    };
    if text.is_empty() {
        return default_body(p, surface, detail_label, on_overflow);
    }
    capped_block(&text, "text-text-secondary", surface, detail_label, on_overflow)
}
```

`shell_body`（cmd + 尾部 stdout/stderr + exit；stdout/stderr 各自封顶）：
```rust
fn shell_body(p: &ToolPayload, surface: ToolSurface, detail_label: String, on_overflow: impl Fn() + Clone + 'static) -> AnyView {
    let cmd = {
        let v = arg_str(p, "cmd");
        if v.is_empty() { arg_str(p, "code") } else { v }
    }.to_string();
    let out = p.result.as_ref().and_then(success_output).cloned();
    let stdout = out.as_ref().and_then(|o| o.get("stdout")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let stderr = out.as_ref().and_then(|o| o.get("stderr")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let exit = out.as_ref().and_then(|o| o.get("exit_code")).and_then(serde_json::Value::as_i64);
    let exit_badge = exit.map(|c| {
        let cls = if c == 0 { "text-success" } else { "text-danger" };
        view! { <span class=format!("text-[10px] font-mono {cls}")>{format!("exit {c}")}</span> }
    });
    view! {
        <div class="flex flex-col gap-1">
            <pre class=format!("{MONO_BLOCK} text-text-primary")>{format!("$ {cmd}")}</pre>
            {(!stdout.is_empty()).then({
                let oo = on_overflow.clone();
                let dl = detail_label.clone();
                move || capped_block(&stdout, "text-text-secondary", surface, dl, oo)
            })}
            {(!stderr.is_empty()).then({
                let oo = on_overflow.clone();
                let dl = detail_label.clone();
                move || capped_block(&stderr, "text-danger/80", surface, dl, oo)
            })}
            {exit_badge}
        </div>
    }
    .into_any()
}
```

`search_body`（扁平命中列表，**删 JsonViewer**）：
```rust
fn search_body(p: &ToolPayload, surface: ToolSurface, detail_label: String, on_overflow: impl Fn() + Clone + 'static) -> AnyView {
    let Some(res) = p.result.as_ref() else {
        return default_body(p, surface, detail_label, on_overflow);
    };
    let hits = search_hits(res);
    if hits.is_empty() {
        return default_body(p, surface, detail_label, on_overflow);
    }
    let cap = match surface { ToolSurface::Inline => MAX_INLINE_LINES, ToolSurface::Detail => usize::MAX };
    let total = hits.len();
    let hidden = total.saturating_sub(cap);
    let shown: Vec<_> = hits.into_iter().take(cap).collect();
    view! {
        <div class="flex flex-col gap-1 text-xs">
            <span class="text-[10px] uppercase tracking-wider text-text-tertiary">
                {format!("{total} results")}
            </span>
            {shown.into_iter().map(|(title, url)| view! {
                <div class="flex flex-col">
                    <span class="text-text-primary truncate">{title}</span>
                    {url.map(|u| view! {
                        <span class="text-[10px] text-text-tertiary truncate">{u}</span>
                    })}
                </div>
            }).collect_view()}
            {(hidden > 0).then(|| overflow_line(hidden, detail_label.clone(), on_overflow.clone()))}
        </div>
    }
    .into_any()
}
```

`default_body`（扁平 key:value，**删 `<details>` + JsonViewer**）：
```rust
fn default_body(p: &ToolPayload, surface: ToolSurface, detail_label: String, on_overflow: impl Fn() + Clone + 'static) -> AnyView {
    // 优先展示 result，其次 args；都压成顶层扁平 key:value 行。
    let source = p.result.clone().or_else(|| p.args.clone());
    let Some(v) = source else {
        return view! { <span class="text-text-tertiary italic text-xs">"…"</span> }.into_any();
    };
    let kv = flat_kv(&v);
    if kv.is_empty() {
        // 非对象（数组/标量）→ 紧凑 pretty JSON，按 surface 封顶。
        let compact = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
        return capped_block(&compact, "text-text-secondary", surface, detail_label, on_overflow);
    }
    let cap = match surface { ToolSurface::Inline => MAX_INLINE_LINES, ToolSurface::Detail => usize::MAX };
    let total = kv.len();
    let hidden = total.saturating_sub(cap);
    let shown: Vec<_> = kv.into_iter().take(cap).collect();
    view! {
        <div class="flex flex-col gap-0.5 text-xs font-mono">
            {shown.into_iter().map(|(k, val)| view! {
                <div class="flex gap-2 min-w-0">
                    <span class="text-text-tertiary shrink-0">{format!("{k}:")}</span>
                    <span class="text-text-secondary truncate">{val}</span>
                </div>
            }).collect_view()}
            {(hidden > 0).then(|| overflow_line(hidden, detail_label.clone(), on_overflow.clone()))}
        </div>
    }
    .into_any()
}
```

删除文件顶部 `use crate::components::json_viewer::JsonViewer;`（第 7 行）。

- [ ] **Step 7: `ToolCard` 加 `surface`/`iteration` prop + 接线 on_overflow**

把 `ToolCard` 签名改为：
```rust
#[component]
#[must_use]
pub fn ToolCard(
    run_id: String,
    tool_id: String,
    tool_name: String,
    #[prop(optional)] surface: ToolSurface,
    #[prop(optional)] iteration: Option<usize>,
) -> impl IntoView {
```
**(a)** 在现有 `let run_for_payload = run_id;` / `let tid_for_payload = tool_id;`（~265-266，这两行把 `run_id`/`tool_id` move 走）**之前**，先各克隆一份给 overflow 闭包：
```rust
    let run_for_overflow = run_id.clone();
    let tid_for_overflow = tool_id.clone();
```

**(b)** 在 `let default_open = kind.default_open();`（~281）**之后**，构造 `detail_label` 与 `on_overflow`（`workspace` 是 `Option<WorkspaceState>`，`WorkspaceState` 为 Copy，可被多个 move 闭包捕获；`iteration`/`default_open` 均 Copy；Inline 溢出才会触发，Detail 不封顶不触发）：
```rust
    let detail_label = t_string!(i18n, tool_card.to_detail).to_string();
    let on_overflow = move || {
        if let (Some(ws), Some(it)) = (workspace, iteration) {
            ws.reveal_tool(run_for_overflow.clone(), it, &tid_for_overflow, default_open);
        }
    };
```

**(c)** 把渲染体的 `render_body` 调用改为传 `surface` + `detail_label` + `on_overflow`（两者按需 clone 进 reactive 闭包）：
```rust
            <Show when=move || expanded.get()>
                <div class="pl-7 pr-2 pb-2">
                    {
                        let oo = on_overflow.clone();
                        let dl = detail_label.clone();
                        move || render_body(kind, &payload.get(), surface, dl.clone(), oo.clone())
                    }
                </div>
            </Show>
```

- [ ] **Step 8: 编译校验（现有调用点靠默认值仍通过）**

Run: `cargo check -p aleph-panel --lib 2>&1 | tail -20`
Expected: 通过。messages.rs / workspace_panel.rs 的现有 `ToolCard` 调用因 `surface`/`iteration` 是 `#[prop(optional)]` 默认 Inline/None 仍编译；`json_viewer.rs` 仍存在（Task 6 才删）但 tool_card 已不引用它（json_viewer 此刻变 dead——`cargo check --lib` 对未使用的 pub 模块不报错，clippy 才会；Task 6 清理）。

- [ ] **Step 9: 跑 tool_card 单测确认无回归**

Run: `cargo test -p aleph-panel --lib tool_card 2>&1 | tail -15`
Expected: PASS（含新 5 测 + 旧测）。

- [ ] **Step 10: 提交**

```bash
git add interfaces/webchat/src/components/tool_card.rs
git commit -m "panel/tool-card: flatten bodies + ToolSurface + overflow->detail (kill JsonViewer/details nesting)"
```

---

### Task 4: 右侧详情栏用 `surface=Detail`（`workspace_panel.rs`）

**Files:**
- Modify: `interfaces/webchat/src/components/workspace_panel.rs::StepCard`（~272-280 ToolCard 渲染处）

**Interfaces:**
- Consumes: `ToolCard` 新 prop `surface`/`iteration`（Task 3）；现有 `StepGroup{ run_id, iteration, tools }`。
- Produces: 无新接口。

- [ ] **Step 1: StepCard 的 ToolCard 传 Detail + iteration**

把 `interfaces/webchat/src/components/workspace_panel.rs` 中 `StepCard` 的 tools 渲染：
```rust
                {tools
                    .into_iter()
                    .map(|(tool_id, tool_name)| {
                        view! {
                            <ToolCard run_id=run_id.clone() tool_id=tool_id tool_name=tool_name />
                        }
                    })
                    .collect_view()}
```
改为（`iteration` 是 `StepCard` 入参 `group.iteration`，已在上文 `let iteration = group.iteration;` 绑定）：
```rust
                {tools
                    .into_iter()
                    .map(|(tool_id, tool_name)| {
                        view! {
                            <ToolCard
                                run_id=run_id.clone()
                                tool_id=tool_id
                                tool_name=tool_name
                                surface=crate::components::tool_card::ToolSurface::Detail
                                iteration=Some(iteration)
                            />
                        }
                    })
                    .collect_view()}
```
确认文件顶部已 `use crate::components::tool_card::{summarize_tools, ToolCard, ToolKind};`——把它扩成含 `ToolSurface`：
```rust
use crate::components::tool_card::{summarize_tools, ToolCard, ToolKind, ToolSurface};
```
并把上面 `surface=` 改用短名 `surface=ToolSurface::Detail`。

- [ ] **Step 2: 编译校验**

Run: `cargo check -p aleph-panel --lib 2>&1 | tail -10`
Expected: 通过。

- [ ] **Step 3: 提交**

```bash
git add interfaces/webchat/src/components/workspace_panel.rs
git commit -m "panel/workspace: render StepCard tools with Detail surface"
```

---

### Task 5: `StepStrip` 三态 + 运行状态行（`messages.rs`）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/messages.rs`（`StepStrip` ~847-914；`MessageBubble` 的 ToolCard 渲染 ~531-539）
- Test: 同文件新增 `#[cfg(test)] mod step_action_tests`

**Interfaces:**
- Consumes: 现有 `ChatState::strip_is_open`/`toggle_strip`、`ChatMessage{content, tool_calls, iteration}`、`WorkspaceState::get_tool_payload`、`tool_card::{ToolKind, tool_headline, tool_icon, ToolSurface}`、`run_id_from_message_id`、i18n `chat.steps`/`chat.working`。
- Produces:
  - `fn latest_step_tool(steps: &[ChatMessage]) -> Option<(String, String)>`（最后一个带工具的步骤的最后一个工具 `(tool_id, tool_name)`）。
  - `fn step_narration_head(steps: &[ChatMessage], max_chars: usize) -> Option<String>`（最后一个非空叙述的首行，截断）。
  - 重做的 `StepStrip` 组件（三态）。

- [ ] **Step 1: 写失败测试（纯逻辑）**

在 `interfaces/webchat/src/platform/wide/views/chat/messages.rs` 末尾追加（紧邻现有 `run_id_tests`）：
```rust
#[cfg(test)]
mod step_action_tests {
    use super::{latest_step_tool, step_narration_head};
    use crate::views::chat::state::{ChatMessage, ToolCallEntry};

    fn msg(id: &str, content: &str, tools: Vec<(&str, &str)>) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: "assistant".into(),
            content: content.to_string(),
            tool_calls: tools.into_iter().map(|(tid, tn)| ToolCallEntry {
                tool_id: tid.to_string(),
                tool_name: tn.to_string(),
                status: "completed".into(),
                duration_ms: None,
            }).collect(),
            is_streaming: false,
            is_intermediate: true,
            error: None,
            model_info: None,
            iteration: Some(1),
            timestamp: None,
            is_final: false,
            text_finalized: false,
            agent_id: None,
            plan_archive: None,
        }
    }

    #[test]
    fn latest_step_tool_picks_last_tool_of_last_step_with_tools() {
        let steps = vec![
            msg("intermediate-r1-1", "searching", vec![("t1", "search")]),
            msg("intermediate-r1-2", "editing", vec![("t2", "file_edit"), ("t3", "bash")]),
        ];
        assert_eq!(
            latest_step_tool(&steps),
            Some(("t3".to_string(), "bash".to_string()))
        );
    }

    #[test]
    fn latest_step_tool_skips_toolless_tail() {
        let steps = vec![
            msg("intermediate-r1-1", "searching", vec![("t1", "search")]),
            msg("intermediate-r1-2", "thinking out loud", vec![]),
        ];
        assert_eq!(
            latest_step_tool(&steps),
            Some(("t1".to_string(), "search".to_string()))
        );
    }

    #[test]
    fn latest_step_tool_none_when_no_tools() {
        let steps = vec![msg("intermediate-r1-1", "just text", vec![])];
        assert_eq!(latest_step_tool(&steps), None);
    }

    #[test]
    fn step_narration_head_takes_last_nonempty_first_line_truncated() {
        let steps = vec![
            msg("intermediate-r1-1", "first step narration", vec![]),
            msg("intermediate-r1-2", "second step\nwith two lines", vec![]),
        ];
        assert_eq!(
            step_narration_head(&steps, 100).as_deref(),
            Some("second step")
        );
        // 截断（UTF-8 安全）
        assert_eq!(
            step_narration_head(&steps, 6).as_deref(),
            Some("second")
        );
    }

    #[test]
    fn step_narration_head_none_when_all_empty() {
        let steps = vec![msg("intermediate-r1-1", "", vec![])];
        assert_eq!(step_narration_head(&steps, 50), None);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p aleph-panel --lib -- latest_step_tool step_narration_head 2>&1 | tail -15`
Expected: FAIL —— `cannot find function latest_step_tool` / `step_narration_head`。

- [ ] **Step 3: 实现两个纯函数**

在 `interfaces/webchat/src/platform/wide/views/chat/messages.rs`（`run_id_from_message_id` 附近的自由函数区）加入：
```rust
/// 最后一个带工具调用的步骤的最后一个工具 `(tool_id, tool_name)`。
/// 用于运行中状态行的「最新动作」。无任何工具时返回 None。
fn latest_step_tool(steps: &[ChatMessage]) -> Option<(String, String)> {
    steps
        .iter()
        .rev()
        .find_map(|m| m.tool_calls.last())
        .map(|t| (t.tool_id.clone(), t.tool_name.clone()))
}

/// 最后一个非空叙述的首行，UTF-8 安全截断到 `max_chars` 个字符。
fn step_narration_head(steps: &[ChatMessage], max_chars: usize) -> Option<String> {
    let raw = steps
        .iter()
        .rev()
        .map(|m| m.content.trim())
        .find(|c| !c.is_empty())?;
    let first_line = raw.lines().next().unwrap_or(raw);
    let truncated: String = first_line.chars().take(max_chars).collect();
    Some(truncated)
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p aleph-panel --lib -- latest_step_tool step_narration_head 2>&1 | tail -15`
Expected: PASS（5 测全绿）。

- [ ] **Step 5: 重做 `StepStrip` 三态**

先扩展文件顶部导入（现为 `use crate::components::tool_card::ToolCard;`）：
```rust
use crate::components::tool_card::{tool_headline, tool_icon, ToolCard, ToolKind};
```
（`tool_headline`/`tool_icon`/`ToolKind` 均为 `tool_card` 的 pub 项；`WorkspaceState` 已在 line 14 导入。）

再把 `interfaces/webchat/src/platform/wide/views/chat/messages.rs` 的整个 `StepStrip` 组件替换为：
```rust
/// 一个 run 的中间步骤容器，三态：
/// - 运行中（`!completed`）：默认收起成「一条会变的状态行」（图标 + 最新动作 +
///   spinner，副行 `└ N 步 · 叙述`）；点击展开成扁平步骤流。
/// - 完成（`completed`）：收起成 `✓ N 步 · 末步摘要`；点击展开。
/// - 展开（任一态）：扁平步骤流，每步一行（无内层滚动条、无嵌套）。
///
/// 展开/收起按 `run_id` 存于 `ChatState`（`strip_open`），承受 keyed `<For>`
/// 的每 token 重挂载（见 `ChatState::strip_open` 注释）。
#[component]
fn StepStrip(steps: Vec<ChatMessage>, completed: bool) -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();
    let run_id = steps
        .first()
        .map(|m| run_id_from_message_id(&m.id))
        .unwrap_or_default();
    // 运行中默认收起（一条会变的行）；完成默认收起。两态默认都收起。
    let open = {
        let run = run_id.clone();
        Memo::new(move |_| chat.strip_is_open(&run, false))
    };
    let count = steps.len();
    let word = t_string!(i18n, chat.steps).to_string();

    // 收起态摘要文案：运行中 = 最新动作 headline；完成 = 末步摘要。
    let steps_for_summary = steps.clone();
    let summary_main = {
        let ws = workspace;
        let run = run_id.clone();
        move || {
            if let Some((tool_id, tool_name)) = latest_step_tool(&steps_for_summary) {
                let kind = ToolKind::from_name(&tool_name);
                let payload = ws.and_then(|w| w.get_tool_payload(&run, &tool_id));
                let icon = tool_icon(&tool_name, kind);
                let headline = tool_headline(kind, &payload).unwrap_or_else(|| {
                    step_narration_head(&steps_for_summary, 60)
                        .unwrap_or_else(|| t_string!(i18n, chat.working).to_string())
                });
                format!("{icon} {headline}")
            } else {
                step_narration_head(&steps_for_summary, 60)
                    .unwrap_or_else(|| t_string!(i18n, chat.working).to_string())
            }
        }
    };

    let run_for_toggle = run_id.clone();
    view! {
        <div class="my-1">
            <div class="w-full rounded-lg glass-inset">
                <button
                    type="button"
                    class="w-full flex flex-col gap-0.5 px-3 py-1.5 text-left
                           text-text-tertiary hover:text-text-secondary"
                    on:click=move |_| chat.toggle_strip(&run_for_toggle, false)
                >
                    <span class="flex items-center gap-2 text-sm">
                        {move || if completed {
                            view! { <span class="text-success shrink-0">"\u{2713}"</span> }.into_any()
                        } else {
                            view! { <span class="shrink-0 inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span> }.into_any()
                        }}
                        <span class="flex-1 min-w-0 truncate">{summary_main}</span>
                        <span class="shrink-0 text-[10px]">
                            {move || if open.get() { "\u{25BE}" } else { "\u{25B8}" }}
                        </span>
                    </span>
                    <span class="text-[10px] uppercase tracking-wider text-text-tertiary/80 pl-3.5">
                        {format!("{count} {word}")}
                    </span>
                </button>
                <Show when=move || open.get()>
                    <div class="px-2 pb-2 flex flex-col gap-1">
                        {steps
                            .clone()
                            .into_iter()
                            .map(|m| view! { <MessageBubble message=m clock=String::new() in_strip=true /> })
                            .collect_view()}
                    </div>
                </Show>
            </div>
        </div>
    }
}
```
> 与旧版差异：(1) 默认收起改为 `strip_is_open(&run, false)`（两态都默认收起）；(2) 收起态从单行 `N steps` 升级为「图标+最新动作」主行 + `N 步` 副行；(3) 展开态去掉 `max-h-[220px] overflow-y-auto` 内层滚动条（改为自然流，扁平）；(4) 删掉旧的 stick-to-bottom `scroll_ref` Effect（不再有内层滚动窗）。确保删除旧 `StepStrip` 里 `scroll_ref` / `Effect::new` 相关代码。

- [ ] **Step 6: MessageBubble 的 ToolCard 传 iteration（左侧 Inline 溢出联动需要）**

把 `interfaces/webchat/src/platform/wide/views/chat/messages.rs` 中 `MessageBubble` 的 `tool_calls_view`：
```rust
        Some(view! {
            <div class="mb-2 flex flex-col gap-1">
                {tools.into_iter().map(|tc| {
                    view! {
                        <ToolCard
                            run_id=run_for_cards.clone()
                            tool_id=tc.tool_id.clone()
                            tool_name=tc.tool_name
                        />
                    }
                }).collect::<Vec<_>>()}
            </div>
        })
```
改为（`message.iteration` 在该作用域可读——`msg_iteration` 已在上文 `let msg_iteration = message.iteration;` 绑定）：
```rust
        let it_for_cards = msg_iteration;
        Some(view! {
            <div class="mb-2 flex flex-col gap-1">
                {tools.into_iter().map(|tc| {
                    view! {
                        <ToolCard
                            run_id=run_for_cards.clone()
                            tool_id=tc.tool_id.clone()
                            tool_name=tc.tool_name
                            iteration=it_for_cards
                        />
                    }
                }).collect::<Vec<_>>()}
            </div>
        })
```
（`surface` 不传 → 默认 `Inline`，正确。`iteration` 为 `Option<usize>` 直接传 `msg_iteration`。）

- [ ] **Step 7: 编译校验 + 单测**

Run: `cargo check -p aleph-panel --lib 2>&1 | tail -15`
Expected: 通过。
Run: `cargo test -p aleph-panel --lib -- latest_step_tool step_narration_head run_id_from 2>&1 | tail -15`
Expected: PASS。

- [ ] **Step 8: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/messages.rs
git commit -m "panel/chat: StepStrip three-state (running=one changing line) + thread iteration to ToolCard"
```

---

### Task 6: 删除 `json_viewer.rs` + 清理引用

**Files:**
- Delete: `interfaces/webchat/src/components/json_viewer.rs`
- Modify: `interfaces/webchat/src/components/mod.rs:13`（删 `pub mod json_viewer;`）
- Modify: `interfaces/webchat/locales/en.json` / `zh.json`（删 `json_viewer` 段）

**Interfaces:**
- Consumes: 无（确认 Task 3 已移除 tool_card 对 JsonViewer 的引用）。
- Produces: 无。

- [ ] **Step 1: 确认无残余引用**

Run: `grep -rn "json_viewer\|JsonViewer" interfaces/webchat/src`
Expected: 仅 `components/mod.rs:13` 与 `components/json_viewer.rs` 自身（无其它消费者）。若出现别处，停下排查。

- [ ] **Step 2: 删除文件 + mod 声明**

```bash
git rm interfaces/webchat/src/components/json_viewer.rs
```
编辑 `interfaces/webchat/src/components/mod.rs`，删除第 13 行：
```rust
pub mod json_viewer;
```

- [ ] **Step 3: 删 locale `json_viewer` 段**

`interfaces/webchat/locales/en.json` 与 `zh.json` 各有：
```json
  "json_viewer": {
    "copy": "..."
  },
```
（约第 1879 行起）整段删除（注意删掉后保持 JSON 合法——前一段的逗号/括号正确）。

- [ ] **Step 4: 编译校验**

Run: `cargo check -p aleph-panel --lib 2>&1 | tail -15`
Expected: 通过（i18n 重新生成，不再含 `json_viewer.copy`；无引用残留）。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/components/mod.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: delete unused json_viewer (flattened tool bodies replace recursive tree)"
```

---

### Task 7: 推理预览 3 → 2（`reasoning.rs`）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/reasoning.rs:23`

**Interfaces:**
- Consumes: 无。
- Produces: 无。

- [ ] **Step 1: 改常量**

把 `interfaces/webchat/src/platform/wide/views/chat/reasoning.rs`：
```rust
const PREVIEW_TAIL_LINES: usize = 3;
```
改为：
```rust
const PREVIEW_TAIL_LINES: usize = 2;
```
（现有 `tail_lines` 测试用字面量 3/2，不依赖此常量，保持绿。）

- [ ] **Step 2: 编译校验 + reasoning 单测**

Run: `cargo test -p aleph-panel --lib tail_lines 2>&1 | tail -10`
Expected: PASS。

- [ ] **Step 3: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/reasoning.rs
git commit -m "panel/chat: tighten reasoning live preview 3->2 lines"
```

---

## Final Verification（全部 Task 后，一次性）

- [ ] **编译全绿**

Run: `cargo check -p aleph-panel --lib 2>&1 | tail -10`
Expected: 通过。

- [ ] **全量 lib 单测**

Run: `cargo test -p aleph-panel --lib 2>&1 | tail -20`
Expected: 全绿（reveal_tool×3、search_hits/flat_kv×5、latest_step_tool/step_narration_head×5、现有 tool_card/timeline/layout/reasoning 测试无回归）。

- [ ] **构建 WASM（运行时 QA 前置；按需）**

Run: `just wasm`（需 `interfaces/webchat/node_modules`；新 worktree 须先 symlink 主检出的 node_modules）。
Expected: WASM 构建成功。随后按 spec §7 走运行时 QA（重编 server 重嵌 dist）。

> **运行时 QA（人工，spec §7）**：运行中左侧只一条会变的行 + `N 步` 副行；完成折成 `✓ N 步…`；点开内联只一层扁平、封顶 8 行；溢出 `… +N → 详情栏` 点击自动开右栏并定位/展开对应卡；左右▾联动；search/default 为扁平列表/键值，无递归树、无 `<details>` 套娃；ChatOnly 点溢出自动切 Split。

---

## Self-Review 记录

- **Spec 覆盖**：D1 左简右详（Task 3 surface + Task 4 Detail + Task 5 StepStrip）✓；D2 内联一层+溢出去右（Task 3 capped_* + overflow_line + reveal_tool）✓；D3 运行状态行图标+最新动作+步数（Task 5 summary_main + 副行）✓；D4 仅 wide（全程不碰 phone）✓；D5 保留▾三角（StepStrip/ToolCard 均保留）✓；删 JsonViewer（Task 6）✓；推理 3→2（Task 7）✓；联动 reveal_tool（Task 2）✓。
- **Placeholder 扫描**：无 TBD/TODO；每个改码步骤含完整代码。
- **类型一致性**：`ToolSurface`（Task 3 定义）→ Task 4/5 引用一致；`reveal_tool(run, it, tool_id, default_open)`（Task 2 定义）→ Task 3 `on_overflow` 调用签名一致；`latest_step_tool`/`step_narration_head`（Task 5 定义+测试+使用）一致；i18n key `tool_card.to_detail`/`chat.working`（Task 1 建）→ Task 3/5 使用一致。
