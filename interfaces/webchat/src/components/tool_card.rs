//! 工具卡片富渲染 —— 把一次工具调用（args/result）按工具类型渲染成
//! diff / shell / 全文 / patch 等富视图。左侧聊天与右侧工作区面板共用。
//!
//! 纯逻辑（ToolKind 分流、diff、截断、汇总）与视图组件分离：逻辑可在
//! 宿主机 `cargo test -p aleph-panel --lib` 下测试。

use crate::components::json_viewer::JsonViewer;
use crate::i18n::*;
use crate::state::layout::{ToolPayload, WorkspaceState};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

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

use serde_json::Value;
use similar::{ChangeTag, TextDiff};

/// 一行 diff：`sign` 为 `'+'`(新增)/`'-'`(删除)/`' '`(上下文)。
#[derive(Debug, Clone, PartialEq, Eq)]
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
        .map(std::string::ToString::to_string)
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

/// 行内图标 —— 先按工具名给几个常见工具更贴切的字形（web_fetch 🌐 /
/// skill 📖 / memory 🧠），否则回落到大类图标。图标即代表动作，让聊天里
/// 一行 `🌐 https://…` 自解释，无需再写工具名。
pub fn tool_icon(tool_name: &str, kind: ToolKind) -> &'static str {
    let n = tool_name.to_lowercase();
    if n.contains("web_fetch") || n.contains("fetch") || n.contains("browse") || n.contains("http")
    {
        "🌐"
    } else if n.contains("skill") {
        "📖"
    } else if n.contains("memory") || n.contains("recall") || n.contains("remember") {
        "🧠"
    } else {
        kind_icon(kind)
    }
}

/// 把多行/含连续空白的参数压成单行：用于在头部用一行文字描述工具调用。
/// `split_whitespace` 是 UTF-8 安全的，CJK 文本不会被切坏。
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Default 大类工具按优先级扫描这些参数键，取首个非空字符串作为标题。
const HEADLINE_KEYS: &[&str] = &[
    "query",
    "q",
    "url",
    "path",
    "file_path",
    "filename",
    "name",
    "skill_id",
    "pattern",
    "description",
    "cmd",
    "command",
    "text",
    "prompt",
];

/// 工具行的自然语言标题 —— 取最能说明这次调用的参数（搜索词 / shell 命令 /
/// 文件路径 / URL……）压成一行。没有可描述的参数时返回 `None`，由调用方
/// 改用大类动词标签（搜索 / 读取 / 执行……）。这取代了老旧的
/// 「工具名 + COMPLETED + 耗时」式标题。
pub fn tool_headline(kind: ToolKind, payload: &Option<ToolPayload>) -> Option<String> {
    match kind {
        ToolKind::FileEdit | ToolKind::FileWrite | ToolKind::FileRead | ToolKind::ApplyPatch => {
            file_path_of(payload).map(|p| collapse_ws(&p))
        }
        _ => {
            let args = payload.as_ref()?.args.as_ref()?;
            let keys: &[&str] = match kind {
                ToolKind::Search => &["query", "q"],
                ToolKind::Bash => &["cmd", "code", "command"],
                _ => HEADLINE_KEYS,
            };
            for k in keys {
                if let Some(s) = args.get(*k).and_then(|v| v.as_str()) {
                    let one = collapse_ws(s);
                    if !one.is_empty() {
                        return Some(one);
                    }
                }
            }
            None
        }
    }
}

