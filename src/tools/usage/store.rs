//! The `tool_usage.json` sidecar: the write side of per-origin invocation
//! records. See [`super`] for why this store exists at all.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::utils::atomic_io::{with_file_lock, write_atomic};

/// Maximum distinct tool names retained per origin.
///
/// An MCP server may advertise hundreds of tools; without a cap one chatty
/// server could grow the sidecar without bound. Calls to a tool beyond the cap
/// are **not dropped** — they land in [`OriginUsage::other_tool_calls`], so
/// `call_count` stays the honest total and a reader can always tell that the
/// per-tool breakdown is partial (no silent truncation).
const MAX_TOOLS_PER_ORIGIN: usize = 64;

/// Where a tool came from, for usage accounting.
///
/// Borrowed on purpose: both implementors already own the id as a field, so
/// the hot path (a builtin returning `None`) allocates nothing and the cold
/// path allocates once, at the point the record is written.
///
/// Deliberately **not** a third general-purpose source enum next to
/// [`crate::tools::service::ToolSource`] and
/// [`crate::tool_metadata::ToolSource`]. Those two answer "what kind of tool is
/// this" for routing and presentation and both have a `Builtin` variant; this
/// one answers "is this thing uninstallable, and under what name", which is why
/// it has no builtin variant at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageOrigin<'a> {
    /// An MCP server's tool, identified by the server id.
    Mcp(&'a str),
    /// A plugin-provided tool, identified by the plugin id.
    Plugin(&'a str),
}

impl UsageOrigin<'_> {
    /// The sidecar key: `mcp:<id>` / `plugin:<id>`.
    ///
    /// The single source for this spelling — the doctor check, the reporting
    /// tool and the RPC handlers all build their lookup keys through
    /// [`Self::mcp_key`] / [`Self::plugin_key`] rather than formatting the
    /// prefix themselves.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Mcp(id) => Self::mcp_key(id),
            Self::Plugin(id) => Self::plugin_key(id),
        }
    }

    /// Sidecar key for an MCP server id, for readers that have the id but not
    /// a live tool.
    #[must_use]
    pub fn mcp_key(server_id: &str) -> String {
        format!("mcp:{server_id}")
    }

    /// Sidecar key for a plugin id, for readers that have the id but not a
    /// live tool.
    #[must_use]
    pub fn plugin_key(plugin_id: &str) -> String {
        format!("plugin:{plugin_id}")
    }
}

/// Aggregate invocation record for one origin.
///
/// Every field is `#[serde(default)]` so a sidecar written by an older build
/// deserializes without migration — the same back-compat discipline as
/// [`crate::skill::usage::UsageStats`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginUsage {
    /// Calls that reached the tool, success or failure.
    #[serde(default)]
    pub call_count: u64,
    /// Subset of `call_count` that came back as an error.
    #[serde(default)]
    pub error_count: u64,
    /// First call this sidecar ever observed for the origin. Distinct from
    /// "installed at", which this store has no way to know — see the module
    /// note on why nothing is seeded at registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<String>,
    /// Per-tool breakdown, capped at [`MAX_TOOLS_PER_ORIGIN`] distinct names.
    /// `BTreeMap` so the emitted JSON is byte-deterministic across runs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, u64>,
    /// Calls whose tool name did not fit under the per-origin cap. Non-zero
    /// means `tools` is a partial view; `call_count` remains exact.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub other_tool_calls: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if contract
const fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl OriginUsage {
    /// `true` when the per-tool breakdown omits at least one call.
    #[must_use]
    pub const fn breakdown_is_partial(&self) -> bool {
        self.other_tool_calls > 0
    }
}

/// Cross-process-safe reader/writer for the tool-usage sidecar.
pub struct ToolUsageStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl ToolUsageStore {
    /// Store backed by an explicit file path. Used by tests and by any caller
    /// that already resolved the location.
    pub fn at(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let lock_path = path.with_extension("json.lock");
        Self { path, lock_path }
    }

    /// Store backed by the default `<data_dir>/tool_usage.json`.
    ///
    /// `None` when no home directory can be resolved — the caller then skips
    /// recording rather than guessing a location (a second spelling of the
    /// Aleph home is undetectable on any machine where `ALEPH_HOME` is unset).
    #[must_use]
    pub fn default_path() -> Option<Self> {
        crate::utils::paths::tool_usage_path().map(Self::at)
    }

