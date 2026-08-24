//! `ToolResultStore` — disk persistence for large tool results.
//!
//! When a tool result exceeds the token threshold, it is written to a
//! session-scoped directory on disk. A compact reference marker is injected
//! into the context window so the LLM can identify that the full output
//! exists but was offloaded.
//!
//! # Shared storage, session-scoped handles
//!
//! The physical storage (blob root + FTS5 `index.db`) is process-wide: boot
//! installs exactly one, and both the gateway `ScopedToolService` seam and the
//! orchestrator `HarnessDeps` seam read it back from a `CapabilitySlot`. The *handle*
//! is what carries the session scope — [`ToolResultStore::for_session`] clones
//! the shared inner and stamps a session key onto it, so blobs land in a
//! per-session subdirectory and index rows carry a `session_id` the reads and
//! the purge filter on.
//!
//! This split exists because the alternative — threading a session id through
//! `persist_if_large` — would have to cross `harness/agent/act.rs`, and that
//! tree is over its R10 line budget. Baking the scope into the handle keeps the
//! call sites byte-identical.
//!
//! Without the scope, two live bugs: `ctx_search` in one session retrieved
//! another agent's tool output (violating INV-ISO), and [`purge_all`] — fired
//! by the denial circuit-breaker after three refusals in *one* session — wiped
//! the blobs and index of every concurrent session, leaving their
//! `[Full output persisted: …]` markers pointing at deleted files.
//!
//! [`purge_all`]: ToolResultStore::purge_all

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use std::time::{Duration, SystemTime};

use crate::sync_primitives::Arc;

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::retrieval::{ContentIndex, IndexOutcome, SearchHit};

/// Prefix used to identify persisted-result reference lines.
const PERSISTED_REF_PREFIX: &str = "[Full output persisted: ";

/// Default retention window for the periodic sweeper. Mirrors opencode's
/// `Truncate.cleanup` cutoff (7 days). Persisted tool-result files older
/// than this are garbage-collected by [`sweep_stale_tool_result_dirs`].
pub const DEFAULT_TOOL_RESULT_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Default sweep cadence for the background TTL task. Matches opencode's
/// hourly cadence — a directory walk over `tool_results/` is cheap and the
/// task does not need to wake more often than that to bound disk usage.
pub const DEFAULT_TOOL_RESULT_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

// =============================================================================
// Process-wide installer
// =============================================================================
//
// Layer 2 / Layer 3 of the tool-result budget both need access to a shared
// `Arc<ToolResultStore>` so the marker path the LLM sees matches a real file
// on disk. Rather than thread the `Arc` through every constructor that builds
// a `ScopedToolService` (gateway path) or an `AgentHarnessRunner` (orchestrator
// path), we install one at server boot via `set_global_tool_result_store` and
// read it back through `global_tool_result_store`. The same singleton is
// consumed by:
//   * `gateway::execution_engine::tool_service_builder::build_request_tool_service`
//   * `orchestrator::harness_bridge::AgentHarnessRunner` (HarnessDeps wiring)
// Tests or alternative bootstraps that prefer per-instance injection can still
// use `ScopedToolService::with_result_store` / `HarnessDeps.result_store`
// directly; the global slot is `Option`-shaped and a `None` value means
// "fall back to in-line truncation only".
/// `FailsClosed`, and this is the one handle in this batch where the five
/// production readers genuinely agree — checked rather than assumed:
/// `ctx_search` returns an empty hit list, `browser_tools::offload_full_content`
/// returns `None` so its caller says so instead of naming a file that does not
/// exist, `tool_service_builder` and `subagent_spawner` leave the scoped
/// service without a store, and `sandbox/workspace`'s denial circuit-breaker
/// skips a `purge_all` that has nothing to purge. Every one of those is "the
/// offload feature is off, fall back to in-line truncation" — the meaning this
/// module's own comment above already assigns to `None`. Nothing unsafe
/// happens; a capability is simply dead and silent.
///
/// ⚠️ One arm is worth a second look in Task 15 and is NOT what picked this
/// variant: `ctx_search`'s note reads "No offloaded tool output is indexed in
/// this session **yet**", which describes an empty session rather than an
/// absent store. Same disposition, wrong sentence.
static GLOBAL_STORE: CapabilitySlot<Arc<ToolResultStore>> =
    CapabilitySlot::new("tools/result-store", MissingSemantics::FailsClosed);

/// Install the process-wide `ToolResultStore`. Idempotent — subsequent calls
/// are silently ignored so multiple boot paths cannot stomp each other.
///
/// First-call side effect: spawns the periodic TTL sweeper
/// ([`spawn_periodic_sweeper`]) over the store's blob root. The sweeper
/// opportunistically reclaims stale per-session dirs left behind by hard
/// crashes (where `Drop` cleanup never ran) and by previous runs of the
/// process, and clears the removed sessions' index rows. Mirrors opencode's
/// `Truncate` background cleanup loop.
///
/// The sweeper is intentionally fire-and-forget: it has no shutdown handle
/// and is dropped only when the process exits. Test boot paths that want
/// deterministic GC should call [`ToolResultStore::sweep_stale`] directly.
pub fn set_global_tool_result_store(store: Arc<ToolResultStore>) {
    // `install` returns false if a prior install won — in that case we
    // intentionally do nothing (the existing sweeper is fine).
    let installed_now = GLOBAL_STORE.install(store.clone());
    if installed_now {
        // Sweep the blob root itself: per-session dirs are its *children*
        // (`blob_dir()` = `base_dir/<sanitized session>`). Rooting the sweep
        // one level up (at `tool_results/`) made the live root the sweep's
        // only entry — session dirs were never GC'd individually, and once
        // every file under the root aged past the TTL the sweep deleted the
        // root itself, `index.db` included, out from under the open index.
        spawn_periodic_sweeper(
            store,
            DEFAULT_TOOL_RESULT_RETENTION,
            DEFAULT_TOOL_RESULT_SWEEP_INTERVAL,
        );
    }
}

/// Read the process-wide `ToolResultStore`, if installed.
///
/// ⚠️ `None` says nothing about whether boot reached this slot. Ask
/// [`global_tool_result_store_slot`]`().outcome()` for that.
#[inline]
pub fn global_tool_result_store() -> Option<Arc<ToolResultStore>> {
    GLOBAL_STORE.get().cloned()
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn global_tool_result_store_slot() -> &'static dyn SlotStatus {
    &GLOBAL_STORE
}

