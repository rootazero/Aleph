pub fn default_enabled() -> bool {
    true
}

pub fn default_max_context_items() -> u32 {
    5
}

pub fn default_retention_days() -> u32 {
    90
}

pub fn default_vector_db() -> String {
    "sqlite-vec".to_string()
}

pub fn default_similarity_threshold() -> f32 {
    0.3
}

pub fn default_compression_enabled() -> bool {
    true
}

pub fn default_compression_idle_timeout() -> u32 {
    300
}

pub fn default_compression_turn_threshold() -> u32 {
    20
}

pub fn default_compression_interval() -> u32 {
    3600
}

pub fn default_compression_batch_size() -> u32 {
    50
}

pub fn default_max_facts_in_context() -> u32 {
    5
}

pub fn default_raw_memory_fallback_count() -> u32 {
    3
}

pub fn default_embedding_timeout_ms() -> u64 {
    10000
}

pub fn default_embedding_batch_size() -> u32 {
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
pub fn default_embedding_max_input_chars() -> usize {
    24000
}

pub fn default_embedding_providers() -> Vec<super::embed::EmbeddingProviderConfig> {
    Vec::new()
}

pub fn default_active_provider_id() -> String {
    String::new()
}

pub fn default_dreaming_enabled() -> bool {
    true
}

pub fn default_dreaming_idle_threshold_seconds() -> u32 {
    900
}

pub fn default_dreaming_window_start() -> String {
    "02:00".to_string()
}

pub fn default_dreaming_window_end() -> String {
    "05:00".to_string()
}

pub fn default_dreaming_max_duration_seconds() -> u32 {
    600
}

pub fn default_weekly_enabled() -> bool {
    true
}

pub fn default_weekly_interval_days() -> u32 {
    7
}

pub fn default_drift_max_pairs_per_run() -> usize {
    20
}

pub fn default_synthesis_min_cluster_size() -> usize {
    3
}

pub fn default_synthesis_max_insights() -> usize {
    10
}

pub fn default_skill_distill_max_per_cycle() -> usize {
    3
}

pub fn default_feedback_distill_max_per_cycle() -> usize {
    3
}

pub fn default_feedback_distill_min_candidates() -> usize {
    3
}

pub fn default_feedback_lookback() -> usize {
    50
}

pub fn default_memory_decay_half_life_days() -> f32 {
    30.0
}

pub fn default_memory_decay_access_boost() -> f32 {
    0.2
}

pub fn default_memory_decay_min_strength() -> f32 {
    0.1
}

pub fn default_memory_decay_protected_types() -> Vec<String> {
    vec!["personal".to_string()]
}

pub fn default_reflection_min_turns() -> u32 {
    5
}

pub fn default_reflection_min_chars() -> u32 {
    200
}

pub fn default_reflection_cooldown() -> u32 {
    30
}

pub fn default_assembler_enabled() -> bool {
    true
}

pub fn default_total_budget() -> u32 {
    8000
}

pub fn default_pool_limit() -> usize {
    20
}

pub fn default_rerank_timeout() -> u64 {
    800
}

pub fn default_user_profile_tokens() -> u32 {
    200
}

pub fn default_session_recent_tokens() -> u32 {
    1500
}

pub fn default_relevant_notes_tokens() -> u32 {
    5000
}

pub fn default_raw_fragments_tokens() -> u32 {
    1000
}

pub fn default_assembly_retention_days() -> u32 {
    14
}

pub fn default_rrf_k() -> u32 {
    60
}

pub fn default_bm25_bonus() -> f32 {
    0.15
}

pub fn default_dedup_similarity_threshold() -> f32 {
    0.95
}

pub fn default_backup_enabled() -> bool {
    true
}

pub fn default_backup_max_files() -> usize {
    7
}

pub fn default_qf_enabled() -> bool {
    true
}

pub fn default_qf_min_sources() -> usize {
    3
}

pub fn default_qf_min_answer_chars() -> usize {
    200
}

pub fn default_qf_llm_gate_enabled() -> bool {
    true
}

pub fn default_compound_enabled() -> bool {
    true
}

pub fn default_max_related_pages() -> usize {
    15
}

pub fn default_related_preview_char_cap() -> usize {
    800
}

pub fn default_related_total_byte_cap() -> usize {
    12 * 1024
}

pub fn default_replan_on_hash_conflict() -> u32 {
    1
}

pub fn default_failure_cooldown_seconds() -> u64 {
    300
}

pub fn default_tx_residue_gc_seconds() -> u64 {
    3600
}

pub fn default_orientation_enabled() -> bool {
    true
}

pub fn default_orientation_max_tokens() -> usize {
    4000
}

pub fn default_orientation_log_rotate_lines() -> usize {
    2000
}

pub fn default_orientation_inject_on_agent_switch() -> bool {
    true
}

pub fn default_profile_enabled() -> bool {
    true
}

pub fn default_profile_min_interval_minutes() -> u32 {
    30
}

pub fn default_profile_inject_interval_turns() -> u32 {
    10
}

pub fn default_profile_max_body_bytes() -> usize {
    2048
}

pub fn default_profile_max_bullets() -> usize {
    20
}

pub fn default_profile_bootstrap_on_first() -> bool {
    true
}