    fn load_map(&self) -> HashMap<String, OriginUsage> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(error = %e, path = %self.path.display(), "corrupted tool usage sidecar; resetting");
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        }
    }

    fn save_map(&self, map: &HashMap<String, OriginUsage>) {
        match serde_json::to_vec_pretty(map) {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&self.path, &bytes) {
                    tracing::warn!(error = %e, "tool usage: atomic write failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "tool usage: serialize failed"),
        }
    }

    /// Read-modify-write under the cross-process file lock, so two Aleph
    /// processes (or two parallel dispatches) cannot lose an increment.
    fn mutate<F>(&self, origin_key: &str, mutator: F)
    where
        F: FnOnce(&mut OriginUsage),
    {
        if origin_key.is_empty() {
            return;
        }
        if let Some(parent) = self.lock_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(error = %e, "tool usage: mkdir failed");
                return;
            }
        }
        let result = with_file_lock(&self.lock_path, |_| {
            let mut map = self.load_map();
            let entry = map.entry(origin_key.to_string()).or_default();
            mutator(entry);
            self.save_map(&map);
            Ok(())
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, origin = origin_key, "tool usage: lock acquisition failed");
        }
    }

    /// Record one call that reached the tool. `ok` distinguishes a returned
    /// result from a returned error; both count as usage.
    ///
    /// Blocking I/O — callers on an async runtime must wrap this in
    /// `spawn_blocking` (see [`record_call_detached`]).
    pub fn record_call(&self, origin_key: &str, tool: &str, ok: bool) {
        let now = now_iso();
        self.mutate(origin_key, |entry| {
            entry.call_count = entry.call_count.saturating_add(1);
            if entry.first_used_at.is_none() {
                entry.first_used_at = Some(now.clone());
            }
            entry.last_used_at = Some(now.clone());
            if !ok {
                entry.error_count = entry.error_count.saturating_add(1);
                entry.last_error_at = Some(now.clone());
            }
            if tool.is_empty() {
                return;
            }
            if let Some(count) = entry.tools.get_mut(tool) {
                *count = count.saturating_add(1);
            } else if entry.tools.len() < MAX_TOOLS_PER_ORIGIN {
                entry.tools.insert(tool.to_string(), 1);
            } else {
                // Over the cap: keep the total honest instead of dropping.
                entry.other_tool_calls = entry.other_tool_calls.saturating_add(1);
            }
        });
    }

    /// Read one origin's record, if it has ever been called.
    #[must_use]
    pub fn get(&self, origin_key: &str) -> Option<OriginUsage> {
        self.load_map().get(origin_key).cloned()
    }

    /// Snapshot the whole map. The reporting surfaces join this against the
    /// live list of configured servers / installed plugins.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, OriginUsage> {
        self.load_map()
    }

    /// Forget an uninstalled MCP server's row, deriving the key the same way
    /// recording does — the shapes must not be spelled out twice.
    pub fn forget_mcp(&self, server_id: &str) {
        self.forget(&UsageOrigin::mcp_key(server_id));
    }

    /// Forget an uninstalled plugin's row. See [`Self::forget_mcp`].
    pub fn forget_plugin(&self, plugin_id: &str) {
        self.forget(&UsageOrigin::plugin_key(plugin_id));
    }

    /// Drop an origin's record — called when the server/plugin is uninstalled
    /// so the sidecar does not accumulate orphan rows.
    pub fn forget(&self, origin_key: &str) {
        if origin_key.is_empty() {
            return;
        }
        let result = with_file_lock(&self.lock_path, |_| {
            let mut map = self.load_map();
            if map.remove(origin_key).is_some() {
                self.save_map(&map);
            }
            Ok(())
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, origin = origin_key, "tool usage: forget lock failed");
        }
    }
}

/// Record a call from an async context without blocking the runtime.
///
/// Awaited rather than fire-and-forget: the write is a few milliseconds after a
/// tool call that took hundreds, and awaiting keeps "the record exists by the
/// time dispatch returns" true — which is what makes this testable at all. A
/// detached task would also let a burst of parallel MCP calls queue up an
/// unbounded number of blocking threads all contending for the same file lock.
pub async fn record_call_detached(origin_key: String, tool: String, ok: bool) {
    let handle = tokio::task::spawn_blocking(move || {
        if let Some(store) = ToolUsageStore::default_path() {
            store.record_call(&origin_key, &tool, ok);
        }
    });
    if let Err(e) = handle.await {
        tracing::warn!(error = %e, "tool usage: record task failed");
    }
}

/// Forget an uninstalled MCP server's usage row.
///
/// Blocking file work, so callers on an async path should hand it to
/// `spawn_blocking`; the write is a single small file under a lock.
///
/// Why at uninstall rather than leaving it to the orphan sweep: a row whose
/// origin is reinstalled under the *same id* is never an orphan, so the sweep
/// never sees it — and the new install silently inherits the old install's
/// call count, `first_used_at` and idle age. The sweep
/// (`tool_usage(forget_orphans)`) stays as the net for removals that do not
/// pass through a call site here.
pub fn forget_mcp(server_id: &str) {
    if let Some(store) = ToolUsageStore::default_path() {
        store.forget_mcp(server_id);
    }
}

