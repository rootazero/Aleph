//! Per-profile tab lifecycle tracking — idle reclamation + tab-count cap.
//!
//! openclaw caps tabs-per-session and reaps idle tabs via LRU
//! (`session-tab-registry.ts`). Aleph's [`ProfileManager`](super::manager::ProfileManager)
//! already reaps idle *profiles* (whole browser sessions); this adds the
//! finer-grained *tab* layer.
//!
//! Scope is deliberately limited to **Managed** profiles — headless browsers
//! Aleph launches and fully owns. `ExistingSession` profiles attach to the
//! user's real Chrome, so their tabs are never tracked or reaped here (R5:
//! don't disturb the user — closing a tab the user is looking at would be hostile).
//!
//! Design notes:
//! - **Pure bookkeeping.** This module never touches a browser. It tracks
//!   last-used timestamps and *selects* which tabs should close; the caller
//!   (the reaper) does the actual `close_tab` and calls [`TabRegistry::forget`].
//! - **Reconcile against truth.** The Managed backend's `open_tab` returns a
//!   `"last"` sentinel rather than a concrete id, so the registry never trusts
//!   open-time ids. [`TabRegistry::select_victims`] reconciles against the live
//!   tab list from `list_tabs` every sweep — stale entries are dropped, newly
//!   seen tabs are aged from first sight.
//! - **Active tab protected.** The single most-recently-used live tab is never
//!   a victim, so the agent's current page is never closed out from under it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::sync_primitives::Mutex;

/// Default ceiling on concurrently-tracked tabs per Managed profile. A runaway
/// agent loop that opens tabs without closing them is capped here; the
/// least-recently-used tabs beyond the cap are closed on the next sweep.
/// (openclaw `DEFAULT_BROWSER_TAB_CLEANUP_MAX_TABS_PER_SESSION = 8`.)
pub const DEFAULT_MAX_TABS_PER_PROFILE: usize = 8;

/// Default per-tab idle timeout (seconds). Shorter than the profile-level idle
/// timeout (1800s) — an unused tab is cheap to reopen, so reclaim it sooner.
pub const DEFAULT_TAB_IDLE_TIMEOUT_SECS: u64 = 600;

struct Tracked {
    last_used: Instant,
}

/// Tracks last-used time per (profile, `tab_id`) so idle / over-cap tabs can be
/// reclaimed. Cheap to share behind the `ProfileManager`'s `Arc`.
#[derive(Default)]
pub struct TabRegistry {
    /// profile → (`tab_id` → last-used).
    tabs: Mutex<HashMap<String, HashMap<String, Tracked>>>,
}

impl TabRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record activity on a tab, resetting its idle timer. Creates the entry if
    /// it is the first time the tab is seen.
    pub fn touch(&self, profile: &str, tab_id: &str) {
        let mut map = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(profile.to_string()).or_default().insert(
            tab_id.to_string(),
            Tracked {
                last_used: Instant::now(),
            },
        );
    }

    /// Forget a tab after it has been closed (or is gone from the live list).
    pub fn forget(&self, profile: &str, tab_id: &str) {
        let mut map = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tabs) = map.get_mut(profile) {
            tabs.remove(tab_id);
        }
    }

    /// Drop all tracking for a profile (e.g. its browser is gone). Stops the
    /// reaper from re-probing a dead profile every sweep.
    pub fn clear_profile(&self, profile: &str) {
        let mut map = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(profile);
    }

    /// Whether any tabs are tracked for a profile — lets the reaper skip
    /// profiles whose browser was never used (avoids spawning a `list_tabs`
    /// round-trip for nothing).
    pub fn has_tabs(&self, profile: &str) -> bool {
        let map = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        map.get(profile).is_some_and(|t| !t.is_empty())
    }

    /// Reconcile the registry against the authoritative live tab list and
    /// return the tab ids that should be closed.
    ///
    /// - Entries for tabs no longer live are dropped.
    /// - Newly-seen live tabs are tracked as of now (so a tab the agent opened
    ///   but never re-touched still ages from first sight).
    /// - The single most-recently-used live tab is always protected.
    /// - Victims = tabs idle ≥ `idle_timeout` ∪ the LRU overflow beyond
    ///   `max_tabs`, minus the protected tab.
    ///
    /// Pure: the caller closes the returned ids and calls [`Self::forget`].
    pub fn select_victims(
        &self,
        profile: &str,
        live_ids: &[String],
        max_tabs: usize,
        idle_timeout: Duration,
    ) -> Vec<String> {
        let max_tabs = max_tabs.max(1);
        let now = Instant::now();
        let mut map = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        let tabs = map.entry(profile.to_string()).or_default();

        // Drop entries whose tab is gone; track newly-seen live tabs as of now.
        tabs.retain(|id, _| live_ids.contains(id));
        for id in live_ids {
            tabs.entry(id.clone()).or_insert(Tracked { last_used: now });
        }

        // Never reap when ≤1 live tab — the agent always keeps a page.
        if live_ids.len() <= 1 {
            return Vec::new();
        }

        // Order live tabs LRU-first (ascending last_used). The last entry is the
        // most-recently-used → protected.
        let mut ordered: Vec<(String, Instant)> = tabs
            .iter()
            .map(|(id, t)| (id.clone(), t.last_used))
            .collect();
        ordered.sort_by_key(|(_, t)| *t);
        let protected: Option<&String> = ordered.last().map(|(id, _)| id);

        let over_cap = ordered.len().saturating_sub(max_tabs);
        let mut victims = Vec::new();
        for (idx, (id, last_used)) in ordered.iter().enumerate() {
            if protected == Some(id) {
                continue;
            }
            let idle = now.saturating_duration_since(*last_used) >= idle_timeout;
            let over = idx < over_cap; // LRU overflow lives at the low indices
            if idle || over {
                victims.push(id.clone());
            }
        }
        victims
    }
}

