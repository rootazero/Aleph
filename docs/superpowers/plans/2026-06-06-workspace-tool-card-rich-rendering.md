# Workspace 工具卡片富渲染 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 WebChat 工作区每个工具调用从「原始 JSON 树」升级为按工具类型分流的富卡片（file_edit→红删绿增 diff、file_write→全文+行号、bash→`$cmd`+输出+退出码、apply_patch→着色 patch），左右两个面共用同一组件，并删掉死抽象 `ToolRendererRegistry`。

**Architecture:** 纯前端（`interfaces/webchat`，crate `aleph-panel`），核心零改动（守 R4）。新建单一 `components/tool_card.rs`：纯逻辑函数（`ToolKind` 分流、`similar` 行级 diff、截断、工具汇总）+ 一个 `ToolCard` Leptos 组件，内部 `match ToolKind` 渲染。右侧 `StepCard` 与左侧 `MessageBubble` 都改用 `<ToolCard>`。展开状态用每卡本地 `RwSignal`（edit/write/patch 默认开，其余默认折叠）。叙述为空时由工具汇总合成占位标题。

**Tech Stack:** Rust + Leptos 0.8 (CSR/wasm)、`similar`（行级 diff）、`syntect`（已有，file_write 高亮）、leptos_i18n（en/zh）。

**测试约定：** 纯逻辑函数放在 `tool_card.rs` 顶层并配 `#[cfg(test)]`，在宿主机用 `cargo test -p aleph-panel --lib` 运行（与现有 `tool_renderer.rs` 测试同机制）。视图组件不做单测，靠 `just wasm` 编译验证。

**关键数据形状（已在面板捕获，无需改核心）：**
- `WorkspaceState.get_tool_payload(run_id, tool_id) -> Option<ToolPayload>`，`ToolPayload { args: Option<Value>, result: Option<Value> }`。
- `args` = 工具入参对象本身，如 `{"old_string":..,"new_string":..,"path":..}`、`{"content":..,"path":..}`、`{"code":..}`、`{"patch":..}`。
- `result` = 外部标签枚举：`{"Success":{"output":<Value>}}` 或 `{"Error":{"error":String,"retryable":bool}}`。
- `bash`/`code_exec` 的 output 字段：`{stdout, stderr, exit_code, duration_ms, success, truncated, ...}`。
- status/duration 反应式来自 `ChatState.messages[*].tool_calls`（`ToolCallEntry { tool_id, tool_name, status, duration_ms }`）。

---

## Task 1: 加 `similar` 依赖 + `ToolKind` 分流骨架

**Files:**
- Modify: `interfaces/webchat/Cargo.toml`
- Create: `interfaces/webchat/src/components/tool_card.rs`
- Modify: `interfaces/webchat/src/components/mod.rs:26`

- [ ] **Step 1: 加依赖**

在 `interfaces/webchat/Cargo.toml` 的 `[dependencies]` 区，紧跟 `syntect` 那行之后加入：

```toml
similar = { version = "2", default-features = false, features = ["text"] }
```

- [ ] **Step 2: 写失败测试（新建 tool_card.rs，只含 ToolKind + 测试）**

新建 `interfaces/webchat/src/components/tool_card.rs`，内容：

```rust
//! 工具卡片富渲染 —— 把一次工具调用（args/result）按工具类型渲染成
//! diff / shell / 全文 / patch 等富视图。左侧聊天与右侧工作区面板共用。
//!
//! 纯逻辑（ToolKind 分流、diff、截断、汇总）与视图组件分离：逻辑可在
//! 宿主机 `cargo test -p aleph-panel --lib` 下测试。

/// 工具大类 —— 决定卡片体如何渲染。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    FileEdit,
    FileWrite,
    ApplyPatch,
    FileRead,
    Bash,
    Search,
    Default,
}

impl ToolKind {
    /// 由工具名（大小写不敏感）映射到大类。未知名 → `Default`。
    pub fn from_name(name: &str) -> ToolKind {
        let n = name.to_lowercase();
        match n.as_str() {
            "file_edit" => ToolKind::FileEdit,
            "file_write" => ToolKind::FileWrite,
            "apply_patch" => ToolKind::ApplyPatch,
            "file_read" => ToolKind::FileRead,
            _ => {
                if n.starts_with("bash")
                    || n.starts_with("shell")
                    || n.starts_with("code_exec")
                    || n.contains("_exec")
                {
                    ToolKind::Bash
                } else if n == "search"
                    || n == "web_search"
                    || n == "grep"
                    || n == "find"
                    || n.starts_with("search")
                    || n.ends_with("_search")
                {
                    ToolKind::Search
                } else {
                    ToolKind::Default
                }
            }
        }
    }

    /// 卡片默认是否展开内容：文件改动类默认展开，其余默认折叠。
    pub fn default_open(self) -> bool {
        matches!(
            self,
            ToolKind::FileEdit | ToolKind::FileWrite | ToolKind::ApplyPatch
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_maps_known_and_unknown() {
        assert_eq!(ToolKind::from_name("file_edit"), ToolKind::FileEdit);
        assert_eq!(ToolKind::from_name("FILE_WRITE"), ToolKind::FileWrite);
        assert_eq!(ToolKind::from_name("apply_patch"), ToolKind::ApplyPatch);
        assert_eq!(ToolKind::from_name("file_read"), ToolKind::FileRead);
        assert_eq!(ToolKind::from_name("bash"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("code_exec"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("python_exec"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("web_search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("hybrid_search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("memory_recall"), ToolKind::Default);
    }

    #[test]
    fn default_open_only_for_file_mutations() {
        assert!(ToolKind::FileEdit.default_open());
        assert!(ToolKind::FileWrite.default_open());
        assert!(ToolKind::ApplyPatch.default_open());
        assert!(!ToolKind::Bash.default_open());
        assert!(!ToolKind::Search.default_open());
        assert!(!ToolKind::FileRead.default_open());
        assert!(!ToolKind::Default.default_open());
    }
}
```

