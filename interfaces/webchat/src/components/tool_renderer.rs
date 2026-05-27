//! Tool renderer registry — the dispatch table used by `WorkspacePanel`
//! to turn a [`ToolCallEntry`] into rich JSX.
//!
//! Mirrors UI-TARS' `tool-renderers/index.ts`: each renderer declares
//! which tool names it claims via `matches`, the registry picks the first
//! match (with a stable iteration order), and a default JSON-dump
//! fallback ensures every tool call has *something* to show.
//!
//! New renderers register at app boot. The registry is provided via
//! Leptos context so any future view can dispatch the same way.

use crate::views::chat::state::ToolCallEntry;
use leptos::prelude::*;

/// Trait every workspace tool renderer implements.
///
/// `matches` is a static predicate over the tool name — keep it cheap
/// (substring / equality), no allocations. `Send + Sync` is required so
/// the registry can ride Leptos' `provide_context` (which is bounded to
/// `Send + Sync + 'static`).
pub trait ToolRenderer: Send + Sync + 'static {
    /// Short stable id for diagnostics (logs / dev tools).
    fn id(&self) -> &'static str;

    /// Whether this renderer claims a given tool call.
    fn matches(&self, entry: &ToolCallEntry) -> bool;

    /// Render the tool call into an [`AnyView`].
    fn render(&self, entry: &ToolCallEntry) -> AnyView;
}

/// Ordered list of renderers. First `matches` wins; a default JSON
/// renderer is always pushed last so dispatch never falls through.
#[derive(Clone)]
pub struct ToolRendererRegistry {
    renderers: std::sync::Arc<Vec<Box<dyn ToolRenderer>>>,
}

impl ToolRendererRegistry {
    /// Empty registry — must `.with` at least one renderer before use.
    /// Prefer [`Self::with_builtins`] for the default Aleph palette.
    pub fn new() -> Self {
        Self {
            renderers: std::sync::Arc::new(Vec::new()),
        }
    }

    /// Default Aleph palette: code / search / default-JSON fallback.
    /// Add new renderers in registration order; ties broken by first-wins.
    pub fn with_builtins() -> Self {
        let renderers: Vec<Box<dyn ToolRenderer>> = vec![
            Box::new(CodeToolRenderer),
            Box::new(SearchToolRenderer),
            Box::new(DefaultJsonRenderer), // always-last fallback
        ];
        Self {
            renderers: std::sync::Arc::new(renderers),
        }
    }

    /// Resolve the renderer claim chain into an [`AnyView`]. Guaranteed
    /// to return *something* — the trailing `DefaultJsonRenderer` claims
    /// every entry.
    pub fn render(&self, entry: &ToolCallEntry) -> AnyView {
        for r in self.renderers.iter() {
            if r.matches(entry) {
                return r.render(entry);
            }
        }
        // Defense in depth — registry is built with a fallback, but if a
        // caller built one without it, surface an explicit empty state
        // instead of panicking.
        view! {
            <div class="text-xs text-text-tertiary italic">
                "No renderer matched (tool: " {entry.tool_name.clone()} ")"
            </div>
        }
        .into_any()
    }

    /// Diagnostic ids in registration order (for dev surfaces / settings).
    pub fn ids(&self) -> Vec<&'static str> {
        self.renderers.iter().map(|r| r.id()).collect()
    }
}

impl Default for ToolRendererRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

// -------- Built-in renderers --------

/// Generic JSON-dump fallback. Always claims.
struct DefaultJsonRenderer;

impl ToolRenderer for DefaultJsonRenderer {
    fn id(&self) -> &'static str {
        "default_json"
    }

    fn matches(&self, _entry: &ToolCallEntry) -> bool {
        true
    }

    fn render(&self, entry: &ToolCallEntry) -> AnyView {
        let name = entry.tool_name.clone();
        let status = entry.status.clone();
        let duration = entry
            .duration_ms
            .map(|d| format!("{d}ms"))
            .unwrap_or_else(|| "—".to_string());
        let tool_id = entry.tool_id.clone();
        view! {
            <div class="flex flex-col gap-2 text-sm">
                <div class="flex items-center justify-between text-xs text-text-tertiary">
                    <span class="font-mono uppercase tracking-wider">"tool"</span>
                    <span class="font-mono">{tool_id}</span>
                </div>
                <h3 class="text-base font-medium text-text-primary">{name}</h3>
                <div class="flex gap-3 text-xs">
                    <span class=move || format!(
                        "px-2 py-0.5 rounded-md font-mono {}",
                        status_pill_class(&status)
                    )>{status.clone()}</span>
                    <span class="text-text-tertiary">"duration: " {duration}</span>
                </div>
                <p class="text-xs text-text-tertiary italic mt-2">
                    "Payload capture not yet wired — see WorkspaceState follow-up."
                </p>
            </div>
        }
        .into_any()
    }
}

