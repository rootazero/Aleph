use crate::memory::notes::orientation::types::OrientationSnapshot;
use crate::sync_primitives::Arc;

/// Escape `&`, `<`, `>` for XML embedding.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Strip YAML frontmatter (`---\n…\n---\n`) from the start of `raw`.
/// If no frontmatter is present, returns `raw` unchanged.
pub fn strip_frontmatter(raw: &str) -> &str {
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return &rest[end + 5..]; // skip "\n---\n"
        }
    }
    raw
}

pub fn render_orientation_envelope(s: &OrientationSnapshot) -> String {
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        "<NoteOrientation>\n<schema>\n{}\n</schema>\n<index_snapshot>\n{}\n</index_snapshot>\n<recent_log>\n{}\n</recent_log>\n</NoteOrientation>",
        esc(&s.schema_text),
        esc(&s.index_text),
        esc(&s.recent_log_tail)
    )
}

/// Process-wide handle used by the SessionEnd evict path.
///
/// `emit_session_end_raw_with_registry` lives in `gateway::session_manager::ops`
/// and has 3 callsites + 2 test fixtures. Threading an optional
/// `Arc<MemoryContextProvider>` argument through every caller would be a
/// 5-file blast radius for what is essentially a single fire-and-forget
/// invalidation. Using an opt-in `OnceCell` keeps the change surgical:
/// `agent_init` registers the MCP once at startup, the session-end path
/// reads the cell and spawns the eviction.
static SESSION_END_MCP: tokio::sync::OnceCell<Arc<super::MemoryContextProvider>> =
    tokio::sync::OnceCell::const_new();

/// Register a `MemoryContextProvider` for SessionEnd-triggered curated
/// invalidation. Idempotent; subsequent calls are a no-op (returns the
/// `Err(_)` from `OnceCell::set` silently).
pub fn register_session_end_mcp(mcp: Arc<super::MemoryContextProvider>) {
    let _ = SESSION_END_MCP.set(mcp);
}

/// Read the registered MCP, if any. Used by
/// `emit_session_end_raw_with_registry` to evict per-session snapshots.
pub fn session_end_mcp() -> Option<Arc<super::MemoryContextProvider>> {
    SESSION_END_MCP.get().cloned()
}

/// Process-wide handle used by the SessionEnd summarization path (Spec B).
///
/// Mirrors the `SESSION_END_MCP` pattern: registered once at startup by
/// `agent_init`, consumed fire-and-forget at session-end in
/// `emit_session_end_raw_with_registry`. The two cells are kept separate so
/// Spec A (cache invalidation) and Spec B (summary production) remain
/// independently removable.
static SESSION_END_SUMMARIZER: tokio::sync::OnceCell<
    Arc<crate::memory::session_search_summary::end_hook::SessionEndSummarizer>,
> = tokio::sync::OnceCell::const_new();

/// Register a `SessionEndSummarizer` for Spec B on-session-end hook firing.
/// Idempotent; subsequent calls are a no-op.
pub fn register_session_end_summarizer(
    summarizer: Arc<crate::memory::session_search_summary::end_hook::SessionEndSummarizer>,
) {
    let _ = SESSION_END_SUMMARIZER.set(summarizer);
}

/// Read the registered `SessionEndSummarizer`, if any.
pub fn session_end_summarizer(
) -> Option<Arc<crate::memory::session_search_summary::end_hook::SessionEndSummarizer>> {
    SESSION_END_SUMMARIZER.get().cloned()
}
