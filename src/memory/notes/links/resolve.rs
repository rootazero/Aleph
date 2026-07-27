//! Wikilink resolution strategy chain — pure functions over a prefetched
//! candidate context (mirrors `graph/`'s pure-over-snapshot pattern, P4).
//!
//! Chain: exact path (1.0) → unique exact filename (0.95) → unique exact
//! alias (0.85) → unique normalized filename-or-alias (0.7) → dangling (0.0).
//! Ambiguity (>1 candidates at a tier) NEVER guesses: a wrong link in a
//! personal vault is worse than no link (deliberately more conservative than
//! codebase-memory-mcp's fuzzy tiers). Reference: registry.c strategy chain.

use std::collections::HashMap;

/// Which strategy resolved a link — persisted into `notes_links.resolved_by`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveStrategy {
    ExactPath,
    ExactFilename,
    Alias,
    Normalized,
}

impl ResolveStrategy {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ExactPath => "exact_path",
            Self::ExactFilename => "exact_filename",
            Self::Alias => "alias",
            Self::Normalized => "normalized",
        }
    }

    /// Per-tier confidence (spec §2.3).
    #[must_use]
    pub const fn confidence(&self) -> f32 {
        match self {
            Self::ExactPath => 1.0,
            Self::ExactFilename => 0.95,
            Self::Alias => 0.85,
            Self::Normalized => 0.7,
        }
    }
}

/// Lifecycle status of a `notes_links` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    Active,
    Dangling,
    Tombstone,
}

impl LinkStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Dangling => "dangling",
            Self::Tombstone => "tombstone",
        }
    }

    /// Unknown values fall back to `Active` so a foreign writer cannot make
    /// rows invisible to the graph (P7: fail toward visibility).
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "dangling" => Self::Dangling,
            "tombstone" => Self::Tombstone,
            _ => Self::Active,
        }
    }
}

/// Result of resolving one raw wikilink target.
#[derive(Debug, Clone)]
pub struct ResolvedLink {
    /// `Some("category/filename")` when uniquely resolved; `None` = dangling.
    pub target: Option<String>,
    /// Strategy confidence; 0.0 when dangling.
    pub confidence: f32,
    pub resolved_by: Option<ResolveStrategy>,
}

impl ResolvedLink {
    fn dangling() -> Self {
        Self {
            target: None,
            confidence: 0.0,
            resolved_by: None,
        }
    }

    fn hit(path: &str, strategy: ResolveStrategy) -> Self {
        Self {
            target: Some(path.to_string()),
            confidence: strategy.confidence(),
            resolved_by: Some(strategy),
        }
    }
}

/// Prefetched candidate tables. Built once per store operation from
/// `notes_index` rows; `resolve` then runs with zero I/O.
pub struct LinkResolveContext {
    paths: HashMap<String, ()>,
    filename_to_paths: HashMap<String, Vec<String>>,
    alias_to_paths: HashMap<String, Vec<String>>,
    /// Normalized filename+alias → paths (tier 4). One merged table: a
    /// normalized key hitting both a filename and an alias of DIFFERENT notes
    /// is ambiguous and must dangle.
    normalized_to_paths: HashMap<String, Vec<String>>,
}

impl LinkResolveContext {
    #[must_use]
    pub fn new(entries: Vec<(String, String, Vec<String>)>) -> Self {
        let mut paths = HashMap::new();
        let mut filename_to_paths: HashMap<String, Vec<String>> = HashMap::new();
        let mut alias_to_paths: HashMap<String, Vec<String>> = HashMap::new();
        let mut normalized_to_paths: HashMap<String, Vec<String>> = HashMap::new();
        let push_unique = |m: &mut HashMap<String, Vec<String>>, k: String, p: &str| {
            let v = m.entry(k).or_default();
            if !v.iter().any(|x| x == p) {
                v.push(p.to_string());
            }
        };
        for (path, filename, aliases) in entries {
            push_unique(&mut filename_to_paths, filename.clone(), &path);
            push_unique(
                &mut normalized_to_paths,
                normalize_link_key(&filename),
                &path,
            );
            for a in &aliases {
                push_unique(&mut alias_to_paths, a.clone(), &path);
                push_unique(&mut normalized_to_paths, normalize_link_key(a), &path);
            }
            paths.insert(path, ());
        }
        Self {
            paths,
            filename_to_paths,
            alias_to_paths,
            normalized_to_paths,
        }
    }
}

