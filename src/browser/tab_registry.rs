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

/// One parsed line of a backend `list_tabs` listing.
///
/// `selected` carries the driver's own answer to "which tab is active" — the
/// `" [selected]"` annotation both drivers append. It used to be parsed and
/// thrown away, which forced every caller to guess "active = last-listed"; a
/// `switch_tab` falsifies that guess, and the guess is what the post-navigation
/// audit and the read-time SSRF re-check run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabLine {
    pub id: String,
    pub url: String,
    /// The listing marked this line as the driver's currently selected tab.
    pub selected: bool,
}

/// Split a playwright-cli markdown tab rendering `"[title](url)"` into its
/// URL. Returns `None` for anything that is not that shape, which is how the
/// caller tells the two drivers' renderings apart.
///
/// Splits on the LAST `"]("` so a title containing brackets still resolves;
/// a URL containing `"]("` would not, and no real one does.
fn markdown_link_url(s: &str) -> Option<&str> {
    let inner = s.strip_prefix('[')?.strip_suffix(')')?;
    let pos = inner.rfind("](")?;
    inner.get(pos + 2..)
}

/// Parse one `list_tabs` line.
///
/// Two renderings reach here, and BOTH are transcribed from live output rather
/// than described from memory — the previous description ("the Playwright CLI
/// format `Tab N: URL`") named a format no driver emits, so every real
/// playwright listing parsed to nothing:
///
/// - chrome-devtools-mcp `list_pages`: `"1: about:blank [selected]"` — a bare
///   URL with an optional trailing ` [selected]` annotation.
/// - `playwright-cli tab-list` (0.1.8): `"- 1: (current) [Title](https://x/)"`
///   — a `- ` bullet, a markdown link, and the selection marked by a leading
///   `(current)` rather than a trailing annotation.
///
/// `"Tab N: URL"` is still tolerated; it has no known emitter and is kept only
/// because tolerating it costs one `strip_prefix`.
///
/// Returns `None` for lines without a numeric id (headers such as
/// `"### Result"` and `"## Pages"` fall out here).
///
/// This is the lower-layer twin of the tab-line question; the browser-tools
/// layer no longer keeps its own copy and calls [`active_tab_id`] instead.
#[must_use]
pub fn parse_tab_line(line: &str) -> Option<TabLine> {
    let line = line.trim();
    // Normalize the two id prefixes ("- N: …" / "Tab N: …") to "N: …" so one
    // parser serves both drivers.
    let rest = line.strip_prefix("- ").unwrap_or(line);
    let rest = rest.strip_prefix("Tab ").unwrap_or(rest);
    let colon = rest.find(": ")?;
    let id = rest.get(..colon)?.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let url_part = rest.get(colon + 2..)?.trim();

    // playwright-cli marks the selected line with a LEADING "(current)".
    let (url_part, current_marker) = match url_part.strip_prefix("(current)") {
        Some(rest) => (rest.trim(), true),
        None => (url_part, false),
    };

    // playwright-cli renders the tab as a markdown link; when it does, the URL
    // is unambiguous and there is no trailing annotation to split.
    if let Some(url) = markdown_link_url(url_part) {
        return Some(TabLine {
            id: id.to_string(),
            url: url.to_string(),
            selected: current_marker,
        });
    }

    // chrome-devtools-mcp: bare URL, with a trailing " [selected]" / " [active]"
    // annotation split off so the URL round-trips through a strict parser AND
    // the marker survives.
    let (url, annotation) = match url_part.rfind(" [") {
        Some(pos) if url_part.ends_with(']') => (
            url_part.get(..pos).unwrap_or(url_part).trim(),
            url_part.get(pos..).unwrap_or("").trim(),
        ),
        _ => (url_part, ""),
    };
    let marker = annotation.to_ascii_lowercase();
    Some(TabLine {
        id: id.to_string(),
        url: url.to_string(),
        selected: current_marker || marker.contains("selected") || marker.contains("active"),
    })
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
        .filter_map(parse_tab_line)
        .map(|t| t.id)
        .collect()
}

/// The active tab of a listing — **the single source for that question**.
///
/// Prefers the driver's explicit `[selected]` marker and falls back to the
/// last-listed line only when the listing carries no marker at all (newly
/// opened tabs append, so "last" is the right guess for a listing that cannot
/// answer). The distinction matters for correctness, not cosmetics: the
/// post-navigation audit and the read-time SSRF re-check must vet the very tab
/// whose content is then read, and after a `switch_tab` the last-listed tab is
/// not that tab.
#[must_use]
pub fn active_tab(tabs_text: &str) -> Option<TabLine> {
    let mut last = None;
    for line in tabs_text.lines() {
        let Some(tab) = parse_tab_line(line) else {
            continue;
        };
        if tab.selected {
            return Some(tab);
        }
        last = Some(tab);
    }
    last
}

/// The active tab's id — see [`active_tab`].
#[must_use]
pub fn active_tab_id(tabs_text: &str) -> Option<String> {
    active_tab(tabs_text).map(|t| t.id)
}

/// The active tab's URL — see [`active_tab`].
pub(crate) fn active_tab_url(tabs_text: &str) -> Option<String> {
    active_tab(tabs_text).map(|t| t.url)
}

