//! Header and stat-card row for the memory console.
//!
//! Pure presentation: the only coupling to `Memory()`'s reactive graph is one
//! `Signal<Loadable<MemoryStats>>` and a refresh callback. Extracted alongside
//! `facets.rs` / `pager.rs` / `toast.rs` / `batch_bar.rs` / `cards.rs` for the
//! same reason each of those was: a self-contained, single-purpose view with
//! no business logic of its own.

use leptos::prelude::*;

use super::data::Loadable;
use crate::api::MemoryStats;
use crate::components::ui::Card;
use crate::i18n::{t, t_string, use_i18n};

#[component]
pub fn MemoryHeader(on_refresh: impl Fn() + Clone + Send + 'static) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <header class="flex items-start justify-between gap-4">
            <div>
                <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                    <svg width="32" height="32" attr:class="w-8 h-8 text-primary" viewBox="0 0 24 24"
                         fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <ellipse cx="12" cy="5" rx="9" ry="3" />
                        <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                    </svg>
                    {t!(i18n, memory.title)}
                </h2>
                <p class="text-text-secondary">{t!(i18n, memory.description)}</p>
            </div>
            <button
                class="flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-lg border border-border \
                       text-text-secondary hover:text-text-primary transition-colors flex-shrink-0"
                on:click=move |_| on_refresh()
            >
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 12a9 9 0 1 1-3-6.7" /><polyline points="21 3 21 9 15 9" />
                </svg>
                {t!(i18n, memory.refresh)}
            </button>
        </header>
    }
}

/// What `MemoryStats::scope` honestly says about the population the numbers
/// describe. Only `"agent"` and `"global"` are contracts the server documents
/// (`handle_stats`) and the CLI's unscoped call is the one path that still
/// reaches `"global"` — the Panel's three callers all pass an agent id, so
/// `"global"` is unreachable here today but stays a real, testable case
/// rather than a wildcard. `Unknown` is everything else, including the
/// empty-string default an un-upgraded core sends when it never set the
/// field: that response is actually store-wide, so it must not be asserted
/// as `Agent` just because it isn't literally `"global"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeLabel {
    Agent,
    Global,
    Unknown,
}

#[must_use]
fn classify_scope(scope: &str) -> ScopeLabel {
    match scope {
        "agent" => ScopeLabel::Agent,
        "global" => ScopeLabel::Global,
        _ => ScopeLabel::Unknown,
    }
}

#[component]
pub fn StatCards(stats: Signal<Loadable<MemoryStats>>) -> impl IntoView {
    let i18n = use_i18n();
    // "—" for both Loading and Failed: the header's Retry and the list's error
    // card already carry the failure; four red boxes would be noise.
    let num = move |pick: fn(&MemoryStats) -> Option<u64>| {
        Signal::derive(move || {
            stats
                .get()
                .as_ready()
                .and_then(pick)
                .map(|n| n.to_string())
                .unwrap_or_else(|| "\u{2014}".to_string())
        })
    };
    // Graph node/edge counts are `None` specifically when the server answered
    // store-wide (`MemoryStats::total_graph_nodes` doc comment: the note graph
    // is per-agent, so there is no honest single number). A bare "—" there
    // reads the same as a load failure; say why instead of leaving it silent.
    let graph_num = move |pick: fn(&MemoryStats) -> Option<u64>| {
        Signal::derive(move || match stats.get().as_ready().cloned() {
            None => "\u{2014}".to_string(),
            Some(s) => match pick(&s) {
                Some(n) => n.to_string(),
                None if s.scope == "global" => {
                    t_string!(i18n, memory.graph_scope_unavailable).to_string()
                }
                None => "\u{2014}".to_string(),
            },
        })
    };
    let scope_label = Signal::derive(move || {
        match stats.get().as_ready().map(|s| classify_scope(&s.scope)) {
            Some(ScopeLabel::Global) => t_string!(i18n, memory.scope_global).to_string(),
            Some(ScopeLabel::Agent) => t_string!(i18n, memory.scope_agent).to_string(),
            // `Unknown` covers the empty-string default an un-upgraded core
            // sends when it never set the field — that response is actually
            // store-wide (the one shape version skew can produce here), so
            // asserting "current agent" would be the one lie this component
            // can tell that's actually reachable. Say nothing instead.
            Some(ScopeLabel::Unknown) | None => String::new(),
        }
    });

    let facts = num(|s| Some(s.total_facts));
    let raws = num(|s| Some(s.total_memories));
    let nodes = graph_num(|s| s.total_graph_nodes);
    let edges = graph_num(|s| s.total_graph_edges);

    view! {
        <div>
            <div class="grid grid-cols-2 md:grid-cols-4 gap-6">
                <StatCard tone="primary" label=t_string!(i18n, memory.compressed_facts).to_string() value=facts />
                <StatCard tone="success" label=t_string!(i18n, memory.raw_memories).to_string() value=raws />
                <StatCard tone="primary" label=t_string!(i18n, memory.graph_nodes).to_string() value=nodes />
                <StatCard tone="success" label=t_string!(i18n, memory.graph_edges).to_string() value=edges />
            </div>
            // Say which population the numbers describe — they used to be a
            // cross-agent mix presented next to an agent-scoped list.
            <p class="text-[10px] text-text-tertiary mt-1.5 uppercase tracking-widest">
                {move || scope_label.get()}
            </p>
        </div>
    }
}

#[component]
fn StatCard(tone: &'static str, label: String, value: Signal<String>) -> impl IntoView {
    let (bg, fg) = if tone == "success" {
        ("bg-success-subtle border-success/10", "text-success")
    } else {
        ("bg-primary-subtle border-primary/10", "text-primary")
    };
    view! {
        <Card class=format!("{bg} p-6 flex flex-col items-start")>
            <span class=format!("text-[10px] font-bold {fg} uppercase tracking-widest mb-1.5")>{label}</span>
            <span class="text-3xl font-bold font-mono">{move || value.get()}</span>
        </Card>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_scope_recognizes_the_two_documented_values() {
        assert_eq!(classify_scope("agent"), ScopeLabel::Agent);
        assert_eq!(classify_scope("global"), ScopeLabel::Global);
    }

    #[test]
    fn classify_scope_treats_anything_else_as_unknown_not_agent() {
        // This is the actual bug: an un-upgraded core that never sets `scope`
        // sends the empty-string default, and that response is store-wide
        // (the one shape version skew can produce here) -- it must not be
        // classified the same as a real, agent-scoped response.
        assert_eq!(classify_scope(""), ScopeLabel::Unknown);
        assert_eq!(classify_scope("something-else"), ScopeLabel::Unknown);
    }
}
