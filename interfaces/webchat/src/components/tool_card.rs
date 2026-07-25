//! Rich tool-card rendering — renders a single tool invocation (args/result)
//! into rich views (diff / shell / full-text / patch) keyed by tool kind. Shared
//! by the left-side chat and the right-side workspace panel.
//!
//! Pure logic (ToolKind dispatch, diff, truncation, summarisation) is separated
//! from view components: logic is host-testable via
//! `cargo test -p aleph-panel --lib`.

use crate::i18n::{t_string, use_i18n};
use crate::state::layout::{ToolPayload, WorkspaceState};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

/// Tool kind — determines how the card body is rendered.
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
    /// Map a tool name (case-insensitive) to a kind. Unknown → `Default`.
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

    /// Whether the card is expanded by default: file-mutation kinds default-open,
    /// the rest default-closed.
    #[must_use]
    pub const fn default_open(self) -> bool {
        matches!(self, Self::FileEdit | Self::FileWrite | Self::ApplyPatch)
    }
}

/// Card rendering surface: left chat (capped) vs right detail pane (full).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolSurface {
    /// Left chat: inline, flat single level, capped at `MAX_INLINE_LINES`; overflow
    /// links to the detail pane.
    #[default]
    Inline,
    /// Right "Tool · Detail" pane: full, flat, uncapped.
    Detail,
}

/// Max visible lines for inline chat cards; the right-side detail pane is uncapped.
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

/// One diff line: `sign` is `'+'`(added) / `'-'`(removed) / `' '`(context).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub sign: char,
    pub text: String,
}

/// Extract output from `{"Success":{"output":..}}`.
#[must_use]
pub fn success_output(result: &Value) -> Option<&Value> {
    result.get("Success").and_then(|s| s.get("output"))
}

/// Extract error text from `{"Error":{"error":..}}`.
pub fn error_message(result: &Value) -> Option<String> {
    result
        .get("Error")
        .and_then(|e| e.get("error"))
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
}

/// Line-level diff (with equal context lines), returns (lines, added, removed).
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

/// Take first `max_lines` lines; returns (display text, hidden count). 0 hidden
/// means not truncated.
#[must_use]
pub fn split_preview(text: &str, max_lines: usize) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return (text.to_string(), 0);
    }
    let shown = lines[..max_lines].join("\n");
    (shown, lines.len() - max_lines)
}

/// Extract flat hit list `(title, url)` from search results `Success.output.results[]`.
/// On missing fields, title/url each fall back (title: `title`->`name`->`"(untitled)"`;
/// url: `url`->`link`->None). Unexpected shapes return empty.
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

/// Flatten a JSON object into top-level `key: value` rows; nested values use compact single-line JSON
/// (`serde_json::to_string`, no indentation), never expanded into collapsible subtrees. Non-objects return empty.
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

/// Aggregate counts by tool kind, used to synthesise placeholder titles when there is no narration.
/// Order is stable (first occurrence first), for deterministic rendering and testing.
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

/// One row in the explore block body: consecutive FileRead merged into one (deduplicated filename join),
/// Search and other read-only tools each get their own row.
#[derive(Debug, Clone, PartialEq)]
pub struct ExploreEntry {
    pub kind: ToolKind,
    /// Synthesised display label (e.g. "a.rs, b.rs" or search term).
    pub label: String,
    /// tool_ids covered by this entry (merged rows contain multiple; click uses first for detail pane).
    pub tool_ids: Vec<String>,
}

