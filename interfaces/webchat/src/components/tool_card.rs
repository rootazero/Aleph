//! 工具卡片富渲染 —— 把一次工具调用（args/result）按工具类型渲染成
//! diff / shell / 全文 / patch 等富视图。左侧聊天与右侧工作区面板共用。
//!
//! 纯逻辑（ToolKind 分流、diff、截断、汇总）与视图组件分离：逻辑可在
//! 宿主机 `cargo test -p aleph-panel --lib` 下测试。

use crate::i18n::{t_string, use_i18n};
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
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let n = name.to_lowercase();
        match n.as_str() {
            "file_edit" => Self::FileEdit,
            "file_write" => Self::FileWrite,
            "apply_patch" => Self::ApplyPatch,
            "file_read" => Self::FileRead,
            _ => {
                if n.starts_with("bash")
                    || n.starts_with("shell")
                    || n.starts_with("code_exec")
                    || n.contains("_exec")
                {
                    Self::Bash
                } else if n == "search"
                    || n == "web_search"
                    || n == "grep"
                    || n == "find"
                    || n.starts_with("search")
                    || n.ends_with("_search")
                {
                    Self::Search
                } else {
                    Self::Default
                }
            }
        }
    }

    /// 卡片默认是否展开内容：文件改动类默认展开，其余默认折叠。
    #[must_use]
    pub const fn default_open(self) -> bool {
        matches!(self, Self::FileEdit | Self::FileWrite | Self::ApplyPatch)
    }
}

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

impl ToolSurface {
    /// Max body lines for this surface: `Inline` caps at `MAX_INLINE_LINES`,
    /// `Detail` is uncapped.
    #[must_use]
    const fn cap(self) -> usize {
        match self {
            ToolSurface::Inline => MAX_INLINE_LINES,
            ToolSurface::Detail => usize::MAX,
        }
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
#[must_use]
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
#[must_use]
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
#[must_use]
pub fn split_preview(text: &str, max_lines: usize) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return (text.to_string(), 0);
    }
    let shown = lines[..max_lines].join("\n");
    (shown, lines.len() - max_lines)
}

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

/// 按工具大类汇总计数，用于「无叙述」时合成占位标题。
/// 顺序固定（首次出现的大类先出），便于稳定渲染与测试。
#[must_use]
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

/// 探索块展开体的一行：连续 FileRead 合并成一条（文件名去重连接），
/// Search 等其余只读工具各自一条。
#[derive(Debug, Clone, PartialEq)]
pub struct ExploreEntry {
    pub kind: ToolKind,
    /// 已合成的展示文案（如 "a.rs, b.rs" 或搜索词）。
    pub label: String,
    /// 该条覆盖的 tool_id（合并行含多个；点击取 first 进详情栏）。
    pub tool_ids: Vec<String>,
}

/// 连续 FileRead 合并去重：将连续的同类文件读取合并为一条，
/// 文件名去重后以逗号连接；其他工具各占一行。
#[must_use]
pub fn explore_entries(items: &[(String, String, Option<String>)]) -> Vec<ExploreEntry> {
    let mut out: Vec<ExploreEntry> = Vec::new();
    for (tool_id, name, headline) in items {
        let kind = ToolKind::from_name(name);
        let label = headline.clone().unwrap_or_else(|| name.clone());
        // 连续 FileRead 合并到上一条（label 去重后逗号连接）。
        if kind == ToolKind::FileRead {
            if let Some(last) = out.last_mut().filter(|e| e.kind == ToolKind::FileRead) {
                last.tool_ids.push(tool_id.clone());
                if !last.label.split(", ").any(|s| s == label) {
                    last.label.push_str(", ");
                    last.label.push_str(&label);
                }
                continue;
            }
        }
        out.push(ExploreEntry {
            kind,
            label,
            tool_ids: vec![tool_id.clone()],
        });
    }
    out
}

/// 文件类工具的路径，用于头部 `📄 path`。非文件工具返回 None。
#[must_use]
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
const fn kind_icon(kind: ToolKind) -> &'static str {
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

