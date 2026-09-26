//! Census: every production `AiProvider` / `ToolService` impl either answers
//! the "must forward" questions itself or is a named leaf.
//!
//! These trait methods default to an answer that is only right for a leaf —
//! a provider or tool service that decides for itself. A decorator that
//! forgets to forward one does not fail to compile; it silently answers the
//! default for everything behind it (the estimator counts reasoning the wire
//! never sends, the local passes keep pruning what the server already clears,
//! a footer names a tool the model cannot call). The list of impls is read
//! from the source, not written here, so an impl added tomorrow is judged too:
//! one that neither forwards nor is a declared leaf turns this red by name.

use std::collections::BTreeSet;

/// Methods every non-leaf `AiProvider` must override.
const PROVIDER_FORWARDS: &[&str] = &["reasoning_replay", "clears_tool_results_server_side"];
/// Methods every non-leaf `ToolService` must override.
const TOOL_SERVICE_FORWARDS: &[&str] = &["recovery_tools"];

/// Impls that answer for themselves: nothing sits behind them, so the trait
/// default is their true answer.
const PROVIDER_LEAVES: &[&str] = &[
    "OllamaProvider",
    "MockProvider",
    "RecordingMockProvider",
    "DummyProvider",
];
const TOOL_SERVICE_LEAVES: &[&str] = &["NullToolService"];

/// Impls that OWN the answer: they override the methods and derive the value
/// from their own state (the wire family and host, the dispatcher's gates)
/// rather than from something they wrap. Every other override must delegate —
/// call the same method on what it wraps — or it is a decorator answering a
/// literal for everything behind it.
const SOURCES: &[&str] = &["HttpProvider", "ScopedToolService"];

struct Impl {
    trait_name: String,
    type_name: String,
    at: String,
    /// Method name → body text, for every `fn` in the impl.
    methods: std::collections::BTreeMap<String, String>,
}

#[test]
fn every_decorator_forwards_what_the_trait_defaults_for_a_leaf() {
    let impls = production_impls();
    let mut offenders = Vec::new();
    let mut seen_leaves = BTreeSet::new();
    for i in &impls {
        let (forwards, leaves) = match i.trait_name.as_str() {
            "AiProvider" => (PROVIDER_FORWARDS, PROVIDER_LEAVES),
            "ToolService" => (TOOL_SERVICE_FORWARDS, TOOL_SERVICE_LEAVES),
            other => panic!("census matched an unexpected trait {other} at {}", i.at),
        };
        if leaves.contains(&i.type_name.as_str()) {
            seen_leaves.insert(i.type_name.clone());
            continue;
        }
        let missing: Vec<&str> = forwards
            .iter()
            .copied()
            .filter(|m| !i.methods.contains_key(*m))
            .collect();
        if !missing.is_empty() {
            offenders.push(format!(
                "{} for {} ({}) does not override {:?}",
                i.trait_name, i.type_name, i.at, missing
            ));
        }
        // An override that answers without asking what it wraps is the same
        // defect as no override, spelled so this check would miss it — unless
        // the impl is the one that owns the answer.
        if !SOURCES.contains(&i.type_name.as_str()) {
            let answering: Vec<&str> = forwards
                .iter()
                .copied()
                .filter(|m| {
                    i.methods
                        .get(*m)
                        .is_some_and(|body| !body.contains(&format!(".{m}(")))
                })
                .collect();
            if !answering.is_empty() {
                offenders.push(format!(
                    "{} for {} ({}) overrides {:?} without delegating to what it wraps",
                    i.trait_name, i.type_name, i.at, answering
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a production impl neither forwards the trait's leaf-only defaults nor is \
         a declared leaf — forward them to what it wraps, or, if it truly answers \
         for itself, add it to the leaf list here:\n  {}",
        offenders.join("\n  ")
    );

    // The leaf lists are claims about the source; a stale entry is a claim
    // nobody checks.
    let declared: BTreeSet<String> = PROVIDER_LEAVES
        .iter()
        .chain(TOOL_SERVICE_LEAVES)
        .map(|s| (*s).to_string())
        .collect();
    let found: BTreeSet<String> = impls.iter().map(|i| i.type_name.clone()).collect();
    let stale_sources: Vec<&&str> = SOURCES.iter().filter(|s| !found.contains(**s)).collect();
    assert!(
        stale_sources.is_empty(),
        "source entries with no impl in src/: {stale_sources:?}"
    );
    let stale: Vec<&String> = declared.difference(&seen_leaves).collect();
    assert!(
        stale.is_empty(),
        "leaf entries with no impl in src/: {stale:?}"
    );
}

/// The scan's own floor: it must see the decorators we know exist, or a
/// green above could mean it saw nothing.
#[test]
fn the_census_sees_the_known_decorators() {
    let found: BTreeSet<String> = production_impls()
        .into_iter()
        .map(|i| i.type_name)
        .collect();
    for known in [
        "HttpProvider",
        "FailoverProvider",
        "MeteringProvider",
        "ThinkLevelProvider",
        "ModelOverrideProvider",
        "MoaProvider",
        "ScopedToolService",
        "McpScopedToolService",
        "AllowlistToolService",
    ] {
        assert!(found.contains(known), "census did not find {known}");
    }
}

fn production_impls() -> Vec<Impl> {
    use crate::utils::source_scan::{code_text, production_text, rust_sources_under};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    for (rel, src) in rust_sources_under(&root) {
        let code = code_text(&production_text(std::path::Path::new(&rel), &src));
        out.extend(impls_in(&code, &rel));
    }
    out
}

/// Every `impl … AiProvider for T` / `impl … ToolService for T` in `code`
/// (production text with comments and literal payloads already removed).
fn impls_in(code: &str, rel: &str) -> Vec<Impl> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(off) = code[search..].find("impl") {
        let start = search + off;
        search = start + 4;
        if !word_boundary(code, start, 4) {
            continue;
        }
        let Some(brace) = code[start..].find('{').map(|b| start + b) else {
            break;
        };
        let header = &code[start..brace];
        let Some(for_at) = header.find(" for ") else {
            continue;
        };
        let trait_name = header[..for_at]
            .trim_end()
            .rsplit(|c: char| c.is_whitespace() || c == ':' || c == '>')
            .next()
            .unwrap_or("");
        if trait_name != "AiProvider" && trait_name != "ToolService" {
            continue;
        }
        let type_name: String = header[for_at + 5..]
            .trim()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let end = block_end(code, brace);
        let line = code[..start].matches('\n').count() + 1;
        out.push(Impl {
            trait_name: trait_name.to_string(),
            type_name,
            at: format!("{rel}:{line}"),
            methods: fn_bodies(&code[brace..end]),
        });
        search = end;
    }
    out
}

/// Every `fn` in an impl block, with its body (signature to closing brace).
fn fn_bodies(block: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let mut search = 0;
    while let Some(off) = block[search..].find("fn ") {
        let at = search + off;
        search = at + 3;
        if !word_boundary(block, at, 2) {
            continue;
        }
        let name: String = block[at + 3..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let Some(open) = block[at..].find('{').map(|o| at + o) else {
            break;
        };
        let end = block_end(block, open);
        out.insert(name, block[at..=end.min(block.len() - 1)].to_string());
        search = end;
    }
    out
}

fn word_boundary(code: &str, at: usize, len: usize) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let before = code[..at].chars().next_back().is_none_or(|c| !is_ident(c));
    let after = code[at + len..].chars().next().is_none_or(|c| !is_ident(c));
    before && after
}

fn block_end(code: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return open + i;
                }
            }
            _ => {}
        }
    }
    code.len()
}