在 `interfaces/webchat/src/components/mod.rs:12`（`pub mod json_viewer;` 附近，按字母序）加入：

```rust
pub mod tool_card;
```

- [ ] **Step 3: 运行测试，确认通过（先验证骨架可编）**

Run: `cargo test -p aleph-panel --lib -- tool_card`
Expected: PASS（2 个测试 `from_name_maps_known_and_unknown`、`default_open_only_for_file_mutations`）。
若报 `similar` 未解析，确认 Step 1 依赖已写入。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/Cargo.toml interfaces/webchat/src/components/tool_card.rs interfaces/webchat/src/components/mod.rs
git commit -m "webchat: scaffold ToolKind dispatch + similar dep for tool-card rendering"
```

---

## Task 2: 纯逻辑函数（result 提取、diff、截断、工具汇总）

**Files:**
- Modify: `interfaces/webchat/src/components/tool_card.rs`

- [ ] **Step 1: 写失败测试**

在 `tool_card.rs` 的 `#[cfg(test)] mod tests` 内、`use super::*;` 之后追加：

```rust
    #[test]
    fn success_output_and_error_extract() {
        let ok = serde_json::json!({"Success": {"output": {"stdout": "hi"}}});
        assert_eq!(
            success_output(&ok).and_then(|o| o.get("stdout")).and_then(|v| v.as_str()),
            Some("hi")
        );
        assert_eq!(error_message(&ok), None);

        let err = serde_json::json!({"Error": {"error": "boom", "retryable": false}});
        assert_eq!(success_output(&err), None);
        assert_eq!(error_message(&err).as_deref(), Some("boom"));
    }

    #[test]
    fn diff_lines_counts_add_remove_equal() {
        let (lines, added, removed) = diff_lines("let x = 1;\nlet y = 2;\n", "let x = 2;\nlet y = 2;\n");
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
        // 至少包含一条 '-'、一条 '+'、一条 ' '(相等的 y 行)
        assert!(lines.iter().any(|l| l.sign == '-'));
        assert!(lines.iter().any(|l| l.sign == '+'));
        assert!(lines.iter().any(|l| l.sign == ' '));
    }

    #[test]
    fn diff_lines_identical_is_zero() {
        let (_lines, added, removed) = diff_lines("same\n", "same\n");
        assert_eq!((added, removed), (0, 0));
    }

    #[test]
    fn split_preview_truncates_beyond_max() {
        let text = "a\nb\nc\nd\ne";
        let (shown, hidden) = split_preview(text, 3);
        assert_eq!(shown, "a\nb\nc");
        assert_eq!(hidden, 2);

        let (shown2, hidden2) = split_preview("a\nb", 5);
        assert_eq!(shown2, "a\nb");
        assert_eq!(hidden2, 0);
    }

    #[test]
    fn summarize_tools_counts_by_kind_in_order() {
        let tools = vec![
            ("t1".to_string(), "file_read".to_string()),
            ("t2".to_string(), "bash".to_string()),
            ("t3".to_string(), "file_read".to_string()),
            ("t4".to_string(), "search".to_string()),
        ];
        let got = summarize_tools(&tools);
        assert_eq!(
            got,
            vec![(ToolKind::FileRead, 2), (ToolKind::Bash, 1), (ToolKind::Search, 1)]
        );
    }

    #[test]
    fn summarize_tools_empty_is_empty() {
        assert!(summarize_tools(&[]).is_empty());
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p aleph-panel --lib -- tool_card`
Expected: FAIL（`success_output`/`error_message`/`diff_lines`/`split_preview`/`summarize_tools`/`DiffLine` 未定义）。

- [ ] **Step 3: 实现纯逻辑**

在 `tool_card.rs` 顶部 `impl ToolKind` 之后（`#[cfg(test)]` 之前）插入：

