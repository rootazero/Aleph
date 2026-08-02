pub const fn default_enabled() -> bool {
    true
}

pub fn default_vector_db() -> String {
    "sqlite-vec".to_string()
}

pub fn default_similarity_threshold() -> f32 {
    crate::config::defaults_override::get_defaults_override()
        .memory_similarity_threshold()
        .unwrap_or(0.3)
}

pub const fn default_embedding_timeout_ms() -> u64 {
    10000
}

pub const fn default_embedding_batch_size() -> u32 {
    32
}

/// Conservative per-input character ceiling before the embedding API call.
///
/// Reasoning: most embedding providers cap input at 8192 tokens. With BPE
/// tokenization English averages ~4 chars/token, CJK ~1.5 chars/token. A
/// 24000-char ceiling keeps English inputs well under the limit while
/// allowing roughly 16000 Chinese characters (still under 8192 tokens).
/// Texts exceeding this are truncated UTF-8-safely before the API call
/// instead of failing the whole batch.
pub const fn default_embedding_max_input_chars() -> usize {
    24000
}

pub const fn default_embedding_providers() -> Vec<super::embed::EmbeddingProviderConfig> {
    Vec::new()
}

pub const fn default_active_provider_id() -> String {
    String::new()
}

pub const fn default_dreaming_enabled() -> bool {
    true
}

pub const fn default_dreaming_idle_threshold_seconds() -> u32 {
    900
}

pub fn default_dreaming_window_start() -> String {
    "02:00".to_string()
}

pub fn default_dreaming_window_end() -> String {
    "05:00".to_string()
}

pub const fn default_dreaming_max_duration_seconds() -> u32 {
    600
}

pub const fn default_drift_max_pairs_per_run() -> usize {
    20
}

pub const fn default_synthesis_min_cluster_size() -> usize {
    3
}

pub const fn default_synthesis_max_insights() -> usize {
    10
}

pub const fn default_skill_distill_max_per_cycle() -> usize {
    3
}

/// Days of inactivity before a skill auto-transitions from
/// `SkillState::Active` to `SkillState::Stale`. Mirrors hermes-agent's
/// `DEFAULT_STALE_AFTER_DAYS = 30` in `agent/curator.py`.
pub const fn default_skill_stale_after_days() -> u32 {
    30
}

pub const fn default_feedback_distill_max_per_cycle() -> usize {
    3
}

pub const fn default_feedback_distill_min_candidates() -> usize {
    3
}

pub const fn default_feedback_lookback() -> usize {
    50
}

pub const fn default_memory_decay_half_life_days() -> f32 {
    // Matches the value `NoteDecayStage` used as a hard-coded constant before
    // this policy was wired in, so activating the config keeps the live decay
    // curve identical for users who never set it (confidence ×e⁻¹≈0.368 after
    // 90 days with no recall).
    90.0
}

pub const fn default_memory_decay_min_strength() -> f32 {
    0.1
}

pub fn default_memory_decay_protected_types() -> Vec<String> {
    vec!["personal".to_string()]
}

pub const fn default_reflection_min_turns() -> u32 {
    5
}

pub const fn default_reflection_min_chars() -> u32 {
    200
}

pub const fn default_reflection_cooldown() -> u32 {
    30
}

pub const fn default_assembler_enabled() -> bool {
    true
}

pub const fn default_pool_limit() -> usize {
    20
}

pub const fn default_rerank_timeout() -> u64 {
    800
}

pub const fn default_user_profile_tokens() -> u32 {
    200
}

pub const fn default_session_recent_tokens() -> u32 {
    1500
}

pub const fn default_relevant_notes_tokens() -> u32 {
    5000
}

pub const fn default_raw_fragments_tokens() -> u32 {
    1000
}

/// Token budget for the `Feedback` slot — user-taught rules/corrections.
/// Small on purpose: these notes are short imperative rules, not prose.
pub const fn default_feedback_tokens() -> u32 {
    500
}

pub const fn default_assembly_retention_days() -> u32 {
    14
}

pub const fn default_rrf_k() -> u32 {
    60
}

pub const fn default_bm25_bonus() -> f32 {
    0.15
}

pub const fn default_qf_enabled() -> bool {
    true
}

pub const fn default_qf_min_sources() -> usize {
    3
}

pub const fn default_qf_min_answer_chars() -> usize {
    200
}

pub const fn default_qf_llm_gate_enabled() -> bool {
    true
}

pub const fn default_compound_enabled() -> bool {
    true
}

pub const fn default_max_related_pages() -> usize {
    15
}

pub const fn default_related_preview_char_cap() -> usize {
    800
}

pub const fn default_related_total_byte_cap() -> usize {
    12 * 1024
}

/// Write-time semantic dedup gate (mem0-style). Enabled by default so
/// near-duplicate notes are collapsed at ingest instead of waiting for the
/// offline dream consolidator. Operators can set `dedup_enabled = false` in
/// `[memory.compound_ingest]` to restore byte-identical ingest.
pub const fn default_dedup_enabled() -> bool {
    true
}

/// Cosine-similarity threshold above which a freshly-planned `Create` is
/// redirected into an `Append` onto the nearest existing note. `0.92` is
/// deliberately conservative: only near-identical notes collapse, leaving
/// genuinely distinct facts to spawn their own page.
pub const fn default_dedup_similarity_threshold() -> f32 {
    0.92
}

/// Cosine threshold at or above which a freshly-planned `Create` is treated as
/// a NOOP and dropped (the note already exists, essentially verbatim). Must be
/// \>= `default_dedup_similarity_threshold`. `0.985` is deliberately very high:
/// only near-identical title+summary+facts collapse to a no-op, so a Create
/// carrying genuinely new facts (which embeds below this) is still merged via
/// Append rather than dropped.
pub const fn default_dedup_noop_threshold() -> f32 {
    0.985
}

/// Admission governance gate. **Off by default**: the gate + review-queue path
/// (Phase C2) ships wired to config but dormant, so existing deployments keep
/// byte-identical immediate-admit ingest until an operator sets
/// `governance_enabled = true` in `[memory.compound_ingest]`.
pub const fn default_governance_enabled() -> bool {
    false
}

pub const fn default_replan_on_hash_conflict() -> u32 {
    1
}

pub const fn default_failure_cooldown_seconds() -> u64 {
    300
}

pub const fn default_tx_residue_gc_seconds() -> u64 {
    3600
}

pub const fn default_orientation_enabled() -> bool {
    true
}

pub const fn default_orientation_max_tokens() -> usize {
    4000
}

pub const fn default_profile_enabled() -> bool {
    true
}

pub const fn default_profile_min_interval_minutes() -> u32 {
    30
}

pub const fn default_profile_max_bullets() -> usize {
    20
}

pub const fn default_profile_bootstrap_on_first() -> bool {
    true
}
