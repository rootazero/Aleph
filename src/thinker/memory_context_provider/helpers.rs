use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
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
/// invalidation. Using an opt-in capability slot keeps the change surgical:
/// `agent_init` registers the MCP once at startup, the session-end path
/// reads the slot and spawns the eviction (and reads the MCP's extension
/// registry for the session-close capture-filter pass).
///
/// `FailsClosed`, like the three siblings below: `emit_session_end_raw` reads
/// it as `if let Some(mcp)` with no `else`, so an uninstalled handle means the
/// curated snapshot is never evicted and `registry` stays `None`, taking the
/// session-close capture-filter pass with it. Nothing is granted; two
/// fire-and-forget effects simply do not happen, and no surface says so.
static SESSION_END_MCP: CapabilitySlot<Arc<super::MemoryContextProvider>> =
    CapabilitySlot::new("memory/session-end-mcp", MissingSemantics::FailsClosed);

/// The five handles in this file, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn session_end_mcp_slot() -> &'static dyn SlotStatus {
    &SESSION_END_MCP
}

/// Register a `MemoryContextProvider` for SessionEnd-triggered curated
/// invalidation. Idempotent; subsequent calls are a no-op (the `false` from
/// [`CapabilitySlot::install`] is discarded).
pub fn register_session_end_mcp(mcp: Arc<super::MemoryContextProvider>) {
    let _ = SESSION_END_MCP.install(mcp);
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
/// `FailsClosed`: `if let Some(summarizer)` with no `else`. An uninstalled
/// handle means no Spec-B summary is produced AND — the second-order cost, and
/// the one the call site spends twenty lines on — nothing registers with
/// `flush::global_registry()`'s readiness gate, so `HybridAssembler::assemble`
/// stops waiting for a snapshot that is never coming. That reads as "ready",
/// which is true, and useless.
static SESSION_END_SUMMARIZER: CapabilitySlot<
    Arc<crate::memory::session_search_summary::end_hook::SessionEndSummarizer>,
> = CapabilitySlot::new(
    "memory/session-end-summarizer",
    MissingSemantics::FailsClosed,
);

#[allow(dead_code)]
pub(crate) fn session_end_summarizer_slot() -> &'static dyn SlotStatus {
    &SESSION_END_SUMMARIZER
}

/// Register a `SessionEndSummarizer` for Spec B on-session-end hook firing.
/// Idempotent; subsequent calls are a no-op.
pub fn register_session_end_summarizer(
    summarizer: Arc<crate::memory::session_search_summary::end_hook::SessionEndSummarizer>,
) {
    let _ = SESSION_END_SUMMARIZER.install(summarizer);
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
/// `FailsClosed`: `if let Some(reflector)` with no `else`, and the reflector
/// self-gates on substance and cooldown anyway — so "no lesson was distilled
/// tonight" is a legitimate outcome that an uninstalled handle produces for a
/// different reason, with no way to tell them apart. Nothing is granted.
static SESSION_REFLECTOR: CapabilitySlot<Arc<crate::memory::session_reflection::SessionReflector>> =
    CapabilitySlot::new("memory/session-reflector", MissingSemantics::FailsClosed);

#[allow(dead_code)]
pub(crate) fn session_reflector_slot() -> &'static dyn SlotStatus {
    &SESSION_REFLECTOR
}

/// Register a `SessionReflector` for on-session-end lesson distillation.
/// Idempotent; subsequent calls are a no-op.
pub fn register_session_reflector(
    reflector: Arc<crate::memory::session_reflection::SessionReflector>,
) {
    let _ = SESSION_REFLECTOR.install(reflector);
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
/// `FailsClosed`: `.map(|cs| ..)` on the read, so an uninstalled handle
/// produces `None` and the Pillar-2 flush never runs — pending raws are not
/// drained into linked notes at session end. They are not lost (the periodic
/// path still consolidates them); what is lost is the immediacy, silently.
static SESSION_END_COMPRESSION: CapabilitySlot<
    Arc<crate::memory::compression::CompressionService>,
> = CapabilitySlot::new(
    "memory/session-end-compression",
    MissingSemantics::FailsClosed,
);

#[allow(dead_code)]
pub(crate) fn session_end_compression_slot() -> &'static dyn SlotStatus {
    &SESSION_END_COMPRESSION
}

