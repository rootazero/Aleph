//! Drift guard for the deployed configuration guides.
//!
//! `docs/guides/*.md` is embedded in the binary (see
//! `crate::config::guides::GUIDE_FILES`) and deployed to `~/.aleph/guides/`,
//! where operators *and* the model read it. A key the server no longer reads
//! is not a typo in a doc: it is an instruction to write a `config.toml` the
//! loader will reject — and the loader's dead-key scan is the only thing that
//! would say so, after the fact.
//!
//! The guard parses the TOML fences of every deployed guide and checks each
//! section and key against the schema the server actually generates, so the
//! guide cannot outlive the fields it teaches.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::skill_doc_drift::collect_field_names;

/// Roots whose children are static structs.
///
/// Every key under these roots must exist in the schema. Roots that are maps
/// (`channels`, `profiles`) hold operator data rather than schema, so their
/// children are deliberately not policed.
const SCOPED_ROOTS: &[&str] = &[
    "agents",
    "behavior",
    "cron",
    "execution",
    "general",
    "generation",
    "memory",
    "policies",
    "providers",
    "search",
];

/// Roots that belong to a foreign dialect (`plugin.toml`), not to `config.toml`.
///
/// A guide may embed a plugin manifest; those sections are not config sections
/// and must not be read as such.
const FOREIGN_ROOTS: &[&str] = &[
    "aleph",
    "capabilities",
    "dependencies",
    "hooks",
    "lib",
    "package",
    "permissions",
    "plugin",
    "prompt",
    "services",
    "tools",
];

/// The minimum number of guides the parser must read.
///
/// A guide set that shrinks to nothing turns this parser into the "not
/// installed" face of a guard that cannot fail: a permanent, meaningless green.
const MIN_GUIDES_SCANNED: usize = 8;

/// The minimum number of TOML sections the parser must find.
///
/// A rewritten fence marker does the same thing to the section arm.
const MIN_SECTIONS_SCANNED: usize = 15;

/// The minimum number of keys the parser must check.
const MIN_KEYS_SCANNED: usize = 40;

/// One drift finding, rendered as the guard's failure output.
#[derive(Debug)]
struct Finding {
    guide: String,
    section: String,
    detail: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]: {}", self.guide, self.section, self.detail)
    }
}

/// The section a TOML header names, and whether it opens an array of tables.
///
/// `[[agents.list]]` and `[agents.list]` differ in the schema, not in the
/// prose: one is a list of rows, the other a single table.
#[derive(Clone)]
struct Header {
    path: String,
    is_array: bool,
}

/// Read the TOML fences of a markdown document.
fn toml_fences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let Some(nl) = after.find('\n') else { break };
        let info = after[..nl].trim().to_ascii_lowercase();
        let body = &after[nl + 1..];
        let Some(end) = body.find("```") else { break };
        if info == "toml" {
            out.push(&body[..end]);
        }
        rest = &body[end + 3..];
    }
    out
}

/// Parse a section header (`[a.b]` / `[[a.b]]`).
fn parse_header(line: &str) -> Option<Header> {
    let mut inner = line.strip_prefix('[')?;
    let is_array = inner.starts_with('[');
    if is_array {
        inner = &inner[1..];
    }
    let inner = inner.strip_suffix(']')?;
    let inner = if is_array {
        inner.strip_suffix(']')?
    } else {
        inner
    };
    if inner.is_empty() {
        return None;
    }
    Some(Header {
        path: inner.to_string(),
        is_array,
    })
}

/// The bare key a TOML assignment defines, if the line is one.
///
/// A call such as `cron_manage(action="create")` is not an assignment: its
/// `=` sits behind a `(`, which no bare key may contain.
fn parse_key(line: &str) -> Option<String> {
    let end = line.find('=')?;
    let name = line[..end].trim();
    if name.is_empty() {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(name.to_string())
}

/// How many `{`/`[` the line opens beyond the ones it closes.
fn nesting_delta(line: &str) -> i32 {
    let mut delta = 0;
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => in_string = !in_string,
            '\\' if in_string => {
                chars.next();
            }
            '{' | '[' if !in_string => delta += 1,
            '}' | ']' if !in_string => delta -= 1,
            _ => {}
        }
    }
    delta
}

