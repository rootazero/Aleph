//! Census: every tool the model is *told about* must be a tool a call can
//! *reach*.
//!
//! # The gap this closes
//!
//! Advertising a builtin and dispatching one are separate acts with no
//! compiler between them. A tool becomes visible to the model through either
//! of two registration shapes:
//!
//! | shape | file | what it does |
//! |---|---|---|
//! | `BUILTIN_TOOL_DEFINITIONS` entry | `definitions.rs` | catalog row, progressive disclosure, `dangerous_tools` validation |
//! | `reg(tools, "name", …)` | `builder/core_tools.rs` | registry-map row (the ten registry-only tools live here) |
//!
//! Neither reaches `ToolRegistry::execute_tool`, which is a hand-written
//! `match` on the tool name. A tool registered but not matched falls through
//! to `_ =>` and answers `Unknown tool: <name>` — *after* its description has
//! been billed on every request that carried the tool list.
//!
//! That is not hypothetical twice over. Three tools (`select_model`, `doctor`,
//! `config_audit`) were found in this state by an earlier logic audit, and the
//! comment recording the fix sits in `tool_registry_impl.rs` to this day. It
//! did not prevent the fourth: `plugin_manage` shipped in the same state on
//! 2026-08-19 — catalog entry, `create_tool_boxed` arm, `reg(` call, no
//! dispatch arm — and was found by a real-machine fixture rather than by the
//! 16k-test suite, because every in-process test asked a registration surface
//! whether the tool existed, and every one of them correctly said yes.
//!
//! # Why the census reads source instead of listing names
//!
//! A guard that enumerates the tools it checks only covers the world as it
//! stood the day it was written — and this guard's whole subject is a name
//! that someone added to some tables and not others. So both the advertised
//! set and the dispatchable set are recovered from the source text, and a
//! tool added tomorrow is checked without anyone remembering to add it here.
//!
//! The same reasoning applies to the *shapes*: this scanner knows about two
//! registration sites because there are two, and
//! [`advertised_tools`] fails loudly if either scan comes back implausibly
//! small — a silently-zero scan is how a census reports "all clear" about a
//! file it never read.

/// Strip line comments and the trailing `#[cfg(test)]` module from Rust source
/// before scanning it.
///
/// Both halves matter and for different reasons. A tool name mentioned in a
/// doc comment is documentation, not dispatch — and the comment recording an
/// earlier fix names three tools, so a comment-blind scanner would credit them
/// to whichever table it was checking. The test module matters because
/// assertion strings inside it contain tool-name literals in exactly the shapes
/// this scanner looks for, which is how a source-level guard comes to be
/// satisfied by its own test fixtures.
///
/// `\r` is dropped first: this repo is checked out CRLF on Windows, and a
/// separator written `"\n#[cfg(test)]"` matches nothing there — the scan then
/// silently covers the test module too.
fn production_source(src: &str) -> String {
    crate::utils::source_scan::strip_comment_lines(&crate::utils::source_scan::production_prefix(
        src,
    ))
}

/// Tool names the model can be told about, from both registration shapes.
///
/// Panics if either scan finds implausibly few names: the failure mode this
/// guards against is a scanner that stops matching (a refactor renames `reg`,
/// rustfmt reflows the catalog) and thereafter passes by finding nothing.
fn advertised_tools() -> std::collections::BTreeSet<String> {
    let catalog_src = production_source(include_str!("definitions.rs"));
    let core_src = production_source(include_str!("builder/core_tools.rs"));

    let mut names = std::collections::BTreeSet::new();

    // Catalog rows: `name: "foo",`
    let mut catalog_count = 0usize;
    for (idx, _) in catalog_src.match_indices("name:") {
        if let Some(n) = quoted_after(&catalog_src[idx + "name:".len()..]) {
            catalog_count += 1;
            names.insert(n);
        }
    }

    // Registry rows: `reg(tools, "foo", …)` — the name is the SECOND argument,
    // and rustfmt puts each argument on its own line, so this cannot be a
    // single-line match.
    let mut reg_count = 0usize;
    for (idx, _) in core_src.match_indices("reg(") {
        let rest = &core_src[idx + "reg(".len()..];
        // Skip the `tools` argument, then take the first string literal.
        if let Some(comma) = rest.find(',') {
            if let Some(n) = quoted_after(&rest[comma + 1..]) {
                reg_count += 1;
                names.insert(n);
            }
        }
    }

    assert!(
        catalog_count > 100,
        "catalog scan found only {catalog_count} entries — the scanner stopped \
         matching `name:` and would now pass by finding nothing"
    );
    assert!(
        reg_count > 20,
        "core_tools scan found only {reg_count} `reg(` calls — the scanner \
         stopped matching and would now pass by finding nothing"
    );
    names
}

