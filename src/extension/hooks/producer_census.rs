//! G4 — every `HookEvent` has a producer outside `src/extension/`, and every
//! alias read off the enum points at one of them.
//!
//! Derived, not listed: the variant set comes from `HookEvent::ALL`, the
//! alias set from the `#[serde(alias = …)]` attributes in `types/hooks.rs`,
//! and the producers from one walk of `src/` reading production code only
//! (`production_text`, then `code_text`: comments and literal payloads gone).
//!
//! # What counts as "fired"
//!
//! `HookEvent::<Variant>` inside the argument list of a call to a hook
//! DISPATCHER, in a file outside `src/extension/`. The dispatcher set is
//! derived as well: the executor's two dispatch methods, closed over "a fn
//! that takes a `HookEvent` parameter and calls a dispatcher" — which is how
//! `fire_global_observer` and `fire_compaction_hook` get in without being
//! named here. A variant merely NAMED in production code is not fired: a
//! gate read (`has_hooks_for(HookEvent::Stop)`) or a fallback value
//! (`unwrap_or(HookEvent::BeforeToolCall)`) both exist today, and a census
//! that counted any mention stays green with `Stop`'s two real fire lines
//! deleted (判据 §3).
//!
//! # What it does not see
//!
//! A fire whose event is not spelled at the call — `let e = HookEvent::X;
//! executor.execute_observers(e, …)` — reads as silent (a false red, the
//! loud direction). None exists today; the red names the variant, and the
//! fix is to pass the literal.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::extension::types::HookEvent;
use crate::utils::source_scan::{code_text, production_text, rust_sources_under};

/// The executor's own dispatch methods: every fire path ends in one of them.
/// Checked against `executor.rs` below, so a rename is red, not silent.
const DISPATCH_SEEDS: [&str; 2] = ["execute_observers", "execute_interceptors"];

const fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `(path relative to the crate, production code)` for every `.rs` file
/// under `src/` — walked and lexed once for the whole module.
fn corpus() -> &'static [(String, String)] {
    static CORPUS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        rust_sources_under(&root)
            .into_iter()
            .map(|(rel, text)| {
                let code = code_text(&production_text(std::path::Path::new(&rel), &text));
                (rel, code)
            })
            .collect()
    })
}

/// Byte offsets of `word` in `code` where neither neighbour is an identifier
/// character.
fn word_at(code: &str, word: &str) -> Vec<usize> {
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(rel) = code.get(from..).and_then(|s| s.find(word)) {
        let at = from + rel;
        let before = code.get(..at).and_then(|s| s.chars().next_back());
        let after = code.get(at + word.len()..).and_then(|s| s.chars().next());
        if !before.is_some_and(is_ident) && !after.is_some_and(is_ident) {
            hits.push(at);
        }
        from = at + word.len();
    }
    hits
}

/// The text between `(` at `open` and its matching `)`.
fn parenthesised(code: &str, open: usize) -> Option<&str> {
    let mut depth = 0usize;
    for (i, c) in code.get(open..)?.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return code.get(open + 1..open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The argument list of every call `callee(…)` in `code`.
fn call_args<'a>(code: &'a str, callee: &str) -> Vec<&'a str> {
    word_at(code, callee)
        .into_iter()
        .filter_map(|at| {
            let rest = code.get(at + callee.len()..)?;
            let open = at + callee.len() + (rest.len() - rest.trim_start().len());
            if !code.get(open..)?.starts_with('(') {
                return None;
            }
            parenthesised(code, open)
        })
        .collect()
}

/// `(name, parameter list, body)` of every `fn` with a body in `code`.
fn fns(code: &str) -> Vec<(&str, &str, &str)> {
    let mut out = Vec::new();
    for at in word_at(code, "fn") {
        let Some(rest) = code.get(at + 2..) else {
            continue;
        };
        let name_start = at + 2 + (rest.len() - rest.trim_start().len());
        let Some(name_len) = code.get(name_start..).map(|s| {
            s.char_indices()
                .find(|&(_, c)| !is_ident(c))
                .map_or(s.len(), |(i, _)| i)
        }) else {
            continue;
        };
        // `fn(…)` is a function-pointer TYPE, not an item: no name.
        if name_len == 0 {
            continue;
        }
        let Some(open) = code
            .get(name_start + name_len..)
            .and_then(|s| s.find('('))
            .map(|i| name_start + name_len + i)
        else {
            continue;
        };
        let Some(params) = parenthesised(code, open) else {
            continue;
        };
        let after_params = open + params.len() + 2;
        // The signature ends at the first `{` (a body) or `;` (a trait item
        // with none); Rust types never contain either.
        let Some(end) = code
            .get(after_params..)
            .and_then(|s| s.find(['{', ';']))
            .map(|i| after_params + i)
        else {
            continue;
        };
        if !code.get(end..).is_some_and(|s| s.starts_with('{')) {
            continue;
        }
        let mut depth = 0usize;
        let body_end = code.get(end..).and_then(|s| {
            s.char_indices().find_map(|(i, c)| {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(end + i)
            })
        });
        if let (Some(name), Some(body)) = (
            code.get(name_start..name_start + name_len),
            body_end.and_then(|e| code.get(end..=e)),
        ) {
            out.push((name, params, body));
        }
    }
    out
}