```rust
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

/// 一行 diff：`sign` 为 `'+'`(新增)/`'-'`(删除)/`' '`(上下文)。
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub sign: char,
    pub text: String,
}

/// 从 `{"Success":{"output":..}}` 取出 output。
pub fn success_output(result: &Value) -> Option<&Value> {
    result.get("Success").and_then(|s| s.get("output"))
}

/// 从 `{"Error":{"error":..}}` 取出错误文案。
pub fn error_message(result: &Value) -> Option<String> {
    result
        .get("Error")
        .and_then(|e| e.get("error"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 行级 diff（带相等的上下文行），返回 (行, 新增数, 删除数)。
pub fn diff_lines(old: &str, new: &str) -> (Vec<DiffLine>, usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut lines = Vec::new();
    let (mut added, mut removed) = (0usize, 0usize);
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => {
                removed += 1;
                '-'
            }
            ChangeTag::Insert => {
                added += 1;
                '+'
            }
            ChangeTag::Equal => ' ',
        };
        let text = change.value().trim_end_matches('\n').to_string();
        lines.push(DiffLine { sign, text });
    }
    (lines, added, removed)
}

/// 取前 `max_lines` 行；返回 (展示文本, 被隐藏行数)。隐藏数为 0 表示未截断。
pub fn split_preview(text: &str, max_lines: usize) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return (text.to_string(), 0);
    }
    let shown = lines[..max_lines].join("\n");
    (shown, lines.len() - max_lines)
}

/// 按工具大类汇总计数，用于「无叙述」时合成占位标题。
/// 顺序固定（首次出现的大类先出），便于稳定渲染与测试。
pub fn summarize_tools(tools: &[(String, String)]) -> Vec<(ToolKind, usize)> {
    let mut order: Vec<ToolKind> = Vec::new();
    let mut counts: std::collections::HashMap<ToolKind, usize> = std::collections::HashMap::new();
    for (_id, name) in tools {
        let kind = ToolKind::from_name(name);
        if !counts.contains_key(&kind) {
            order.push(kind);
        }
        *counts.entry(kind).or_insert(0) += 1;
    }
    order.into_iter().map(|k| (k, counts[&k])).collect()
}
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p aleph-panel --lib -- tool_card`
Expected: PASS（共 8 个测试）。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/components/tool_card.rs
git commit -m "webchat: pure helpers for tool-card (diff, truncate, result extract, summary)"
```

---

## Task 3: `ToolCard` 组件 + 各工具体渲染

**Files:**
- Modify: `interfaces/webchat/src/components/tool_card.rs`

> 视图代码不做单测；本任务靠 `just wasm` 编译验证（Step 4）。逻辑已在 Task 2 覆盖。

- [ ] **Step 1: 加视图所需 import**

在 `tool_card.rs` 顶部（文件级 doc 注释之后、`pub enum ToolKind` 之前）加：

```rust
use crate::components::json_viewer::JsonViewer;
use crate::i18n::*;
use crate::state::layout::{ToolPayload, WorkspaceState};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;
```

- [ ] **Step 2: 加路径提取助手 + 头部子视图**

在纯逻辑函数之后（`#[cfg(test)]` 之前）加：

```rust
/// 文件类工具的路径，用于头部 `📄 path`。非文件工具返回 None。
pub fn file_path_of(payload: &Option<ToolPayload>) -> Option<String> {
    let args = payload.as_ref()?.args.as_ref()?;
    for key in ["path", "file_path", "filename"] {
        if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

/// 工具大类图标字形。
fn kind_icon(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::FileEdit => "✏️",
        ToolKind::FileWrite => "📝",
        ToolKind::ApplyPatch => "🩹",
        ToolKind::FileRead => "📄",
        ToolKind::Bash => "❯",
        ToolKind::Search => "🔍",
        ToolKind::Default => "•",
    }
}
```

- [ ] **Step 3: 加 `ToolCard` 组件 + 分流体**

在文件内追加（`#[cfg(test)]` 之前）：