/// 共享工具卡片：头部（图标 + 一行文字标题 + 运行指示 + diff统计 + 折叠箭头）
/// + 可展开体。标题取最能说明调用的参数（搜索词/命令/路径/URL），不再渲染
/// 工具名与「COMPLETED · 耗时」。左侧聊天与右侧工作区面板都渲染它。展开状态为
/// 每卡本地信号：文件改动类默认展开，其余默认折叠。
#[component]
pub fn ToolCard(run_id: String, tool_id: String, tool_name: String) -> impl IntoView {
    let workspace = use_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
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

    let run_for_payload = run_id;
    let tid_for_payload = tool_id;
    let payload = Memo::new(move |_| {
        workspace
            .as_ref()
            .and_then(|ws| ws.get_tool_payload(&run_for_payload, &tid_for_payload))
    });

    let expanded = RwSignal::new(kind.default_open());

    // diff 统计（仅 FileEdit 有意义）：从 args 的 old/new 计算。
    let diff_stat = move || {
        if kind != ToolKind::FileEdit {
            return None;
        }
        let p = payload.get();
        let args = p.as_ref()?.args.as_ref()?;
        let old = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let (_lines, added, removed) = diff_lines(old, new);
        Some((added, removed))
    };

    let icon = tool_icon(&tool_name, kind);

    // 标题：取最能说明这次调用的参数压成一行（搜索词 / 命令 / 路径 / URL）；
    // 没有参数时回落到大类动词标签，Default 工具回落到工具名本身。
    let tn = tool_name.clone();
    let headline = move || {
        if let Some(h) = tool_headline(kind, &payload.get()) {
            return h;
        }
        match kind {
            ToolKind::FileEdit => t_string!(i18n, tool_card.cat_edit).to_string(),
            ToolKind::FileWrite => t_string!(i18n, tool_card.cat_write).to_string(),
            ToolKind::ApplyPatch => t_string!(i18n, tool_card.cat_patch).to_string(),
            ToolKind::FileRead => t_string!(i18n, tool_card.cat_read).to_string(),
            ToolKind::Bash => t_string!(i18n, tool_card.cat_run).to_string(),
            ToolKind::Search => t_string!(i18n, tool_card.cat_search).to_string(),
            ToolKind::Default => tn.clone(),
        }
    };

    let running = move || matches!(status.get(), Some((s, _)) if s == "running");
    let failed = move || matches!(status.get(), Some((s, _)) if s == "failed");

    view! {
        <div class="rounded-lg glass-inset hover:bg-surface-raised/30 transition-colors">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-2 py-1 text-left"
                on:click=move |_| expanded.update(|e| *e = !*e)
            >
                <span class="text-sm shrink-0 leading-none">{icon}</span>
                <span class=move || {
                    let base = "flex-1 min-w-0 truncate text-sm";
                    if failed() {
                        format!("{base} text-danger")
                    } else {
                        format!("{base} text-text-secondary")
                    }
                }>
                    {headline}
                </span>
                <Show when=running>
                    <span class="shrink-0 inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
                </Show>
                {move || match diff_stat() {
                    Some((a, r)) => view! {
                        <span class="shrink-0 text-[10px] font-mono">
                            <span class="text-success">{format!("+{a}")}</span>
                            " "
                            <span class="text-danger">{format!("-{r}")}</span>
                        </span>
                    }.into_any(),
                    None => view! { <span /> }.into_any(),
                }}
                <span class="shrink-0 text-text-tertiary text-[10px]">
                    {move || if expanded.get() { "▾" } else { "▸" }}
                </span>
            </button>
            <Show when=move || expanded.get()>
                <div class="pl-7 pr-2 pb-2">
                    {move || render_body(kind, &payload.get())}
                </div>
            </Show>
        </div>
    }
}

/// 单行等宽容器样式。
const MONO_BLOCK: &str = "font-mono text-xs whitespace-pre-wrap break-words leading-relaxed";
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
        <div class=format!("{MONO_BLOCK} rounded-md glass-inset overflow-x-auto")>
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

/// 截断的等宽文本块 + 「展开全部 / 收起」（i18n）。
#[component]
fn CollapsibleText(text: String, extra_class: &'static str) -> impl IntoView {
    let i18n = use_i18n();
    let (preview, hidden) = split_preview(&text, MAX_PREVIEW_LINES);
    if hidden == 0 {
        return view! {
            <pre class=format!("{MONO_BLOCK} {extra_class} overflow-x-auto")>{text}</pre>
        }
        .into_any();
    }
    let show_all = RwSignal::new(false);
    let full = text;
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
                    t_string!(i18n, tool_card.collapse).to_string()
                } else {
                    format!("{} (+{hidden})", t_string!(i18n, tool_card.expand_all))
                }}
            </button>
        </div>
    }
    .into_any()
}

fn write_body(p: &ToolPayload) -> AnyView {
    let content = arg_str(p, "content").to_string();
    view! { <CollapsibleText text=content extra_class="" /> }.into_any()
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
            DiffLine {
                sign,
                text: raw.to_string(),
            }
        })
        .collect();
    diff_view(lines)
}

