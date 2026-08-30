//! Source-level census: a provider's declared `SearchCapabilities` must match
//! the parameters its request builder actually sends.
//!
//! # Why this cannot be a runtime test
//!
//! A capability bit is a *promise about the wire*. At runtime the only way to
//! check it is to send a request and inspect it, which means an HTTP mock per
//! provider — and seven of the nine providers hardcode their endpoint, so
//! there is nowhere to point the mock. The source is the only place where all
//! nine can be asked the same question.
//!
//! # Why it is not a hand-written table
//!
//! A list of "who supports what" written here would be a second statement of
//! a fact the code already owns, and the two would drift (D.0.37: a guard
//! that enumerates its own inputs is structurally blind to whatever it did
//! not enumerate). Both sides are derived from source instead:
//!
//! * the accessor names are derived from `options.rs` — any `pub fn`/`pub
//!   const fn` whose body reads `self.recency` is a recency accessor, by
//!   construction;
//! * each provider's declaration is parsed out of its own `capabilities()`.

use crate::utils::source_scan::{code_text, production_prefix};
use std::collections::{BTreeMap, BTreeSet};

const OPTIONS_SRC: &str = include_str!("../options.rs");

/// Which `SearchOptions` member a dimension is expressed through.
const DIMENSIONS: &[(&str, &[&str])] = &[
    ("recency", &["self.recency"]),
    ("full_content", &["self.include_full_content"]),
    (
        "domain_filter",
        &["self.include_domains", "self.exclude_domains"],
    ),
];

/// Every provider source file, keyed by provider name.
fn provider_sources() -> BTreeMap<&'static str, &'static str> {
    // include_str! needs literals, so this list is explicit — but the
    // self-assertion below pins its length against the directory listing.
    BTreeMap::from([
        ("bing", include_str!("bing.rs")),
        ("brave", include_str!("brave.rs")),
        ("duckduckgo", include_str!("duckduckgo.rs")),
        ("exa", include_str!("exa.rs")),
        ("firecrawl", include_str!("firecrawl.rs")),
        ("google", include_str!("google.rs")),
        ("jina", include_str!("jina.rs")),
        ("searxng", include_str!("searxng.rs")),
        ("tavily", include_str!("tavily.rs")),
    ])
}

/// This file's view of a Rust source: no `#[cfg(test)]` items, no comments,
/// no string/char literal payloads. `code_text` composed AFTER
/// `production_prefix` (the order its own doc recommends) means neither
/// this scanner's own marker text nor a provider's format strings can be
/// mistaken for the code they merely describe — see `code_text`'s doc for
/// why a naive quote-walk is unsafe here.
fn production_view(src: &str) -> String {
    code_text(&production_prefix(src))
}

/// Accessor fn names in `options.rs` whose body reads one of `members`.
fn accessors_reading(members: &[&str]) -> BTreeSet<String> {
    let src = production_view(OPTIONS_SRC);
    let mut current: Option<String> = None;
    let mut found = BTreeSet::new();
    for line in src.lines() {
        let t = line.trim_start();
        if let Some(rest) = t
            .strip_prefix("pub fn ")
            .or_else(|| t.strip_prefix("pub const fn "))
        {
            current = rest.split('(').next().map(str::to_string);
        }
        if members.iter().any(|m| line.contains(m)) {
            if let Some(name) = &current {
                found.insert(name.clone());
            }
        }
    }
    found
}

