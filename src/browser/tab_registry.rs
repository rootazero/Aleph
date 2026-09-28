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

use crate::sync_primitives::{Mutex, RwLock};

use super::error::BrowserError;

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

/// What Aleph last knew about one tab's IDENTITY, as opposed to its
/// activity ([`Tracked`]) — the two answer different questions and a tab can
/// be idle-but-present or fresh-but-gone.
///
/// Both fields are `Option` because the discovery points know different
/// halves. The cdp backend's tab id IS the CDP `targetId` (the browser's own
/// name for the page — `cdp_backend/tabs.rs`'s module doc), so it records
/// `target_id: Some(..)`; the playwright-cli / chrome-devtools-mcp drivers
/// discover tabs as listing ROWS and never learn a targetId, so they record
/// `target_id: None` — and for them [`TabRegistry::resolve_identity`] can
/// never claim `TabGone`, because there is no target to check a live
/// enumeration against (判据 §8: "I cannot tell" is not a verdict).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabIdentity {
    pub target_id: Option<String>,
    pub last_url: Option<String>,
}

/// Tracks last-used time per (profile, `tab_id`) so idle / over-cap tabs can be
/// reclaimed. Cheap to share behind the `ProfileManager`'s `Arc`.
///
/// Since the B3 round it is also the identity registry: which `targetId` and
/// which last-seen URL each (profile, `tab_id`) maps to. Identity is what
/// survives to answer "is the tab I mean still the tab I mean" — the
/// question a listing's row ORDER was measured unable to answer across a
/// re-attach (附录 D.9.19). One registry holds both because the two share a
/// lifecycle: [`Self::forget`] (a deliberate close) and
/// [`Self::clear_profile`] (the browser is gone) drop both halves together.
#[derive(Default)]
pub struct TabRegistry {
    /// profile → (`tab_id` → last-used).
    tabs: Mutex<HashMap<String, HashMap<String, Tracked>>>,
    /// profile → (`tab_id` → identity). Reads (`resolve_identity`,
    /// `last_url`) outnumber writes (discovery points), hence the RwLock.
    identities: RwLock<HashMap<String, HashMap<String, TabIdentity>>>,
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
    ///
    /// Drops the IDENTITY too: a tab Aleph deliberately closed must answer
    /// [`BrowserError::TabNotFound`] afterwards, not `TabGone` — `TabGone` is
    /// reserved for "gone WITHOUT us closing it" (the page's own
    /// `window.close`, an engine restart), because the model did not do it
    /// and needs to be told.
    pub fn forget(&self, profile: &str, tab_id: &str) {
        let mut map = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tabs) = map.get_mut(profile) {
            tabs.remove(tab_id);
        }
        let mut ids = self.identities.write().unwrap_or_else(|e| e.into_inner());
        if let Some(tabs) = ids.get_mut(profile) {
            tabs.remove(tab_id);
        }
    }

    /// Drop all tracking for a profile (e.g. its browser is gone). Stops the
    /// reaper from re-probing a dead profile every sweep.
    pub fn clear_profile(&self, profile: &str) {
        let mut map = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(profile);
        let mut ids = self.identities.write().unwrap_or_else(|e| e.into_inner());
        ids.remove(profile);
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

    /// Record what a discovery point learned about a tab's identity. Every
    /// place that learns "tab X has targetId Y / is at URL Z" calls this —
    /// for the cdp backend that is `open_tab` / `switch_tab` / `list_tabs` /
    /// `navigate`, and the manager's reaper sweep covers the old drivers.
    ///
    /// **Upsert with merge, not replace**: each discovery point knows a
    /// different half (the backend knows the targetId; a `list_tabs` sweep
    /// knows the URL), so a `None` field KEEPS the previously recorded value
    /// and a `Some` field overwrites it. A sweep's URL must not erase the
    /// backend's targetId, or the very next `resolve_identity` loses its
    /// ability to call `TabGone`.
    pub fn record_identity(
        &self,
        profile: &str,
        tab_id: &str,
        target_id: Option<String>,
        url: Option<String>,
    ) {
        let mut map = self.identities.write().unwrap_or_else(|e| e.into_inner());
        let entry = map
            .entry(profile.to_string())
            .or_default()
            .entry(tab_id.to_string())
            .or_default();
        if let Some(t) = target_id {
            entry.target_id = Some(t);
        }
        if let Some(u) = url {
            entry.last_url = Some(u);
        }
    }

    /// Answer "is the tab I mean still the tab I mean" from the recorded
    /// identity and a FRESH target enumeration.
    ///
    /// Three answers, three different facts:
    ///
    /// - never recorded → [`BrowserError::TabNotFound`] ("I don't know this
    ///   id" — 判据 §8, not a verdict about the browser);
    /// - recorded WITH a targetId that `live_target_ids` no longer carries →
    ///   [`BrowserError::TabGone`], with the last recorded URL so the reader
    ///   can recognise which page vanished;
    /// - anything else (target live, or no targetId was ever recorded — the
    ///   old drivers' shape, for which gone-ness is unknowable here) →
    ///   `Ok` with what was recorded.
    ///
    /// The `targetId` check, not a listing's row order, is the arbiter:
    /// enumeration order was measured to permute across a re-attach in the
    /// same run (附录 D.9.19), so a position-derived answer is a guess this
    /// function exists to NOT make.
    pub fn resolve_identity(
        &self,
        profile: &str,
        tab_id: &str,
        live_target_ids: &[String],
    ) -> Result<TabIdentity, BrowserError> {
        let map = self.identities.read().unwrap_or_else(|e| e.into_inner());
        let Some(identity) = map.get(profile).and_then(|tabs| tabs.get(tab_id)) else {
            return Err(BrowserError::TabNotFound(tab_id.to_string()));
        };
        match &identity.target_id {
            Some(target) if !live_target_ids.contains(target) => Err(BrowserError::TabGone {
                tab_id: tab_id.to_string(),
                last_url: identity.last_url.clone(),
            }),
            _ => Ok(identity.clone()),
        }
    }

    /// The last URL recorded for this tab, if any — the URL-drift input the
    /// ref pre-dispatch check reads (a ref minted against `last_url` that no
    /// longer matches the tab's current URL is the `StaleReason::Navigated`
    /// case made visible one layer up).
    pub fn last_url(&self, profile: &str, tab_id: &str) -> Option<String> {
        let map = self.identities.read().unwrap_or_else(|e| e.into_inner());
        map.get(profile)?.get(tab_id)?.last_url.clone()
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
/// This is the ONE parser for both drivers' listings, and it is private: the
/// only callers are [`parse_tab_lines`] below and, through it, the two text
/// backends. Everything downstream of a backend now works on `&[TabLine]`,
/// so no other layer can grow a second reading of a tab line — which is what
/// the tool layer had, answering "active" with `.next_back()` while
/// `browser_tabs {switch}` falsified it.
#[must_use]
fn parse_tab_line(line: &str) -> Option<TabLine> {
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

/// Every addressable row of a backend listing, in listing order.
///
/// The seam between "a driver printed some text" and "Aleph has tabs". Lines
/// that carry no numeric id (`"### Result"`, `"## Pages"`, a `no pages open`
/// notice) are not rows and fall out here — which makes an unreadable listing
/// an EMPTY answer, never a fabricated one. Empty means "no tab I can
/// address", and every caller must read it that way (判据 §8): the
/// post-navigation audit then skips with a log line rather than vetting a URL
/// it invented.
#[must_use]
pub(crate) fn parse_tab_lines(text: &str) -> Vec<TabLine> {
    text.lines().filter_map(parse_tab_line).collect()
}

/// The ids of a listing, in order.
///
/// Renamed from `parse_tab_ids`: after the trait change it parses nothing, and
/// a name that says "parse" over a projection is a comment that lies about the
/// code beneath it.
#[must_use]
pub(crate) fn tab_ids(tabs: &[TabLine]) -> Vec<String> {
    tabs.iter().map(|t| t.id.clone()).collect()
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
///
/// ⚠️ **Known exception, measured on real hardware (browser-live-view plan 1,
/// round 2):** this "last-listed is the right guess" fallback does NOT hold
/// across a `close`/re-attach cycle under `attach --cdp`. A fresh attach
/// session's listing DOES carry the driver's own marker (so the fallback
/// itself is not even reached in that case) — but that marker was observed to
/// name the WRONG tab: the CLI's own idea of "current" after a fresh attach
/// is drawn from CDP's target enumeration, not inherited from before the
/// disconnect, and that enumeration's order was measured to differ between
/// the first attach and the re-attach in the same run (the launch's own
/// `about:blank` and the profile's actual page traded places). Picking
/// "last-listed" as an override in that situation was tried and picked the
/// wrong tab too. Recovering the right tab across a re-attach needs a
/// persistent record kept ONE level up (`ProfileManager::tab_registry` in
/// `manager.rs`), not a smarter read of any single listing — see
/// `docs/reference/FEATURE_LOCATOR.md` §3.12 (附录 D.9.19) and
/// `qa/README.md`'s "Known gap: tab identity does not survive a re-attach".
///
/// The signature change to `&[TabLine]` does not touch that gap: the wrong
/// marker is produced by the driver's enumeration, upstream of anything this
/// function can see. Recovering the right tab across a re-attach still needs
/// the persistent record one level up (`ProfileManager::tab_registry`).
#[must_use]
pub fn active_tab(tabs: &[TabLine]) -> Option<&TabLine> {
    let mut last = None;
    for tab in tabs {
        if tab.selected {
            return Some(tab);
        }
        last = Some(tab);
    }
    last
}

/// The active tab's id — see [`active_tab`].
#[must_use]
pub fn active_tab_id(tabs: &[TabLine]) -> Option<String> {
    active_tab(tabs).map(|t| t.id.clone())
}

/// The active tab's URL — see [`active_tab`].
pub(crate) fn active_tab_url(tabs: &[TabLine]) -> Option<String> {
    active_tab(tabs).map(|t| t.url.clone())
}

/// The current URL of `tab_id` in this listing, if present.
///
/// `rfind`, as before: a listing that names one id twice is answered by its
/// last occurrence, which is the one a driver appends.
pub(crate) fn tab_url_for(tabs: &[TabLine], tab_id: &str) -> Option<String> {
    tabs.iter().rfind(|t| t.id == tab_id).map(|t| t.url.clone())
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
        let rows = parse_tab_lines(text);
        assert_eq!(tab_ids(&rows), ids(&["0", "1"]));
        // The `(current)` marker is the driver's own answer to "which tab is
        // active"; without it this listing would fall back to last-listed and
        // only accidentally agree.
        assert_eq!(active_tab_id(&rows).as_deref(), Some("1"));
        assert_eq!(
            active_tab_url(&rows).as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            tab_url_for(&rows, "0").as_deref(),
            Some("about:blank"),
            "the non-selected line must resolve too"
        );
    }

    /// The `(current)` marker really is load-bearing: when it names a line
    /// that is NOT last, the parser must follow the marker.
    #[test]
    fn the_playwright_current_marker_beats_last_listed() {
        let rows = parse_tab_lines("- 0: (current) [A](https://a.com/)\n- 1: [B](https://b.com/)");
        assert_eq!(active_tab_id(&rows).as_deref(), Some("0"));
        assert_eq!(active_tab_url(&rows).as_deref(), Some("https://a.com/"));
    }

    /// The two drivers' real renderings, both through the ONE entry point the
    /// backends now call. Verbatim output, not a description of it: the
    /// previous description named a format no driver emits, and every real
    /// playwright listing parsed to nothing while the tests stayed green.
    #[test]
    fn parse_tab_lines_reads_both_drivers_real_listings() {
        let playwright = "### Result\n                    - 0: [](about:blank)\n                    - 1: (current) [Example Domain](https://example.com/)";
        let rows = parse_tab_lines(playwright);
        assert_eq!(tab_ids(&rows), ids(&["0", "1"]));
        assert_eq!(active_tab_id(&rows).as_deref(), Some("1"));
        assert_eq!(
            active_tab_url(&rows).as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(tab_url_for(&rows, "0").as_deref(), Some("about:blank"));

        let mcp = "## Pages\n1: https://a.com [selected]\n2: https://b.com";
        let rows = parse_tab_lines(mcp);
        assert_eq!(tab_ids(&rows), ids(&["1", "2"]));
        assert_eq!(active_tab_id(&rows).as_deref(), Some("1"));
        assert_eq!(tab_url_for(&rows, "2").as_deref(), Some("https://b.com"));

        // A listing that parses to nothing is an EMPTY answer, never a
        // sentinel row: an empty vec means "no tabs I can address", and the
        // audit's `None` landing URL is what makes it skip rather than vet a
        // URL it invented (判据 §8).
        assert!(parse_tab_lines("### Result\nno pages open").is_empty());
        assert!(parse_tab_lines("").is_empty());
    }

    /// `active_tab` now borrows out of the caller's rows, so the caller can
    /// hold the whole listing and address the same tab it vetted. Identity,
    /// not equality: the returned reference must point INTO the slice.
    #[test]
    fn active_tab_borrows_the_row_it_names_out_of_the_callers_listing() {
        let rows = parse_tab_lines("1: https://a.com\n2: https://b.com [selected]");
        let picked = active_tab(&rows).expect("a marked row");
        assert_eq!(picked.id, "2");
        assert!(
            std::ptr::eq(picked, &rows[1]),
            "active_tab must borrow the caller's row, not clone one"
        );
        // No marker anywhere: last-listed is the documented fallback.
        let unmarked = parse_tab_lines("1: https://a.com\n2: https://b.com");
        assert_eq!(active_tab(&unmarked).map(|t| t.id.as_str()), Some("2"));
        assert!(active_tab(&[]).is_none());
    }

    /// The chrome-devtools-mcp `list_pages` listing, copied verbatim from a
    /// live session. Kept alongside the playwright case so one parser is
    /// proven against BOTH drivers' real output rather than against a format
    /// that was only ever written down here.
    #[test]
    fn parse_reads_the_real_chrome_devtools_mcp_listing() {
        let rows = parse_tab_lines("## Pages\n1: about:blank [selected]");
        assert_eq!(tab_ids(&rows), ids(&["1"]));
        assert_eq!(active_tab_id(&rows).as_deref(), Some("1"));
        assert_eq!(active_tab_url(&rows).as_deref(), Some("about:blank"));
    }

    #[test]
    fn parse_tab_ids_handles_both_formats() {
        let text = "1: https://a.com\nTab 2: https://b.com [selected]\nnoise\nTab x: bad";
        assert_eq!(tab_ids(&parse_tab_lines(text)), ids(&["1", "2"]));
        assert!(tab_ids(&parse_tab_lines("")).is_empty());
    }

    #[test]
    fn active_tab_prefers_the_selected_marker_over_the_last_line() {
        // The marker is the driver's own answer; "last-listed" is only the
        // fallback for a listing that carries no marker.
        let rows = parse_tab_lines("1: https://a.com [selected]\nTab 2: http://10.0.0.1/x");
        assert_eq!(active_tab_id(&rows).as_deref(), Some("1"));
        assert_eq!(active_tab_url(&rows).as_deref(), Some("https://a.com"));
        // …and the URL still has the annotation stripped when the marked tab
        // is the annotated one.
        let rows = parse_tab_lines("1: https://a.com\nTab 2: http://10.0.0.1/x [selected]");
        assert_eq!(active_tab_id(&rows).as_deref(), Some("2"));
        assert_eq!(active_tab_url(&rows).as_deref(), Some("http://10.0.0.1/x"));
    }

    #[test]
    fn active_tab_falls_back_to_last_listed_without_a_marker() {
        let rows = parse_tab_lines("1: https://a.com\nTab 2: https://b.com");
        assert_eq!(active_tab_id(&rows).as_deref(), Some("2"));
        assert_eq!(active_tab_url(&rows).as_deref(), Some("https://b.com"));
        assert_eq!(active_tab_id(&parse_tab_lines("")), None);
        assert_eq!(active_tab_url(&parse_tab_lines("noise only")), None);
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
        let rows = parse_tab_lines("1: https://a.com\n2: http://10.0.0.1/x [selected]");
        assert_eq!(tab_url_for(&rows, "1").as_deref(), Some("https://a.com"));
        assert_eq!(
            tab_url_for(&rows, "2").as_deref(),
            Some("http://10.0.0.1/x")
        );
        assert_eq!(tab_url_for(&rows, "9"), None);
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

    // ---- Identity layer (B3) -------------------------------------------------

    use crate::browser::error::BrowserError;

    /// A recorded tab whose target is absent from the live enumeration is
    /// GONE, structurally — never a "last listed row" guess. That guess was
    /// measured wrong on real hardware (附录 D.9.19: CDP's enumeration order
    /// is not an identity across a re-attach), so the answer comes from the
    /// recorded targetId or it does not come at all.
    #[test]
    fn tab_gone_is_structured_not_a_last_row_guess() {
        let reg = TabRegistry::new();
        reg.record_identity("p", "t1", Some("TARGET-1".into()), Some("https://a/".into()));
        // After re-attach the target is gone; the registry must NOT fall back to
        // "last listed row".
        let err = reg.resolve_identity("p", "t1", &[]).unwrap_err();
        assert!(matches!(err, BrowserError::TabGone { ref tab_id, .. } if tab_id == "t1"));
    }

    /// Review Focus #5: the page closed its own tab (`window.close`) between
    /// `list_tabs` and the action. The live enumeration no longer carries the
    /// target, so the answer is `TabGone` — not a generic action failure, and
    /// not a silently re-pointed one.
    #[test]
    fn page_closed_tab_between_list_and_action_is_tab_gone() {
        let reg = TabRegistry::new();
        reg.record_identity("p", "t1", Some("TARGET-1".into()), None);
        let err = reg
            .resolve_identity("p", "t1", &["TARGET-2".to_string()])
            .unwrap_err();
        assert!(matches!(err, BrowserError::TabGone { .. }));
    }

    /// Never recorded is "I don't know" (判据 §8), not TabGone: the registry
    /// cannot claim a tab it never saw has vanished.
    #[test]
    fn unknown_tab_id_says_so_instead_of_tab_gone() {
        let reg = TabRegistry::new();
        let err = reg.resolve_identity("p", "t9", &["TARGET-2".to_string()]).unwrap_err();
        assert!(matches!(err, BrowserError::TabNotFound(_)));
    }

    /// A recorded target that IS in the live enumeration resolves fine, and
    /// hands back what was recorded.
    #[test]
    fn a_live_target_resolves_to_what_was_recorded() {
        let reg = TabRegistry::new();
        reg.record_identity("p", "t1", Some("TARGET-1".into()), Some("https://a/".into()));
        let id = reg
            .resolve_identity("p", "t1", &["TARGET-1".to_string(), "TARGET-2".to_string()])
            .expect("the target is live");
        assert_eq!(id.target_id.as_deref(), Some("TARGET-1"));
        assert_eq!(id.last_url.as_deref(), Some("https://a/"));
    }

    /// The old drivers discover tabs as listing rows — they never learn a
    /// targetId. For those the registry can never claim TabGone: it holds no
    /// target to check the live enumeration against, and "I cannot tell" is
    /// `Ok` with what was recorded, not a verdict (判据 §8).
    #[test]
    fn a_recorded_identity_without_a_target_id_cannot_be_called_gone() {
        let reg = TabRegistry::new();
        reg.record_identity("p", "3", None, Some("https://b/".into()));
        let id = reg
            .resolve_identity("p", "3", &[])
            .expect("no targetId was ever recorded, so gone-ness is unknowable");
        assert_eq!(id.target_id, None);
        assert_eq!(id.last_url.as_deref(), Some("https://b/"));
    }

    /// Merge, don't clobber: a later recording that knows only the URL (the
    /// manager's sweep over a `list_tabs` listing) must not erase the
    /// targetId the cdp backend recorded, and vice versa.
    #[test]
    fn record_identity_merges_what_each_discovery_point_knows() {
        let reg = TabRegistry::new();
        reg.record_identity("p", "t1", Some("TARGET-1".into()), None);
        reg.record_identity("p", "t1", None, Some("https://a/".into()));
        let id = reg
            .resolve_identity("p", "t1", &["TARGET-1".to_string()])
            .expect("live");
        assert_eq!(id.target_id.as_deref(), Some("TARGET-1"));
        assert_eq!(id.last_url.as_deref(), Some("https://a/"));
        // …and a fresh observation of either half DOES overwrite that half.
        reg.record_identity("p", "t1", Some("TARGET-1".into()), Some("https://b/".into()));
        assert_eq!(reg.last_url("p", "t1").as_deref(), Some("https://b/"));
    }

    /// `last_url` is the URL-drift input T6's ref precheck reads: the latest
    /// recorded observation, or `None` when the tab was never recorded.
    #[test]
    fn last_url_is_the_latest_observation_or_none() {
        let reg = TabRegistry::new();
        assert_eq!(reg.last_url("p", "t1"), None);
        reg.record_identity("p", "t1", Some("TARGET-1".into()), None);
        assert_eq!(reg.last_url("p", "t1"), None);
        reg.record_identity("p", "t1", None, Some("https://a/".into()));
        assert_eq!(reg.last_url("p", "t1").as_deref(), Some("https://a/"));
    }

    /// A tab WE closed is forgotten: asking afterwards is `TabNotFound`, not
    /// `TabGone`. `TabGone` is reserved for "gone without us closing it" —
    /// the page's own `window.close`, an engine restart — because the
    /// recovery differs (the model did not do it, so it needs telling).
    #[test]
    fn a_deliberately_closed_tab_is_forgotten_not_reported_gone() {
        let reg = TabRegistry::new();
        reg.record_identity("p", "t1", Some("TARGET-1".into()), Some("https://a/".into()));
        reg.forget("p", "t1");
        let err = reg.resolve_identity("p", "t1", &[]).unwrap_err();
        assert!(matches!(err, BrowserError::TabNotFound(_)));
        // Same for the profile-wide wipe: the browser itself is gone, and the
        // registry stops claiming it ever knew these tabs.
        reg.record_identity("p", "t2", Some("TARGET-2".into()), None);
        reg.clear_profile("p");
        let err = reg.resolve_identity("p", "t2", &[]).unwrap_err();
        assert!(matches!(err, BrowserError::TabNotFound(_)));
    }
}