```rust
/// 共享工具卡片：头部（图标+名+状态+耗时+路径+diff统计）+ 可展开体。
/// 左侧聊天与右侧工作区面板都渲染它。展开状态为每卡本地信号：
/// 文件改动类默认展开，其余默认折叠。
#[component]
pub fn ToolCard(run_id: String, tool_id: String, tool_name: String) -> impl IntoView {
    let workspace = use_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let kind = ToolKind::from_name(&tool_name);

    let tid_for_status = tool_id.clone();
    let status = Memo::new(move |_| {
        chat.messages
            .get()
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .find_map(|t| {
                if t.tool_id == tid_for_status {
                    Some((t.status.clone(), t.duration_ms))
                } else {
                    None
                }
            })
    });

    let run_for_payload = run_id.clone();
    let tid_for_payload = tool_id.clone();
    let payload = Memo::new(move |_| {
        workspace
            .as_ref()
            .and_then(|ws| ws.get_tool_payload(&run_for_payload, &tid_for_payload))
    });

    let expanded = RwSignal::new(kind.default_open());
    let path_label = move || file_path_of(&payload.get());

    // diff 统计（仅 FileEdit 有意义）：从 args 的 old/new 计算。
    let diff_stat = move || {
        if kind != ToolKind::FileEdit {
            return None;
        }
        let p = payload.get();
        let args = p.as_ref()?.args.as_ref()?;
        let old = args.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
        let new = args.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
        let (_lines, added, removed) = diff_lines(old, new);
        Some((added, removed))
    };

    let icon = kind_icon(kind);
    let name_for_head = tool_name.clone();

    view! {
        <div class="rounded-md border border-border/60 bg-surface-sunken/40">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-3 py-2 text-left
                       hover:bg-surface-raised/40 transition-colors"
                on:click=move |_| expanded.update(|e| *e = !*e)
            >
                <span class="text-xs">{icon}</span>
                <span class="text-xs font-mono text-text-secondary">{name_for_head}</span>
                {move || {
                    match status.get() {
                        Some((s, dur)) => {
                            let dur_txt = dur.map(|d| format!(" · {d}ms")).unwrap_or_default();
                            view! {
                                <span class="text-[10px] uppercase tracking-wider text-text-tertiary">
                                    {format!("{s}{dur_txt}")}
                                </span>
                            }
                            .into_any()
                        }
                        None => view! { <span /> }.into_any(),
                    }
                }}
                {move || match diff_stat() {
                    Some((a, r)) => view! {
                        <span class="text-[10px] font-mono">
                            <span class="text-success">{format!("+{a}")}</span>
                            " "
                            <span class="text-danger">{format!("-{r}")}</span>
                        </span>
                    }.into_any(),
                    None => view! { <span /> }.into_any(),
                }}
                {move || match path_label() {
                    Some(p) => view! {
                        <span class="ml-auto text-[11px] font-mono text-text-tertiary truncate max-w-[50%]">
                            {format!("📄 {p}")}
                        </span>
                    }.into_any(),
                    None => view! { <span class="ml-auto" /> }.into_any(),
                }}
            </button>
            <Show when=move || expanded.get()>
                <div class="px-3 pb-2">
                    {move || render_body(kind, &payload.get())}
                </div>
            </Show>
        </div>
    }
}

/// 单行等宽容器样式。
const MONO_BLOCK: &str =
    "font-mono text-xs whitespace-pre-wrap break-words leading-relaxed";
/// 大输出/大文件预览的最大行数。
const MAX_PREVIEW_LINES: usize = 20;

/// 按工具大类渲染卡片体。
fn render_body(kind: ToolKind, payload: &Option<ToolPayload>) -> AnyView {
    let Some(p) = payload else {
        return view! { <span class="text-text-tertiary italic text-xs">"…"</span> }.into_any();
    };
    // 错误优先：任何工具失败都先显示错误文案。
    if let Some(res) = p.result.as_ref() {
        if let Some(err) = error_message(res) {
            return view! {
                <pre class=format!("{MONO_BLOCK} text-danger")>{err}</pre>
            }
            .into_any();
        }
    }
    match kind {
        ToolKind::FileEdit => edit_body(p),
        ToolKind::FileWrite => write_body(p),
        ToolKind::ApplyPatch => patch_body(p),
        ToolKind::Bash => shell_body(p),
        ToolKind::FileRead => read_body(p),
        ToolKind::Search => search_body(p),
        ToolKind::Default => default_body(p),
    }
}

fn arg_str<'a>(p: &'a ToolPayload, key: &str) -> &'a str {
    p.args
        .as_ref()
        .and_then(|a| a.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// 把 diff 行渲染为红删/绿增/中性上下文。
fn diff_view(lines: Vec<DiffLine>) -> AnyView {
    view! {
        <div class=format!("{MONO_BLOCK} rounded border border-border/60 overflow-x-auto")>
            {lines.into_iter().map(|l| {
                let cls = match l.sign {
                    '+' => "block px-2 bg-success/10 text-success",
                    '-' => "block px-2 bg-danger/10 text-danger",
                    _ => "block px-2 text-text-secondary",
                };
                let line = format!("{} {}", l.sign, l.text);
                view! { <span class=cls>{line}</span> }
            }).collect_view()}
        </div>
    }
    .into_any()
}

fn edit_body(p: &ToolPayload) -> AnyView {
    let old = arg_str(p, "old_string");
    let new = arg_str(p, "new_string");
    let (lines, _a, _r) = diff_lines(old, new);
    diff_view(lines)
}

/// 截断的等宽文本块 + 「展开全部 / 收起」。
fn collapsible_text(text: String, extra_class: &'static str) -> AnyView {
    let (preview, hidden) = split_preview(&text, MAX_PREVIEW_LINES);
    if hidden == 0 {
        return view! { <pre class=format!("{MONO_BLOCK} {extra_class} overflow-x-auto")>{text}</pre> }
            .into_any();
    }
    let show_all = RwSignal::new(false);
    let full = text.clone();
    view! {
        <div>
            <pre class=format!("{MONO_BLOCK} {extra_class} overflow-x-auto")>
                {move || if show_all.get() { full.clone() } else { preview.clone() }}
            </pre>
            <button
                type="button"
                class="mt-1 text-[10px] uppercase tracking-wider text-text-tertiary hover:text-primary"
                on:click=move |_| show_all.update(|s| *s = !*s)
            >
                {move || if show_all.get() {
                    "收起".to_string()
                } else {
                    format!("展开全部 (+{hidden})")
                }}
            </button>
        </div>
    }
    .into_any()
}

fn write_body(p: &ToolPayload) -> AnyView {
    let content = arg_str(p, "content").to_string();
    collapsible_text(content, "")
}

fn patch_body(p: &ToolPayload) -> AnyView {
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
    diff_view(lines)
}

fn shell_body(p: &ToolPayload) -> AnyView {
    let cmd = arg_str(p, "code").to_string();
    let out = p.result.as_ref().and_then(success_output);
    let stdout = out
        .and_then(|o| o.get("stdout"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let stderr = out
        .and_then(|o| o.get("stderr"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let exit = out
        .and_then(|o| o.get("exit_code"))
        .and_then(|v| v.as_i64());
    let exit_badge = exit.map(|c| {
        let cls = if c == 0 { "text-success" } else { "text-danger" };
        view! { <span class=format!("text-[10px] font-mono {cls}")>{format!("exit {c}")}</span> }
    });
    view! {
        <div class="flex flex-col gap-1">
            <pre class=format!("{MONO_BLOCK} text-text-primary")>{format!("$ {cmd}")}</pre>
            {(!stdout.is_empty()).then(|| collapsible_text(stdout, "text-text-secondary"))}
            {(!stderr.is_empty()).then(|| collapsible_text(stderr, "text-danger/80"))}
            {exit_badge}
        </div>
    }
    .into_any()
}

fn read_body(p: &ToolPayload) -> AnyView {
    let out = p.result.as_ref().and_then(success_output);
    let text = match out {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| other.to_string()),
        None => String::new(),
    };
    if text.is_empty() {
        return default_body(p);
    }
    collapsible_text(text, "text-text-secondary")
}

fn search_body(p: &ToolPayload) -> AnyView {
    let query = {
        let q = arg_str(p, "query");
        if q.is_empty() { arg_str(p, "q") } else { q }
    }
    .to_string();
    let out = p.result.as_ref().and_then(success_output);
    let count = out
        .and_then(|o| o.get("results"))
        .and_then(|v| v.as_array())
        .map(|a| a.len());
    view! {
        <div class="flex flex-col gap-1 text-xs">
            <pre class=format!("{MONO_BLOCK} text-text-primary")>{format!("🔍 {query}")}</pre>
            {count.map(|c| view! {
                <span class="text-[10px] uppercase tracking-wider text-text-tertiary">
                    {format!("{c} results")}
                </span>
            })}
            {move || match out.cloned() {
                Some(v) => view! { <JsonViewer value=v /> }.into_any(),
                None => view! { <span /> }.into_any(),
            }}
        </div>
    }
    .into_any()
}

fn default_body(p: &ToolPayload) -> AnyView {
    view! {
        <div class="flex flex-col gap-2 text-xs">
            {match p.args.clone() {
                Some(v) => view! {
                    <details class="rounded-md border border-border/60 bg-surface-sunken/60">
                        <summary class="px-3 py-1.5 cursor-pointer text-text-tertiary font-mono uppercase tracking-wider">"input"</summary>
                        <div class="px-3 py-2 overflow-x-auto"><JsonViewer value=v /></div>
                    </details>
                }.into_any(),
                None => view! { <span /> }.into_any(),
            }}
            {match p.result.clone() {
                Some(v) => view! {
                    <details class="rounded-md border border-border/60 bg-surface-sunken/60" open=true>
                        <summary class="px-3 py-1.5 cursor-pointer text-text-tertiary font-mono uppercase tracking-wider">"result"</summary>
                        <div class="px-3 py-2 overflow-x-auto"><JsonViewer value=v /></div>
                    </details>
                }.into_any(),
                None => view! { <span /> }.into_any(),
            }}
        </div>
    }
    .into_any()
}
```