/// Merge consecutive FileRead with dedup: merge consecutive same-kind file reads into one entry,
/// deduplicated filenames joined by comma; other tools each get their own row.
#[must_use]
pub fn explore_entries(items: &[(String, String, Option<String>)]) -> Vec<ExploreEntry> {
    let mut out: Vec<ExploreEntry> = Vec::new();
    for (tool_id, name, headline) in items {
        let kind = ToolKind::from_name(name);
        let label = headline.clone().unwrap_or_else(|| name.clone());
        // Consecutive FileRead merged into the previous entry (label deduped and comma-joined).
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

/// Path for file-type tools, used in the header `📄 path`. Non-file tools return None.
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

/// Icon glyph for a tool kind.
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

/// Inline icon — first check the tool name for more specific glyphs (web_fetch 🌐 /
/// skill 📖 / memory 🧠), otherwise fall back to the kind icon. The icon represents the action so that
/// so a line `🌐 https://…` is self-explanatory without repeating the tool name.
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

/// Collapse multi-line / whitespace-heavy args into one line: used for the single-line tool-call description in the header.
/// `split_whitespace` is UTF-8 safe — CJK text is never cut mid-character.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Default-kind tools scan these param keys in priority order, taking the first non-empty string as the title.
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

/// Natural-language title for a tool row — picks the most descriptive parameter (query / shell command /
/// file path / URL…) and collapses it into one line. Returns `None` when there is no descriptive parameter,
/// so callers fall back to a kind verb label (search / read / execute…). This replaces the old
/// "tool name + COMPLETED + duration" style title.
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

/// Shared tool card: header (icon + single-line title + run indicator + diff stats + collapse arrow)
/// + expandable body. The title picks the most descriptive parameter (query / command / path / URL), no longer rendering
/// the tool name and "COMPLETED · duration". Both the left chat and right workspace panel render it. Expand state is
/// a per-card local signal: file-mutation kinds default-open, the rest default-closed.
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
    // Read the shared `tool_id → status` index rather than rescanning the whole
    // transcript per card per streamed token (see `ToolIndex`). Absent context
    // (storybook) falls back to the same lookup done directly.
    let tool_index = use_context::<crate::views::chat::state::ToolIndex>();
    let status = Memo::new(move |_| match tool_index {
        Some(idx) => idx.status_of(&tid_for_status),
        None => chat
            .messages
            .with(|msgs| crate::views::chat::state::find_tool_status(msgs, &tid_for_status)),
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
            ws.inspect(crate::state::inspector::InspectorTarget::Tool {
                run_id: run_for_overflow.clone(),
                tool_id: tid_for_overflow.clone(),
            });
        }
    };

    // diff stats (meaningful only for FileEdit): computed from args old/new.
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

    // Title: collapse the most descriptive parameter into one line (query / command / path / URL);
    // when there is no parameter, fall back to the kind verb label; Default tools fall back to the tool name itself.
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
    // Fourth state: the run settled while this row was still running and the
    // authoritative summary never named it (see `state::TOOL_STATUS_UNKNOWN`).
    // Rendered as a muted dash — neither a lie about success nor a false alarm.
    let unknown = move || matches!(status.get(), Some((s, _, _)) if s == crate::views::chat::state::TOOL_STATUS_UNKNOWN);

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
                <Show when=unknown>
                    <span
                        class="shrink-0 text-[11px] text-text-tertiary"
                        title=move || t_string!(i18n, tool_card.status_unknown).to_string()
                    >"–"</span>
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

/// Single-line monospace container style.
const MONO_BLOCK: &str = "font-mono text-xs whitespace-pre-wrap break-words leading-relaxed";

/// Render card body by tool kind. `surface` controls capping: Inline caps at `MAX_INLINE_LINES`
/// and shows a "-> detail panel" overflow line; Detail is full. `detail_label` is the resolved
/// localised "detail panel" text.
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

/// Red-remove / green-add / neutral-context diff rendering, capped by surface (Inline over MAX_INLINE_LINES
/// truncated + "-> detail panel"), flat, no nesting.
fn capped_diff(
    lines: Vec<DiffLine>,
    surface: ToolSurface,
    detail_label: String,
    on_overflow: impl Fn() + Clone + 'static,
) -> AnyView {
    let cap = surface.cap();
    let total = lines.len();
    let hidden = total.saturating_sub(cap);
    view! {
        <div>
            <div class=format!("{MONO_BLOCK} rounded-md glass-inset overflow-x-auto")>
                {lines.into_iter().take(cap).map(|l| {
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

/// Render multi-line text capped by surface. Inline over `MAX_INLINE_LINES` is truncated with
/// an appended "… +N -> detail panel" line (click fires `on_overflow`); Detail is full.
/// No inner collapsible sections — this is the core of the flattening.
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

/// Unified "… +N -> detail panel" overflow line. `detail_label` is the resolved localised
/// text (e.g. "detail panel").
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
    view! {
        <div class="flex flex-col gap-1 text-xs">
            <span class="text-[10px] uppercase tracking-wider text-text-tertiary">
                {format!("{total} results")}
            </span>
            {hits.into_iter().take(cap).map(|(title, url)| view! {
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
    // Prefer result over args; both flattened to top-level key:value rows.
    let source = p.result.clone().or_else(|| p.args.clone());
    let Some(v) = source else {
        return view! { <span class="text-text-tertiary italic text-xs">"…"</span> }.into_any();
    };
    let kv = flat_kv(&v);
    if kv.is_empty() {
        // Non-object (array/scalar) -> compact pretty JSON, capped by surface.
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
    view! {
        <div class="flex flex-col gap-0.5 text-xs font-mono">
            {kv.into_iter().take(cap).map(|(k, val)| view! {
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
        // At least one '-', one '+', one ' '(equal y line)
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
        // Three top-level keys; the nested value flattened to compact single-line JSON, not expanded into a subtree
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
            (
                "t4".into(),
                "web_search".into(),
                Some("panel bug".to_string()),
            ),
            ("t5".into(), "file_read".into(), Some("c.rs".to_string())), // search 打断后新起一条
        ];
        let entries = explore_entries(&items);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].label, "a.rs, b.rs");
        assert_eq!(
            entries[0].tool_ids,
            vec!["t1".to_string(), "t2".to_string(), "t3".to_string()]
        );
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