/// Lowercase + fold full-width ASCII (U+FF01..=U+FF5E and ideographic space
/// U+3000) to half-width + trim. Zero-dep normalization for tier 4.
#[must_use]
pub fn normalize_link_key(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFF00 + 0x20).unwrap_or(c),
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// Run the strategy chain for one raw wikilink target.
#[must_use]
pub fn resolve(raw_target: &str, ctx: &LinkResolveContext) -> ResolvedLink {
    // Tier 1: contains '/' → exact path or dangling (never falls through:
    // a path-form link names one specific note; guessing another is wrong).
    if raw_target.contains('/') {
        if ctx.paths.contains_key(raw_target) {
            return ResolvedLink::hit(raw_target, ResolveStrategy::ExactPath);
        }
        return ResolvedLink::dangling();
    }
    // Tier 2: unique exact filename.
    match ctx.filename_to_paths.get(raw_target).map(Vec::as_slice) {
        Some([one]) => return ResolvedLink::hit(one, ResolveStrategy::ExactFilename),
        Some([_, ..]) => return ResolvedLink::dangling(), // ambiguous — never guess
        _ => {}
    }
    // Tier 3: unique exact alias.
    match ctx.alias_to_paths.get(raw_target).map(Vec::as_slice) {
        Some([one]) => return ResolvedLink::hit(one, ResolveStrategy::Alias),
        Some([_, ..]) => return ResolvedLink::dangling(),
        _ => {}
    }
    // Tier 4: unique normalized filename-or-alias.
    match ctx
        .normalized_to_paths
        .get(&normalize_link_key(raw_target))
        .map(Vec::as_slice)
    {
        Some([one]) => ResolvedLink::hit(one, ResolveStrategy::Normalized),
        _ => ResolvedLink::dangling(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> LinkResolveContext {
        LinkResolveContext::new(vec![
            ("reference/rust".into(), "rust".into(), vec![]),
            (
                "personal/bob-smith".into(),
                "bob-smith".into(),
                vec!["Bob".into()],
            ),
            ("project/API Design".into(), "API Design".into(), vec![]),
            // Two notes share filename "dup" → filename tier is ambiguous.
            ("a/dup".into(), "dup".into(), vec![]),
            ("b/dup".into(), "dup".into(), vec![]),
        ])
    }

    #[test]
    fn tier1_exact_path_wins() {
        let r = resolve("reference/rust", &ctx());
        assert_eq!(r.target.as_deref(), Some("reference/rust"));
        assert!((r.confidence - 1.0).abs() < 1e-6);
        assert!(matches!(r.resolved_by, Some(ResolveStrategy::ExactPath)));
    }

    #[test]
    fn tier1_unknown_path_dangles() {
        // Contains '/' but not indexed → dangling, NOT filename fallback.
        let r = resolve("nope/rust", &ctx());
        assert!(r.target.is_none());
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn tier2_unique_filename() {
        let r = resolve("rust", &ctx());
        assert_eq!(r.target.as_deref(), Some("reference/rust"));
        assert!((r.confidence - 0.95).abs() < 1e-6);
        assert!(matches!(
            r.resolved_by,
            Some(ResolveStrategy::ExactFilename)
        ));
    }

    #[test]
    fn tier3_alias_when_no_filename_hit() {
        let r = resolve("Bob", &ctx());
        assert_eq!(r.target.as_deref(), Some("personal/bob-smith"));
        assert!((r.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn tier4_normalized_unique() {
        // Case fold: "api design" → "API Design"; full-width fold: "ｒｕｓｔ" → "rust".
        let r = resolve("api design", &ctx());
        assert_eq!(r.target.as_deref(), Some("project/API Design"));
        assert!((r.confidence - 0.7).abs() < 1e-6);
        let r2 = resolve("ｒｕｓｔ", &ctx());
        assert_eq!(r2.target.as_deref(), Some("reference/rust"));
    }

    #[test]
    fn ambiguity_never_guesses() {
        let r = resolve("dup", &ctx());
        assert!(r.target.is_none(), "2 filename candidates must dangle");
        assert!(r.resolved_by.is_none());
    }

    #[test]
    fn miss_dangles_with_zero_confidence() {
        let r = resolve("no-such-note", &ctx());
        assert!(r.target.is_none());
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn normalize_folds_case_and_fullwidth() {
        assert_eq!(normalize_link_key("ＡＢＣ　ｄｅｆ"), "abc def");
        assert_eq!(normalize_link_key("  API Design "), "api design");
    }
}