// =============================================================================
// ToolResultStore
// =============================================================================

/// Filename of the FTS5 retrieval index inside the shared store root. One DB
/// for the whole process; rows are separated by their `session_id` column.
const INDEX_DB_NAME: &str = "index.db";

/// The physical storage every handle shares: the blob root and the lazily
/// opened FTS5 index that lives inside it. Held behind an `Arc` so a
/// session-scoped handle costs a pointer clone, not a second `index.db`.
struct StoreInner {
    base_dir: PathBuf,
    /// Lazily-opened retrieval index. `None` inside the `OnceLock` means an
    /// open attempt failed once and indexing/search degrade to no-ops.
    index: OnceLock<Option<ContentIndex>>,
}

impl Drop for StoreInner {
    fn drop(&mut self) {
        // Cleanup hangs off the *inner*, not the handle: a session-scoped
        // handle going out of scope must not delete the root every other live
        // session is still writing into.
        if self.base_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.base_dir) {
                tracing::warn!(
                    dir = %self.base_dir.display(),
                    error = %e,
                    "failed to clean up tool result store"
                );
            }
        }
    }
}

/// Store that offloads large tool outputs to disk, scoped to one session.
///
/// The storage is shared (see the module docs); the *scope* lives on the
/// handle. An empty scope is the legacy unscoped root — reachable only from
/// bootstraps that have no session context (tests, and the boot-time handle
/// itself before [`Self::for_session`] narrows it). It is a scope like any
/// other, not a wildcard: an unscoped handle cannot see or purge a scoped
/// session's output, so "forgot to scope it" degrades to "sees nothing" rather
/// than to a cross-session leak.
///
/// Alongside the raw `.txt` blobs, the store lazily maintains an FTS5
/// [`ContentIndex`] (`index.db`) so the model can BM25-search offloaded
/// output via `ctx_search` instead of re-reading whole files. The index is
/// opened on first use and shares the directory's Drop / TTL-sweep lifecycle.
pub struct ToolResultStore {
    inner: Arc<StoreInner>,
    /// Session key this handle reads, writes and purges under. Empty = the
    /// unscoped root scope (see above). Must be byte-equal to
    /// [`crate::tools::turn_context::current_session_key`] for the read path
    /// (`ctx_search`) to find what the write path stored.
    session: String,
}

impl ToolResultStore {
    /// Create the process-wide store rooted at
    /// `<config_dir>/data/tool_results/{root}/`.
    ///
    /// `root` names the *directory*, not a session: the returned handle is
    /// unscoped. Boot installs it as the shared singleton and the two seams
    /// that own session context ([`crate::gateway::execution_engine`]'s tool
    /// service builder and the orchestrator's harness bridge) narrow it with
    /// [`Self::for_session`].
    ///
    /// The root comes from [`crate::utils::paths::get_data_dir`], which is the
    /// single place `ALEPH_HOME` is honored. Re-deriving it from
    /// `dirs::home_dir()` — as this did until 2026-08-04 — put every offloaded
    /// tool result in the *real* `~/.aleph` no matter which home the operator
    /// selected, so two instances with different `ALEPH_HOME`s shared one
    /// `tool_results/` tree and the isolation knob silently did not cover the
    /// one artifact on this path that outlives the run.
    pub fn new(root: &str) -> std::io::Result<Self> {
        let base_dir = crate::utils::paths::get_data_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp").join(".aleph").join("data"))
            .join("tool_results")
            .join(root);

        create_private_dir(&base_dir)?;
        Ok(Self {
            inner: Arc::new(StoreInner {
                base_dir,
                index: OnceLock::new(),
            }),
            session: String::new(),
        })
    }

