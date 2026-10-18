//! G4 — every `HookEvent` has a producer outside `src/extension/`, and every
//! alias read off the enum points at one of them.
//!
//! Derived, not listed: variants and aliases come from the actual API legacy
//! `HookEvent` enum; unit tests also reconcile it with `HookEvent::ALL`,
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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
use crate::extension::types::HookEvent;
use crate::utils::source_scan::{
    code_keeping_literals, code_text, production_prefix, production_text, rust_sources_under_root,
};

/// The executor's own dispatch methods: every fire path ends in one of them.
/// Checked against `executor.rs` below, so a rename is red, not silent.
const DISPATCH_SEEDS: [&str; 2] = ["execute_observers", "execute_interceptors"];

const fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Errors carry the supplied project root at every filesystem/scanning boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardError(pub String);

impl std::fmt::Display for GuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for GuardError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEnumInventory {
    pub variants: Vec<String>,
    pub aliases: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducerReport {
    pub root: PathBuf,
    pub inventory: HookEnumInventory,
    pub files_scanned: usize,
    pub dispatchers: BTreeSet<String>,
    pub producers: BTreeMap<String, Vec<String>>,
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
fn dispatchers(corpus: &[(String, String)]) -> BTreeSet<String> {
    let candidates: Vec<(String, String)> = corpus
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
fn producers_of(
    variant: &str,
    dispatchers: &BTreeSet<String>,
    corpus: &[(String, String)],
) -> Vec<String> {
    let needle = format!("HookEvent::{variant}");
    let mut hits = Vec::new();
    for (rel, code) in corpus {
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

/// Parse the actual fieldless legacy declaration, using the shared lexer to
/// distinguish live enum/attribute syntax from comments and literal payloads.
/// This intentionally bounded parser accepts the current line-oriented enum
/// and wrapped serde attributes; unsupported layouts fail loudly.
pub fn parse_hook_enum(raw: &str) -> Result<HookEnumInventory, GuardError> {
    let production = production_prefix(raw);
    let code = code_text(&production);
    let kept = code_keeping_literals(&production);
    let lines: Vec<_> = code.lines().collect();
    let literal_lines: Vec<_> = kept.lines().collect();
    let openings: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == "pub enum HookEvent {")
        .map(|(i, _)| i)
        .collect();
    if openings.len() != 1 {
        return Err(GuardError(format!(
            "expected one real legacy HookEvent enum opening; found {}",
            openings.len()
        )));
    }
    let open = openings[0];
    let close = lines
        .iter()
        .enumerate()
        .skip(open + 1)
        .find(|(_, line)| line.trim() == "}")
        .map(|(i, _)| i)
        .ok_or_else(|| GuardError("missing HookEvent enum closing brace".into()))?;
    let mut inventory = HookEnumInventory {
        variants: Vec::new(),
        aliases: Vec::new(),
    };
    let mut attr = String::new();
    let mut pending = Vec::new();
    for i in open + 1..close {
        let line = lines[i].trim();
        if line.is_empty() {
            continue;
        }
        if !attr.is_empty() || line.starts_with("#[") {
            attr.push_str(literal_lines[i].trim());
            attr.push(' ');
            if !line.ends_with(']') {
                continue;
            }
            for at in word_at(&attr, "alias") {
                let value = attr[at + "alias".len()..].trim_start();
                let Some(value) = value
                    .strip_prefix('=')
                    .map(str::trim_start)
                    .and_then(|v| v.strip_prefix('"'))
                else {
                    return Err(GuardError("unsupported HookEvent alias attribute".into()));
                };
                let end = value
                    .find('"')
                    .ok_or_else(|| GuardError("unterminated HookEvent alias".into()))?;
                let alias = &value[..end];
                if alias.is_empty() || !alias.chars().all(is_ident) {
                    return Err(GuardError("unsupported/empty HookEvent alias".into()));
                }
                pending.push(alias.to_string());
            }
            attr.clear();
            continue;
        }
        let variant = line.trim_end_matches(',');
        if variant.is_empty()
            || !variant.starts_with(|c: char| c.is_ascii_alphabetic())
            || !variant.chars().all(is_ident)
        {
            return Err(GuardError(format!(
                "unsupported HookEvent variant at line {}: {line}",
                i + 1
            )));
        }
        if inventory.variants.iter().any(|v| v == variant) {
            return Err(GuardError(format!("duplicate HookEvent variant {variant}")));
        }
        inventory.variants.push(variant.to_string());
        inventory
            .aliases
            .extend(pending.drain(..).map(|alias| (alias, variant.to_string())));
    }
    if inventory.variants.is_empty() {
        return Err(GuardError("empty HookEvent enum".into()));
    }
    if !attr.is_empty() || !pending.is_empty() {
        return Err(GuardError("unattached HookEvent alias".into()));
    }
    Ok(inventory)
}

fn derive_hook_producers(
    root: &Path,
    raw_enum: &str,
    corpus: &[(String, String)],
) -> Result<ProducerReport, GuardError> {
    let fail = |message: String| GuardError(format!("{}: {message}", root.display()));
    let inventory = parse_hook_enum(raw_enum).map_err(|e| fail(e.to_string()))?;
    if corpus.is_empty() {
        return Err(fail("empty corpus".into()));
    }
    let mut seen = BTreeSet::new();
    let mut code = Vec::new();
    for (rel, raw) in corpus {
        let path = Path::new(rel);
        if !rel.starts_with("src/")
            || rel.contains('\\')
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || !seen.insert(rel.clone())
        {
            return Err(fail(format!("invalid/duplicate corpus path {rel}")));
        }
        // Absolute path for parent-module preprocessing, relative label for policy.
        let absolute = if root.is_absolute() {
            root.join(path)
        } else {
            std::env::current_dir()
                .map_err(|e| fail(e.to_string()))?
                .join(root)
                .join(path)
        };
        code.push((rel.clone(), code_text(&production_text(&absolute, raw))));
    }
    if !code.iter().any(|(_, text)| !text.trim().is_empty()) {
        return Err(fail("empty production corpus".into()));
    }
    let set = dispatchers(&code);
    let producers = inventory
        .variants
        .iter()
        .map(|v| (v.clone(), producers_of(v, &set, &code)))
        .collect();
    Ok(ProducerReport {
        root: root.to_path_buf(),
        inventory,
        files_scanned: code.len(),
        dispatchers: set,
        producers,
    })
}

/// Shared production/fixture derivation. Corpus entries are RAW source labeled
/// `src/...` relative to this root. No process-global state or leaked authority.
pub fn scan_hook_producers(
    root: &Path,
    raw_enum: &str,
    corpus: &[(String, String)],
) -> Result<ProducerReport, GuardError> {
    let report = derive_hook_producers(root, raw_enum, corpus)?;
    let missing: Vec<_> = report
        .producers
        .iter()
        .filter(|(_, sites)| sites.is_empty())
        .map(|(v, _)| v.clone())
        .collect();
    let dangling: Vec<_> = report
        .inventory
        .aliases
        .iter()
        .filter(|(_, v)| report.producers.get(v).is_none_or(|sites| sites.is_empty()))
        .map(|(alias, v)| format!("{alias} -> {v}"))
        .collect();
    let aliasless: Vec<_> = report
        .inventory
        .variants
        .iter()
        .filter(|v| {
            !report
                .inventory
                .aliases
                .iter()
                .any(|(_, target)| target == *v)
        })
        .collect();
    if !missing.is_empty() || !dangling.is_empty() || !aliasless.is_empty() {
        return Err(GuardError(format!("{}: missing HookEvent producers outside src/extension/: {missing:?}; aliases whose target is never fired: {dangling:?}; variants without aliases: {aliasless:?}; dispatchers: {:?}", root.display(), report.dispatchers)));
    }
    Ok(report)
}

/// Authority is supplied by this root's filesystem, including for fixtures.
pub fn scan_hook_fixture(root: &Path, authority: &Path) -> Result<ProducerReport, GuardError> {
    let fail = |message: String| GuardError(format!("{}: {message}", root.display()));
    if authority.as_os_str().is_empty()
        || authority
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(fail(format!(
            "invalid authority path {}",
            authority.display()
        )));
    }
    let path = root.join(authority);
    let raw =
        std::fs::read_to_string(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let corpus = rust_sources_under_root(root, Path::new("src")).map_err(fail)?;
    scan_hook_producers(root, &raw, &corpus)
}

pub fn scan_production_hook_producers() -> Result<ProducerReport, GuardError> {
    scan_hook_fixture(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        Path::new("crates/aleph-slot-api/src/hooks.rs"),
    )
}

#[cfg(test)]
fn production_report() -> ProducerReport {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(root.join("crates/aleph-slot-api/src/hooks.rs"))
        .expect("API legacy HookEvent authority");
    let corpus = rust_sources_under_root(root, Path::new("src")).expect("Core producer sources");
    derive_hook_producers(root, &raw, &corpus).expect("nonempty production census")
}

#[cfg(test)]
#[test]
fn every_hook_event_has_a_producer_outside_src_extension() {
    let report = production_report();
    let dispatchers = &report.dispatchers;
    let mut silent = Vec::new();
    for event in HookEvent::ALL {
        let variant = format!("{event:?}");
        if report.producers[&variant].is_empty() {
            silent.push(variant);
        }
    }
    assert!(
        silent.is_empty(),
        "declared hook events with no dispatch outside src/extension/: {silent:?} \
         (dispatchers: {dispatchers:?})"
    );
}

#[cfg(test)]
#[test]
fn every_alias_targets_a_fired_event() {
    let report = production_report();
    let aliases = &report.inventory.aliases;
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
        .filter(|(_, variant)| report.producers[variant].is_empty())
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

#[cfg(test)]
#[test]
fn all_lists_every_variant_exactly_once() {
    // `ALL` is the roster the census walks; a variant left out of it would
    // also be left out of the census. Every declared variant name read off
    // the enum source must parse (through its PascalCase alias) into `ALL`.
    let report = production_report();
    let declared = &report.inventory.variants;
    assert_eq!(
        declared.iter().collect::<BTreeSet<_>>().len(),
        declared.len()
    );
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

#[cfg(test)]
#[test]
fn the_dispatcher_derivation_sees_its_seeds_and_the_wrappers_they_reach() {
    // The seeds are real `HookExecutor` methods (a rename is red here, not a
    // silently empty dispatcher set), and the closure reaches the public
    // fire-and-forget wrapper every global seam goes through.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = rust_sources_under_root(root, Path::new("src")).unwrap();
    let corpus: Vec<_> = corpus
        .into_iter()
        .map(|(rel, raw)| {
            let code = code_text(&production_text(&root.join(&rel), &raw));
            (rel, code)
        })
        .collect();
    let executor = corpus
        .iter()
        .find(|(rel, _)| rel == "src/extension/hooks/executor.rs")
        .map(|(_, code)| code.as_str())
        .expect("executor.rs is in the corpus");
    let defined: Vec<&str> = fns(executor).into_iter().map(|(n, _, _)| n).collect();
    for seed in DISPATCH_SEEDS {
        assert!(defined.contains(&seed), "no `fn {seed}` in executor.rs");
    }
    let set = dispatchers(&corpus);
    assert!(
        set.contains("fire_global_observer"),
        "derived dispatchers: {set:?}"
    );
}