/// 行内图标 —— `先按工具名给几个常见工具更贴切的字形（web_fetch` 🌐 /
/// skill 📖 / memory 🧠），否则回落到大类图标。图标即代表动作，让聊天里
/// 一行 `🌐 https://…` 自解释，无需再写工具名。
#[must_use]
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
#[must_use]
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
#[must_use]
pub fn ToolCard(
    run_id: String,
    tool_id: String,
    tool_name: String,
    #[prop(optional)] surface: ToolSurface,
) -> impl IntoView {
    let workspace = use_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    let kind = ToolKind::from_name(&tool_name);

    let tid_for_status = tool_id.clone();
    let tid_for_expand = tool_id.clone();
    let status = Memo::new(move |_| {
        chat.messages
            .get()
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .find_map(|t| {
                if t.tool_id == tid_for_status {
                    Some((t.status.clone(), t.duration_ms, t.started_at_ms))
                } else {
                    None
                }
            })
    });

    let run_for_overflow = run_id.clone();
    let tid_for_overflow = tool_id.clone();

    let run_for_payload = run_id;
    let tid_for_payload = tool_id;
    let payload = Memo::new(move |_| {
        workspace
            .as_ref()
            .and_then(|ws| ws.get_tool_payload(&run_for_payload, &tid_for_payload))
    });

    // Expand state lives in the shared `WorkspaceState` (override set keyed by
    // tool_id), not a card-local signal, for two reasons: (1) the keyed `<For>`
    // rendering this card remounts on every streamed token (row_key folds in
    // content length), which would reset a card-local signal to its default
    // mid-run; (2) the same tool is rendered by two cards — the chat bubble and
    // the workspace timeline — that must stay in sync. The set stores tool_ids
    // toggled *away from* `default_open`, so default-open kinds need no seeding.
    // Storybook (no `WorkspaceState`) falls back to a card-local signal.
    let default_open = kind.default_open();
    let local_toggled = RwSignal::new(false);
    let tid_open = tid_for_expand.clone();
    let expanded = Memo::new(move |_| {
        let toggled =
            workspace.map_or_else(|| local_toggled.get(), |ws| ws.is_event_toggled(&tid_open));
        default_open ^ toggled
    });
    let on_toggle = move |_: web_sys::MouseEvent| {
        if let Some(ws) = workspace {
            ws.toggle_event(&tid_for_expand);
        } else {
            local_toggled.update(|t| *t = !*t);
        }
    };

    let detail_label = t_string!(i18n, tool_card.to_detail).to_string();
    let on_overflow = move || {
        if let Some(ws) = workspace {
            ws.select_tool(run_for_overflow.clone(), tid_for_overflow.clone());
        }
    };

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

    let running = move || matches!(status.get(), Some((s, _, _)) if s == "running");
    let failed = move || matches!(status.get(), Some((s, _, _)) if s == "failed");
    let succeeded = move || matches!(status.get(), Some((s, _, _)) if s == "completed");

    // Shared 1s clock for the live elapsed timer on long-running rows. Only
    // read inside the `running` branch below, so done/failed rows never
    // subscribe to the tick (see run_clock.rs perf contract).
    let tick = use_context::<crate::state::run_clock::SecondTick>();

    view! {
        <div class="rounded-md hover:bg-surface-raised/40 transition-colors">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-2 py-1 text-left"
                on:click=on_toggle
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
                    {
                        let status = status;
                        move || {
                            match (tick, status.get()) {
                                (Some(t), Some((_, _, Some(start)))) => {
                                    let elapsed = t.0.get() - start;
                                    (elapsed >= crate::state::run_clock::LONG_RUN_THRESHOLD_MS)
                                        .then(|| view! {
                                            <span class="shrink-0 text-[10px] font-mono text-text-tertiary tabular-nums">
                                                {crate::state::run_clock::fmt_elapsed(elapsed)}
                                            </span>
                                        })
                                }
                                _ => None,
                            }
                        }
                    }
                </Show>
                <Show when=succeeded>
                    <span class="shrink-0 text-[11px] text-success">"✓"</span>
                    // Sub-second completions just show the ✓ — a "0s" label reads as
                    // broken, not fast (final-review F4).
                    {move || status.get().and_then(|(_, d, _)| d).filter(|d| *d >= 1000).map(|d| view! {
                        <span class="shrink-0 text-[10px] font-mono text-text-tertiary">
                            {crate::state::run_clock::fmt_elapsed(d as i64)}
                        </span>
                    })}
                </Show>
                <Show when=failed>
                    <span class="shrink-0 text-[11px] text-danger">"✗"</span>
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
                    {
                        let oo = on_overflow.clone();
                        let dl = detail_label.clone();
                        move || render_body(kind, &payload.get(), surface, dl.clone(), oo.clone())
                    }
                </div>
            </Show>
        </div>
    }
}

/// 单行等宽容器样式。
const MONO_BLOCK: &str = "font-mono text-xs whitespace-pre-wrap break-words leading-relaxed";