- [ ] **Step 4: 编译 wasm 验证（视图代码无法宿主测试）**

Run: `just wasm`
Expected: 编译成功，生成 `interfaces/webchat/dist/aleph_panel_bg.wasm`。若报未用 import（如 i18n 暂未用到 `use crate::i18n::*;`），删除该行；本任务的视图未用 i18n，**删掉 Step 1 中的 `use crate::i18n::*;`**（叙述兜底的 i18n 在 Task 5 引入）。

- [ ] **Step 5: 跑逻辑测试确认无回归**

Run: `cargo test -p aleph-panel --lib -- tool_card`
Expected: PASS（8 个）。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/components/tool_card.rs
git commit -m "webchat: ToolCard component with per-kind bodies (diff/shell/write/patch/read/search)"
```

---

## Task 4: 右侧工作区面板接线（StepCard → ToolCard）

**Files:**
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`

- [ ] **Step 1: 改 import**

`workspace_panel.rs:11` 删除 `use crate::components::json_viewer::JsonViewer;`。
在 `use crate::components::markdown::MarkdownRenderer;` 下一行加：

```rust
use crate::components::tool_card::ToolCard;
```

`workspace_panel.rs:15` 把 `use crate::state::layout::{FilePreview, LayoutMode, ToolPayload, WorkspaceState};` 改为（去掉 `ToolPayload`，本文件不再直接用）：