    /// Narrow a store handle to one session: same storage, session-scoped
    /// reads, writes and purges.
    ///
    /// `session` must be the wire session key (`SessionKey::to_key_string`),
    /// because `ctx_search` resolves its own scope from
    /// [`crate::tools::turn_context::current_session_key`]. A mismatch here is
    /// silent — the writes land under one key and the searches look under
    /// another, returning zero hits forever.
    #[must_use]
    pub fn for_session(store: &Arc<Self>, session: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::clone(&store.inner),
            session: session.into(),
        })
    }

    /// Construct a store rooted at an arbitrary base directory. The
    /// caller must create the directory before this returns. Exposed
    /// for tests in adjacent modules (e.g. `result_processing`,
    /// `scoped`) that need a `ToolResultStore` without touching
    /// `~/.aleph/`.
    #[doc(hidden)]
    #[must_use]
    pub fn with_dir_for_tests(base_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                base_dir,
                index: OnceLock::new(),
            }),
            session: String::new(),
        }
    }

    /// Directory this handle's blobs live in: the root for the unscoped scope,
    /// a per-session subdirectory otherwise. Keeping each session's blobs in
    /// their own directory is what lets [`Self::purge_all`] wipe one session
    /// without touching another's files.
    fn blob_dir(&self) -> PathBuf {
        self.blob_dir_for(&self.session)
    }

    /// Blob directory for an arbitrary session key under this store's root.
    /// Shared by [`Self::blob_dir`] and [`Self::purge_all`], which must reach
    /// earlier-epoch directories the read scope widens to.
    fn blob_dir_for(&self, key: &str) -> PathBuf {
        if key.is_empty() {
            self.inner.base_dir.clone()
        } else {
            self.inner.base_dir.join(sanitize_for_filename(key))
        }
    }

    /// Persist the content to disk if it exceeds `threshold_tokens`.
    ///
    /// Returns a reference marker string if the content was persisted, or
    /// `None` if the content is small enough to remain in the context window.
    pub fn persist_if_large(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        content: &str,
        threshold_tokens: usize,
    ) -> Option<String> {
        let tokens = estimate_tokens_smart(content);
        if tokens <= threshold_tokens {
            return None;
        }

        // Use a sanitized filename: {tool_call_id}_{tool_name}.txt
        let safe_name = format!(
            "{}_{}.txt",
            sanitize_for_filename(tool_call_id),
            sanitize_for_filename(tool_name)
        );
        let dir = self.blob_dir();
        if let Err(e) = create_private_dir(&dir) {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "failed to create session blob dir for tool result"
            );
            return None;
        }
        let path = dir.join(&safe_name);

        if let Err(e) = write_private_file(&path, content) {
            tracing::warn!(
                tool_call_id = tool_call_id,
                tool_name = tool_name,
                error = %e,
                "failed to persist tool result to disk"
            );
            return None;
        }

        let marker = format!(
            "{}{} ({} tokens, {})]",
            PERSISTED_REF_PREFIX,
            path.display(),
            tokens,
            tool_name,
        );
        Some(marker)
    }

    /// Lazily open (once) the FTS5 retrieval index in the shared `base_dir`.
    /// Returns `None` if the index could not be opened — callers degrade to
    /// no-ops. The DB is shared across sessions; the scoping happens in the
    /// queries below, not in the file layout.
    fn index(&self) -> Option<&ContentIndex> {
        self.inner
            .index
            .get_or_init(|| {
                let db_path = self.inner.base_dir.join(INDEX_DB_NAME);
                match ContentIndex::open(&db_path) {
                    Ok(idx) => Some(idx),
                    Err(e) => {
                        tracing::warn!(
                            db = %db_path.display(),
                            error = %e,
                            "failed to open tool-result content index; ctx_search disabled"
                        );
                        None
                    }
                }
            })
            .as_ref()
    }

    /// Index an offloaded tool output into the retrieval store so the model
    /// can BM25-search it via `ctx_search`. Best-effort: returns `None` (and
    /// logs) on any failure, and `Some(outcome)` with the section count on
    /// success. Callers use the outcome to build the model-facing hint.
    pub fn index_output(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        content: &str,
    ) -> Option<IndexOutcome> {
        let idx = self.index()?;
        // Label the rows `{tool_name}:{short_id}` rather than the bare call
        // id: a hit's `source` is surfaced to the model as "the tool call the
        // section came from", and a naked UUID says nothing about which tool
        // produced it. The id suffix keeps the label unique, so re-indexing
        // the same call still replaces its own chunks (the `(source,
        // chunk_no)` identity) instead of colliding with another call of the
        // same tool.
        let source = format!("{tool_name}:{}", short_call_id(tool_call_id));
        match idx.index_text(&self.session, &source, tool_name, content) {
            Ok(out) => Some(out),
            Err(e) => {
                tracing::warn!(
                    tool_call_id,
                    tool_name,
                    error = %e,
                    "failed to index tool output for retrieval"
                );
                None
            }
        }
    }

    /// Session keys this handle reads under: its own key plus every earlier
    /// epoch of the same base session key. A compaction-driven session split
    /// (`epoch + 1`) seeds the child with the parent's `[Full output
    /// persisted: …]` markers verbatim while the index rows stay keyed to the
    /// parent epoch — without the widened read scope, `ctx_search` in the
    /// child finds nothing for exactly the content its in-context markers
    /// advertise. Epochs of one base key are one trust domain (same agent,
    /// same conversation), so the widening cannot cross INV-ISO; writes and
    /// purges stay exact-key. Non-epoch and unparseable keys degrade to the
    /// single exact key.
    fn read_scope_keys(&self) -> Vec<String> {
        let mut keys = vec![self.session.clone()];
        if let Some(parsed) = crate::routing::session_key::SessionKey::parse(&self.session) {
            for epoch in 0..parsed.epoch() {
                let key = parsed.with_epoch(epoch).to_key_string();
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    /// BM25-search tool output previously offloaded **by this session** —
    /// including earlier epochs of the same base session key (see
    /// [`Self::read_scope_keys`]). Returns up to `limit` hits, most relevant
    /// first. Empty when nothing is indexed or the index is unavailable —
    /// never errors out to the caller.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let Some(idx) = self.index() else {
            return Vec::new();
        };
        let keys = self.read_scope_keys();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        idx.search_sessions(&key_refs, query, limit)
            .unwrap_or_default()
    }

    /// Number of indexed sections readable by this handle — its own session
    /// plus earlier epochs of the same base key (see
    /// [`Self::read_scope_keys`]). `0` when the index is empty or unavailable.
    pub fn indexed_sections(&self) -> usize {
        let Some(idx) = self.index() else {
            return 0;
        };
        let keys = self.read_scope_keys();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        idx.len_sessions(&key_refs).unwrap_or(0)
    }

    /// Remove the shared base directory and all its contents — *every*
    /// session's blobs plus the index. Process-teardown scale; a session that
    /// wants only its own artifacts gone calls [`Self::purge_all`].
    pub fn cleanup(&self) {
        if self.inner.base_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.inner.base_dir) {
                tracing::warn!(
                    dir = %self.inner.base_dir.display(),
                    error = %e,
                    "failed to clean up tool result store"
                );
            }
        }
    }

    /// Purge **this session's** offloaded tool results — the `.txt` blobs *and*
    /// the FTS5 index entries — while keeping the store usable for the rest of
    /// the session (the directories and `index.db` survive; only this session's
    /// contents go).
    ///
    /// This is the **anti-reference-bypass** countermeasure (maps `OpenSquilla`'s
    /// `StaleOutputCache.purge`). Offloaded output is otherwise retrievable
    /// indefinitely via `read_file` on the `[Full output persisted: …]` marker
    /// path or via `ctx_search` over the index — neither of which consults the
    /// approval gate. So when a session trips the denial circuit-breaker, the
    /// gate's enforcement could be sidestepped by mining a result that was
    /// cached under an earlier, more permissive moment. Wiping both vectors at
    /// the trip closes that hole. It fires only on the brute-force threshold,
    /// so ordinary large-output workflows are never disturbed.
    ///
    /// The session scope is load-bearing: three refusals in one Panel tab used
    /// to delete every *other* tab's blobs and index rows too, leaving their
    /// in-context `[Full output persisted: …]` markers pointing at files that
    /// no longer existed.
    ///
    /// The purge iterates the FULL read scope ([`Self::read_scope_keys`]) — this
    /// handle's own key **plus every earlier epoch** of the same base session.
    /// A compaction split seeds the child (epoch N+1) with the parent's
    /// `[Full output persisted: …]` markers and lets `ctx_search` / `read_file`
    /// read the parent's (epoch N) blobs and index rows. Purging only the
    /// current epoch would leave exactly that pre-split content minable after
    /// the denial breaker trips — the hole this countermeasure exists to close.
    /// Widening stays inside INV-ISO (epochs of one base key are one trust
    /// domain); it never touches another agent or conversation.
    pub fn purge_all(&self) {
        let idx = self.index();
        for key in self.read_scope_keys() {
            // 1. Remove this key's offloaded `.txt` blobs (the
            //    `read_file`-via-marker vector). Scoped by directory: each
            //    (base-key, epoch) wrote into its own `blob_dir_for(key)`.
            if let Ok(entries) = std::fs::read_dir(self.blob_dir_for(&key)) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("txt") {
                        if let Err(e) = std::fs::remove_file(&path) {
                            tracing::warn!(
                                file = %path.display(),
                                error = %e,
                                "purge_all: failed to remove offloaded tool-result blob"
                            );
                        }
                    }
                }
            }
            // 2. Clear this key's FTS5 rows (the `ctx_search` vector). `index()`
            // opens it lazily, so this also catches an on-disk `index.db` left by
            // an earlier turn that was never reopened this run.
            if let Some(idx) = idx {
                if let Err(e) = idx.clear(&key) {
                    tracing::warn!(
                        session = %key,
                        error = %e,
                        "purge_all: failed to clear offloaded-output index"
                    );
                }
            }
        }
    }

    /// One-shot TTL GC pass over the **shared blob root**: remove per-session
    /// dirs whose newest file is older than `cutoff`, then clear the removed
    /// sessions' FTS index rows — so `ctx_search` cannot return hits whose
    /// backing blobs are gone, and the index cannot grow without bound.
    /// Returns the number of directories removed.
    ///
    /// Directory names are `sanitize_for_filename(session_key)`, which is not
    /// invertible; rather than guess the key back from the dir name, the
    /// distinct session ids are listed from the index and matched *forward*
    /// through the same sanitizer. Process-wide by design (the TTL is not a
    /// per-session policy). This is the synchronous core of
    /// [`spawn_periodic_sweeper`], exposed so test and admin bootstraps can
    /// trigger deterministic GC.
    pub fn sweep_stale(&self, cutoff: Duration) -> usize {
        let removed = sweep_stale_dirs_collect(&self.inner.base_dir, cutoff);
        if removed.is_empty() {
            return 0;
        }
        // Index-row GC for the sessions whose blob dirs just went away. The
        // lazy open is fine here: a failure degrades to blob-only GC and the
        // rows are retried on the next sweep.
        if let Some(idx) = self.index() {
            match idx.list_sessions() {
                Ok(sessions) => {
                    for session in sessions {
                        if removed
                            .iter()
                            .any(|dir| *dir == sanitize_for_filename(&session))
                        {
                            if let Err(e) = idx.clear(&session) {
                                tracing::warn!(
                                    session = %session,
                                    error = %e,
                                    "tool_result sweep: failed to clear index rows for removed session"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "tool_result sweep: failed to list indexed sessions"
                    );
                }
            }
        }
        removed.len()
    }
}

// =============================================================================
// Standalone helpers
// =============================================================================

/// Scan `text` for a `[Full output persisted: ...]` reference line and return
/// the first matching line if found.
#[must_use]
pub fn extract_persisted_ref(text: &str) -> Option<&str> {
    text.lines()
        .find(|line| line.starts_with(PERSISTED_REF_PREFIX))
}

/// Replace the per-call file path inside any persisted-output marker with a
/// fixed placeholder, so two byte-identical tool results compare equal.
///
/// The marker embeds a path whose file name is unique per dispatch. That is
/// correct for the file system and wrong for anything that fingerprints the
/// model-facing result: both loop detectors
/// ([`redundant_calls`](crate::tools::redundant_calls) and
/// [`no_progress`](crate::tools::no_progress)) bucket by `(tool, args)` and
/// then require every result in the bucket to be identical before they nudge.
/// An over-budget result is replaced wholesale by this marker, so the unique
/// path made every repeat differ and the `all_identical` gate could never
/// fire — inverting both detectors exactly where they matter most: the bigger
/// and more expensive the loop, the less likely it was to be caught.
///
/// The size + tool tail is deliberately kept: identical content offloads to an
/// identical token count, so the placeholder loses no discriminating power that
/// a fingerprint of the whole payload would have had.
#[must_use]
pub fn stabilize_persisted_ref(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(PERSISTED_REF_PREFIX) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.starts_with(PERSISTED_REF_PREFIX) {
            out.push_str(PERSISTED_REF_PREFIX);
            out.push_str("<offloaded>");
            // Keep " (<n> tokens, <tool>)]" — the part that still varies with
            // the content rather than with the dispatch.
            if let Some(tail) = line.rfind(" (") {
                out.push_str(&line[tail..]);
            }
        } else {
            out.push_str(line);
        }
    }
    std::borrow::Cow::Owned(out)
}

// =============================================================================
// TTL sweeper for stale per-session dirs
// =============================================================================

/// Sweep stale persisted-result directories under `root` and remove any whose
/// most-recent file mtime is older than `cutoff`. Empty directories are
/// removed unconditionally.
///
/// Returns the number of directories removed. Errors on individual entries
/// are logged at WARN and skipped so a single permission denial cannot stall
/// the sweep.
///
/// This is the directory-only convenience wrapper around the sweep core; it
/// does **not** clear index rows (it has no store handle). Production GC goes
/// through [`ToolResultStore::sweep_stale`], which does both. Exposed publicly
/// so test bootstraps and tools (e.g. an admin `aleph` CLI) can trigger a
/// one-shot dir GC pass without owning the background task.
pub fn sweep_stale_tool_result_dirs(root: &Path, cutoff: Duration) -> usize {
    sweep_stale_dirs_collect(root, cutoff).len()
}

/// Core of the sweep: removes stale per-session dirs under `root` and returns
/// the *names* of the directories removed, so [`ToolResultStore::sweep_stale`]
/// can clear the matching sessions' index rows.
fn sweep_stale_dirs_collect(root: &Path, cutoff: Duration) -> Vec<String> {
    let entries = match std::fs::read_dir(root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(
                root = %root.display(),
                error = %e,
                "tool_result sweep: read_dir failed"
            );
            return Vec::new();
        }
    };
    let now = SystemTime::now();
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // The store layout is one directory per session_id; ignore stray
        // files at the root (`index.db` included) so we never delete
        // user-placed artifacts or the live retrieval index.
        let is_dir = entry.file_type().is_ok_and(|ft| ft.is_dir());
        if !is_dir {
            continue;
        }
        if dir_is_stale(&path, now, cutoff) {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => removed.push(entry.file_name().to_string_lossy().into_owned()),
                Err(e) => {
                    tracing::warn!(
                        dir = %path.display(),
                        error = %e,
                        "tool_result sweep: remove_dir_all failed"
                    );
                }
            }
        }
    }
    removed
}

