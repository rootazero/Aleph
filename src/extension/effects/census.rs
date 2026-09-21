//! G1 — the registration-returns-a-Disposer census (spec §3.5).
//!
//! Source-level, because at runtime a registration that bypassed the scope
//! looks exactly like one that went through it: both mutate shared state,
//! and only the next unmount would tell — silently.

use std::path::{Path, PathBuf};

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

/// The non-test, non-comment text of a source file.
fn production_text(path: &Path) -> String {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let cut = src.find("#[cfg(test)]\nmod tests").unwrap_or(src.len());
    src[..cut]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
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
#[test]
fn register_mcp_has_exactly_one_production_caller_and_it_is_the_effect() {
    let root = repo_root().join("src");
    let mut hits: Vec<String> = Vec::new();
    // This function's own assert_eq! message below contains the literal
    // `.register_mcp(` substring it is scanning for, so the walk necessarily
    // matches this file too; route that self-match into a self-count
    // instead of `hits` — same shape as `projection.rs`'s
    // `is_this_file`/`found_here` (keyed on the path suffix, not the bare
    // file name, since `src/capability/census.rs` also exists and must stay
    // in the scan).
    let mut found_here = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let rel = path.strip_prefix(&root).unwrap().display().to_string();
            let is_this_file = rel == "extension/effects/census.rs";
            let text = production_text(&path);
            for (i, line) in text.lines().enumerate() {
                if line.contains(".register_mcp(") && !line.contains("fn ") {
                    if is_this_file {
                        found_here += 1;
                    } else {
                        hits.push(format!("{rel}:{}", i + 1));
                    }
                }
            }
        }
    }
    assert!(
        found_here >= 1,
        "the scanner did not see its own assertion string in \
         extension/effects/census.rs — it is not reading the tree it thinks it is"
    );
    assert_eq!(
        hits.len(),
        1,
        "`.register_mcp(` must have exactly one production caller (the memory_extension \
         effect); found: {hits:?}"
    );
    assert!(
        hits[0].starts_with("extension/loader.rs:"),
        "the one caller must be register_memory_extension_effect in loader.rs, not {}",
        hits[0]
    );
}
