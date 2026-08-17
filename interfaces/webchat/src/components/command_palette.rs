//! Command Palette — the ⌘K / Ctrl+K overlay.
//!
//! Inspired by openhuman (`app/src/components/commands/CommandPalette.tsx`,
//! built on cmdk + Radix Dialog) but rewritten as a self-contained Leptos
//! component with zero new dependencies. Lives entirely on the panel side
//! and routes through `leptos_router::use_navigate` for navigation actions
//! and `crate::appearance::apply_mode` for theme actions (the canonical
//! helper `theme_toggle.rs` shares).
//!
//! Open/close is driven by `crate::state::hotkey::HotkeyState` so the same
//! signal can be flipped from anywhere (currently only the ⌘K listener and
//! the inner Esc handler).

use crate::appearance::{apply_mode, ThemeMode};
use crate::i18n::{t_string, use_i18n};
use crate::state::hotkey::HotkeyState;
use leptos::ev::keydown;
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

/// Category bucket for grouping in the rendered list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Navigation,
    Theme,
    Diagnostics,
}

impl Group {
    const fn label(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Theme => "Theme",
            Self::Diagnostics => "Diagnostics",
        }
    }

    /// Display order — Navigation first, Diagnostics last.
    const fn order(self) -> u8 {
        match self {
            Self::Navigation => 0,
            Self::Theme => 1,
            Self::Diagnostics => 2,
        }
    }
}

/// A single executable entry in the palette. Built fresh per render so the
/// captured `use_navigate` closure stays bound to the live reactive owner.
struct Action {
    id: &'static str,
    label: String,
    keywords: &'static [&'static str],
    group: Group,
    run: Box<dyn FnMut()>,
}

/// Reload the panel page.
fn reload_page() {
    if let Some(w) = web_sys::window() {
        let _ = w.location().reload();
    }
}

/// Build the action list. Called fresh every render so dynamic labels
/// (i18n) re-resolve and `use_navigate` is captured inside a live owner.
fn build_actions() -> Vec<Action> {
    let i18n = use_i18n();
    let navigate = use_navigate();

    let mk_nav = |id: &'static str,
                  label: String,
                  kw: &'static [&'static str],
                  route: &'static str|
     -> Action {
        let nav = navigate.clone();
        Action {
            id,
            label,
            keywords: kw,
            group: Group::Navigation,
            run: Box::new(move || nav(route, Default::default())),
        }
    };

    vec![
        mk_nav(
            "nav.chat",
            t_string!(i18n, nav.chat).to_string(),
            &["chat", "conversation", "聊天"],
            "/chat",
        ),
        mk_nav(
            "nav.dashboard",
            t_string!(i18n, nav.dashboard).to_string(),
            &["dashboard", "home", "overview", "仪表盘", "首页"],
            "/dashboard",
        ),
        mk_nav(
            "nav.memory",
            t_string!(i18n, nav.memory).to_string(),
            // "canvas" left this list when the whiteboard claimed the word:
            // matching it here would race the real /canvas entry below.
            &["memory", "knowledge", "记忆"],
            "/memory",
        ),
        mk_nav(
            "nav.canvas",
            t_string!(i18n, nav.canvas).to_string(),
            &["canvas", "whiteboard", "画布", "白板"],
            "/canvas",
        ),
        mk_nav(
            "nav.agents",
            t_string!(i18n, nav.agents).to_string(),
            &["agents", "智能体"],
            "/agents",
        ),
        mk_nav(
            "nav.teams",
            t_string!(i18n, nav.teams).to_string(),
            &["teams", "kanban", "团队"],
            "/teams",
        ),
        mk_nav(
            "nav.settings",
            t_string!(i18n, nav.settings).to_string(),
            &["settings", "preferences", "config", "设置"],
            "/settings",
        ),
        Action {
            id: "theme.light",
            label: "Theme: Light".to_string(),
            keywords: &["theme", "light", "明亮"],
            group: Group::Theme,
            run: Box::new(|| apply_mode(ThemeMode::Light)),
        },
        Action {
            id: "theme.dark",
            label: "Theme: Dark".to_string(),
            keywords: &["theme", "dark", "暗黑"],
            group: Group::Theme,
            run: Box::new(|| apply_mode(ThemeMode::Dark)),
        },
        Action {
            id: "theme.system",
            label: "Theme: System".to_string(),
            keywords: &["theme", "system", "auto", "跟随系统"],
            group: Group::Theme,
            run: Box::new(|| apply_mode(ThemeMode::System)),
        },
        Action {
            id: "diag.reload",
            label: "Reload Panel".to_string(),
            keywords: &["reload", "refresh", "重载", "刷新"],
            group: Group::Diagnostics,
            run: Box::new(reload_page),
        },
    ]
}

