//! Batch 2 — session-end reflection ("lessons learned").
//!
//! When a *substantive* session ends, [`SessionReflector::reflect`] makes a
//! single LLM call to distill first-person LESSONS — not a digest of what was
//! discussed, but what to do differently next time, user preferences observed,
//! and approaches that worked or failed. The distilled text is written as a
//! [`RawMemorySource::Reflection`] row, which the compound ingestor then turns
//! into `feedback/lessons` notes (via the lesson-tuned source prompt).
//!
//! Gating (driven by [`ReflectionConfig`], an opt-in feature, default off):
//!   - `enabled`          — master switch (the only default-off flag: the
//!     open-loop sub-flags default on, so flipping `enabled` alone lights the
//!     whole lessons + open-loops pipeline).
//!   - `min_turns`        — skip trivial sessions (too few messages).
//!   - `min_user_chars`   — skip sessions the user barely engaged with.
//!   - `cooldown_minutes` — per-agent throttle so back-to-back session ends
//!     don't fire a reflection (and an LLM call) every time. Persisted to
//!     `compression_metadata` (the dream-watermark table) when a backend is
//!     wired, so a daemon restart cannot reset the window.
//!
//! The reflector is independent of the Spec B `SessionEndSummarizer` (which
//! produces the `/end-summary` digest for `session_search`): the two run from
//! separate global cells so either can be removed without touching the other.

use crate::sync_primitives::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::types::memory::ReflectionConfig;
use crate::error::AlephError;
use crate::gateway::session_store::SessionStore;
use crate::memory::session_compactor::summary_engine::strip_analysis_block;
use crate::memory::session_search_summary::synthesizer::SummaryLlm;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::sync_primitives::Arc;

/// Maximum number of turns loaded for reflection (mirrors the synthesizer).
const REFLECT_INPUT_MAX_TURNS: usize = 50;

/// Maximum total content characters loaded for reflection (~8k tokens).
const REFLECT_INPUT_MAX_CHARS: usize = 32_000;

/// Sentinel the LLM returns when a session carries no durable lesson.
const NO_LESSON_SENTINEL: &str = "NONE";

/// `compression_metadata` consumer name for the persisted cooldown watermark
/// (same table + key shape `feedback_distill` uses for its cursor).
const COOLDOWN_CONSUMER: &str = "session_reflection";

/// Distils session-end lessons into ingestable [`RawMemorySource::Reflection`]
/// rows.
pub struct SessionReflector {
    store: Arc<dyn RawMemoryStore>,
    session_store: Arc<dyn SessionStore>,
    llm: Arc<dyn SummaryLlm>,
    config: ReflectionConfig,
    /// `agent_id` -> unix-seconds of last reflection attempt. Fast path in
    /// front of the persisted watermark below; on its own (no
    /// `cooldown_store`) it degrades to the old restart-resettable throttle.
    last_reflect: Mutex<HashMap<String, i64>>,
    /// Optional persistence for the cooldown watermark. Without it a daemon
    /// restart reset the throttle, so a crash-loop (or frequent restarts)
    /// could fire one LLM call per agent per restart.
    cooldown_store: Option<crate::memory::store::MemoryBackend>,
}

impl SessionReflector {
    pub fn new(
        store: Arc<dyn RawMemoryStore>,
        session_store: Arc<dyn SessionStore>,
        llm: Arc<dyn SummaryLlm>,
        config: ReflectionConfig,
    ) -> Self {
        Self {
            store,
            session_store,
            llm,
            config,
            last_reflect: Mutex::new(HashMap::new()),
            cooldown_store: None,
        }
    }

    /// Persist the per-agent cooldown watermark to `compression_metadata`
    /// (builder-style). The in-memory map stays as the fast path.
    #[must_use]
    pub fn with_cooldown_store(mut self, backend: crate::memory::store::MemoryBackend) -> Self {
        self.cooldown_store = Some(backend);
        self
    }

