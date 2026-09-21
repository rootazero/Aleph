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

/// Drops every line whose trimmed start is `//` — both `//` line comments
/// and `///` / `//!` doc comments (they all start with `//`). Shared by
/// `production_text` and `assert_mod_tests_is_last_item` so the cut and the
/// brace walk agree on one derivation of "what is code": a `// }` inside a
/// census file's `mod tests` must not desync the walk any more than it
/// desyncs the disposer-return scan.
fn strip_line_comments(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The non-test, non-comment text of a source file.
fn production_text(path: &Path) -> String {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let cut = src.find("#[cfg(test)]\nmod tests").unwrap_or(src.len());
    strip_line_comments(&src[..cut])
}

/// Confirms `mod tests` is the last item in one of [`CENSUS_FILES`]: from
/// its opening `{`, brace depth must return to zero exactly once, with
/// nothing but whitespace following the closing `}`. A registration placed
/// after `mod tests` is production code `production_text`'s cut-at-marker
/// scan never sees — the census's green only covers the shape it
/// recognises.
///
/// Deliberately a standalone check, NOT folded into `production_text`
/// itself: `production_text` is also used by
/// `register_mcp_has_exactly_one_production_caller_and_it_is_the_effect`,
/// which walks every `.rs` file under `src/`. A dry run of this exact check
/// against the whole tree found 159 files where `mod tests` is legitimately
/// *not* the file's last item (a second `#[cfg(test)] mod other_tests {}`,
/// or `pub use` re-exports placed after the test module — both are common,
/// correct Rust style outside the five files this census owns). Scoping the
/// check to `CENSUS_FILES` closes the actual blind spot (a producer slipped
/// in after `mod tests` in one of the five files this guard answers for)
/// without making the crate-wide `register_mcp` walk depend on an
/// assumption that is false almost everywhere else in the crate.
///
/// Counts braces outside `"..."` string literals only — braces inside a
/// test's own assertion/format-string text would otherwise desync the
/// depth count. Char literals (e.g. `'{'`) are not special-cased (that
/// needs lookahead to distinguish from a lifetime like `'a`); none of the
/// five census files' test modules contain one today.
///
/// Walks `strip_line_comments`'d text, not the raw file: a `// }` sitting
/// on its own line inside `mod tests` would otherwise desync the brace
/// count and make this fire on a file that is fine — a misfiring guard is
/// worse than a silent one.
fn assert_mod_tests_is_last_item(path: &Path) {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let Some(marker_start) = src.find("#[cfg(test)]\nmod tests") else {
        return; // No marker: nothing to verify.
    };
    let tail = strip_line_comments(&src[marker_start..]);
    let Some(brace_start) = tail.find('{') else {
        panic!(
            "{}: found `#[cfg(test)]\\nmod tests` with no opening brace",
            path.display()
        );
    };
    let body = &tail[brace_start..];

    let mut depth: i32 = 0;
    let mut closed_at: Option<usize> = None;
    let mut chars = body.char_indices();
    while let Some((idx, c)) = chars.next() {
        match c {
            '"' => {
                let rest = chars.by_ref();
                while let Some((_, sc)) = rest.next() {
                    if sc == '\\' {
                        rest.next(); // skip the escaped character
                    } else if sc == '"' {
                        break;
                    }
                }
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    closed_at = Some(idx + '}'.len_utf8());
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(end) = closed_at else {
        panic!(
            "{}: `mod tests` never closes — unbalanced braces",
            path.display()
        );
    };
    assert!(
        body[end..].trim().is_empty(),
        "{}: `mod tests` must be the last item in the file — the census does not scan past it",
        path.display()
    );
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
        assert_mod_tests_is_last_item(&root.join(rel));
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