fn status_pill_class(status: &str) -> &'static str {
    match status {
        "running" => "bg-warning/15 text-warning",
        "completed" => "bg-success/15 text-success",
        "failed" => "bg-danger/15 text-danger",
        _ => "bg-surface-sunken text-text-secondary",
    }
}

/// Renderer for shell-style / code-exec tools — highlights the monospace
/// nature and shows a richer header. Claims tools whose name starts with
/// "bash", "shell", "code_exec", or contains "_exec".
struct CodeToolRenderer;

impl ToolRenderer for CodeToolRenderer {
    fn id(&self) -> &'static str {
        "code"
    }

    fn matches(&self, entry: &ToolCallEntry) -> bool {
        let n = entry.tool_name.to_lowercase();
        n.starts_with("bash")
            || n.starts_with("shell")
            || n.starts_with("code_exec")
            || n.contains("_exec")
    }

    fn render(&self, entry: &ToolCallEntry) -> AnyView {
        let name = entry.tool_name.clone();
        let status = entry.status.clone();
        let duration = entry
            .duration_ms
            .map(|d| format!("{d}ms"))
            .unwrap_or_else(|| "—".to_string());
        view! {
            <div class="flex flex-col gap-2">
                <div class="flex items-center gap-2 text-xs text-text-tertiary uppercase tracking-wider">
                    <span class="font-mono">"$"</span>
                    <span>{name.clone()}</span>
                </div>
                <div class="rounded-md border border-border bg-surface-sunken p-3 font-mono text-xs text-text-secondary">
                    "// command + output capture pending payload wiring"
                </div>
                <div class="flex gap-3 text-xs">
                    <span class=move || format!(
                        "px-2 py-0.5 rounded-md font-mono {}",
                        status_pill_class(&status)
                    )>{status.clone()}</span>
                    <span class="text-text-tertiary">"duration: " {duration}</span>
                </div>
            </div>
        }
        .into_any()
    }
}

/// Renderer for search / lookup tools. Claims `search_*`, `*_search`,
/// `web_search`, `grep`, `find`.
struct SearchToolRenderer;

impl ToolRenderer for SearchToolRenderer {
    fn id(&self) -> &'static str {
        "search"
    }

    fn matches(&self, entry: &ToolCallEntry) -> bool {
        let n = entry.tool_name.to_lowercase();
        n.starts_with("search_")
            || n.ends_with("_search")
            || n == "web_search"
            || n == "grep"
            || n == "find"
    }

    fn render(&self, entry: &ToolCallEntry) -> AnyView {
        let name = entry.tool_name.clone();
        let status = entry.status.clone();
        view! {
            <div class="flex flex-col gap-2">
                <div class="flex items-center gap-2 text-xs text-text-tertiary uppercase tracking-wider">
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3" viewBox="0 0 24 24"
                         fill="none" stroke="currentColor" stroke-width="2"
                         stroke-linecap="round" stroke-linejoin="round">
                        <circle cx="11" cy="11" r="8"/>
                        <line x1="21" y1="21" x2="16.65" y2="16.65"/>
                    </svg>
                    <span>{name.clone()}</span>
                </div>
                <p class="text-xs text-text-tertiary italic">
                    "Results panel pending payload wiring — once tool outputs "
                    "are captured, hits land here in a clickable list."
                </p>
                <span class=move || format!(
                    "self-start px-2 py-0.5 rounded-md font-mono text-xs {}",
                    status_pill_class(&status)
                )>{status.clone()}</span>
            </div>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ToolCallEntry {
        ToolCallEntry {
            tool_id: "t-0".into(),
            tool_name: name.into(),
            status: "completed".into(),
            duration_ms: Some(42),
        }
    }

    #[test]
    fn code_renderer_claims_bash_and_exec() {
        let r = CodeToolRenderer;
        assert!(r.matches(&entry("bash")));
        assert!(r.matches(&entry("bash_run")));
        assert!(r.matches(&entry("code_exec")));
        assert!(r.matches(&entry("shell")));
        assert!(r.matches(&entry("python_exec")));
        assert!(!r.matches(&entry("memory_recall")));
    }

    #[test]
    fn search_renderer_claims_search_family() {
        let r = SearchToolRenderer;
        assert!(r.matches(&entry("web_search")));
        assert!(r.matches(&entry("search_files")));
        assert!(r.matches(&entry("hybrid_search")));
        assert!(r.matches(&entry("grep")));
        assert!(r.matches(&entry("find")));
        assert!(!r.matches(&entry("memory_recall")));
    }

    #[test]
    fn default_renderer_claims_everything() {
        let r = DefaultJsonRenderer;
        assert!(r.matches(&entry("anything")));
        assert!(r.matches(&entry("")));
    }

    #[test]
    fn registry_ids_in_registration_order() {
        let reg = ToolRendererRegistry::with_builtins();
        assert_eq!(reg.ids(), vec!["code", "search", "default_json"]);
    }
}