/// True iff `dir` has no entries newer than `cutoff` from `now`. An empty
/// directory is always considered stale (it serves no purpose). Errors
/// reading individual files are treated as "fresh" — the sweep prefers
/// false negatives (skip removal) over false positives (kill live state).
fn dir_is_stale(dir: &Path, now: SystemTime, cutoff: Duration) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            // Unreadable entry → assume live, keep dir.
            return false;
        };
        let mtime = meta.modified().unwrap_or(now);
        if now.duration_since(mtime).unwrap_or(Duration::ZERO) < cutoff {
            return false;
        }
    }
    // Either no entries (empty dir → stale) or every entry was older than
    // `cutoff` (we'd have early-returned `false` otherwise).
    true
}

/// Spawn a background Tokio task that periodically calls
/// [`ToolResultStore::sweep_stale`] on the store's blob root.
///
/// Fire-and-forget: the task is detached (and keeps the store's shared inner
/// alive). Multiple calls with the same store will spawn multiple sweepers —
/// callers should invoke this once per store (the
/// [`set_global_tool_result_store`] entry point enforces that: its slot
/// installs once, and the sweeper is spawned only on the install that won).
///
/// No-op if there is no running Tokio runtime — log at DEBUG and return so
/// that non-async test bootstraps don't panic.
pub fn spawn_periodic_sweeper(
    store: Arc<ToolResultStore>,
    retention: Duration,
    interval: Duration,
) {
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::debug!(
            root = %store.inner.base_dir.display(),
            "tool_result sweeper not spawned: no Tokio runtime"
        );
        return;
    }
    tokio::spawn(async move {
        // First sweep is delayed by one interval so boot doesn't block on
        // a synchronous fs walk; opencode follows the same pattern.
        let mut ticker = tokio::time::interval(interval);
        // Skip the initial "fire immediately" tick.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let store = Arc::clone(&store);
            // Move the blocking fs walk off the async runtime.
            let removed =
                match tokio::task::spawn_blocking(move || store.sweep_stale(retention)).await {
                    Ok(count) => count,
                    Err(e) => {
                        tracing::warn!(error = %e, "tool_result sweeper task failed");
                        0
                    }
                };
            if removed > 0 {
                tracing::info!(removed, "tool_result sweeper reclaimed stale session dirs");
            }
        }
    });
}

