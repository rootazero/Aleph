use crate::memory::notes::orientation::types::OrientationSnapshot;
use crate::sync_primitives::Arc;
use crate::thinker::xml_util::escape_xml;

/// Strip YAML frontmatter (`---\n…\n---\n`) from the start of `raw`.
/// If no frontmatter is present, returns `raw` unchanged.
#[must_use]
pub fn strip_frontmatter(raw: &str) -> &str {
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return &rest[end + 5..]; // skip "\n---\n"
        }
    }
    raw
}

#[must_use]
pub fn render_orientation_envelope(s: &OrientationSnapshot) -> String {
    // Escaping goes through `xml_util`, the single source of truth — a
    // hand-rolled copy here would silently miss the next hardening of the
    // shared helper (it already diverged once: `escape_xml_attr` exists).
    format!(
        "<NoteOrientation>\n<schema>\n{}\n</schema>\n<index_snapshot>\n{}\n</index_snapshot>\n<recent_log>\n{}\n</recent_log>\n</NoteOrientation>",
        escape_xml(&s.schema_text),
        escape_xml(&s.index_text),
        escape_xml(&s.recent_log_tail)
    )
}

/// Process-wide handle used by the `SessionEnd` evict path.
///
/// `emit_session_end_raw` lives in `gateway::session_manager::ops`
/// and has 3 callsites + 2 test fixtures. Threading an optional
/// `Arc<MemoryContextProvider>` argument through every caller would be a
/// 5-file blast radius for what is essentially a single fire-and-forget
/// invalidation. Using an opt-in `OnceCell` keeps the change surgical:
/// `agent_init` registers the MCP once at startup, the session-end path
/// reads the cell and spawns the eviction (and reads the MCP's extension
/// registry for the session-close capture-filter pass).
static SESSION_END_MCP: tokio::sync::OnceCell<Arc<super::MemoryContextProvider>> =
    tokio::sync::OnceCell::const_new();

/// Register a `MemoryContextProvider` for SessionEnd-triggered curated
/// invalidation. Idempotent; subsequent calls are a no-op (returns the
/// `Err(_)` from `OnceCell::set` silently).
pub fn register_session_end_mcp(mcp: Arc<super::MemoryContextProvider>) {
    let _ = SESSION_END_MCP.set(mcp);
}

/// Read the registered MCP, if any. Used by
/// `emit_session_end_raw` to evict per-session snapshots.
pub fn session_end_mcp() -> Option<Arc<super::MemoryContextProvider>> {
    SESSION_END_MCP.get().cloned()
}

/// Process-wide handle used by the `SessionEnd` summarization path (Spec B).
///
/// Mirrors the `SESSION_END_MCP` pattern: registered once at startup by
/// `agent_init`, consumed fire-and-forget at session-end in
/// `emit_session_end_raw`. The two cells are kept separate so
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

/// Process-wide handle for the session-end reflection path (Batch 2).
///
/// Mirrors the two cells above: registered once at startup by `agent_init`
/// (only when `[memory.reflection] enabled = true`), consumed fire-and-forget
/// at session-end in `emit_session_end_raw`. Kept separate so the
/// reflection feature stays independently removable from Spec A/Spec B.
static SESSION_REFLECTOR: tokio::sync::OnceCell<
    Arc<crate::memory::session_reflection::SessionReflector>,
> = tokio::sync::OnceCell::const_new();

/// Register a `SessionReflector` for on-session-end lesson distillation.
/// Idempotent; subsequent calls are a no-op.
pub fn register_session_reflector(
    reflector: Arc<crate::memory::session_reflection::SessionReflector>,
) {
    let _ = SESSION_REFLECTOR.set(reflector);
}

/// Read the registered `SessionReflector`, if any.
pub fn session_reflector() -> Option<Arc<crate::memory::session_reflection::SessionReflector>> {
    SESSION_REFLECTOR.get().cloned()
}

/// Process-wide handle for the real-time session-end flush (Real-time Memory
/// Pillar 2). Registered once at startup by `agent_init` (only when a
/// `CompressionService` is configured), consumed fire-and-forget at session-end
/// in `emit_session_end_raw` to drain pending raws into linked
/// notes immediately. Kept separate from the cells above so the flush feature
/// stays independently removable.
static SESSION_END_COMPRESSION: tokio::sync::OnceCell<
    Arc<crate::memory::compression::CompressionService>,
> = tokio::sync::OnceCell::const_new();

/// Register a `CompressionService` for the on-session-end real-time flush.
/// Idempotent; subsequent calls are a no-op (first call wins).
pub fn register_session_end_compression(
    compression: Arc<crate::memory::compression::CompressionService>,
) {
    let _ = SESSION_END_COMPRESSION.set(compression);
}

/// Read the registered `CompressionService`, if any. Used by
/// `emit_session_end_raw` to spawn the session-end flush.
pub fn session_end_compression() -> Option<Arc<crate::memory::compression::CompressionService>> {
    SESSION_END_COMPRESSION.get().cloned()
}

/// Process-wide opt-in flag for injecting last session's open loops into the
/// next session's curated context (Batch 2 — `[memory.reflection]
/// open_loop_inject_prompt`). Mirrors the `OnceCell` idiom above to avoid
/// threading a bool through the `MemoryContextProvider` constructor and its
/// callers. Set once at startup by `agent_init`; read in `capture_curated`.
/// Unset (default) reads `false`, so the open-loops block is never injected
/// unless explicitly enabled.
static OPEN_LOOP_INJECT: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();

/// Enable/disable open-loop injection. Idempotent; first call wins.
pub fn set_open_loop_inject(enabled: bool) {
    let _ = OPEN_LOOP_INJECT.set(enabled);
}

/// Whether last session's open loops should be injected into curated context.
pub fn open_loop_inject() -> bool {
    OPEN_LOOP_INJECT.get().copied().unwrap_or(false)
}