/// Register a `CompressionService` for the on-session-end real-time flush.
/// Idempotent; subsequent calls are a no-op (first call wins).
pub fn register_session_end_compression(
    compression: Arc<crate::memory::compression::CompressionService>,
) {
    let _ = SESSION_END_COMPRESSION.install(compression);
}

/// Read the registered `CompressionService`, if any. Used by
/// `emit_session_end_raw` to spawn the session-end flush.
pub fn session_end_compression() -> Option<Arc<crate::memory::compression::CompressionService>> {
    SESSION_END_COMPRESSION.get().cloned()
}

/// Process-wide opt-in flag for injecting last session's open loops into the
/// next session's curated context (Batch 2 — `[memory.reflection]
/// open_loop_inject_prompt`). Mirrors the capability-slot idiom above to avoid
/// threading a bool through the `MemoryContextProvider` constructor and its
/// callers. Set once at startup by `agent_init`; read in `capture_curated`.
/// Unset (default) reads `false`, so the open-loops block is never injected
/// unless explicitly enabled.
///
/// The one `IndistinguishableDefault` in this file, and the sentence above is
/// the derivation: [`open_loop_inject`] ends in `.unwrap_or(false)`, so an
/// uninstalled flag is byte-identical to an operator who left
/// `[memory.reflection] open_loop_inject_prompt` off. Note this handle is
/// installed with the config value either way — `false` is a legitimate
/// INSTALL here, not an absence, which is exactly the distinction the slot can
/// now record and the bare flag could not.
static OPEN_LOOP_INJECT: CapabilitySlot<bool> = CapabilitySlot::new(
    "memory/open-loop-inject",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "false -- open loops are never injected, exactly as if \
                   [memory.reflection] open_loop_inject_prompt were off",
    },
);

#[allow(dead_code)]
pub(crate) fn open_loop_inject_slot() -> &'static dyn SlotStatus {
    &OPEN_LOOP_INJECT
}

/// Enable/disable open-loop injection. Idempotent; first call wins.
pub fn set_open_loop_inject(enabled: bool) {
    let _ = OPEN_LOOP_INJECT.install(enabled);
}

/// Whether last session's open loops should be injected into curated context.
pub fn open_loop_inject() -> bool {
    OPEN_LOOP_INJECT.get().copied().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All five handles reach the roster under the ids Task 11 will read, and
    /// the one that carries a `reads_as` sentence carries the right one.
    ///
    /// No runtime tie on `OPEN_LOOP_INJECT` on purpose: `curated.rs`'s tests
    /// call `set_open_loop_inject(true)`, so "an uninstalled read is false"
    /// would pass or fail on libtest's scheduling. A flaky guard teaches
    /// people to re-run.
    #[test]
    fn the_accessors_expose_all_five_handles_to_the_roster() {
        assert_eq!(session_end_mcp_slot().id(), "memory/session-end-mcp");
        assert_eq!(
            session_end_summarizer_slot().id(),
            "memory/session-end-summarizer"
        );
        assert_eq!(session_reflector_slot().id(), "memory/session-reflector");
        assert_eq!(
            session_end_compression_slot().id(),
            "memory/session-end-compression"
        );
        assert_eq!(open_loop_inject_slot().id(), "memory/open-loop-inject");

        for slot in [
            session_end_mcp_slot(),
            session_end_summarizer_slot(),
            session_reflector_slot(),
            session_end_compression_slot(),
        ] {
            assert!(
                matches!(slot.missing(), MissingSemantics::FailsClosed),
                "{} is a fire-and-forget session-end hook: absence must be \
                 FailsClosed, got {:?}",
                slot.id(),
                slot.missing()
            );
        }

        let MissingSemantics::IndistinguishableDefault { reads_as } =
            open_loop_inject_slot().missing()
        else {
            panic!(
                "expected IndistinguishableDefault, got {:?}",
                open_loop_inject_slot().missing()
            );
        };
        assert!(
            reads_as.contains("false"),
            "must name open_loop_inject()'s real fallback; got {reads_as:?}"
        );
    }
}