/// The seeds, closed over "takes a `HookEvent` parameter and calls a
/// dispatcher" until nothing new joins.
fn dispatchers() -> BTreeSet<String> {
    let candidates: Vec<(String, String)> = corpus()
        .iter()
        .flat_map(|(_, code)| fns(code))
        .filter(|(_, params, _)| !word_at(params, "HookEvent").is_empty())
        .map(|(name, _, body)| (name.to_string(), body.to_string()))
        .collect();
    let mut set: BTreeSet<String> = DISPATCH_SEEDS.iter().map(ToString::to_string).collect();
    loop {
        let grown: Vec<String> = candidates
            .iter()
            .filter(|(name, body)| {
                !set.contains(name) && set.iter().any(|d| !call_args(body, d).is_empty())
            })
            .map(|(name, _)| name.clone())
            .collect();
        if grown.is_empty() {
            return set;
        }
        set.extend(grown);
    }
}

/// `file:line` of every dispatch call outside `src/extension/` that passes
/// `HookEvent::<variant>`.
fn producers_of(variant: &str, dispatchers: &BTreeSet<String>) -> Vec<String> {
    let needle = format!("HookEvent::{variant}");
    let mut hits = Vec::new();
    for (rel, code) in corpus() {
        if rel.starts_with("src/extension/") {
            continue;
        }
        for d in dispatchers {
            for args in call_args(code, d) {
                if word_at(args, &needle).is_empty() {
                    continue;
                }
                let offset = args.as_ptr() as usize - code.as_ptr() as usize;
                let line = code.get(..offset).map_or(0, |s| s.lines().count());
                hits.push(format!("{rel}:{line}"));
            }
        }
    }
    hits
}

/// The body of `pub enum HookEvent { … }` in `types/hooks.rs`, raw (the
/// alias strings are what is being read).
///
/// Line-ending tolerant: split on lines first so the search doesn't
/// depend on whether `include_str!` saw LF (Linux/Mac checkout) or
/// CRLF (Windows checkout with `core.autocrlf=true`). The previous
/// `split("\n}\n")` form failed on Windows because the embedded
/// bytes were `\r\n}\r\n` and `\n}\n` never matched — `enum_body`
/// then returned the entire rest of the file, which the test then
/// walked line-by-line (290 lines) instead of the 24-variant enum
/// body. The hook events were unchanged; the assertion failed
/// because the parser was. See producer_census_test_history for
/// the full regression.
fn enum_body() -> &'static str {
    let raw = include_str!("../types/hooks.rs");
    let lines: Vec<&str> = raw.lines().collect();
    let open_idx = lines
        .iter()
        .position(|l| l.trim_start().starts_with("pub enum HookEvent {"))
        .expect("HookEvent enum opening line");
    let close_idx = lines
        .iter()
        .enumerate()
        .skip(open_idx + 1)
        .find(|(_, l)| l.trim() == "}")
        .map(|(i, _)| i)
        .expect("HookEvent enum closing brace");
    // Return only the inner body so callers can `lines()` it without
    // the opening 'pub enum HookEvent {' or closing '}' slipping
    // through their filters. Box::leak satisfies the &'static str
    // signature callers expect; the 24-line slice is a one-time cost
    // per test process.
    let joined: &'static str =
        Box::leak(lines[open_idx + 1..close_idx].join("\n").into_boxed_str());
    joined
}

