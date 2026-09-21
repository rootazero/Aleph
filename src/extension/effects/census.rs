//! G1 — the registration-returns-a-Disposer census (spec §3.5).
//!
//! Source-level, because at runtime a registration that bypassed the scope
//! looks exactly like one that went through it: both mutate shared state,
//! and only the next unmount would tell — silently.

use std::path::{Path, PathBuf};

use crate::utils::source_scan::{production_code_text, rust_sources_under};

/// Files that own the six effect producers. A new producer goes in one of
/// these, or this list grows in the same commit.
const CENSUS_FILES: [&str; 5] = [
    "src/extension/registrar/api.rs",
    "src/extension/registrar/mcp_registrar.rs",
    "src/extension/loader.rs",
    "src/extension/service_ops.rs",
    "src/extension/slash_effect.rs",
];

/// Name prefixes that mean "this writes something into the runtime".
const EFFECT_VERBS: [&str; 5] = ["register_", "load_", "start_", "add_", "mount_"];

/// (file, fn, why) — a crate-visible effect verb that legitimately does not
/// return a `Disposer`. Every entry must still exist, or the census is red.
const EXEMPT: [(&str, &str, &str); 1] = [(
    "src/extension/service_ops.rs",
    "start_service",
    "operator verb behind `services.start`; the plugin's `service` step disposer \
     stops every registered service of the plugin, so a manual start never \
     outlives the mount",
)];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The text either census scans: `production_code_text` (`utils::source_scan`)
/// — the empty string for a file declared a test module by its parent (this
/// file included: `effects/mod.rs` declares `#[cfg(test)] mod census;`),
/// else comments stripped, every `#[cfg(test)]` item blanked (line numbers
/// kept), and every string/char literal's payload blanked too. Layered
/// defenses against a census's own message strings and marker constants
/// self-matching, not one mechanism — see the two `#[test]` fns below for
/// which layer actually carries each one today. Reads `path` itself; panics
/// (this is test-only scanning code) if it cannot.
fn production_text(path: &Path) -> String {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    production_code_text(path, &src)
}

/// Every crate-visible `fn` declaration in `text` whose name starts with an
/// effect verb, with its signature (declaration up to the opening brace).
fn crate_visible_effect_fns(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        let vis_ok = t.starts_with("pub fn ")
            || t.starts_with("pub async fn ")
            || t.starts_with("pub(crate) fn ")
            || t.starts_with("pub(crate) async fn ");
        if !vis_ok {
            continue;
        }
        let after_fn = &t[t.find("fn ").unwrap() + 3..];
        let name: String = after_fn
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !EFFECT_VERBS.iter().any(|v| name.starts_with(v)) {
            continue;
        }
        // Signature may span lines: read until the body's `{`.
        let mut sig = t.to_string();
        while !sig.contains('{') {
            match lines.next() {
                Some(l) => {
                    sig.push(' ');
                    sig.push_str(l.trim());
                }
                None => break,
            }
        }
        out.push((name, sig));
    }
    out
}

#[test]
fn every_crate_visible_registration_returns_a_disposer() {
    let root = repo_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut disposer_returning = 0usize;
    let mut exempt_seen: Vec<(&str, &str)> = Vec::new();

    for rel in CENSUS_FILES {
        let text = production_text(&root.join(rel));
        for (name, sig) in crate_visible_effect_fns(&text) {
            if let Some((f, n, _)) = EXEMPT.iter().find(|(f, n, _)| *f == rel && *n == name) {
                exempt_seen.push((f, n));
                continue;
            }
            let returns_disposer = sig
                .split("->")
                .nth(1)
                .is_some_and(|ret| ret.contains("Disposer"));
            if returns_disposer {
                disposer_returning += 1;
            } else {
                offenders.push(format!("{rel}::{name} — `{}`", sig.trim()));
            }
        }
    }

    // Self-count: six producers exist today; a scanner that finds fewer is
    // not reading what it thinks it is.
    assert!(
        disposer_returning >= 6,
        "census found only {disposer_returning} Disposer-returning registration fns — \
         the scanner is blind (six producers are known to exist)"
    );
    for (f, n, _) in EXEMPT {
        assert!(
            exempt_seen.contains(&(f, n)),
            "stale exemption: {f}::{n} no longer exists — remove it from EXEMPT"
        );
    }
    assert!(
        offenders.is_empty(),
        "crate-visible registration fns that do not return a Disposer (spec §3.1: every \
         effect is owned by the plugin's EffectScope; either return `Disposer`, make it \
         `pub(super)` so it is an inner write path of one that does, or add an EXEMPT \
         entry with a reason):\n  {}",
        offenders.join("\n  ")
    );
}

/// The memory registry's plugin path has exactly one caller, in `loader.rs`
/// (`register_memory_extension_effect`). A second caller would be a
/// `[memory]` registration outside the scope.
///
/// Walks every `.rs` file under `src/` via `rust_sources_under` and scans
/// each through `production_code_text`, so no self-count is needed: this
/// file's own `.register_mcp(` needle — spelled as a string literal in the
/// `.contains(...)` call below — never reaches the scanned text, because
/// `census.rs` is itself `declared_as_a_test_module` (its parent,
/// `effects/mod.rs`, declares `#[cfg(test)] mod census;`), which
/// `production_code_text` answers before it ever gets to blanking literal
/// payloads. `code_text`'s literal-blanking is still real defense-in-depth
/// — it is what protects a needle spelled in live, non-test-declared code
/// (e.g. a future file that quotes this method name in a log message) — but
/// it is not what is empirically load-bearing for THIS file today; see the
/// P1.14 addendum report for the mutation that measured this directly.
#[test]
fn register_mcp_has_exactly_one_production_caller_and_it_is_the_effect() {
    let root = repo_root().join("src");
    let mut hits: Vec<String> = Vec::new();
    for (rel, src) in rust_sources_under(&root) {
        let path = repo_root().join(&rel);
        let text = production_code_text(&path, &src);
        for (i, line) in text.lines().enumerate() {
            if line.contains(".register_mcp(") && !line.contains("fn ") {
                hits.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "`.register_mcp(` must have exactly one production caller (the memory_extension \
         effect); found: {hits:?}"
    );
    assert!(
        hits[0].starts_with("src/extension/loader.rs:"),
        "the one caller must be register_memory_extension_effect in loader.rs, not {}",
        hits[0]
    );
}