// =============================================================================
// Owner-only spill files
// =============================================================================
//
// Spilled blobs are the one artifact on this path that outlives the run (7-day
// TTL) and they carry whatever `web_fetch` / `browser` / MCP captured, so they
// get the same owner-only treatment the vault, `config.toml` and the node token
// already get (`src/secrets/vault.rs`, `src/config/save.rs`). Windows is a
// no-op, matching those call sites' `cfg(unix)` gate.

/// Create `dir` and its parents, then clamp it to owner-only on Unix.
///
/// The chmod runs even when the directory already existed: `DirBuilder::mode`
/// would only reach directories this call creates, leaving every install made
/// before this hardening on a `0o755` root forever.
///
/// A chmod failure is logged rather than propagated. The blobs themselves are
/// created `0o600` independently of this, so a filesystem without Unix
/// permissions loses a defense-in-depth layer instead of losing every offloaded
/// result (the caller's failure mode is "no spill at all").
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "failed to restrict tool result dir to owner-only"
            );
        }
    }
    Ok(())
}

/// Write `content` to `path`, creating it owner-only on Unix.
///
/// `truncate(true)` restores what the `std::fs::write` this replaced implied:
/// a re-persist under the same call id writing *shorter* content would
/// otherwise leave the previous blob's tail behind, and `read_file` /
/// `ctx_search` would serve those stale bytes as current output.
///
/// `mode` applies only to a file this call creates; a blob left `0o644` by a
/// pre-hardening run keeps that mode until the TTL sweep reclaims it, shielded
/// meanwhile by the `0o700` parent.
fn write_private_file(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(content.as_bytes())
}

/// Last 8 chars of a tool call id (char-safe). Call-id *prefixes* are constant
/// (`toolu_…`, `call_…`), so the tail is the distinctive part — the head would
/// collide across every call of the run.
fn short_call_id(id: &str) -> &str {
    let skip = id.chars().count().saturating_sub(8);
    match id.char_indices().nth(skip) {
        Some((byte_idx, _)) => &id[byte_idx..],
        None => id,
    }
}