```rust
use crate::state::layout::{FilePreview, LayoutMode, WorkspaceState};
```

- [ ] **Step 2: StepCard 改用 ToolCard**

`workspace_panel.rs:166-174`，把 `ActivityRow` 替换为 `ToolCard`：

```rust
            <div class="flex flex-col gap-2">
                {tools
                    .clone()
                    .into_iter()
                    .map(|(tool_id, tool_name)| {
                        view! {
                            <ToolCard run_id=run_id.clone() tool_id=tool_id tool_name=tool_name />
                        }
                    })
                    .collect_view()}
            </div>
```

- [ ] **Step 3: 删除 `ActivityRow`、`PayloadBlock`、`file_path_of`（已迁入 tool_card）**

删除 `workspace_panel.rs` 中：
- `file_path_of` 函数（约 56-67 行，含其 doc 注释）。
- `ActivityRow` 组件（约 180-254 行，含其 doc 注释）。
- `PayloadBlock` 组件（约 280-314 行，含其 doc 注释）。

- [ ] **Step 4: 编译验证**

Run: `just wasm`
Expected: 成功。若报 `file_path_of` / `ToolPayload` / `JsonViewer` 仍被引用，说明有遗漏的删除点，按报错处理（应已全部移除）。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/components/workspace_panel.rs
git commit -m "webchat: right workspace panel renders ToolCard, drop ActivityRow/PayloadBlock"
```

---

## Task 5: 左侧聊天接线（MessageBubble → ToolCard）+ 叙述兜底

**Files:**
- Modify: `interfaces/webchat/src/views/chat/messages.rs`
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`
- Modify: `interfaces/webchat/src/components/tool_card.rs`
- Modify: `interfaces/webchat/locales/en.json`, `interfaces/webchat/locales/zh.json`

- [ ] **Step 1: 加叙述兜底纯函数 + 测试（tool_card.rs）**

在 `tool_card.rs` 的 `summarize_tools` 之后加：

```rust
/// 大类的中性英文标签（i18n 在视图层覆盖；这里给纯函数一个稳定回退）。
pub fn kind_label(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::FileEdit => "edit",
        ToolKind::FileWrite => "write",
        ToolKind::ApplyPatch => "patch",
        ToolKind::FileRead => "read",
        ToolKind::Bash => "run",
        ToolKind::Search => "search",
        ToolKind::Default => "tool",
    }
}

/// 由工具调用合成一句占位标题（叙述为空时用）。形如
/// `read×2 · run×1 · search×1`。无工具时返回空串。
pub fn synthesize_step_title(tools: &[(String, String)]) -> String {
    let parts: Vec<String> = summarize_tools(tools)
        .into_iter()
        .map(|(k, n)| format!("{}×{}", kind_label(k), n))
        .collect();
    parts.join(" · ")
}
```

在 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn synthesize_title_formats_counts() {
        let tools = vec![
            ("a".into(), "file_read".into()),
            ("b".into(), "file_read".into()),
            ("c".into(), "bash".into()),
        ];
        assert_eq!(synthesize_step_title(&tools), "read×2 · run×1");
    }

    #[test]
    fn synthesize_title_empty_when_no_tools() {
        assert_eq!(synthesize_step_title(&[]), "");
    }
```

- [ ] **Step 2: 运行测试，确认通过**

Run: `cargo test -p aleph-panel --lib -- tool_card`
Expected: PASS（10 个）。

- [ ] **Step 3: 右面板 StepCard 用兜底标题**

`workspace_panel.rs` 顶部 import 区加：

```rust
use crate::components::tool_card::synthesize_step_title;
```

`workspace_panel.rs` StepCard 中 `let has_narration = !narration.is_empty();` 这一段（约 132-134 行）改为：

```rust
    let narration = group.narration.clone();
    let fallback_title = synthesize_step_title(&group.tools);
    let tools = group.tools.clone();