    /// Reflect on a just-ended session. Idempotent at the throttle level and
    /// fire-and-forget at the call site: every skip path returns `Ok(())`.
    pub async fn reflect(&self, agent_id: &str, session_id: &str) -> Result<(), AlephError> {
        if !self.config.enabled {
            return Ok(());
        }

        // Per-agent cooldown — checked before any work so a flurry of session
        // ends costs at most one reflection per window.
        if self.in_cooldown(agent_id) {
            return Ok(());
        }

        // Load the recent transcript window.
        let transcript = self
            .session_store
            .load_window(
                agent_id,
                session_id,
                REFLECT_INPUT_MAX_TURNS,
                REFLECT_INPUT_MAX_CHARS,
            )
            .await
            .map_err(|e| AlephError::other(format!("reflect: load_window failed: {e}")))?;

        // Substance gates — skip trivial sessions without an LLM call.
        if transcript.len() < self.config.min_turns as usize {
            return Ok(());
        }
        let user_chars: usize = transcript
            .iter()
            .filter(|(role, _)| role.eq_ignore_ascii_case("user"))
            .map(|(_, content)| content.len())
            .sum();
        if user_chars < self.config.min_user_chars as usize {
            return Ok(());
        }

        // Record the attempt *before* the LLM call so a NONE result still
        // consumes the cooldown window (no repeated spend on the same agent).
        self.mark_attempt(agent_id);

        let prompt = build_reflection_prompt(&transcript, self.config.open_loop_tracking);
        let output = self.llm.complete(&prompt).await?;
        let stripped = strip_analysis_block(&output);

        // With open-loop tracking on, the model returns two labelled sections.
        // Split them: lessons keep flowing to the feedback/lessons notes, while
        // open loops are persisted beside MEMORY.md for next-session injection
        // (R5 — "AI proactively reaches out"). Off → the whole output is the lessons body
        // (legacy behaviour, byte-for-byte unchanged).
        let lessons = if self.config.open_loop_tracking {
            let (lessons, loops) = split_sections(&stripped);
            self.write_open_loops(agent_id, &loops).await;
            lessons
        } else {
            normalize_section(&stripped)
        };

        // Nothing worth remembering — don't write an empty/NONE note.
        if lessons.is_empty() {
            return Ok(());
        }

        let raw = RawMemory::new(lessons, RawMemorySource::Reflection)
            .with_agent(agent_id)
            .with_session(session_id);
        self.store.insert_raw_memory(&raw).await?;
        Ok(())
    }

    /// Persist this session's open loops beside the agent's `MEMORY.md`
    /// (`OPEN_LOOPS.md`) so the next session can inject them. Overwrites on
    /// every reflection (resolved loops disappear) and removes the file when
    /// there are none. Best-effort: a write failure is logged, never propagated
    /// (P7 — memory is degradable).
    async fn write_open_loops(&self, agent_id: &str, loops: &str) {
        let Some(path) = open_loops_path(agent_id) else {
            return;
        };
        if loops.trim().is_empty() {
            // No open loops this round — clear any stale file.
            let _ = tokio::fs::remove_file(&path).await;
            return;
        }
        if let Some(parent) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                tracing::warn!("reflect: open-loops mkdir failed: {e}");
                return;
            }
        }
        if let Err(e) = tokio::fs::write(&path, loops.as_bytes()).await {
            tracing::warn!("reflect: open-loops write failed: {e}");
        }
    }

    /// True when `agent_id` reflected within the cooldown window.
    fn in_cooldown(&self, agent_id: &str) -> bool {
        if self.config.cooldown_minutes == 0 {
            return false;
        }
        let window = i64::from(self.config.cooldown_minutes) * 60;
        let now = chrono::Utc::now().timestamp();
        matches!(self.last_reflect_at(agent_id), Some(last) if now - last < window)
    }

    /// Unix-seconds of the last reflection attempt for `agent_id`: in-memory
    /// map first (fast path), falling back to the persisted watermark so a
    /// daemon restart cannot reset the throttle. A persisted hit warms the
    /// map; a read failure degrades to "no prior attempt" (P7 — the throttle
    /// is protective, never gating correctness).
    fn last_reflect_at(&self, agent_id: &str) -> Option<i64> {
        {
            let guard = self.last_reflect.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(&last) = guard.get(agent_id) {
                return Some(last);
            }
        }
        let store = self.cooldown_store.as_ref()?;
        match store.get_dream_watermark(COOLDOWN_CONSUMER, agent_id) {
            Ok(Some(last)) => {
                let mut guard = self.last_reflect.lock().unwrap_or_else(|e| e.into_inner());
                guard.insert(agent_id.to_string(), last);
                Some(last)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "reflect: cooldown watermark read failed; treating as no prior attempt"
                );
                None
            }
        }
    }

    fn mark_attempt(&self, agent_id: &str) {
        let now = chrono::Utc::now().timestamp();
        {
            let mut guard = self.last_reflect.lock().unwrap_or_else(|e| e.into_inner());
            guard.insert(agent_id.to_string(), now);
        }
        // Write-through to the persisted watermark. Best-effort: a failure
        // just leaves the throttle process-local, the old behaviour.
        if let Some(store) = &self.cooldown_store {
            if let Err(e) = store.set_dream_watermark(COOLDOWN_CONSUMER, agent_id, now) {
                tracing::warn!(
                    error = %e,
                    "reflect: cooldown watermark persist failed (non-fatal)"
                );
            }
        }
    }
}

