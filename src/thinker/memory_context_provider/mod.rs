//! Async memory context provider — fetches relevant memories before prompt assembly.

use crate::config::types::memory::MemoryInjectionMode;
use crate::memory::assembler::hybrid::LlmReranker;
use crate::memory::assembler::WorkingMemoryAssembler;
use crate::memory::curated::{CuratedConfig, CuratedMemoryStore, CuratedSnapshot};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock as TokioRwLock;

/// Configuration for memory context retrieval.
pub struct MemoryContextConfig {
    /// Maximum number of facts to retrieve.
    pub max_facts: usize,
    /// Minimum cosine similarity threshold.
    pub similarity_threshold: f32,
    /// Maximum characters for the formatted output.
    pub max_output_chars: usize,
}

impl Default for MemoryContextConfig {
    fn default() -> Self {
        Self {
            max_facts: 5,
            similarity_threshold: 0.3,
            max_output_chars: 8000, // ~2000 tokens
        }
    }
}

/// No-op reranker used when no [`AiProvider`] is supplied. Always errors →
/// `HybridAssembler` transparently falls back to the deterministic skeleton.
struct NoopReranker;

#[async_trait]
impl LlmReranker for NoopReranker {
    async fn complete(
        &self,
        _prompt: &str,
        _model: Option<&str>,
    ) -> Result<String, crate::error::AlephError> {
        Err(crate::error::AlephError::config(
            "NoopReranker: no AiProvider configured".to_string(),
        ))
    }
}

/// Cache key shared by both frozen maps: `(agent_id, session_key)`.
type SnapshotKey = (String, String);

/// A frozen cache value plus the instant it was frozen. The timestamp exists
/// solely so [`freeze_into`] can reap by age — it is the only reader, which is
/// why it lives here on the cache entry rather than on `CuratedSnapshot`.
/// `pub(crate)` only to match the visibility of the `MemoryContextProvider`
/// fields typed on it — a private type behind a `pub(crate)` field is a
/// `private_interfaces` warning, not a tighter bound.
pub(crate) struct FrozenEntry<T> {
    value: T,
    frozen: Instant,
}

/// Hard cap on entries per frozen map. A live session holds exactly one, so
/// anything approaching this is sessions that never closed; the oldest is
/// evicted and its next prompt build simply re-captures.
const MAX_FROZEN_SNAPSHOTS: usize = 256;

/// Belt-and-suspenders reap age. `invalidate_curated` at session end is the
/// primary cleanup; this sweeps peers that never start a new session (a channel
/// DM may never emit `SessionEnd`), whose entry would otherwise stay resident
/// for the whole process lifetime — each pinning a rendered MEMORY.md +
/// USER.md + OPEN_LOOPS or an orientation envelope. Deliberately long: evicting
/// a still-live session costs a disk re-read *and* re-keys the provider
/// prompt-cache prefix this cache exists to keep byte-stable.
const FROZEN_SNAPSHOT_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// Freeze `value` under `key` and return whatever is cached there afterwards.
///
/// The whole get-or-insert happens inside the caller's single write guard, so
/// "first write wins / frozen for the session" actually holds under
/// concurrency: a racing capture that lost never overwrites the winner, and
/// both callers go on to render the same bytes. The capture itself stays
/// *outside* the guard on purpose — a cold disk read for one agent must not
/// stall every other session's prompt build.
///
/// The single growth point is also the hygiene point (mirroring
/// `gateway::voice::streaming::relay::StreamRegistry`): expired entries are
/// reaped and the oldest evicted while at capacity, before the insert.
fn freeze_into<T: Clone>(
    map: &mut HashMap<SnapshotKey, FrozenEntry<T>>,
    key: SnapshotKey,
    value: T,
) -> T {
    map.retain(|_, e| e.frozen.elapsed() < FROZEN_SNAPSHOT_TTL);
    while map.len() >= MAX_FROZEN_SNAPSHOTS {
        let oldest = map
            .iter()
            .min_by_key(|(_, e)| e.frozen)
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                tracing::warn!(
                    agent_id = %k.0,
                    session_key = %k.1,
                    "frozen memory-snapshot cache at capacity — evicting oldest"
                );
                map.remove(&k);
            }
            None => break,
        }
    }
    map.entry(key)
        .or_insert(FrozenEntry {
            value,
            frozen: Instant::now(),
        })
        .value
        .clone()
}

/// Per-(agent_id, `session_key`) curated snapshot cache. Frozen until
/// invalidation; see [`MemoryContextProvider::build_curated_message`].
type CuratedSnapshotCache =
    Arc<TokioRwLock<HashMap<SnapshotKey, FrozenEntry<Arc<CuratedSnapshot>>>>>;

/// Per-(agent_id, `session_key`) frozen orientation envelope. `None` values
/// are cached too: a notes-less agent resolves to "no envelope" once per
/// session instead of re-reading disk every prompt build. Same invalidation
/// points as [`CuratedSnapshotCache`]; see
/// [`MemoryContextProvider::build_orientation_message_cached`].
type OrientationSnapshotCache = Arc<TokioRwLock<HashMap<SnapshotKey, FrozenEntry<Option<String>>>>>;

/// Provides pre-fetched memory context for prompt injection.
pub struct MemoryContextProvider {
    pub(crate) assembler: Arc<dyn WorkingMemoryAssembler>,
    pub(crate) config: MemoryContextConfig,
    /// Controls whether memory is auto-injected (Context/Hybrid) or gated behind tools (Tools).
    pub(crate) injection_mode: MemoryInjectionMode,
    /// Plugin-contributed enhancements to the retrieved envelope.
    /// Default-empty registry means no plugins registered = no-op.
    pub(crate) extensions:
        crate::sync_primitives::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    /// Optional wiki orientation provider for injecting structural context.
    pub(crate) orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    /// Token budget for orientation snapshots.
    pub(crate) orientation_budget: crate::memory::notes::orientation::types::TokenBudget,
    /// Optional user-profile synthesizer for injecting profile context.
    pub(crate) profile: Option<Arc<dyn crate::memory::notes::profile::ProfileSynthesizer>>,
    /// Per-(agent_id, `session_key`) frozen snapshot. Built on first prompt
    /// build for the session; reused until evicted by compression / `SessionEnd`,
    /// or by the TTL / capacity sweep in [`freeze_into`] for sessions that never
    /// close.
    pub(crate) curated_snapshots: CuratedSnapshotCache,
    /// Per-(agent_id, `session_key`) frozen orientation envelope. Orientation
    /// lands in the Stable curated zone of the system prompt, so re-reading it
    /// from disk each build would churn the provider prompt-cache prefix
    /// whenever the wiki mutated mid-session. Frozen on first build; evicted at
    /// the same points as `curated_snapshots` (session end / post-compression).
    pub(crate) orientation_snapshots: OrientationSnapshotCache,
    /// Per-agent `CuratedMemoryStore`. Loaded lazily on first capture.
    pub(crate) curated_stores: Arc<DashMap<String, Arc<CuratedMemoryStore>>>,
    /// Char-budget config for both MEMORY.md and USER.md rendering.
    pub(crate) curated_config: CuratedConfig,
    /// Test-only override for the curated MEMORY.md root directory.
    /// Real path: `~/.aleph/agents/<agent_id>/MEMORY.md`. Tests redirect
    /// to a tempdir to keep filesystem state isolated.
    #[cfg(test)]
    pub(crate) curated_root_override: Option<std::path::PathBuf>,
}

mod constructor;
mod curated;
mod helpers;
mod memory;
mod orientation;

pub use helpers::*;

#[cfg(test)]
mod tests;