```

把 StepCard 里渲染 narration 的 `<Show when=move || has_narration>...</Show>` 块（约 160-164 行）替换为：

```rust
            {if !narration.is_empty() {
                view! {
                    <div class="text-sm text-text-primary leading-relaxed aleph-step-narration">
                        <MarkdownRenderer content=narration.clone() />
                    </div>
                }.into_any()
            } else if !fallback_title.is_empty() {
                view! {
                    <div class="text-xs text-text-tertiary font-mono">{fallback_title.clone()}</div>
                }.into_any()
            } else {
                view! { <span /> }.into_any()
            }}
```

- [ ] **Step 4: 左侧 MessageBubble 工具区改用 ToolCard**

`messages.rs` 顶部 import 区（与其他 `use crate::components::...` 同区）加：

```rust
use crate::components::tool_card::ToolCard;
```

把 `messages.rs:375-442` 的 `tool_calls_view`（整段 chip 列表）替换为：

```rust
    let tool_calls_view = if has_tools {
        let tools = message.tool_calls.clone();
        let run_for_cards = message_run_id.clone();
        Some(view! {
            <div class="mb-2 flex flex-col gap-1">
                {tools.into_iter().map(|tc| {
                    view! {
                        <ToolCard
                            run_id=run_for_cards.clone()
                            tool_id=tc.tool_id.clone()
                            tool_name=tc.tool_name.clone()
                        />
                    }
                }).collect::<Vec<_>>()}
            </div>
        })
    } else {
        None
    };
```

> 说明：`workspace` (`use_context::<WorkspaceState>()`，messages.rs:329) 仍被 `focused`/`iteration_label` 使用，保留不动。`focus_tool_row` 不再被调用但为 `pub` 方法，不产生告警，按「不删无关代码」保留。

- [ ] **Step 5: 加 i18n key（保持 en/zh 结构一致）**

> 本计划的视图文案（`收起`/`展开全部`/`exit`/`results`/`$`）为内联硬编码，够用；此步仅为「无渲染器」清理铺路（Task 6 会删 `tool_renderer` key）。本步**不新增** i18n key —— 跳过编辑 json，直接进 Step 6。

- [ ] **Step 6: 编译验证 + 逻辑测试**

Run: `just wasm && cargo test -p aleph-panel --lib -- tool_card`
Expected: wasm 编译成功；10 个逻辑测试 PASS。

- [ ] **Step 7: 提交**

```bash
git add interfaces/webchat/src/views/chat/messages.rs interfaces/webchat/src/components/workspace_panel.rs interfaces/webchat/src/components/tool_card.rs
git commit -m "webchat: left chat renders ToolCard + synthesized step-title fallback"
```

---

## Task 6: 删除死抽象 `ToolRendererRegistry`

**Files:**
- Delete: `interfaces/webchat/src/components/tool_renderer.rs`
- Modify: `interfaces/webchat/src/components/mod.rs:26`
- Modify: `interfaces/webchat/src/app.rs:28`, `interfaces/webchat/src/app.rs:70-72`
- Modify: `interfaces/webchat/src/views/chat/messages.rs:325-328`（过时注释）
- Modify: `interfaces/webchat/locales/en.json:1825-1827`, `interfaces/webchat/locales/zh.json:1825-1827`

- [ ] **Step 1: 删文件与导出**

```bash
git rm interfaces/webchat/src/components/tool_renderer.rs
```

`components/mod.rs:26` 删除 `pub mod tool_renderer;`。

- [ ] **Step 2: 删 app.rs 的 import 与 provide_context**

`app.rs:28` 删除 `use crate::components::tool_renderer::ToolRendererRegistry;`。
`app.rs` 删除注册块（约 70-72 行，含上方两行注释）：

```rust
    // (code / search / json-fallback); future renderers register by
    // extending the constructor.
    provide_context(ToolRendererRegistry::with_builtins());
```

> 若这三行上方还有一行起头注释（如 `// Tool renderer registry ...`），一并删除整段相关注释，使其不悬空。

- [ ] **Step 3: 清 messages.rs 过时注释**

`messages.rs:325-328` 把提到 “dispatches the call through the ToolRendererRegistry” 的注释段改为：

```rust
    // Tool calls render as ToolCard rows. WorkspaceState (when present)
    // lets a card look up its captured args/result payload; without it
    // (e.g. storybook) cards degrade to header-only.
```

- [ ] **Step 4: 删 i18n `tool_renderer` 块**

`en.json` 把结尾（1823-1828 区）：

```json
    "consecutive_failures": "Consecutive failures"
  },
  "tool_renderer": {
    "no_renderer_prefix": "No renderer matched (tool: "
  }
}
```

改为（删除 `tool_renderer` 块并补上闭合）：

```json
    "consecutive_failures": "Consecutive failures"
  }
}
```

`zh.json` 同理，把：

```json
    "consecutive_failures": "连续失败次数"
  },
  "tool_renderer": {
    "no_renderer_prefix": "未匹配到渲染器 (工具: "
  }
}
```

改为：

```json
    "consecutive_failures": "连续失败次数"
  }
}
```