/// Build the lessons-extraction prompt. Asks for reusable first-person lessons
/// and an explicit [`NO_LESSON_SENTINEL`] escape hatch so trivial sessions
/// produce no note (R7 — the model, not a heuristic, decides what is durable).
///
/// When `track_open_loops` is set, the prompt additionally asks for unresolved
/// follow-ups in a second labelled section (see [`split_sections`]); the LLM —
/// not a heuristic — decides what is still open (R7/R9: one call, zero extra
/// middleware).
fn build_reflection_prompt(messages: &[(String, String)], track_open_loops: bool) -> String {
    let today = chrono::Utc::now().format("%Y-%m-%d");
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "You are reflecting on a conversation that just ended, to record durable \
         LESSONS for your future self. Today is {today}. This is NOT a summary of \
         what was discussed — it is what you LEARNED about how to work better.\n\n\
         Extract only genuinely reusable lessons, such as:\n\
         - User preferences and corrections (what the user values, how they want things done)\n\
         - Mistakes or detours you made, and how to avoid them next time\n\
         - Approaches that worked in a specific situation, and the remedy that fixed a failure\n\n\
         Write each lesson as one concise first-person bullet phrased as \
         evidence → implication: name the concrete situation, quote the user \
         verbatim when they said it, then state the future default. Shape: \
         When <situation>, the user said \"<quote>\" → next time <default>.\n\n\
         Rules for durable lessons:\n\
         - Keep identifiers verbatim and greppable — file paths, commands, error \
         strings, config keys, names. Never paraphrase them.\n\
         - Use absolute dates (e.g. {today}), never relative ones (\"today\", \
         \"last week\").\n\
         - Do not record environment-dependent transient failures (network blips, \
         rate limits, a service being briefly down) as permanent truths.\n\
         - Do not record negative assertions like \"tool X is broken\"; record the \
         remedy or workaround that succeeded instead.\n\
         - Gate every bullet: will a future agent plausibly act better for having \
         it? Drop anything trivial, one-off, or already obvious.\n"
    ));
    prompt.push_str(&format!(
        "If no bullet survives the gate, output exactly: {NO_LESSON_SENTINEL}\n"
    ));

    if track_open_loops {
        prompt.push_str(
            "\nALSO identify any OPEN LOOPS — unresolved questions, promised follow-ups, \
             or tasks left incomplete when the conversation ended — that you should \
             proactively pick back up next time. Write each as one short actionable \
             bullet from your perspective (\"Follow up on…\", \"Check whether…\"). Only \
             include loops that are genuinely still open; if the user already got what \
             they needed, there are none.\n",
        );
    }

    prompt.push_str("\n--- Conversation ---");
    for (role, content) in messages {
        prompt.push('\n');
        prompt.push_str(&format!("[{role}]: {content}"));
    }
    prompt.push_str("\n--- End conversation ---\n\n");

    if track_open_loops {
        prompt.push_str(&format!(
            "Respond in exactly these two sections, keeping the headers verbatim:\n\
             {LESSONS_HEADER}\n<one bullet per lesson, or {NO_LESSON_SENTINEL}>\n\n\
             {OPEN_LOOPS_HEADER}\n<one bullet per open loop, or {NO_LESSON_SENTINEL}>"
        ));
    } else {
        prompt.push_str("Output the lessons now (bullets, or NONE).");
    }
    prompt
}

/// Section header the model echoes for the lessons block in open-loop mode.
const LESSONS_HEADER: &str = "LESSONS:";
/// Section header the model echoes for the open-loops block in open-loop mode.
const OPEN_LOOPS_HEADER: &str = "OPEN_LOOPS:";