/// First double-quoted `[a-z0-9_]+` literal in `s`, if it starts one (modulo
/// whitespace). Returns `None` for anything else so a `name:` belonging to
/// some unrelated struct does not enter the census.
fn quoted_after(s: &str) -> Option<String> {
    let s = s.trim_start();
    let rest = s.strip_prefix('"')?;
    let end = rest.find('"')?;
    let ident = &rest[..end];
    if !ident.is_empty()
        && ident
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        Some(ident.to_string())
    } else {
        None
    }
}

/// Tool names `ToolRegistry::execute_tool` has a match arm for, including
/// or-patterns (`"a" | "b" => …`).
fn dispatchable_tools() -> std::collections::BTreeSet<String> {
    let src = production_source(include_str!("registry/tool_registry_impl.rs"));
    let mut names = std::collections::BTreeSet::new();
    let mut count = 0usize;

    for (idx, _) in src.match_indices("=>") {
        // Walk backwards over the pattern, collecting string literals joined
        // by `|`. Stops at the first thing that is neither.
        let mut head = &src[..idx];
        loop {
            let trimmed = head.trim_end();
            let Some(open) = trimmed.strip_suffix('"') else {
                break;
            };
            let Some(start) = open.rfind('"') else { break };
            let ident = &open[start + 1..];
            if ident.is_empty()
                || !ident
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                break;
            }
            names.insert(ident.to_string());
            count += 1;
            let before = &open[..start];
            match before.trim_end().strip_suffix('|') {
                Some(next) => head = next,
                None => break,
            }
        }
    }

    assert!(
        count > 100,
        "dispatch scan found only {count} string match arms — the scanner \
         stopped matching and would now pass by finding nothing"
    );
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every advertised builtin must have a dispatch arm.
    ///
    /// Proven to fail by name: deleting the `"plugin_manage" =>` arm from
    /// `tool_registry_impl.rs` makes this print `plugin_manage` and fail —
    /// which is exactly the state the repo shipped in until 2026-08-19.
    #[test]
    fn every_advertised_builtin_tool_is_dispatchable() {
        let advertised = advertised_tools();
        let dispatchable = dispatchable_tools();

        let missing: Vec<&str> = advertised
            .iter()
            .filter(|n| !dispatchable.contains(*n))
            .map(String::as_str)
            .collect();

        assert!(
            missing.is_empty(),
            "these builtin tools are advertised to the model but have no arm in \
             `ToolRegistry::execute_tool`, so every call answers \"Unknown tool\" \
             while their descriptions are billed on every request: {missing:?}\n\
             Add a match arm in \
             src/executor/builtin_registry/registry/tool_registry_impl.rs — \
             registering a tool is not the same act as dispatching one."
        );
    }

    /// The census must be looking at the real tables.
    ///
    /// Without this, a scanner that matched nothing would satisfy the test
    /// above vacuously. The inner `assert!`s cover the "found nothing" case;
    /// this covers "found something, but not the thing we mean" by naming
    /// tools from each shape.
    #[test]
    fn the_census_sees_both_registration_shapes() {
        let advertised = advertised_tools();
        // From BUILTIN_TOOL_DEFINITIONS.
        assert!(
            advertised.contains("file_read"),
            "catalog shape not seen in the census"
        );
        // Registry-only: registered via `reg(` and deliberately absent from
        // the catalog (see REGISTRY_ONLY_DESCRIPTIONS).
        assert!(
            advertised.contains("scratchpad"),
            "registry-only shape not seen in the census"
        );
        assert!(
            advertised.contains("plugin_manage"),
            "plugin_manage not seen — it is registered through both shapes, so \
             its absence means the census is reading the wrong files"
        );
    }

    /// Comments are documentation, not dispatch.
    ///
    /// The comment above the `select_model` arm names three tools. A
    /// comment-blind scanner would credit them to whichever table it read, and
    /// this guard's entire subject is a name present in some tables and not
    /// others.
    #[test]
    fn the_scanner_ignores_comments_and_test_modules() {
        let stripped = production_source(
            "// name: \"ghost_tool\",\nname: \"real_tool\",\n#[cfg(test)]\nname: \"test_tool\",\n",
        );
        assert!(!stripped.contains("ghost_tool"), "comment line survived");
        assert!(!stripped.contains("test_tool"), "test module survived");
        assert!(
            stripped.contains("real_tool"),
            "production line was dropped"
        );
    }

    /// CRLF checkouts must strip the same way.
    ///
    /// A `"\n#[cfg(test)]"` separator matches nothing under CRLF, and the scan
    /// then silently covers the test module — green on CI, wrong on Windows.
    #[test]
    fn the_scanner_strips_the_test_module_on_a_crlf_checkout() {
        let stripped =
            production_source("name: \"real_tool\",\r\n#[cfg(test)]\r\nname: \"test_tool\",\r\n");
        assert!(
            !stripped.contains("test_tool"),
            "CRLF checkout kept the test module in the production scan"
        );
        assert!(stripped.contains("real_tool"));
    }
}