fn shell_body(p: &ToolPayload) -> AnyView {
    let cmd = {
        let v = arg_str(p, "cmd");
        if v.is_empty() {
            arg_str(p, "code")
        } else {
            v
        }
    }
    .to_string();
    let out = p.result.as_ref().and_then(success_output).cloned();
    let stdout = out
        .as_ref()
        .and_then(|o| o.get("stdout"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let stderr = out
        .as_ref()
        .and_then(|o| o.get("stderr"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let exit = out
        .as_ref()
        .and_then(|o| o.get("exit_code"))
        .and_then(serde_json::Value::as_i64);
    let exit_badge = exit.map(|c| {
        let cls = if c == 0 {
            "text-success"
        } else {
            "text-danger"
        };
        view! { <span class=format!("text-[10px] font-mono {cls}")>{format!("exit {c}")}</span> }
    });
    view! {
        <div class="flex flex-col gap-1">
            <pre class=format!("{MONO_BLOCK} text-text-primary")>{format!("$ {cmd}")}</pre>
            {(!stdout.is_empty()).then(|| view! { <CollapsibleText text=stdout extra_class="text-text-secondary" /> })}
            {(!stderr.is_empty()).then(|| view! { <CollapsibleText text=stderr extra_class="text-danger/80" /> })}
            {exit_badge}
        </div>
    }
    .into_any()
}

fn read_body(p: &ToolPayload) -> AnyView {
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
        return default_body(p);
    }
    view! { <CollapsibleText text=text extra_class="text-text-secondary" /> }.into_any()
}

fn search_body(p: &ToolPayload) -> AnyView {
    // 查询词已由头部标题展示，这里只渲染命中数 + 结果 JSON。
    let out = p.result.as_ref().and_then(success_output).cloned();
    let count = out
        .as_ref()
        .and_then(|o| o.get("results"))
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len);
    view! {
        <div class="flex flex-col gap-1 text-xs">
            {count.map(|c| view! {
                <span class="text-[10px] uppercase tracking-wider text-text-tertiary">
                    {format!("{c} results")}
                </span>
            })}
            {move || match out.clone() {
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
                    <details class="rounded-md glass-inset">
                        <summary class="px-3 py-1.5 cursor-pointer text-text-tertiary font-mono uppercase tracking-wider">"input"</summary>
                        <div class="px-3 py-2 overflow-x-auto"><JsonViewer value=v /></div>
                    </details>
                }.into_any(),
                None => view! { <span /> }.into_any(),
            }}
            {match p.result.clone() {
                Some(v) => view! {
                    <details class="rounded-md glass-inset" open=true>
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_output_and_error_extract() {
        let ok = serde_json::json!({"Success": {"output": {"stdout": "hi"}}});
        assert_eq!(
            success_output(&ok)
                .and_then(|o| o.get("stdout"))
                .and_then(|v| v.as_str()),
            Some("hi")
        );
        assert_eq!(error_message(&ok), None);

        let err = serde_json::json!({"Error": {"error": "boom", "retryable": false}});
        assert_eq!(success_output(&err), None);
        assert_eq!(error_message(&err).as_deref(), Some("boom"));
    }

    #[test]
    fn diff_lines_counts_add_remove_equal() {
        let (lines, added, removed) =
            diff_lines("let x = 1;\nlet y = 2;\n", "let x = 2;\nlet y = 2;\n");
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
            vec![
                (ToolKind::FileRead, 2),
                (ToolKind::Bash, 1),
                (ToolKind::Search, 1)
            ]
        );
    }

    #[test]
    fn summarize_tools_empty_is_empty() {
        assert!(summarize_tools(&[]).is_empty());
    }

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

    fn payload(args: Value) -> Option<ToolPayload> {
        Some(ToolPayload {
            args: Some(args),
            result: None,
        })
    }

    #[test]
    fn tool_headline_picks_salient_arg_by_kind() {
        // search → query (collapsed to one line)
        assert_eq!(
            tool_headline(
                ToolKind::Search,
                &payload(serde_json::json!({"query": "美股 暴跌\n 关税"}))
            )
            .as_deref(),
            Some("美股 暴跌 关税")
        );
        // bash → cmd
        assert_eq!(
            tool_headline(
                ToolKind::Bash,
                &payload(serde_json::json!({"cmd": "cargo test"}))
            )
            .as_deref(),
            Some("cargo test")
        );
        // file ops → path
        assert_eq!(
            tool_headline(
                ToolKind::FileRead,
                &payload(serde_json::json!({"file_path": "src/main.rs"}))
            )
            .as_deref(),
            Some("src/main.rs")
        );
        // default → url via key scan (web_fetch shape)
        assert_eq!(
            tool_headline(
                ToolKind::Default,
                &payload(serde_json::json!({"url": "https://example.com"}))
            )
            .as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn tool_headline_none_when_no_descriptive_arg() {
        assert_eq!(tool_headline(ToolKind::Search, &None), None);
        assert_eq!(
            tool_headline(ToolKind::Default, &payload(serde_json::json!({"foo": 1}))),
            None
        );
        // empty/whitespace-only value is treated as absent
        assert_eq!(
            tool_headline(
                ToolKind::Search,
                &payload(serde_json::json!({"query": "   "}))
            ),
            None
        );
    }

    #[test]
    fn tool_icon_overrides_then_falls_back_to_kind() {
        assert_eq!(tool_icon("web_fetch", ToolKind::Default), "🌐");
        assert_eq!(tool_icon("skill_read", ToolKind::Default), "📖");
        assert_eq!(tool_icon("memory_recall", ToolKind::Default), "🧠");
        // unknown default name → bullet (kind icon)
        assert_eq!(tool_icon("some_tool", ToolKind::Default), "•");
        // search keeps the magnifier from its kind
        assert_eq!(tool_icon("ctx_search", ToolKind::Search), "🔍");
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