/// The current URL of `tab_id` as reported by `list_tabs`, if present.
pub(crate) fn tab_url_for(tabs_text: &str, tab_id: &str) -> Option<String> {
    tabs_text
        .lines()
        .filter_map(parse_tab_line)
        .rfind(|t| t.id == tab_id)
        .map(|t| t.url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The real `playwright-cli tab-list` listing, copied verbatim from
    /// `playwright-cli 0.1.8` (`- <id>: [<title>](<url>)`, with the selected
    /// line prefixed `(current) `). Every line of it used to parse to `None`:
    /// the pre-colon segment is `- 0`, which is not all-digits, so the whole
    /// listing yielded no tabs at all.
    ///
    /// That failure is silent and it is not cosmetic — with no parsed lines
    /// [`active_tab_id`] returns `None`, so the tab id falls back to the
    /// `"last"` sentinel and `post_nav::audit_listing` runs over an empty
    /// listing, i.e. the post-navigation SSRF audit passes having checked
    /// nothing.
    #[test]
    fn parse_reads_the_real_playwright_cli_listing() {
        let text = "### Result\n                    - 0: [](about:blank)\n                    - 1: (current) [Example Domain](https://example.com/)";
        assert_eq!(parse_tab_ids(text), ids(&["0", "1"]));
        // The `(current)` marker is the driver's own answer to "which tab is
        // active"; without it this listing would fall back to last-listed and
        // only accidentally agree.
        assert_eq!(active_tab_id(text).as_deref(), Some("1"));
        assert_eq!(
            active_tab_url(text).as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            tab_url_for(text, "0").as_deref(),
            Some("about:blank"),
            "the non-selected line must resolve too"
        );
    }

    /// The `(current)` marker really is load-bearing: when it names a line
    /// that is NOT last, the parser must follow the marker.
    #[test]
    fn the_playwright_current_marker_beats_last_listed() {
        let text = "- 0: (current) [A](https://a.com/)\n- 1: [B](https://b.com/)";
        assert_eq!(active_tab_id(text).as_deref(), Some("0"));
        assert_eq!(active_tab_url(text).as_deref(), Some("https://a.com/"));
    }

    /// The chrome-devtools-mcp `list_pages` listing, copied verbatim from a
    /// live session. Kept alongside the playwright case so one parser is
    /// proven against BOTH drivers' real output rather than against a format
    /// that was only ever written down here.
    #[test]
    fn parse_reads_the_real_chrome_devtools_mcp_listing() {
        let text = "## Pages\n1: about:blank [selected]";
        assert_eq!(parse_tab_ids(text), ids(&["1"]));
        assert_eq!(active_tab_id(text).as_deref(), Some("1"));
        assert_eq!(active_tab_url(text).as_deref(), Some("about:blank"));
    }

    #[test]
    fn parse_tab_ids_handles_both_formats() {
        let text = "1: https://a.com\nTab 2: https://b.com [selected]\nnoise\nTab x: bad";
        assert_eq!(parse_tab_ids(text), ids(&["1", "2"]));
        assert!(parse_tab_ids("").is_empty());
    }

    #[test]
    fn active_tab_prefers_the_selected_marker_over_the_last_line() {
        // The marker is the driver's own answer; "last-listed" is only the
        // fallback for a listing that carries no marker.
        let text = "1: https://a.com [selected]\nTab 2: http://10.0.0.1/x";
        assert_eq!(active_tab_id(text).as_deref(), Some("1"));
        assert_eq!(active_tab_url(text).as_deref(), Some("https://a.com"));
        // …and the URL still has the annotation stripped when the marked tab
        // is the annotated one.
        let text = "1: https://a.com\nTab 2: http://10.0.0.1/x [selected]";
        assert_eq!(active_tab_id(text).as_deref(), Some("2"));
        assert_eq!(active_tab_url(text).as_deref(), Some("http://10.0.0.1/x"));
    }

    #[test]
    fn active_tab_falls_back_to_last_listed_without_a_marker() {
        let text = "1: https://a.com\nTab 2: https://b.com";
        assert_eq!(active_tab_id(text).as_deref(), Some("2"));
        assert_eq!(active_tab_url(text).as_deref(), Some("https://b.com"));
        assert_eq!(active_tab_id(""), None);
        assert_eq!(active_tab_url("noise only"), None);
    }

    #[test]
    fn parse_tab_line_reports_the_selection_marker() {
        let plain = parse_tab_line("1: https://a.com").unwrap();
        assert!(!plain.selected);
        assert_eq!(plain.url, "https://a.com");
        let marked = parse_tab_line("Tab 2: https://b.com [selected]").unwrap();
        assert!(marked.selected);
        assert_eq!(marked.url, "https://b.com");
        // An unrelated bracket annotation is not a selection claim.
        let other = parse_tab_line("3: https://c.com [background]").unwrap();
        assert!(!other.selected);
        assert_eq!(other.url, "https://c.com");
    }

    #[test]
    fn tab_url_for_matches_id() {
        let text = "1: https://a.com\n2: http://10.0.0.1/x [selected]";
        assert_eq!(tab_url_for(text, "1").as_deref(), Some("https://a.com"));
        assert_eq!(tab_url_for(text, "2").as_deref(), Some("http://10.0.0.1/x"));
        assert_eq!(tab_url_for(text, "9"), None);
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