/// The sections and keys a TOML fence defines.
fn scan_fence(fence: &str) -> Vec<(Header, Option<String>)> {
    let mut out = Vec::new();
    let mut header = Header {
        path: String::new(),
        is_array: false,
    };
    let mut depth = 0i32;
    for line in fence.lines() {
        let trimmed = line.trim();
        if depth == 0 {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                depth += nesting_delta(trimmed);
                continue;
            }
            if let Some(parsed) = parse_header(trimmed) {
                header = parsed;
                out.push((header.clone(), None));
                continue;
            }
            if let Some(key) = parse_key(trimmed) {
                out.push((header.clone(), Some(key)));
            }
        }
        depth += nesting_delta(trimmed);
    }
    out
}

/// The schema node candidates a path resolves to.
fn variants<'a>(node: &'a Value, defs: &'a Map<String, Value>, depth: u8) -> Vec<&'a Value> {
    let mut out = Vec::new();
    let Some(depth) = depth.checked_sub(1) else {
        return out;
    };
    if let Some(name) = node
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix("#/$defs/"))
    {
        if let Some(target) = defs.get(name) {
            out.extend(variants(target, defs, depth));
        }
    }
    out.push(node);
    for key in ["allOf", "anyOf", "oneOf"] {
        if let Some(branches) = node.get(key).and_then(Value::as_array) {
            for branch in branches {
                if branch.is_object() {
                    out.extend(variants(branch, defs, depth));
                }
            }
        }
    }
    out
}

/// Resolve a dotted section path against the schema.
///
/// Follows the composition keywords rather than assuming a shape, because the
/// same section can arrive as a bare `$ref` (`[general]`), an `anyOf` with a
/// null branch (`Option<BehaviorConfig>`), an array (`[[agents.list]]`) or a
/// map (`[providers.*]`). Every branch is unioned: this can only widen the
/// accepted set, which is the direction this guard errs in on purpose.
fn resolve<'a>(
    root: &'a Value,
    defs: &'a Map<String, Value>,
    path: &[&str],
) -> Option<Vec<&'a Value>> {
    let mut current = variants(root, defs, 8);
    for segment in path {
        let mut next: Vec<&'a Value> = Vec::new();
        for node in &current {
            for variant in variants(node, defs, 8) {
                if let Some(child) = variant
                    .get("properties")
                    .and_then(Value::as_object)
                    .and_then(|props| props.get(*segment))
                {
                    next.extend(variants(child, defs, 8));
                }
                // A map-valued section: the segment is a map key and the map's
                // value type is the schema of what follows.
                if let Some(value) = variant
                    .get("additionalProperties")
                    .filter(|v| v.is_object())
                {
                    next.extend(variants(value, defs, 8));
                }
                // An array-valued section: the segment addresses a field of the
                // element (`[[agents.list]]` then `[agents.list.identity]`).
                // The element's own field wins over the element schema, so a
                // nested table is checked against ITS fields and not against
                // the row's.
                if let Some(items) = variant.get("items").filter(|v| v.is_object()) {
                    let element = variants(items, defs, 8);
                    let field: Vec<&'a Value> = element
                        .iter()
                        .filter_map(|item| {
                            item.get("properties")
                                .and_then(Value::as_object)
                                .and_then(|props| props.get(*segment))
                        })
                        .flat_map(|child| variants(child, defs, 8))
                        .collect();
                    if field.is_empty() {
                        next.extend(element);
                    } else {
                        next.extend(field);
                    }
                }
            }
        }
        if next.is_empty() {
            return None;
        }
        current = next;
    }
    Some(current)
}

/// The live top-level section names of `config.toml`.
fn live_roots(schema: &Value) -> BTreeSet<String> {
    let mut roots: BTreeSet<String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default();
    for (path, is_retired) in crate::config::dead_keys::tolerated_roots() {
        if is_retired {
            continue;
        }
        if let Some(root) = path.split('.').next() {
            roots.insert(root.to_string());
        }
    }
    roots
}