/// Extract numeric tab ids from a backend `list_tabs` listing.
///
/// Handles both the Chrome `DevTools` MCP format `"N: URL"` and the Playwright
/// CLI format `"Tab N: URL"`. Lower-layer twin of the `(id, url)` parser in the
/// browser-tools layer — the reaper lives in the `browser` crate layer and may
/// not reach up into `builtin_tools`, and it only needs the ids.
#[must_use]
pub fn parse_tab_ids(tabs_text: &str) -> Vec<String> {
    tabs_text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("Tab ").unwrap_or(line);
            let colon = rest.find(": ")?;
            let id = rest.get(..colon)?.trim();
            if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                Some(id.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_tab_ids_handles_both_formats() {
        let text = "1: https://a.com\nTab 2: https://b.com [selected]\nnoise\nTab x: bad";
        assert_eq!(parse_tab_ids(text), ids(&["1", "2"]));
        assert!(parse_tab_ids("").is_empty());
    }

    #[test]
    fn never_reaps_when_one_or_zero_live_tabs() {
        let reg = TabRegistry::new();
        reg.touch("p", "1");
        assert!(reg
            .select_victims("p", &ids(&["1"]), 8, Duration::from_secs(0))
            .is_empty());
        assert!(reg
            .select_victims("p", &[], 8, Duration::from_secs(0))
            .is_empty());
    }

    #[test]
    fn protects_most_recently_used_tab() {
        let reg = TabRegistry::new();
        // Two tabs, both idle (timeout 0), but the active one is protected.
        reg.touch("p", "1");
        reg.touch("p", "2"); // touched last → most-recently-used
        let victims = reg.select_victims("p", &ids(&["1", "2"]), 8, Duration::from_secs(0));
        assert_eq!(victims, ids(&["1"]));
    }

    #[test]
    fn enforces_cap_via_lru() {
        let reg = TabRegistry::new();
        // Touch in order 1,2,3 → 1 is LRU. Cap of 2 with a long idle timeout
        // closes exactly the single LRU overflow tab (3 is protected, active).
        reg.touch("p", "1");
        reg.touch("p", "2");
        reg.touch("p", "3");
        let victims = reg.select_victims("p", &ids(&["1", "2", "3"]), 2, Duration::from_secs(3600));
        assert_eq!(victims, ids(&["1"]));
    }

    #[test]
    fn drops_stale_entries_and_tracks_new_ones() {
        let reg = TabRegistry::new();
        reg.touch("p", "1");
        reg.touch("p", "2");
        // Live list no longer has "1" (closed elsewhere) but has a new "9".
        // No victims (long timeout, under cap) but the registry reconciles.
        let victims = reg.select_victims("p", &ids(&["2", "9"]), 8, Duration::from_secs(3600));
        assert!(victims.is_empty());
        reg.forget("p", "2");
        assert!(reg.has_tabs("p")); // "9" still tracked
        reg.forget("p", "9");
        assert!(!reg.has_tabs("p"));
    }

    #[test]
    fn no_victims_when_under_cap_and_fresh() {
        let reg = TabRegistry::new();
        reg.touch("p", "1");
        reg.touch("p", "2");
        let victims = reg.select_victims("p", &ids(&["1", "2"]), 8, Duration::from_secs(3600));
        assert!(victims.is_empty());
    }
}
