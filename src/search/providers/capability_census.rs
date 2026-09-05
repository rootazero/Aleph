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
///
/// `pub(super)` so `error_funnel_census` asks the same list its own
/// question — a second enumeration would be two authors for one fact, and
/// the self-assertion below only pins one of them.
pub(super) fn provider_sources() -> BTreeMap<&'static str, &'static str> {
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
///
/// `pub(super)` for `error_funnel_census`, whose forbidden markers need the
/// same guarantee that a comment or a string literal cannot trip them.
pub(super) fn production_view(src: &str) -> String {
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

/// How one field inside this file's `capabilities()` body is declared.
///
/// Three states, not two, because a bit is not always a property of the
/// backend alone: Bing's `freshness` has no `Year` spelling, so its `recency`
/// is `options.bing_freshness().is_some()` — it *does* send the parameter,
/// just not for every value. Reading that expression as `false` (the old
/// two-state parse, which only looked for the literal `true`) would fail the
/// census against a provider that is telling the truth; reading it as `true`
/// would be a lie for `Recency::Year`. It gets its own state and its own
/// extra obligation — see
/// `a_conditional_bit_must_name_the_mapper_it_derives_from`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Declared {
    /// The field is absent, or literally `false`.
    Never,
    /// The field is literally `true`.
    Always,
    /// The field is an expression — sometimes on the wire, sometimes not.
    Conditional(String),
}

impl Declared {
    /// Does this declaration put the parameter on the wire *at all*?
    ///
    /// `Conditional` counts as yes: the census's question is "is there a code
    /// path that sends it", and a bit that is true for three of four values
    /// is backed by a request builder that really does send the parameter.
    const fn can_send(&self) -> bool {
        !matches!(self, Self::Never)
    }

    /// A label for assertion messages.
    const fn describe(&self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Always => "always",
            Self::Conditional(_) => "conditionally",
        }
    }
}

/// The body of this file's `capabilities()` function, or `None` when there is
/// no override (trait default => every bit false).
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
fn capabilities_body(src: &str) -> Option<String> {
    let code = production_view(src);
    let sig_start = code.find("fn capabilities(")?;
    let brace_offset = code[sig_start..].find('{')?;
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
    Some(code[body_start..end].to_string())
}

/// How `field` is declared inside this file's `capabilities()` body.
///
/// The value expression runs from `field:` to the delimiter that closes it
/// *at nesting depth zero* — `matches!(a, b)` and `f(x, y)` both contain
/// commas that do not end the field, and a naive `split` on the comma would
/// truncate the expression and misread it as something it is not.
fn declared_bit(src: &str, field: &str) -> Declared {
    let Some(body) = capabilities_body(src) else {
        return Declared::Never; // no override => trait default => all false
    };
    let needle = format!("{field}:");
    let Some(at) = body.find(&needle) else {
        return Declared::Never; // field not named => struct literal cannot compile
    };
    let rest = &body[at + needle.len()..];
    let mut depth = 0i32;
    let mut end = rest.len();
    for (offset, ch) in rest.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    end = offset; // closing brace of the struct literal
                    break;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                end = offset;
                break;
            }
            _ => {}
        }
    }
    match rest[..end].trim() {
        "true" => Declared::Always,
        "false" => Declared::Never,
        expr => Declared::Conditional(expr.to_string()),
    }
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
    .filter(|n| {
        !matches!(
            n.as_str(),
            "mod.rs" | "base.rs" | "capability_census.rs" | "error_funnel_census.rs"
        )
    })
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
                declared.can_send(),
                uses,
                "provider `{name}` declares it {} carries {dim}, but its request \
                 builder {} the parameter. A capability bit is a promise about the \
                 wire: declaring one you do not send makes the registry route \
                 requests to you that you will silently drop; not declaring one you \
                 do send hides you from requests you could have answered.",
                declared.describe(),
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

/// A conditional bit has to be *derived from* the mapper it is about, not
/// re-stated as a second decision.
///
/// Without this, `Conditional` is a hole in the census rather than a third
/// answer: any expression at all counts as "sends it sometimes", so a
/// provider could write `recency: false || true` and buy silence. The
/// obligation is the same one the whole file exists to enforce, one level up
/// — the bit and the wire must have a single derivation, so the expression
/// must name an accessor (or an options member) for *its own* dimension.
/// `bing.rs` passes because it writes `options.bing_freshness().is_some()`,
/// which is literally the call `search` makes; it would fail the moment
/// somebody replaced that with a hand-written `match` on `Recency`.
#[test]
fn a_conditional_bit_must_name_the_mapper_it_derives_from() {
    let mut conditionals = 0usize;
    for (dim, members) in DIMENSIONS {
        let accessors = accessors_reading(members);
        for (name, src) in provider_sources() {
            let Declared::Conditional(expr) = declared_bit(src, dim) else {
                continue;
            };
            conditionals += 1;
            let derived = accessors.iter().any(|a| expr.contains(&format!("{a}(")))
                || members
                    .iter()
                    .any(|m| expr.contains(&m.replace("self.", "options.")));
            assert!(
                derived,
                "provider `{name}` declares {dim} conditionally as `{expr}`, which \
                 names no mapper for that dimension. A conditional bit is only \
                 honest while it is the same expression the request builder decides \
                 on; re-stating the condition here gives the sort key and the wire \
                 two authors and no compiler between them."
            );
        }
    }
    assert!(
        conditionals > 0,
        "no conditional declaration was found at all — this guard would pass \
         vacuously. `bing.rs::capabilities` derives `recency` from \
         `options.bing_freshness()`; if that was flattened back to a literal, \
         delete this test with it rather than leaving it green over nothing."
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
    assert_eq!(
        declared_bit(src, "recency"),
        Declared::Always,
        "a string literal containing the old boundary heuristic's own \"\\n    }}\" \
         marker must not truncate the body before the real declaration"
    );
}

/// The three states have to come out of the same body, and the expression
/// parser has to survive a comma that is not the field's own.
///
/// `matches!(a, b)` is the shape that breaks a `split(',')` reader: the comma
/// inside the macro's argument list would end the expression early, leaving
/// `matches!(options.recency` — which still parses as `Conditional`, so the
/// truncation is invisible from the outcome alone. The assertion is on the
/// expression text for exactly that reason.
#[test]
fn declared_bit_reads_all_three_states_and_keeps_a_nested_comma() {
    let src = r#"
fn capabilities(&self, options: &SearchOptions) -> SearchCapabilities {
    SearchCapabilities {
        domain_filter: false,
        recency: !matches!(options.recency, Some(Recency::Year)),
        full_content: true,
    }
}
"#;
    assert_eq!(declared_bit(src, "domain_filter"), Declared::Never);
    assert_eq!(declared_bit(src, "full_content"), Declared::Always);
    assert_eq!(
        declared_bit(src, "recency"),
        Declared::Conditional("!matches!(options.recency, Some(Recency::Year))".to_string()),
        "the expression must survive the comma inside `matches!`"
    );
    assert!(declared_bit(src, "recency").can_send());
}

/// A provider with no `capabilities()` override declares nothing — the trait
/// default — and that has to read as `Never` for every dimension rather than
/// as "the parser could not find it". `jina.rs` is the live instance.
#[test]
fn a_provider_without_an_override_declares_nothing() {
    assert!(capabilities_body("fn name(&self) -> &str { \"x\" }").is_none());
    for (dim, _) in DIMENSIONS {
        assert_eq!(declared_bit("fn other() {}", dim), Declared::Never);
    }
}