/// Audit every deployed guide against the schema the server generates.
fn audit() -> (Vec<Finding>, usize, usize, usize) {
    let schema = crate::config::generate_config_schema_json();
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let roots = live_roots(&schema);

    let mut findings = Vec::new();
    let mut guides = 0usize;
    let mut sections = 0usize;
    let mut keys = 0usize;

    for (name, content) in crate::config::guides::GUIDE_FILES {
        guides += 1;
        let mut seen_headers: BTreeSet<String> = BTreeSet::new();
        for fence in toml_fences(content) {
            for (header, key) in scan_fence(fence) {
                let path: Vec<&str> = header.path.split('.').collect();
                if !seen_headers.insert(header.path.clone()) {
                    continue;
                }
                sections += 1;

                let root = path[0];
                if FOREIGN_ROOTS.contains(&root) {
                    continue;
                }
                if !roots.contains(root) {
                    findings.push(Finding {
                        guide: (*name).to_string(),
                        section: header.path.clone(),
                        detail: format!("`{root}` is not a config.toml section"),
                    });
                    continue;
                }
                if !SCOPED_ROOTS.contains(&root) {
                    continue;
                }
                let Some(nodes) = resolve(&schema, &defs, &path) else {
                    findings.push(Finding {
                        guide: (*name).to_string(),
                        section: header.path.clone(),
                        detail: "section does not exist in the config schema".to_string(),
                    });
                    continue;
                };
                // A header's shape must match the schema's: `[[x]]` is a list
                // of rows, `[x]` is one table.
                let schema_is_array = nodes
                    .iter()
                    .any(|node| node.get("items").is_some() && node.get("properties").is_none());
                if schema_is_array != header.is_array {
                    findings.push(Finding {
                        guide: (*name).to_string(),
                        section: header.path.clone(),
                        detail: if header.is_array {
                            "array-of-tables header, but the schema holds a single table"
                                .to_string()
                        } else {
                            "table header, but the schema holds a list of rows".to_string()
                        },
                    });
                }
                let _ = key;
            }
        }

        // The key check runs per section header, so it needs the resolved
        // names; recompute them here, keyed by section path.
        let mut names_by_section: BTreeSet<(String, String)> = BTreeSet::new();
        for fence in toml_fences(content) {
            for (header, key) in scan_fence(fence) {
                let Some(key) = key else { continue };
                names_by_section.insert((header.path.clone(), key));
            }
        }
        let mut current: Option<String> = None;
        for fence in toml_fences(content) {
            let mut depth = 0i32;
            for line in fence.lines() {
                let trimmed = line.trim();
                if depth == 0 {
                    if let Some(header) = parse_header(trimmed) {
                        current = Some(header.path.clone());
                    } else if let Some(key) = parse_key(trimmed) {
                        let Some(section) = current.clone() else {
                            continue;
                        };
                        let path: Vec<&str> = section.split('.').collect();
                        let root = path[0];
                        if FOREIGN_ROOTS.contains(&root) || !SCOPED_ROOTS.contains(&root) {
                            continue;
                        }
                        if !roots.contains(root) {
                            continue;
                        }
                        let Some(nodes) = resolve(&schema, &defs, &path) else {
                            continue;
                        };
                        // A map-valued section holds operator data, not schema.
                        let is_map = nodes.iter().any(|node| {
                            node.get("additionalProperties").is_some()
                                && node.get("properties").is_none()
                        });
                        if is_map {
                            continue;
                        }
                        let mut names = BTreeSet::new();
                        for node in &nodes {
                            collect_field_names(node, &defs, 8, &mut names);
                        }
                        keys += 1;
                        if !names.contains(&key) {
                            findings.push(Finding {
                                guide: (*name).to_string(),
                                section: section.clone(),
                                detail: format!("`{key}` is not a field of this section"),
                            });
                        }
                    }
                }
                depth += nesting_delta(trimmed);
            }
        }
    }

    (findings, guides, sections, keys)
}

#[test]
fn every_deployed_guide_section_and_key_exists_in_the_config_schema() {
    let (findings, guides, sections, keys) = audit();
    assert!(
        guides >= MIN_GUIDES_SCANNED,
        "the guard read {guides} guides, expected at least {MIN_GUIDES_SCANNED} — \
         the guide set shrank and the audit no longer means anything"
    );
    assert!(
        sections >= MIN_SECTIONS_SCANNED,
        "the guard parsed {sections} TOML sections, expected at least {MIN_SECTIONS_SCANNED} — \
         the fence parser no longer sees the guides"
    );
    assert!(
        keys >= MIN_KEYS_SCANNED,
        "the guard checked {keys} keys, expected at least {MIN_KEYS_SCANNED} — \
         the key arm has silently degraded to no checking"
    );
    assert!(
        findings.is_empty(),
        "the deployed guides drifted from the config schema:\n{}",
        findings
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