- [ ] **Step 5: 编译验证（含 i18n key 校验）**

Run: `just wasm`
Expected: 成功。leptos_i18n 在 build 时校验 en/zh key 集合一致——若报 key 缺失/多余，核对两个 json 是否都删了 `tool_renderer`。

- [ ] **Step 6: 全量逻辑测试**

Run: `cargo test -p aleph-panel --lib`
Expected: PASS（含 tool_card 10 个；不再有 tool_renderer 测试）。

- [ ] **Step 7: 提交**

```bash
git add interfaces/webchat/src/components/mod.rs interfaces/webchat/src/app.rs interfaces/webchat/src/views/chat/messages.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "webchat: remove dead ToolRendererRegistry abstraction (zero consumers, R10/P6)"
```

---

## Task 7: 收尾验证 + 热替换部署

**Files:** 无（构建/部署/验证）

- [ ] **Step 1: 格式与 lint（仅本次触碰文件）**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph/interfaces/webchat
rustfmt --edition 2021 src/components/tool_card.rs src/components/workspace_panel.rs src/views/chat/messages.rs src/app.rs src/components/mod.rs
```
Expected: 无报错（文件被规范化）。若有改动，`git add` 这些文件并 `git commit -m "webchat: rustfmt tool-card touched files"`。

- [ ] **Step 2: 全链刷新（wasm → 重编 binary → 热替换 daemon）**

> 见 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」：panel 改动必须重编 `aleph-server` 才生效。

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
just wasm
cargo build --release -p alephcore --bin aleph-server
```
Expected: 两步均成功。

- [ ] **Step 3: 热替换运行中的 .app daemon**

Run（参照 CLAUDE.md，.app daemon 路径）:
```bash
mv /Applications/Aleph.app/Contents/MacOS/aleph-server{,.bak}
cp target/release/aleph-server /Applications/Aleph.app/Contents/MacOS/
pkill -f 'Aleph.app/Contents/MacOS/aleph-server' || true
```
Expected: Tauri supervisor 自动 relaunch 新 binary 并 reload webview。
（若用 `cargo run` 起的 dev daemon：改用 `./target/release/aleph-server stop` 后重启。）

- [ ] **Step 4: 人工 UI 验收（用户执行）**

在 Panel 里发一个会触发文件读写+bash 的任务，确认：
1. `file_edit` 展开显示红删绿增 diff，头部有 `+N -M`。
2. `file_write` 展开显示全文（>20 行可「展开全部」）。
3. `bash` 显示 `$ cmd` + stdout/stderr + `exit N` 徽章。
4. edit/write/patch 默认展开；bash/search/read 默认折叠。
5. 左侧聊天与右侧面板都显示富卡片；StepStrip 完成后仍折叠成一行。
6. 某步无叙述时显示 `read×2 · run×1` 之类占位标题。

- [ ] **Step 5: 最终提交（若 Step 1 产生格式化改动且未提交）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status --short
# 若有未提交的本次相关改动：
git add -p
git commit -m "webchat: finalize tool-card rich rendering"
```

---

## Self-Review 记录

**Spec coverage（逐条对照 spec §3 决策 / §5 文件 / §7 验收）：**
- 两个面富渲染 → Task 4（右）+ Task 5（左，共用 `ToolCard`）✅
- file_edit 纯前端 before→after → Task 2 `diff_lines` + Task 3 `edit_body` ✅
- 展开默认值（edit/write/patch 开，其余折叠）→ Task 1 `default_open` + Task 3 `ToolCard` 本地信号 ✅
- 渲染 + 叙述兜底 → Task 5 `synthesize_step_title` + StepCard/MessageBubble 接线 ✅
- 方案 A 删死抽象 → Task 6 ✅
- 新依赖 similar → Task 1 ✅
- v1 diff 不做行内语法高亮（仅 +/- 着色）→ Task 3 `diff_view` ✅
- 截断（大输出/大文件）→ Task 2 `split_preview` + Task 3 `collapsible_text` ✅
- bash 命令+stdout/stderr+退出码 → Task 3 `shell_body` ✅
- apply_patch 着色 → Task 3 `patch_body` ✅
- 核心零改动（仅 interfaces/webchat + locales）→ 全任务 ✅

**Placeholder scan：** 无 TBD/TODO；所有代码步骤含完整代码。Task 5 Step 5 明确「跳过、不新增 i18n key」，非占位。

**Type consistency：** `ToolKind`/`DiffLine`/`success_output`/`error_message`/`diff_lines`/`split_preview`/`summarize_tools`/`synthesize_step_title`/`kind_label`/`file_path_of`/`ToolCard(run_id,tool_id,tool_name)` 命名在各任务间一致；`ToolCard` 签名与右(Task4)/左(Task5)调用点一致；删除 `ActivityRow`/`PayloadBlock`/`file_path_of`(workspace_panel) 后无残留引用（Task4 Step3 + Task6）。