/// Replace characters unsafe for filenames with underscores.
fn sanitize_for_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ========================================================================
    // The process-global handle, as a capability slot
    // ========================================================================

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_tool_result_store_slot();
        assert_eq!(slot.id(), "tools/result-store");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

    /// Build a test store rooted in a scratch directory instead of ~/.aleph/.
    ///
    /// The guard comes back with it: it owns the tree, so it must be bound in
    /// the caller's frame. See [`crate::utils::scratch::scratch_root`].
    fn test_store(_name: &str) -> (tempfile::TempDir, ToolResultStore, PathBuf) {
        let (scratch, base) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore::with_dir_for_tests(base.clone());
        (scratch, store, base)
    }

    /// Two session-scoped handles over one shared store — the production shape
    /// (boot installs one store; each run narrows it with `for_session`).
    fn shared_pair(name: &str) -> (Arc<ToolResultStore>, Arc<ToolResultStore>, PathBuf) {
        let (_scratch, root, base) = test_store(name);
        let root = Arc::new(root);
        let a = ToolResultStore::for_session(&root, "agent:main:sess-a");
        let b = ToolResultStore::for_session(&root, "agent:main:sess-b");
        (a, b, base)
    }

    #[test]
    fn small_result_not_persisted() {
        let (_scratch, store, _base) = test_store("small_result_not_persisted");
        // threshold = 10_000 tokens; short content is well under
        let result = store.persist_if_large("call_1", "read_file", "hello world", 10_000);
        assert!(result.is_none(), "short content should not be persisted");
    }

    #[test]
    fn large_result_persisted_and_recoverable() {
        let (_scratch, store, base) = test_store("large_result_persisted");
        // Generate content that is definitely > 1 token (threshold = 1)
        let content = "a".repeat(1000);
        let result = store.persist_if_large("call_abc", "bash", &content, 1);
        assert!(result.is_some(), "large content should be persisted");
        let marker = result.unwrap();
        assert!(
            marker.starts_with(PERSISTED_REF_PREFIX),
            "marker must start with prefix: {marker}"
        );
        // Verify a .txt file was created and its content matches
        let files: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "exactly one file should be written");
        let written = std::fs::read_to_string(files[0].path()).unwrap();
        assert_eq!(written, content, "written content must match original");
    }

    #[test]
    fn cleanup_removes_directory() {
        let (_scratch, base) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore::with_dir_for_tests(base.clone());
        assert!(base.exists());
        store.cleanup();
        assert!(!base.exists(), "cleanup should remove the base directory");
    }

    #[test]
    fn extract_persisted_ref_finds_marker() {
        let text =
            "some output\n[Full output persisted: /tmp/foo.txt (1234 tokens, bash)]\nmore text";
        let found = extract_persisted_ref(text);
        assert!(found.is_some(), "should find marker line");
        assert!(found.unwrap().contains("Full output persisted"));
    }

    #[test]
    fn extract_persisted_ref_returns_none_when_absent() {
        let text = "no marker here\njust regular output";
        let found = extract_persisted_ref(text);
        assert!(found.is_none(), "should return None when no marker present");
    }

    #[test]
    fn purge_all_removes_blobs_and_clears_index() {
        let (_scratch, store, base) = test_store("purge_all_removes_blobs");
        // Clean any residue from a prior run so the count assertions are exact.
        if let Ok(entries) = std::fs::read_dir(&base) {
            for e in entries.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let content = "secret-token-abcdef ".repeat(200);
        // Offload a large result (writes a `.txt` blob) and index it (so
        // ctx_search could later mine it — the reference-bypass vector).
        assert!(store
            .persist_if_large("call_x", "bash", &content, 1)
            .is_some());
        let _ = store.index_output("call_x", "bash", &content);
        assert!(store.indexed_sections() > 0, "content should be indexed");

        let txt_count = |base: &std::path::Path| {
            std::fs::read_dir(base)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("txt"))
                .count()
        };
        assert_eq!(
            txt_count(&base),
            1,
            "one offloaded blob present before purge"
        );

        store.purge_all();

        assert_eq!(
            txt_count(&base),
            0,
            "offloaded blobs must be gone after purge"
        );
        assert_eq!(
            store.indexed_sections(),
            0,
            "index must be empty after purge"
        );
        assert!(
            store.search("secret-token-abcdef", 5).is_empty(),
            "ctx_search must find nothing after purge"
        );
    }

    // -------------------------------------------------------------------
    // Session scoping (INV-ISO) — B2
    // -------------------------------------------------------------------

    #[test]
    fn ctx_search_cannot_reach_another_sessions_offloaded_output() {
        // Panel multi-session is the norm: two runs share the one store boot
        // installed. Session A's `ctx_search` must not surface B's tool output.
        let (a, b, _base) = shared_pair("scope_search_isolation");
        let a_text = format!("{}{}", "alpha ".repeat(50), "secretalpha ".repeat(200));
        let b_text = format!("{}{}", "beta ".repeat(50), "secretbeta ".repeat(200));

        assert!(a.persist_if_large("call_a", "bash", &a_text, 1).is_some());
        let _ = a.index_output("call_a", "bash", &a_text);
        assert!(b.persist_if_large("call_b", "bash", &b_text, 1).is_some());
        let _ = b.index_output("call_b", "bash", &b_text);

        assert!(
            a.search("secretbeta", 5).is_empty(),
            "session A must not retrieve session B's offloaded output"
        );
        assert!(
            b.search("secretalpha", 5).is_empty(),
            "session B must not retrieve session A's offloaded output"
        );
        // ...and each still finds its own (the scope must not break retrieval).
        assert!(!a.search("secretalpha", 5).is_empty());
        assert!(!b.search("secretbeta", 5).is_empty());
    }

    #[test]
    fn purge_all_in_one_session_leaves_other_sessions_intact() {
        // The denial circuit-breaker fires `purge_all` after 3 refusals in ONE
        // session. It used to wipe every concurrent session's blobs and index,
        // leaving their `[Full output persisted: …]` markers dangling.
        let (a, b, _base) = shared_pair("scope_purge_isolation");
        let a_text = "alpha payload ".repeat(300);
        let b_text = "beta payload ".repeat(300);

        let a_marker = a.persist_if_large("call_a", "bash", &a_text, 1).unwrap();
        let _ = a.index_output("call_a", "bash", &a_text);
        let b_marker = b.persist_if_large("call_b", "bash", &b_text, 1).unwrap();
        let _ = b.index_output("call_b", "bash", &b_text);
        let b_blob = marker_path(&b_marker);
        assert!(b_blob.exists());
        assert!(marker_path(&a_marker).exists());

        a.purge_all();

        assert!(
            !marker_path(&a_marker).exists(),
            "the purging session's own blob must be gone"
        );
        assert_eq!(a.indexed_sections(), 0, "A's index rows must be gone");
        assert!(
            b_blob.exists(),
            "a bystander session's blob must survive another session's purge"
        );
        assert!(b.indexed_sections() > 0, "B's index rows must survive");
        assert!(
            !b.search("beta payload", 5).is_empty(),
            "B's ctx_search must still work after A's purge"
        );
    }

    /// Extract the blob path out of a `[Full output persisted: <path> (…)]`
    /// marker (the same shape `result_processing::parse_marker_path` parses).
    fn marker_path(marker: &str) -> PathBuf {
        let rest = marker.strip_prefix(PERSISTED_REF_PREFIX).unwrap();
        let end = rest.find(" (").unwrap();
        PathBuf::from(&rest[..end])
    }

    // -------------------------------------------------------------------
    // TTL sweeper
    // -------------------------------------------------------------------

    use std::time::Duration;

    fn sweeper_root(_name: &str) -> (tempfile::TempDir, PathBuf) {
        let (scratch, base) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&base).unwrap();
        (scratch, base)
    }

    /// Set mtime backwards by `secs` via `filetime` so the dir looks stale.
    /// We use the `filetime` crate which is already a transitive dep; if
    /// not available, fall back to creating the dir and immediately
    /// claiming it's old enough by passing `cutoff = Duration::ZERO`.
    fn touch_old(path: &Path, secs: u64) {
        let when = SystemTime::now() - Duration::from_secs(secs);
        let ft = filetime::FileTime::from_system_time(when);
        // Set mtime on the dir and every file inside it.
        let _ = filetime::set_file_mtime(path, ft);
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let _ = filetime::set_file_mtime(entry.path(), ft);
            }
        }
    }

    #[test]
    fn sweep_removes_stale_dir() {
        let (_scratch, root) = sweeper_root("removes_stale_dir");
        let stale = root.join("session_old");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("call_1_bash.txt"), "old data").unwrap();
        // 14 days old, retention 7 days → stale.
        touch_old(&stale, 14 * 24 * 60 * 60);

        let removed = sweep_stale_tool_result_dirs(&root, DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 1, "should remove the one stale dir");
        assert!(!stale.exists(), "stale dir should be gone");
    }

    #[test]
    fn sweep_preserves_fresh_dir() {
        let (_scratch, root) = sweeper_root("preserves_fresh_dir");
        let fresh = root.join("session_live");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(fresh.join("call_1_bash.txt"), "fresh data").unwrap();

        let removed = sweep_stale_tool_result_dirs(&root, DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 0, "fresh dir must not be touched");
        assert!(fresh.exists(), "fresh dir should survive sweep");
    }

    #[test]
    fn sweep_removes_empty_dir() {
        let (_scratch, root) = sweeper_root("removes_empty_dir");
        let empty = root.join("session_empty");
        std::fs::create_dir_all(&empty).unwrap();
        // No files inside — empty session dir is always stale.
        let removed = sweep_stale_tool_result_dirs(&root, DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 1);
        assert!(!empty.exists());
    }

    #[test]
    fn sweep_ignores_stray_files_at_root() {
        // We never delete root-level files — only sub-directories — so a
        // user-placed README at the root survives an aggressive cutoff.
        let (_scratch, root) = sweeper_root("ignores_stray_files");
        let stray = root.join("README.txt");
        std::fs::write(&stray, "do not delete").unwrap();

        let removed = sweep_stale_tool_result_dirs(&root, Duration::ZERO);
        assert_eq!(removed, 0);
        assert!(stray.exists(), "stray top-level file must survive sweep");
    }

    #[test]
    fn sweep_on_missing_root_is_noop() {
        let missing = std::env::temp_dir()
            .join("aleph_test_tool_result_sweeper")
            .join("definitely_missing_xyz");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(!missing.exists());
        let removed = sweep_stale_tool_result_dirs(&missing, DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 0);
    }

    #[test]
    fn sweep_at_store_root_removes_stale_sessions_but_keeps_root_and_index() {
        // The sweeper used to root at the *parent* of the blob root, so its
        // only entry was the live root itself: once every file under it aged
        // past the TTL, `remove_dir_all` deleted the root INCLUDING index.db
        // out from under the open index connection. Rooted correctly, an
        // all-stale sweep removes the per-session dirs and nothing else.
        let (_scratch, root_store, base) = test_store("sweep_correct_root");
        let root = Arc::new(root_store);
        let a = ToolResultStore::for_session(&root, "agent:main:sess-a");
        let text = "stale payload ".repeat(200);
        assert!(a.persist_if_large("call_a", "bash", &text, 1).is_some());
        let _ = a.index_output("call_a", "bash", &text);

        let sess_dir = base.join(sanitize_for_filename("agent:main:sess-a"));
        assert!(sess_dir.exists());
        // 14 days old, retention 7 days → stale.
        touch_old(&sess_dir, 14 * 24 * 60 * 60);

        let removed = root.sweep_stale(DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 1, "the stale session dir should be removed");
        assert!(!sess_dir.exists());
        assert!(
            base.exists(),
            "the live blob root must survive an all-stale sweep"
        );
        assert!(
            base.join(INDEX_DB_NAME).exists(),
            "index.db must survive the sweep"
        );
        // The index connection is still usable after the sweep.
        let fresh = ToolResultStore::for_session(&root, "agent:main:sess-fresh");
        let fresh_text = "freshterm payload ".repeat(200);
        let _ = fresh.index_output("call_f", "bash", &fresh_text);
        assert!(
            !fresh.search("freshterm", 5).is_empty(),
            "store must remain functional after the sweep"
        );
    }

    #[test]
    fn sweep_clears_index_rows_of_removed_sessions_only() {
        // Index rows used to be keyed forever: sweeping a session's blobs left
        // its rows behind, so ctx_search returned hits whose backing blobs
        // were gone and the index grew without bound.
        let (_scratch, root_store, base) = test_store("sweep_index_gc");
        let root = Arc::new(root_store);
        let a = ToolResultStore::for_session(&root, "agent:main:sess-a");
        let b = ToolResultStore::for_session(&root, "agent:main:sess-b");
        let a_text = "alphaonly payload ".repeat(200);
        let b_text = "betaonly payload ".repeat(200);
        assert!(a.persist_if_large("call_a", "bash", &a_text, 1).is_some());
        let _ = a.index_output("call_a", "bash", &a_text);
        assert!(b.persist_if_large("call_b", "bash", &b_text, 1).is_some());
        let _ = b.index_output("call_b", "bash", &b_text);
        assert!(a.indexed_sections() > 0);

        // Only A's dir goes stale; B stays fresh.
        touch_old(
            &base.join(sanitize_for_filename("agent:main:sess-a")),
            14 * 24 * 60 * 60,
        );

        let removed = root.sweep_stale(DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 1);
        assert_eq!(
            a.indexed_sections(),
            0,
            "the removed session's index rows must be GC'd with its blobs"
        );
        assert!(
            a.search("alphaonly", 5).is_empty(),
            "ctx_search must not surface hits whose backing blobs are gone"
        );
        assert!(
            b.indexed_sections() > 0,
            "a live session's rows must survive"
        );
        assert!(!b.search("betaonly", 5).is_empty());
    }

    // -------------------------------------------------------------------
    // Epoch-aware retrieval (session split) + hit source labeling
    // -------------------------------------------------------------------

    #[test]
    fn child_epoch_search_finds_parent_epoch_rows() {
        // A compaction-driven session split moves the run to epoch+1 but seeds
        // the child with the parent's `[Full output persisted: …]` markers
        // verbatim. The child's read scope must therefore cover earlier epochs
        // of the same base key, or ctx_search finds nothing for exactly the
        // content the inherited markers advertise.
        let (_scratch, root, _base) = test_store("epoch_aware_search");
        let root = Arc::new(root);
        let parent_key = crate::routing::session_key::SessionKey::Main {
            agent_id: "agent-a".to_string(),
            main_key: "main".to_string(),
            epoch: 0,
        };
        let child_key = parent_key.with_next_epoch();
        let parent = ToolResultStore::for_session(&root, parent_key.to_key_string());
        let child = ToolResultStore::for_session(&root, child_key.to_key_string());

        let text = "epochblob payload ".repeat(200);
        assert!(parent
            .persist_if_large("call_p", "bash", &text, 1)
            .is_some());
        let _ = parent.index_output("call_p", "bash", &text);

        assert!(
            !child.search("epochblob", 5).is_empty(),
            "child epoch must retrieve rows indexed under the parent epoch"
        );
        assert!(
            child.indexed_sections() > 0,
            "inherited rows must count as indexed for the child"
        );
        // A different base key still sees nothing (INV-ISO across agents).
        let other = ToolResultStore::for_session(&root, "agent:other:main:s1");
        assert!(other.search("epochblob", 5).is_empty());
    }

    #[test]
    fn purge_all_from_child_epoch_wipes_parent_epoch_content() {
        // Anti-reference-bypass: after the denial breaker trips in a child epoch
        // (post-compaction split), the parent epoch's offloaded blobs AND index
        // rows — which the child can read via the widened scope — must ALSO be
        // purged, or a paused session can still mine pre-split content.
        let (_scratch, root, _base) = test_store("epoch_aware_purge");
        let root = Arc::new(root);
        let parent_key = crate::routing::session_key::SessionKey::Main {
            agent_id: "agent-a".to_string(),
            main_key: "main".to_string(),
            epoch: 0,
        };
        let child_key = parent_key.with_next_epoch();
        let parent = ToolResultStore::for_session(&root, parent_key.to_key_string());
        let child = ToolResultStore::for_session(&root, child_key.to_key_string());

        let text = "epochsecret payload ".repeat(200);
        let marker = parent
            .persist_if_large("call_p", "bash", &text, 1)
            .expect("parent blob persisted");
        let _ = parent.index_output("call_p", "bash", &text);
        let parent_blob = marker_path(&marker);
        assert!(parent_blob.exists());
        assert!(
            !child.search("epochsecret", 5).is_empty(),
            "precondition: child can read the parent-epoch content"
        );

        // The breaker fires on the CHILD handle.
        child.purge_all();

        assert!(
            !parent_blob.exists(),
            "parent-epoch blob must be purged when the child trips the breaker"
        );
        assert!(
            child.search("epochsecret", 5).is_empty(),
            "parent-epoch index rows must be unreachable after the child's purge"
        );
        assert_eq!(child.indexed_sections(), 0);
    }

    #[test]
    fn search_hit_source_names_the_originating_tool() {
        // The hit `source` is surfaced to the model as "the tool call the
        // section came from" — a bare call-id UUID says nothing. It must lead
        // with the tool name and keep an id suffix for reindex identity.
        let (_scratch, store, _base) = test_store("source_label");
        let text = "labelcheck payload ".repeat(200);
        let _ = store.index_output("toolu_01ABCDEFGH", "web_fetch", &text);
        let hits = store.search("labelcheck", 3);
        assert!(!hits.is_empty());
        assert!(
            hits[0].source.starts_with("web_fetch:"),
            "hit source must lead with the tool name, got {}",
            hits[0].source
        );
        assert!(
            hits[0].source.ends_with("ABCDEFGH"),
            "hit source must keep an id suffix for uniqueness, got {}",
            hits[0].source
        );
        // Re-indexing the same call replaces its chunks (same composed source).
        let before = store.indexed_sections();
        let _ = store.index_output("toolu_01ABCDEFGH", "web_fetch", &text);
        assert_eq!(
            store.indexed_sections(),
            before,
            "re-indexing the same call must replace, not append"
        );
    }

    // -------------------------------------------------------------------
    // Owner-only spill files
    // -------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn spilled_blob_and_its_session_dir_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (_scratch, root, base) = test_store("owner_only_spill");
        let root = Arc::new(root);
        let key = "agent:main:sess-perm";
        let store = ToolResultStore::for_session(&root, key);

        // Pre-create the session dir world-readable: the chmod has to reach a
        // directory it did not create, which is the only half that protects
        // installs that predate this hardening.
        let dir = base.join(sanitize_for_filename(key));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        let marker = store
            .persist_if_large("call_perm", "web_fetch", &"secret ".repeat(500), 1)
            .expect("large content is persisted");
        let blob = marker_path(&marker);

        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700,
            "session blob dir must be owner-only"
        );
        assert_eq!(
            std::fs::metadata(&blob).unwrap().permissions().mode() & 0o777,
            0o600,
            "spilled blob must be owner-only"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_private_dir_tightens_a_pre_existing_root() {
        use std::os::unix::fs::PermissionsExt;

        // `ToolResultStore::new` resolves its root through `get_data_dir`, which
        // reads a process-global env var, so the boot root is asserted at the
        // helper both call sites now share.
        let (_scratch, root) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

        create_private_dir(&root).unwrap();

        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700,
            "an existing store root must be clamped, not left as created"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn re_persisting_shorter_content_leaves_no_stale_tail() {
        // Same call id → same path. Dropping `std::fs::write`'s implicit
        // truncate would leave the longer blob's tail for `read_file` /
        // `ctx_search` to serve as current output.
        let (_scratch, store, _base) = test_store("respill_truncates");
        let long = "L".repeat(4000);
        let short = "S".repeat(400);

        let first = store
            .persist_if_large("call_same", "bash", &long, 1)
            .expect("first spill persisted");
        let second = store
            .persist_if_large("call_same", "bash", &short, 1)
            .expect("second spill persisted");
        assert_eq!(
            marker_path(&first),
            marker_path(&second),
            "precondition: the re-persist targets the same path"
        );

        let on_disk = std::fs::read_to_string(marker_path(&second)).unwrap();
        assert_eq!(on_disk, short, "re-persist must not leave a stale tail");
    }
}
