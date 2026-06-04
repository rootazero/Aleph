//! Unified tool-name repair — the single canonical resolver that maps a
//! model-emitted tool name to a canonical offered name.
//!
//! Models routinely emit a tool name that does not byte-match the offered set:
//! case drift (`Read` → `read`), the dot/underscore confusion LLMs introduce
//! between display and wire form (`web.search` ↔ `web_search`), or an outright
//! typo (`web_serch` → `web_search`). Before this module each repair tier lived
//! in a different place and covered a different surface:
//!
//! * `harness::agent::act` repaired **case only**, and only on the native
//!   dispatch path.
//! * `tools::text_tool_call::resolve_name` repaired **dot/underscore only**, and
//!   only on the plain-text promotion path.
//! * The Levenshtein typo logic (`builtin_tools::meta_tools::levenshtein_distance`)
//!   was wired only into command *suggestion* and tool *discovery* — never into
//!   actual dispatch, so a native call carrying a typo'd name bounced straight
//!   to `ToolNotFound`.
//!
//! This is openclaw's `ToolCallRepairNameResolver` abstraction mapped to a pure
//! Rust function: a single tiered resolver that both the native dispatch path
//! and the plain-text promotion path call, so every entry point gets the same
//! robustness from one source of truth.
//!
//! It stays deliberately conservative. Exact match always wins, and every loose
//! tier (case, separator, fuzzy) **abstains on ambiguity** — if two offered
//! tools are equally good candidates the resolver returns `None` and the call
//! falls through to the normal unknown-tool path so the model self-corrects.
//! The fuzzy tier in particular can never mis-route between two similarly-named
//! tools, and because repair only rewrites the *name* the downstream guardrail
//! and approval pipeline still gate the resolved call. R10-safe: this is
//! mechanical identifier repair against a fixed offered set, not intent
//! inference.

/// Which tier produced a resolution. Carried back to the caller for logging so
/// an operator can see *why* a name was rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairTier {
    /// Byte-exact — the emitted name was already an offered tool.
    Exact,
    /// Unique ASCII-case-folded match (`Read` → `read`).
    Case,
    /// Unique `.` ↔ `_` separator swap, case-insensitive
    /// (`web.search` → `web_search`).
    Separator,
    /// Bounded, unambiguous Levenshtein typo repair (`web_serch` → `web_search`).
    Fuzzy,
}

/// A resolved tool name plus the tier that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    /// The canonical offered name to dispatch.
    pub name: String,
    /// How the emitted name resolved to `name`.
    pub tier: RepairTier,
}

/// Minimum emitted-name length before the fuzzy tier engages. Below this, edit
/// distance is too coarse to distinguish a typo from a different tool, so we
/// decline rather than risk a mis-route.
const FUZZY_MIN_LEN: usize = 3;

/// Resolve `emitted` against the `offered` tool names, returning the canonical
/// offered name and the tier that matched, or `None` when nothing resolves
/// safely.
///
/// Tiers are tried in increasing looseness; the first hit wins:
/// 1. **Exact** — `emitted` is already an offered name.
/// 2. **Case** — a *unique* offered name equals `emitted` ignoring ASCII case.
/// 3. **Separator** — a *unique* offered name equals `emitted` once `.`/`_` are
///    normalized (case-insensitive).
/// 4. **Fuzzy** — a *single* offered name is within the edit-distance threshold
///    AND strictly closer than every other candidate.
///
/// A `Repair { tier: Exact, .. }` result means no rewrite is needed; callers
/// should treat it as a no-op.
pub fn repair_tool_name(emitted: &str, offered: &[&str]) -> Option<Repair> {
    if emitted.is_empty() || offered.is_empty() {
        return None;
    }

    // Tier 1: exact.
    if offered.contains(&emitted) {
        return Some(Repair {
            name: emitted.to_string(),
            tier: RepairTier::Exact,
        });
    }

    let lower = emitted.to_ascii_lowercase();

    // Tier 2: case-insensitive, unique.
    if let Some(hit) = unique_match(offered, |o| o.eq_ignore_ascii_case(emitted)) {
        return Some(Repair {
            name: hit,
            tier: RepairTier::Case,
        });
    }

    // Tier 3: separator swap (`.` ↔ `_`), case-insensitive, unique. Only worth
    // attempting when the emitted name actually carries a separator.
    if lower.contains('.') || lower.contains('_') {
        let normalized = normalize_sep(&lower);
        if let Some(hit) =
            unique_match(offered, |o| normalize_sep(&o.to_ascii_lowercase()) == normalized)
        {
            return Some(Repair {
                name: hit,
                tier: RepairTier::Separator,
            });
        }
    }

    // Tier 4: bounded, unambiguous fuzzy typo repair.
    fuzzy_match(&lower, offered).map(|name| Repair {
        name,
        tier: RepairTier::Fuzzy,
    })
}

/// Collapse `.` to `_` so two names that differ only in separator style compare
/// equal. (`web.search` and `web_search` both normalize to `web_search`.)
fn normalize_sep(s: &str) -> String {
    s.replace('.', "_")
}