/// 按工具大类渲染卡片体。`surface` 决定封顶：Inline 封顶 `MAX_INLINE_LINES`
/// 并在溢出处显示「→ 详情栏」联动行；Detail 全量。`detail_label` 为已解析的
/// 本地化「详情栏」文案。
pub(crate) fn render_body(
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
            return capped_block(&err, "text-danger", surface, detail_label, on_overflow);
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

fn arg_str<'a>(p: &'a ToolPayload, key: &str) -> &'a str {
    p.args
        .as_ref()
        .and_then(|a| a.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn edit_body(
    p: &ToolPayload,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let old = arg_str(p, "old_string");
    let new = arg_str(p, "new_string");
    let (lines, _a, _r) = diff_lines(old, new);
    capped_diff(lines, surface, detail_label, on_overflow)
}

fn patch_body(
    p: &ToolPayload,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
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
    capped_diff(lines, surface, detail_label, on_overflow)
}

/// 红删/绿增/中性上下文的 diff 渲染，按 surface 封顶（Inline 超 MAX_INLINE_LINES
/// 截断 + 「→ 详情栏」），扁平无嵌套。
fn capped_diff(
    lines: Vec<DiffLine>,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let cap = surface.cap();
    let total = lines.len();
    let hidden = total.saturating_sub(cap);
    let shown: Vec<DiffLine> = lines.into_iter().take(cap).collect();
    view! {
        <div>
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
            </div>
            {(hidden > 0).then(|| overflow_line(hidden, detail_label.clone(), on_overflow.clone()))}
        </div>
    }
    .into_any()
}

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
    let cap = surface.cap();
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

fn write_body(
    p: &ToolPayload,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let content = arg_str(p, "content").to_string();
    capped_block(&content, "", surface, detail_label, on_overflow)
}

fn shell_body(
    p: &ToolPayload,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
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

fn read_body(
    p: &ToolPayload,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
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
    capped_block(
        &text,
        "text-text-secondary",
        surface,
        detail_label,
        on_overflow,
    )
}

fn search_body(
    p: &ToolPayload,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let Some(res) = p.result.as_ref() else {
        return default_body(p, surface, detail_label, on_overflow);
    };
    let hits = search_hits(res);
    if hits.is_empty() {
        return default_body(p, surface, detail_label, on_overflow);
    }
    let cap = surface.cap();
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

fn default_body(
    p: &ToolPayload,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    // 优先展示 result，其次 args；都压成顶层扁平 key:value 行。
    let source = p.result.clone().or_else(|| p.args.clone());
    let Some(v) = source else {
        return view! { <span class="text-text-tertiary italic text-xs">"…"</span> }.into_any();
    };
    let kv = flat_kv(&v);
    if kv.is_empty() {
        // 非对象（数组/标量）→ 紧凑 pretty JSON，按 surface 封顶。
        let compact = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
        return capped_block(
            &compact,
            "text-text-secondary",
            surface,
            detail_label,
            on_overflow,
        );
    }
    let cap = surface.cap();
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
        assert_eq!(
            hits[0],
            ("美股暴跌".to_string(), Some("https://a.com".to_string()))
        );
        assert_eq!(
            hits[1],
            ("关税新闻".to_string(), Some("https://b.com".to_string()))
        );
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
        assert_eq!(
            map.get("nested").map(String::as_str),
            Some("{\"a\":1,\"b\":[2,3]}")
        );
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

    #[test]
    fn explore_entries_merges_consecutive_reads_dedup() {
        let items = vec![
            ("t1".into(), "file_read".into(), Some("a.rs".to_string())),
            ("t2".into(), "file_read".into(), Some("b.rs".to_string())),
            ("t3".into(), "file_read".into(), Some("a.rs".to_string())), // dup 去重
            ("t4".into(), "web_search".into(), Some("panel bug".to_string())),
            ("t5".into(), "file_read".into(), Some("c.rs".to_string())), // search 打断后新起一条
        ];
        let entries = explore_entries(&items);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].label, "a.rs, b.rs");
        assert_eq!(entries[0].tool_ids, vec!["t1".to_string(), "t2".to_string(), "t3".to_string()]);
        assert_eq!(entries[1].kind, ToolKind::Search);
        assert_eq!(entries[1].label, "panel bug");
        assert_eq!(entries[2].label, "c.rs");
    }

    #[test]
    fn explore_entries_headline_fallback_is_tool_name() {
        let items = vec![("t1".into(), "file_read".into(), None)];
        let entries = explore_entries(&items);
        assert_eq!(entries[0].label, "file_read");
    }
}