/// Substring score: 0 = no match, >0 = better. Word-start matches outrank
/// mid-word; label matches outrank keyword matches. Empty query returns
/// `u32::MAX` so everything passes through.
pub(crate) fn score(query: &str, label: &str, keywords: &[&str]) -> u32 {
    if query.is_empty() {
        return u32::MAX;
    }
    let q = query.to_lowercase();
    let l = label.to_lowercase();
    let mut best: u32 = 0;
    if l.starts_with(&q) {
        best = best.max(100);
    } else if l.contains(&q) {
        if l.split(|c: char| !c.is_alphanumeric())
            .any(|t| t.starts_with(&q))
        {
            best = best.max(70);
        } else {
            best = best.max(40);
        }
    }
    for k in keywords {
        let kl = k.to_lowercase();
        if kl.starts_with(&q) {
            best = best.max(35);
        } else if kl.contains(&q) {
            best = best.max(20);
        }
    }
    best
}

/// One filtered row in display order.
struct Row {
    id: &'static str,
    label: String,
    group: Group,
}

/// Filter + group + sort. We pull labels (owned String) so the resulting
/// rows are independent of the Action vector's lifetime — necessary because
/// the click handler will re-run the action by id from a fresh snapshot.
fn filter_and_group(actions: &[Action], query: &str) -> Vec<Row> {
    let mut scored: Vec<(u32, &Action)> = actions
        .iter()
        .filter_map(|a| {
            let s = score(query, &a.label, a.keywords);
            if s == 0 {
                None
            } else {
                Some((s, a))
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        let ga = a.1.group.order();
        let gb = b.1.group.order();
        ga.cmp(&gb).then_with(|| b.0.cmp(&a.0))
    });

    scored
        .into_iter()
        .map(|(_, a)| Row {
            id: a.id,
            label: a.label.clone(),
            group: a.group,
        })
        .collect()
}

/// Run the action at `selected_idx` from a freshly-built snapshot. The
/// Enter keyboard handler only knows the index; mouse clicks know the id
/// directly (see `run_by_id`).
fn run_selected(query: String, selected_idx: usize) {
    let mut actions = build_actions();
    let rows = filter_and_group(&actions, &query);
    if rows.is_empty() {
        return;
    }
    let safe = selected_idx.min(rows.len() - 1);
    let target_id = rows[safe].id;
    if let Some(a) = actions.iter_mut().find(|a| a.id == target_id) {
        (a.run)();
    }
}

/// Run a specific action by id (mouse-click path).
fn run_by_id(id: &'static str) {
    let mut actions = build_actions();
    if let Some(a) = actions.iter_mut().find(|a| a.id == id) {
        (a.run)();
    }
}

#[component]
#[must_use]
pub fn CommandPalette() -> impl IntoView {
    let hotkey = expect_context::<HotkeyState>();
    let open = hotkey.palette_open;

    let query = RwSignal::new(String::new());
    let selected = RwSignal::new(0usize);

    // Reset on close.
    Effect::new(move |_| {
        if !open.get() {
            query.set(String::new());
            selected.set(0);
        }
    });

    // Inner keydown listener — only acts while the palette is open. ↑/↓
    // walks the filtered list; Enter runs the selected action; Esc closes.
    window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
        if !open.get_untracked() {
            return;
        }
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                selected.update(|i| *i = i.saturating_add(1));
            }
            "ArrowUp" => {
                ev.prevent_default();
                selected.update(|i| *i = i.saturating_sub(1));
            }
            "Enter" => {
                ev.prevent_default();
                run_selected(query.get_untracked(), selected.get_untracked());
                open.set(false);
            }
            _ => {}
        }
    });

    view! {
        <Show when=move || open.get() fallback=|| ()>
            <div
                class="fixed inset-0 z-[60] bg-black/40 aleph-scrim"
                data-tauri-drag-region="false"
                on:click=move |_| open.set(false)
            />
            <div
                class="fixed left-1/2 top-[18vh] -translate-x-1/2 z-[61] w-[min(640px,calc(100vw-32px))] \
                       glass bg-surface-overlay/85 border border-border \
                       rounded-2xl shadow-2xl overflow-hidden animate-pop-in"
                data-tauri-drag-region="false"
                role="dialog"
                aria-label="Command palette"
            >
                <input
                    type="text"
                    autofocus
                    placeholder="Type a command or search…"
                    class="w-full px-4 py-3 bg-transparent outline-none border-b border-border \
                           text-text-primary placeholder:text-text-tertiary text-sm"
                    on:input=move |ev| {
                        query.set(event_target_value(&ev));
                        selected.set(0);
                    }
                    prop:value=move || query.get()
                />
                <div class="max-h-[50vh] overflow-y-auto py-2">
                    {move || render_list(
                        &filter_and_group(&build_actions(), &query.get()),
                        selected.get(),
                        open,
                    )}
                </div>
                <div class="px-4 py-2 border-t border-border flex items-center gap-3 text-[10px] text-text-tertiary">
                    <span class="font-mono">"↑↓"</span>" navigate"
                    <span class="font-mono">"Enter"</span>" run"
                    <span class="font-mono">"Esc"</span>" close"
                </div>
            </div>
        </Show>
    }
}