/// Return the single offered name satisfying `pred`, or `None` when zero or
/// more than one match — abstaining on ambiguity so a loose tier can never
/// silently pick between two equally-valid tools.
fn unique_match(offered: &[&str], pred: impl Fn(&str) -> bool) -> Option<String> {
    let mut hit: Option<&str> = None;
    for &o in offered {
        if pred(o) {
            if hit.is_some() {
                return None; // ambiguous → decline
            }
            hit = Some(o);
        }
    }
    hit.map(str::to_string)
}

/// Find the single closest offered name within the edit-distance threshold,
/// requiring a *strict* margin over the runner-up. Returns `None` when the
/// emitted name is too short to fuzzy-match safely, when nothing falls within
/// threshold, or when the nearest distance is shared by two candidates.
fn fuzzy_match(needle_lower: &str, offered: &[&str]) -> Option<String> {
    use crate::builtin_tools::meta_tools::levenshtein_distance;

    if needle_lower.len() < FUZZY_MIN_LEN {
        return None;
    }
    // Tight thresholds for auto-dispatch repair (tighter than the "did you
    // mean?" suggestion path, which can afford to be permissive): at most one
    // edit for short identifiers, two for longer ones.
    let threshold = if needle_lower.len() <= 6 { 1 } else { 2 };

    let mut best_dist = usize::MAX;
    let mut best_name: Option<&str> = None;
    let mut best_is_unique = false;
    for &o in offered {
        let d = levenshtein_distance(&o.to_ascii_lowercase(), needle_lower);
        if d > threshold {
            continue;
        }
        if d < best_dist {
            best_dist = d;
            best_name = Some(o);
            best_is_unique = true;
        } else if d == best_dist {
            best_is_unique = false; // tie at the current best → ambiguous
        }
    }

    best_name.filter(|_| best_is_unique).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(emitted: &str, offered: &[&str]) -> Option<(String, RepairTier)> {
        repair_tool_name(emitted, offered).map(|r| (r.name, r.tier))
    }

    #[test]
    fn exact_match_is_noop_tier() {
        assert_eq!(
            tier("read", &["read", "write"]),
            Some(("read".into(), RepairTier::Exact))
        );
    }

    #[test]
    fn case_drift_repairs() {
        assert_eq!(
            tier("Read", &["read", "write"]),
            Some(("read".into(), RepairTier::Case))
        );
        assert_eq!(
            tier("WEB_SEARCH", &["web_search"]),
            Some(("web_search".into(), RepairTier::Case))
        );
    }

    #[test]
    fn separator_swap_repairs_both_directions() {
        assert_eq!(
            tier("web.search", &["web_search"]),
            Some(("web_search".into(), RepairTier::Separator))
        );
        assert_eq!(
            tier("file_read", &["file.read"]),
            Some(("file.read".into(), RepairTier::Separator))
        );
    }

    #[test]
    fn separator_and_case_combined() {
        // `Web.Search` → case + separator both differ; normalize handles it.
        assert_eq!(
            tier("Web.Search", &["web_search"]),
            Some(("web_search".into(), RepairTier::Separator))
        );
    }

    #[test]
    fn fuzzy_repairs_single_typo() {
        assert_eq!(
            tier("web_serch", &["web_search", "read_file"]),
            Some(("web_search".into(), RepairTier::Fuzzy))
        );
    }

    #[test]
    fn fuzzy_abstains_on_ambiguous_nearest() {
        // `reaf` is distance 1 from both `read` and `real` → decline.
        assert_eq!(tier("reaf", &["read", "real"]), None);
    }

    #[test]
    fn fuzzy_abstains_below_min_length() {
        // 2-char emitted name never fuzzy-matches even at distance 1.
        assert_eq!(tier("rd", &["read"]), None);
    }

    #[test]
    fn fuzzy_respects_threshold() {
        // Two edits on a 4-char name exceeds the short-name threshold of 1.
        assert_eq!(tier("reet", &["read"]), None);
        // Two edits on a longer name is within the long-name threshold of 2.
        assert_eq!(
            tier("web_serh", &["web_search"]),
            Some(("web_search".into(), RepairTier::Fuzzy))
        );
    }

    #[test]
    fn case_abstains_when_two_offered_collide_on_case() {
        // Pathological: two tools differing only in case → decline rather than
        // guess. (The registry would normally reject such a pair, but the
        // resolver must not mis-route if it ever happens.)
        assert_eq!(tier("read", &["Read", "READ"]), None);
    }

    #[test]
    fn unknown_name_returns_none() {
        assert_eq!(tier("totally_unrelated", &["read", "write"]), None);
    }

    #[test]
    fn empty_inputs_return_none() {
        assert_eq!(tier("", &["read"]), None);
        assert_eq!(tier("read", &[]), None);
    }

    #[test]
    fn exact_wins_over_fuzzy_neighbor() {
        // `read` is offered exactly; even though `real` is one edit away, exact
        // short-circuits.
        assert_eq!(
            tier("read", &["read", "real"]),
            Some(("read".into(), RepairTier::Exact))
        );
    }
}
