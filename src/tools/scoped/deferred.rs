//! The deferred exposure tier: tools held back from the model's initial tool
//! array, discoverable on demand via `tool_search`, and — crucially —
//! **re-materialized into that array once found**.
//!
//! # The bug this type exists to fix
//!
//! `deferred` used to be a plain `BTreeSet` set once at construction and never
//! mutated. `metadata_schema()` strips deferred names from the array handed to
//! the provider as native tools, and nothing ever put them back. So the model
//! could call `tool_search`, receive a tool's full input schema, be told it was
//! "ready to call" — and then have no native `tool_use` channel to call it
//! through. `[tools] defer_mcp_tools` was not a feature but a trap: the tools it
//! deferred became permanently uncallable.
//!
//! Making the set shrink on discovery is what Claude Code's ToolSearch does, and
//! it keeps the whole mechanism model-initiated: the harness never decides what
//! to expose, it only honours what the model went looking for.
//!
//! # R7 / R10
//!
//! The partition is static (MCP vs builtin — a source, not a meaning) and every
//! un-defer is caused by a `tool_search` call the MODEL chose to make. Nothing
//! reads the user's message. This lives in the tool-presentation layer
//! (`src/tools/scoped/`), not in `src/harness/` — zero harness budget.
//!
//! # Ordering invariant
//!
//! The set only ever SHRINKS within a run. A tool that materializes stays
//! materialized, so the tools array cannot oscillate between turns (which would
//! thrash the prompt cache it rides in). The one-time invalidation on discovery
//! is the price of the tool becoming callable at all — and it is paid only on
//! the turn the model asks.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;

use crate::sync_primitives::Arc;

/// Shared, interior-mutable deferred-tool set.
///
/// Held by both `ScopedToolService` (which filters on it) and `ToolSearchTool`
/// (which shrinks it). They share an `Arc` rather than pointing at each other:
/// the service owns the registry that owns the tool, so a back-reference would
/// be an `Arc` cycle and a leak.
#[derive(Debug, Default)]
pub struct DeferredTools {
    names: ArcSwap<BTreeSet<String>>,
    /// Bumped on every mutation. `ScopedToolService::metadata_schema` folds this
    /// into its cache key exactly as it already does for the tool-health
    /// generation, so a discovered tool appears in the next turn's array instead
    /// of being masked by a stale cached schema.
    generation: AtomicU64,
}

impl DeferredTools {
    /// An empty tier — `defer_mcp_tools` off. Every predicate short-circuits.
    #[must_use]
    pub fn empty() -> Arc<Self> {
        Arc::new(Self::default())
    }

    #[must_use]
    pub fn new(names: BTreeSet<String>) -> Arc<Self> {
        Arc::new(Self {
            names: ArcSwap::from_pointee(names),
            generation: AtomicU64::new(0),
        })
    }

    #[must_use]
    pub fn is_deferred(&self, name: &str) -> bool {
        self.names.load().contains(name)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.load().is_empty()
    }

    /// Snapshot of the currently-deferred names.
    #[must_use]
    pub fn snapshot(&self) -> std::sync::Arc<BTreeSet<String>> {
        self.names.load_full()
    }

    /// Monotonic mutation counter — the cache key ingredient.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Promote `names` out of the deferred tier so they re-enter the model's
    /// tool array on the next turn. Called by `tool_search` with exactly the
    /// tools it just handed the model a schema for.
    ///
    /// A no-op (no generation bump, no cache invalidation) when none of `names`
    /// was actually deferred — so a `tool_search` that only matches resident
    /// tools costs the prompt cache nothing.
    pub fn undefer(&self, names: &[String]) {
        // Fast path: nothing to promote — no store, no generation bump.
        if names.iter().all(|n| !self.names.load().contains(n)) {
            return;
        }
        // `rcu` retries the clone-and-store on concurrent modification, so two
        // PARALLEL `tool_search` promotions merge instead of the last store
        // silently reinstating a name the other just removed (`tool_search`
        // declares itself concurrent-safe, so same-batch promotions are real).
        // The set only ever shrinks, so the retry loop trivially terminates.
        let prev = self.names.rcu(|current| {
            let mut next = (**current).clone();
            for name in names {
                next.remove(name);
            }
            next
        });
        // Bump only if THIS call actually removed something (rcu returns the
        // set it replaced); a racing promotion of disjoint names bumps for
        // itself.
        if names.iter().any(|n| prev.contains(n)) {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    /// The headline fix: a discovered tool stops being deferred, so the next
    /// `metadata_schema()` puts it back in the model's callable tool array.
    #[test]
    fn undefer_promotes_the_named_tools() {
        let d = DeferredTools::new(set(&["mcp:slack:post", "mcp:jira:create"]));
        assert!(d.is_deferred("mcp:slack:post"));

        d.undefer(&["mcp:slack:post".to_string()]);

        assert!(!d.is_deferred("mcp:slack:post"), "discovered ⇒ callable");
        assert!(d.is_deferred("mcp:jira:create"), "others are untouched");
    }

    /// Discovery must invalidate the schema cache, or the tool stays invisible
    /// to the provider even though it is no longer deferred.
    #[test]
    fn undefer_bumps_the_generation() {
        let d = DeferredTools::new(set(&["mcp:slack:post"]));
        let before = d.generation();
        d.undefer(&["mcp:slack:post".to_string()]);
        assert!(d.generation() > before);
    }

    /// A search that matched only resident tools must not touch the generation —
    /// an unnecessary bump re-keys the tools array and costs a prompt-cache write.
    #[test]
    fn undefer_of_nothing_deferred_is_free() {
        let d = DeferredTools::new(set(&["mcp:slack:post"]));
        let before = d.generation();
        d.undefer(&["bash".to_string(), "read_file".to_string()]);
        assert_eq!(
            d.generation(),
            before,
            "no mutation ⇒ no cache invalidation"
        );
    }

    /// The set only shrinks, so the tools array cannot oscillate across turns.
    #[test]
    fn the_set_only_ever_shrinks() {
        let d = DeferredTools::new(set(&["a", "b"]));
        d.undefer(&["a".to_string()]);
        d.undefer(&["a".to_string()]);
        assert_eq!(d.snapshot().len(), 1);
        assert!(d.is_deferred("b"));
    }
}