/// Split a two-section reflection output into `(lessons, open_loops)`.
///
/// Robust to the model omitting either header: everything before the
/// `OPEN_LOOPS:` marker (case-insensitive) is lessons, the rest is open loops.
/// Each section is normalised via [`normalize_section`] so a `NONE`/empty body
/// collapses to an empty string. UTF-8 safe (header matching never slices a
/// multibyte char — P7).
fn split_sections(output: &str) -> (String, String) {
    let lower = output.to_ascii_lowercase();
    let marker = OPEN_LOOPS_HEADER.to_ascii_lowercase();
    let (lessons_raw, loops_raw) = match lower.find(&marker) {
        // `to_ascii_lowercase` preserves byte length/positions, and the marker
        // is ASCII, so `idx` is a valid char boundary in `output`.
        Some(idx) => (&output[..idx], &output[idx..]),
        None => (output, ""),
    };
    (
        normalize_section(&strip_header(lessons_raw, LESSONS_HEADER)),
        normalize_section(&strip_header(loops_raw, OPEN_LOOPS_HEADER)),
    )
}

/// Drop a leading case-insensitive `header` from `section` (after trimming
/// leading whitespace). Returns the remainder, or the trimmed input unchanged
/// when the header is absent. `str::get` keeps the slice on a char boundary.
fn strip_header(section: &str, header: &str) -> String {
    let trimmed = section.trim_start();
    match trimmed.get(..header.len()) {
        Some(prefix) if prefix.eq_ignore_ascii_case(header) => trimmed[header.len()..].to_string(),
        _ => trimmed.to_string(),
    }
}

/// Trim a section and collapse the `NONE` sentinel (or an empty body) to "".
fn normalize_section(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case(NO_LESSON_SENTINEL) {
        String::new()
    } else {
        t.to_string()
    }
}