/// Render the grouped row list with the current selection highlighted.
fn render_list(rows: &[Row], selected_idx: usize, open: RwSignal<bool>) -> AnyView {
    if rows.is_empty() {
        return view! {
            <div class="px-4 py-8 text-center text-sm text-text-tertiary">
                "No matching commands."
            </div>
        }
        .into_any();
    }

    let mut out: Vec<AnyView> = Vec::new();
    let mut last_group: Option<Group> = None;
    let safe_idx = selected_idx.min(rows.len() - 1);

    for (idx, row) in rows.iter().enumerate() {
        if last_group != Some(row.group) {
            let heading = row.group.label();
            out.push(
                view! {
                    <div class="px-4 pt-2 pb-1 text-[10px] uppercase tracking-widest text-text-tertiary">
                        {heading}
                    </div>
                }
                .into_any(),
            );
            last_group = Some(row.group);
        }
        let id = row.id;
        let label = row.label.clone();
        let is_selected = idx == safe_idx;
        let row_class = if is_selected {
            "w-full text-left px-4 py-2 flex items-center gap-3 text-sm transition-colors cursor-pointer bg-primary/12 text-text-primary"
        } else {
            "w-full text-left px-4 py-2 flex items-center gap-3 text-sm transition-colors cursor-pointer text-text-secondary hover:bg-sidebar-active/70 hover:text-text-primary"
        };
        out.push(
            view! {
                <button
                    type="button"
                    class=row_class
                    on:click=move |_| {
                        run_by_id(id);
                        open.set(false);
                    }
                >
                    <span class="flex-1 truncate">{label}</span>
                    {if is_selected {
                        view! { <span class="text-[10px] font-mono text-text-tertiary">"↵"</span> }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }}
                </button>
            }
            .into_any(),
        );
    }

    view! { <div>{out}</div> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_passes_all() {
        assert_eq!(score("", "Anything", &["k1"]), u32::MAX);
    }

    #[test]
    fn label_prefix_beats_substring_beats_keyword() {
        let prefix = score("set", "Settings", &["preferences"]);
        let mid = score("ings", "Settings", &["preferences"]);
        let kw_prefix = score("pre", "Settings", &["preferences"]);
        let kw_mid = score("ref", "Settings", &["preferences"]);
        assert!(prefix > mid);
        assert!(prefix > kw_prefix);
        assert!(kw_prefix > kw_mid);
    }

    #[test]
    fn case_insensitive() {
        assert!(score("SET", "settings", &[]) > 0);
        assert!(score("set", "SETTINGS", &[]) > 0);
    }

    #[test]
    fn no_match_returns_zero() {
        assert_eq!(score("zzz", "Settings", &["preferences"]), 0);
    }

    #[test]
    fn word_start_beats_random_substring() {
        // "mem" matches inside "Remember" mid-word vs at start of "Memory"
        let start = score("mem", "Memory", &[]);
        let mid = score("mem", "Remember", &[]);
        assert!(start > mid);
    }
}