/// The literal value of one field inside this file's `capabilities()` body.
///
/// The body's end is found by brace-balance counting from the `{` that
/// opens it — not by an indentation-anchored string search (the previous
/// implementation searched for the literal text `"\n    }"`, which happens
/// to fall out of rustfmt's current formatting of a three-field struct
/// literal for all nine files today, but is defined by rustfmt's behaviour
/// rather than by this function's own syntax). Braces inside string/char
/// literal payloads are already gone by the time this scan sees them
/// (`production_view` composes `code_text`, which replaces each literal
/// with a non-brace sentinel), so every `{`/`}` counted here is a real code
/// delimiter — see
/// `declared_bit_survives_a_body_containing_the_old_boundary_heuristics_own_marker`
/// for the fixture that pins this against the old heuristic.
fn declared_bit(src: &str, field: &str) -> bool {
    let code = production_view(src);
    let Some(sig_start) = code.find("fn capabilities(") else {
        return false; // no override => trait default => all false
    };
    let Some(brace_offset) = code[sig_start..].find('{') else {
        return false; // signature without a body — malformed, treat as absent
    };
    let body_start = sig_start + brace_offset;
    let mut depth = 0i32;
    let mut end = code.len();
    for (offset, ch) in code[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + offset + 1; // include the closing brace
                    break;
                }
            }
            _ => {}
        }
    }
    code[body_start..end].contains(&format!("{field}: true"))
}

#[test]
fn the_census_sees_every_provider_file() {
    let files: BTreeSet<String> = std::fs::read_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/search/providers"),
    )
    .expect("providers dir")
    .filter_map(|e| e.ok())
    .filter_map(|e| e.file_name().into_string().ok())
    .filter(|n| n.ends_with(".rs"))
    .filter(|n| !matches!(n.as_str(), "mod.rs" | "base.rs" | "capability_census.rs"))
    .map(|n| n.trim_end_matches(".rs").to_string())
    .collect();
    let known: BTreeSet<String> = provider_sources()
        .keys()
        .map(|k| (*k).to_string())
        .collect();
    assert_eq!(
        files, known,
        "a provider file appeared or vanished without the census being told"
    );
}

#[test]
fn every_declared_capability_is_backed_by_a_parameter_that_is_actually_sent() {
    let mut checked = 0usize;
    for (dim, members) in DIMENSIONS {
        let accessors = accessors_reading(members);
        for (name, src) in provider_sources() {
            let prod = production_view(src);
            let uses = accessors.iter().any(|a| prod.contains(&format!("{a}(")))
                || members
                    .iter()
                    .any(|m| prod.contains(&m.replace("self.", "options.")));
            let declared = declared_bit(src, dim);
            assert_eq!(
                declared,
                uses,
                "provider `{name}` declares {dim}={declared} but its request builder \
                 {} the parameter. A capability bit is a promise about the wire: \
                 declaring one you do not send makes the registry route requests to \
                 you that you will silently drop; not declaring one you do send hides \
                 you from requests you could have answered.",
                if uses { "does send" } else { "never sends" }
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked,
        DIMENSIONS.len() * 9,
        "the census must compare every dimension against every provider"
    );
}

/// Pins `declared_bit`'s brace-balance boundary against the old
/// indentation-anchored heuristic it replaced (`body.find("\n    }")`).
///
/// This fixture's `capabilities()` body opens a string literal, spanning
/// two physical lines, whose payload contains the exact byte sequence
/// `"\n    }"` — a newline, four spaces, and a closing brace — BEFORE the
/// real `recency: true` declaration. Against the old implementation this
/// is exactly the false negative the review finding predicted: `body.find`
/// locates that sequence inside the string payload, long before the
/// function's actual closing brace, and truncates the slice there — so
/// `recency: true`, sitting after the truncation point, is never seen and
/// `declared_bit` answers `false` for a provider that really does declare
/// `true`. A brace-balance scan over `code_text`'s output is unaffected:
/// the `}` inside the string was never emitted (string interiors are
/// replaced by a single `"` sentinel), so it is never counted as a real
/// delimiter.
#[test]
fn declared_bit_survives_a_body_containing_the_old_boundary_heuristics_own_marker() {
    let src = r#"
fn capabilities(&self) -> SearchCapabilities {
    let _marker = "line one
    }";
    SearchCapabilities {
        domain_filter: false,
        recency: true,
        full_content: false,
    }
}
"#;
    assert!(
        declared_bit(src, "recency"),
        "a string literal containing the old boundary heuristic's own \"\\n    }}\" \
         marker must not truncate the body before the real declaration"
    );
}