/// Resolve `~/.aleph/agents/<agent_id>/OPEN_LOOPS.md` — beside the curated
/// `MEMORY.md` that `MemoryContextProvider::agent_memory_path` renders, so the
/// injection side reads the same location. `None` if the home dir is
/// unresolvable.
fn open_loops_path(agent_id: &str) -> Option<PathBuf> {
    let base = crate::discovery::aleph_home_dir().ok()?;
    // Sanitize the agent id so it cannot traverse out of the agents directory.
    let safe_id = agent_id.replace(['/', '\\', '\0'], "_").replace("..", "__");
    Some(base.join("agents").join(safe_id).join("OPEN_LOOPS.md"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::memory::session_search_summary::synthesizer::test_support::{
        make_store, InMemorySessionStore, MockSummaryLlm,
    };

    fn cfg(enabled: bool, min_turns: u32, min_user_chars: u32, cooldown: u32) -> ReflectionConfig {
        ReflectionConfig {
            enabled,
            min_turns,
            min_user_chars,
            cooldown_minutes: cooldown,
            open_loop_tracking: false,
            open_loop_inject_prompt: false,
        }
    }

    /// A transcript that easily clears the default substance gates.
    fn substantive() -> Arc<InMemorySessionStore> {
        InMemorySessionStore::new().with_messages(
            "agent-1",
            "sess-1",
            &[
                (
                    "user",
                    "I really prefer concise answers, stop over-explaining.",
                ),
                ("assistant", "Understood, I'll keep it short."),
                (
                    "user",
                    "Also the build kept failing because of the wasm step.",
                ),
                ("assistant", "Right, the panel needs a rebuild after wasm."),
                ("user", "Yes, remember that for next time."),
            ],
        )
    }

    async fn reflection_count(store: &Arc<dyn RawMemoryStore>, agent: &str) -> usize {
        store
            .get_raw_by_source(RawMemorySource::Reflection, agent, 100)
            .await
            .unwrap()
            .len()
    }

    #[tokio::test]
    async fn skips_when_disabled() {
        let store = make_store();
        let llm = MockSummaryLlm::with_response("- I should keep answers short");
        let reflector = SessionReflector::new(
            store.clone(),
            substantive(),
            llm.clone(),
            cfg(false, 1, 0, 0),
        );
        reflector.reflect("agent-1", "sess-1").await.unwrap();
        assert_eq!(llm.call_count(), 0, "disabled: no LLM call");
        assert_eq!(reflection_count(&store, "agent-1").await, 0);
    }

    #[tokio::test]
    async fn skips_when_too_few_turns() {
        let store = make_store();
        let llm = MockSummaryLlm::with_response("- lesson");
        let reflector = SessionReflector::new(
            store.clone(),
            substantive(), // 5 messages
            llm.clone(),
            cfg(true, 50, 0, 0),
        );
        reflector.reflect("agent-1", "sess-1").await.unwrap();
        assert_eq!(llm.call_count(), 0, "below min_turns: no LLM call");
        assert_eq!(reflection_count(&store, "agent-1").await, 0);
    }

    #[tokio::test]
    async fn skips_when_too_few_user_chars() {
        let store = make_store();
        let llm = MockSummaryLlm::with_response("- lesson");
        let reflector = SessionReflector::new(
            store.clone(),
            substantive(),
            llm.clone(),
            cfg(true, 1, 100_000, 0),
        );
        reflector.reflect("agent-1", "sess-1").await.unwrap();
        assert_eq!(llm.call_count(), 0, "below min_user_chars: no LLM call");
        assert_eq!(reflection_count(&store, "agent-1").await, 0);
    }

    #[tokio::test]
    async fn writes_reflection_when_qualified() {
        let store = make_store();
        let llm = MockSummaryLlm::with_response("- I should rebuild wasm before the panel");
        let reflector = SessionReflector::new(
            store.clone(),
            substantive(),
            llm.clone(),
            cfg(true, 1, 0, 0),
        );
        reflector.reflect("agent-1", "sess-1").await.unwrap();
        assert_eq!(llm.call_count(), 1, "qualified: exactly one LLM call");

        let rows = store
            .get_raw_by_source(RawMemorySource::Reflection, "agent-1", 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "one reflection row written");
        assert!(rows[0].content.contains("rebuild wasm"));
        assert_eq!(rows[0].session_id.as_deref(), Some("sess-1"));
    }

    #[tokio::test]
    async fn skips_write_when_llm_returns_none() {
        let store = make_store();
        let llm = MockSummaryLlm::with_response("NONE");
        let reflector = SessionReflector::new(
            store.clone(),
            substantive(),
            llm.clone(),
            cfg(true, 1, 0, 0),
        );
        reflector.reflect("agent-1", "sess-1").await.unwrap();
        assert_eq!(llm.call_count(), 1, "LLM is consulted");
        assert_eq!(
            reflection_count(&store, "agent-1").await,
            0,
            "NONE result writes no row"
        );
    }

    #[tokio::test]
    async fn cooldown_blocks_second_reflection() {
        let store = make_store();
        let llm = MockSummaryLlm::with_response("- a real lesson");
        // 30-minute cooldown; two calls in the same instant — the second must
        // be throttled.
        let reflector = SessionReflector::new(
            store.clone(),
            substantive(),
            llm.clone(),
            cfg(true, 1, 0, 30),
        );
        reflector.reflect("agent-1", "sess-1").await.unwrap();
        reflector.reflect("agent-1", "sess-1").await.unwrap();
        assert_eq!(llm.call_count(), 1, "cooldown blocks the second LLM call");
        assert_eq!(reflection_count(&store, "agent-1").await, 1);
    }

    #[tokio::test]
    async fn cooldown_survives_restart_via_persisted_watermark() {
        use crate::memory::store::sqlite::SqliteMemoryBackend;

        let backend = Arc::new(SqliteMemoryBackend::in_memory().expect("in-memory backend"));
        let store = make_store();
        let llm = MockSummaryLlm::with_response("- a real lesson");

        let reflector = SessionReflector::new(
            store.clone(),
            substantive(),
            llm.clone(),
            cfg(true, 1, 0, 30),
        )
        .with_cooldown_store(backend.clone());
        reflector.reflect("agent-1", "sess-1").await.unwrap();
        assert_eq!(llm.call_count(), 1);

        // "Restart": a brand-new reflector (fresh in-memory map) sharing the
        // same backend must still be throttled by the persisted watermark.
        let restarted = SessionReflector::new(
            store.clone(),
            substantive(),
            llm.clone(),
            cfg(true, 1, 0, 30),
        )
        .with_cooldown_store(backend);
        restarted.reflect("agent-1", "sess-1").await.unwrap();
        assert_eq!(
            llm.call_count(),
            1,
            "persisted cooldown must survive a restart"
        );
        assert_eq!(reflection_count(&store, "agent-1").await, 1);
    }

    #[test]
    fn prompt_includes_conversation_and_none_sentinel() {
        let msgs = vec![
            ("user".to_string(), "hello there".to_string()),
            ("assistant".to_string(), "hi".to_string()),
        ];
        let p = build_reflection_prompt(&msgs, false);
        assert!(
            p.contains("hello there"),
            "prompt must embed the transcript"
        );
        assert!(p.contains("LESSONS"), "prompt must ask for lessons");
        assert!(
            p.contains(NO_LESSON_SENTINEL),
            "prompt must offer the NONE escape hatch"
        );
        // Off mode must NOT ask for the two-section open-loops format.
        assert!(!p.contains(OPEN_LOOPS_HEADER));
    }

    #[test]
    fn prompt_carries_durability_rules() {
        let msgs = vec![("user".to_string(), "please stop paraphrasing".to_string())];
        let p = build_reflection_prompt(&msgs, false);
        assert!(
            p.contains("evidence → implication"),
            "must ask for evidence → implication phrasing"
        );
        assert!(
            p.contains("verbatim and greppable"),
            "must demand verbatim greppable identifiers"
        );
        assert!(
            p.contains("absolute dates"),
            "must demand absolute, not relative, dates"
        );
        assert!(
            p.contains("transient failures"),
            "anti-rot: transient failures must not fossilize"
        );
        assert!(
            p.contains("remedy or workaround"),
            "anti-rot: store the remedy, not the failure narrative"
        );
        assert!(
            p.contains("act better for having"),
            "must carry the future-usefulness gate"
        );
        // Today's date is embedded so the model can absolutize relatives.
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert!(p.contains(&today), "prompt must state today's date");
    }

    #[test]
    fn tracking_prompt_requests_open_loops_section() {
        let msgs = vec![("user".to_string(), "is the deploy done?".to_string())];
        let p = build_reflection_prompt(&msgs, true);
        assert!(p.contains(LESSONS_HEADER), "must keep the lessons header");
        assert!(
            p.contains(OPEN_LOOPS_HEADER),
            "tracking mode must request the open-loops section"
        );
        assert!(
            p.contains("OPEN LOOPS"),
            "must explain what an open loop is"
        );
    }

    #[test]
    fn split_sections_separates_lessons_and_loops() {
        let out = "LESSONS:\n- I should rebuild wasm first\n\nOPEN_LOOPS:\n- Follow up on the deploy status";
        let (lessons, loops) = split_sections(out);
        assert!(lessons.contains("rebuild wasm"));
        assert!(
            !lessons.contains("Follow up"),
            "loops must not leak into lessons"
        );
        assert!(loops.contains("Follow up on the deploy"));
    }

    #[test]
    fn split_sections_handles_none_in_either_section() {
        let (lessons, loops) = split_sections("LESSONS:\nNONE\n\nOPEN_LOOPS:\n- chase the build");
        assert!(lessons.is_empty(), "NONE lessons collapse to empty");
        assert!(loops.contains("chase the build"));

        let (lessons, loops) = split_sections("LESSONS:\n- a real lesson\n\nOPEN_LOOPS:\nNONE");
        assert!(lessons.contains("a real lesson"));
        assert!(loops.is_empty(), "NONE loops collapse to empty");
    }

    #[test]
    fn split_sections_without_marker_is_all_lessons() {
        let (lessons, loops) = split_sections("- just a lesson, model omitted headers");
        assert!(lessons.contains("just a lesson"));
        assert!(loops.is_empty());
    }

    #[test]
    fn strip_header_is_utf8_safe_with_cjk_prefix() {
        // A CJK-leading section shorter in chars than the header byte-len must
        // not panic (P7) and must be returned unchanged (no header present).
        let s = strip_header("待办：跟进部署", OPEN_LOOPS_HEADER);
        assert!(s.contains("跟进部署"));
    }

    #[test]
    fn normalize_section_collapses_none_and_empty() {
        assert_eq!(normalize_section("  none  "), "");
        assert_eq!(normalize_section("NONE"), "");
        assert_eq!(normalize_section("   "), "");
        assert_eq!(normalize_section("- real"), "- real");
    }
}