/// `(alias, variant)` pairs: every alias in an attribute applies to the next
/// variant line after it. An attribute rustfmt wrapped over several lines is
/// read whole.
fn declared_aliases() -> Vec<(String, String)> {
    let mut pending: Vec<String> = Vec::new();
    let mut attr = String::new();
    let mut out = Vec::new();
    for raw in enum_body().lines() {
        let line = raw.trim();
        if !attr.is_empty() || line.starts_with("#[") {
            attr.push_str(line);
            if !line.ends_with(']') {
                continue;
            }
            const OPEN: &str = "alias = \"";
            let mut rest = attr.as_str();
            while let Some(after) = rest.find(OPEN).and_then(|at| rest.get(at + OPEN.len()..)) {
                let end = after.find('"').expect("closing quote");
                pending.extend(after.get(..end).map(str::to_string));
                rest = after.get(end + 1..).unwrap_or_default();
            }
            attr.clear();
            continue;
        }
        let variant = line.trim_end_matches(',');
        if !variant.is_empty() && variant.chars().all(|c| c.is_ascii_alphanumeric()) {
            for alias in pending.drain(..) {
                out.push((alias, variant.to_string()));
            }
        }
    }
    out
}

#[test]
fn every_hook_event_has_a_producer_outside_src_extension() {
    let dispatchers = dispatchers();
    let mut silent = Vec::new();
    for event in HookEvent::ALL {
        let variant = format!("{event:?}");
        if producers_of(&variant, &dispatchers).is_empty() {
            silent.push(variant);
        }
    }
    assert!(
        silent.is_empty(),
        "declared hook events with no dispatch outside src/extension/: {silent:?} \
         (dispatchers: {dispatchers:?})"
    );
}

#[test]
fn every_alias_targets_a_fired_event() {
    let dispatchers = dispatchers();
    let aliases = declared_aliases();
    // Reader health, derived: every variant carries at least its PascalCase
    // alias, so a reader that misses one variant's attribute is red here.
    for event in HookEvent::ALL {
        let variant = format!("{event:?}");
        assert!(
            aliases.iter().any(|(_, v)| *v == variant),
            "the alias reader saw no attribute on {variant} — it rotted, or the \
             variant lost its PascalCase alias"
        );
    }
    let dangling: Vec<String> = aliases
        .iter()
        .filter(|(_, variant)| producers_of(variant, &dispatchers).is_empty())
        .map(|(alias, variant)| format!("{alias} -> {variant}"))
        .collect();
    assert!(
        dangling.is_empty(),
        "aliases whose target is never fired: {dangling:?}"
    );
    // The two this round adds are visible to the reader (a reader that
    // cannot see them cannot guard them).
    assert!(aliases
        .iter()
        .any(|(a, v)| a == "PostCompact" && v == "AfterCompaction"));
    assert!(aliases
        .iter()
        .any(|(a, v)| a == "PermissionDenied" && v == "PermissionDenied"));
}

#[test]
fn all_lists_every_variant_exactly_once() {
    // `ALL` is the roster the census walks; a variant left out of it would
    // also be left out of the census. Every declared variant name read off
    // the enum source must parse (through its PascalCase alias) into `ALL`.
    let declared: Vec<&str> = enum_body()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("///") && !l.starts_with("#["))
        .map(|l| l.trim_end_matches(','))
        .collect();
    assert_eq!(
        declared.len(),
        HookEvent::ALL.len(),
        "declared {declared:?} vs ALL"
    );
    for name in declared {
        let e: HookEvent = serde_json::from_str(&format!("\"{name}\""))
            .unwrap_or_else(|_| panic!("{name} parses via its PascalCase alias"));
        assert!(HookEvent::ALL.contains(&e), "{name} missing from ALL");
    }
}

#[test]
fn the_dispatcher_derivation_sees_its_seeds_and_the_wrappers_they_reach() {
    // The seeds are real `HookExecutor` methods (a rename is red here, not a
    // silently empty dispatcher set), and the closure reaches the public
    // fire-and-forget wrapper every global seam goes through.
    let executor = corpus()
        .iter()
        .find(|(rel, _)| rel == "src/extension/hooks/executor.rs")
        .map(|(_, code)| code.as_str())
        .expect("executor.rs is in the corpus");
    let defined: Vec<&str> = fns(executor).into_iter().map(|(n, _, _)| n).collect();
    for seed in DISPATCH_SEEDS {
        assert!(defined.contains(&seed), "no `fn {seed}` in executor.rs");
    }
    let set = dispatchers();
    assert!(
        set.contains("fire_global_observer"),
        "derived dispatchers: {set:?}"
    );
}