/// Forget an uninstalled plugin's usage row. See [`forget_mcp`].
pub fn forget_plugin(plugin_id: &str) {
    if let Some(store) = ToolUsageStore::default_path() {
        store.forget_plugin(plugin_id);
    }
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn store() -> (tempfile::TempDir, ToolUsageStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolUsageStore::at(tmp.path().join("tool_usage.json"));
        (tmp, store)
    }

    #[test]
    fn origin_keys_are_namespaced_by_kind() {
        assert_eq!(UsageOrigin::Mcp("ctx7").key(), "mcp:ctx7");
        assert_eq!(UsageOrigin::Plugin("ctx7").key(), "plugin:ctx7");
        // A plugin and an MCP server may legitimately share an id (a plugin
        // that ships a server of the same name); the prefix is what keeps
        // their counters from merging.
        assert_ne!(UsageOrigin::Mcp("x").key(), UsageOrigin::Plugin("x").key());
    }

    #[test]
    fn record_then_read_roundtrips() {
        let (tmp, store) = store();
        store.record_call("mcp:ctx7", "query-docs", true);
        store.record_call("mcp:ctx7", "query-docs", true);
        store.record_call("mcp:ctx7", "resolve-id", false);

        let reopened = ToolUsageStore::at(tmp.path().join("tool_usage.json"));
        let usage = reopened.get("mcp:ctx7").unwrap();
        assert_eq!(usage.call_count, 3);
        assert_eq!(usage.error_count, 1);
        assert_eq!(usage.tools.get("query-docs"), Some(&2));
        assert_eq!(usage.tools.get("resolve-id"), Some(&1));
        assert!(usage.first_used_at.is_some());
        assert!(usage.last_used_at.is_some());
        assert!(usage.last_error_at.is_some());
        assert!(!usage.breakdown_is_partial());
    }

    #[test]
    fn a_successful_call_leaves_the_error_stamp_alone() {
        let (_tmp, store) = store();
        store.record_call("mcp:a", "t", true);
        let usage = store.get("mcp:a").unwrap();
        assert_eq!(usage.error_count, 0);
        assert!(
            usage.last_error_at.is_none(),
            "a clean call must not stamp last_error_at"
        );
    }

    #[test]
    fn never_called_origin_has_no_row() {
        let (_tmp, store) = store();
        store.record_call("mcp:used", "t", true);
        assert!(
            store.get("mcp:never").is_none(),
            "absence of a row IS the never-used signal; nothing may seed a zero row"
        );
    }

    #[test]
    fn empty_origin_is_ignored() {
        let (_tmp, store) = store();
        store.record_call("", "t", true);
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn breakdown_cap_keeps_the_total_honest() {
        let (_tmp, store) = store();
        for i in 0..(MAX_TOOLS_PER_ORIGIN + 5) {
            store.record_call("mcp:chatty", &format!("tool{i}"), true);
        }
        let usage = store.get("mcp:chatty").unwrap();
        assert_eq!(
            usage.tools.len(),
            MAX_TOOLS_PER_ORIGIN,
            "breakdown is capped"
        );
        assert_eq!(
            usage.other_tool_calls, 5,
            "overflow is counted, not dropped"
        );
        assert_eq!(
            usage.call_count,
            MAX_TOOLS_PER_ORIGIN as u64 + 5,
            "the total must stay exact even when the breakdown truncates"
        );
        assert!(usage.breakdown_is_partial());
    }

    #[test]
    fn a_tool_already_in_the_breakdown_still_counts_past_the_cap() {
        let (_tmp, store) = store();
        for i in 0..MAX_TOOLS_PER_ORIGIN {
            store.record_call("mcp:chatty", &format!("tool{i}"), true);
        }
        store.record_call("mcp:chatty", "tool0", true);
        let usage = store.get("mcp:chatty").unwrap();
        assert_eq!(usage.tools.get("tool0"), Some(&2));
        assert_eq!(usage.other_tool_calls, 0);
    }

    #[test]
    fn the_forget_verbs_agree_with_the_keys_recording_writes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ToolUsageStore::at(tmp.path().join("tool_usage.json"));
        store.record_call(&UsageOrigin::mcp_key("srv"), "read", true);
        store.record_call(&UsageOrigin::plugin_key("plg"), "ping", true);
        assert_eq!(store.snapshot().len(), 2);

        store.forget_mcp("srv");
        store.forget_plugin("plg");
        assert!(
            store.snapshot().is_empty(),
            "a key spelled differently here than at the record site would \
             leave the row behind, and a same-id reinstall would inherit it"
        );
    }

    /// Plugin removal has three independent writers (the `plugins.uninstall`
    /// RPC, the hub/extensions lifecycle path, and the `aleph plugins
    /// uninstall` CLI), each deleting the directory itself. Nothing forces a
    /// fourth to drop the usage row, so this counts them instead: the row
    /// would otherwise survive, and a reinstall under the same id is never an
    /// orphan, so the sweep never catches it either.
    ///
    /// (MCP needs no equivalent: all six `remove_server` call sites funnel
    /// through one actor method, which is where the forget lives.)
    ///
    /// The scan reads intent from the function name, which only sees the
    /// spellings it was taught — so it also asserts it still matches at least
    /// the three known sites. A rename that drops a site out of the scan turns
    /// this red rather than leaving it quietly green.
    #[test]
    fn every_plugin_removal_site_forgets_its_usage_row() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 100, "walk found suspiciously few sources");

        let mut checked = 0usize;
        let mut offenders: Vec<String> = Vec::new();
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let production = crate::utils::source_scan::production_prefix(&text);
            for chunk in production.split("fn ").skip(1) {
                if !chunk.contains("default_plugins_dir()") || !chunk.contains("remove_dir_all") {
                    continue;
                }
                // Install paths delete a plugin directory too — to overwrite an
                // existing copy, or to clean up a failed download. Those must
                // NOT forget: an in-place upgrade is the same plugin, and
                // wiping its history would report a long-serving plugin as
                // brand new. Only removal-by-intent qualifies, and intent is
                // legible only in the name.
                let signature = chunk.lines().next().unwrap_or("");
                if !signature.contains("uninstall") && !signature.contains("remove_plugin") {
                    continue;
                }
                checked += 1;
                // Must go through the chokepoint, not a bare `forget_plugin`:
                // a plugin owns more than one sidecar row now (its usage
                // record *and* its durable activation preference in
                // `plugins.toml`), and a site that remembers one and forgets
                // the other fails silently in both directions — a same-id
                // reinstall either inherits `enabled = false`, or arrives
                // looking like a long-serving idle plugin.
                if !chunk.contains("forget_plugin_sidecars") {
                    offenders.push(format!(
                        "{}: {}",
                        file.file_name().unwrap_or_default().to_string_lossy(),
                        chunk.lines().next().unwrap_or("?").trim()
                    ));
                }
            }
        }

        assert!(
            checked >= 3,
            "expected at least the three known plugin removal sites; found \
             {checked} — the scan stopped matching, so its green means nothing"
        );
        assert!(
            offenders.is_empty(),
            "these delete a plugin's directory without going through \
             `extension::plugin_state::forget_plugin_sidecars`, so a reinstall \
             under the same id inherits the old call counts, idle age, or \
             disabled flag: {offenders:?}"
        );
    }

    #[test]
    fn forget_drops_the_row() {
        let (_tmp, store) = store();
        store.record_call("plugin:gone", "t", true);
        assert!(store.get("plugin:gone").is_some());
        store.forget("plugin:gone");
        assert!(store.get("plugin:gone").is_none());
    }

    #[test]
    fn corrupted_sidecar_resets_instead_of_breaking_the_call() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tool_usage.json");
        std::fs::write(&path, b"{not json").unwrap();
        let store = ToolUsageStore::at(&path);
        assert!(store.snapshot().is_empty());
        store.record_call("mcp:a", "t", true);
        assert_eq!(store.get("mcp:a").unwrap().call_count, 1);
    }

    #[test]
    fn older_sidecar_without_the_newer_fields_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tool_usage.json");
        std::fs::write(&path, br#"{"mcp:legacy":{"call_count":4}}"#).unwrap();
        let store = ToolUsageStore::at(&path);
        let usage = store.get("mcp:legacy").unwrap();
        assert_eq!(usage.call_count, 4);
        assert_eq!(usage.error_count, 0);
        assert!(usage.tools.is_empty());
        assert!(usage.last_used_at.is_none());
    }

    /// The file lock has to serialize read-modify-write across threads (and,
    /// by the same mechanism, across processes) or parallel tool dispatch
    /// silently under-counts.
    #[test]
    fn concurrent_records_do_not_lose_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tool_usage.json");
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let store = ToolUsageStore::at(&path);
                barrier.wait();
                for _ in 0..25 {
                    store.record_call("mcp:hot", "t", true);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let usage = ToolUsageStore::at(&path).get("mcp:hot").unwrap();
        assert_eq!(usage.call_count, 100);
        assert_eq!(usage.tools.get("t"), Some(&100));
    }
}
