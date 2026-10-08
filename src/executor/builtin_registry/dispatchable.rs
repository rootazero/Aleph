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

/// Tool names the model can be told about, from all three registration
/// shapes.
///
/// Panics if any scan finds implausibly few names: the failure mode this
/// guards against is a scanner that stops matching (a refactor renames `reg`,
/// rustfmt reflows the catalog, a future shape moves) and thereafter passes
/// by finding nothing.
///
/// `pub` so cross-crate consumers (the `dangerous_tools` test that pins
/// every denylist entry against a real tool) can verify a conditionally
/// registered tool exists without rebuilding the registry. The set returned
/// here is a SOURCE-LEVEL set, not a runtime one: a tool registered in the
/// constructor's `if let Some(ref X) = config.X { … }` block will appear here
/// regardless of whether `config.X` is `Some` at runtime — the assertion the
/// census exists to make is "if you wire it, you dispatch it", which is a
/// source-level invariant.
pub fn advertised_tools() -> std::collections::BTreeSet<String> {
    let catalog_src = production_source(include_str!("definitions.rs"));
    let core_src = production_source(include_str!("builder/core_tools.rs"));
    let constructor_src = production_source(include_str!("builder/constructor/mod.rs"));

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

    // Constructor's conditional registration block: every conditional tool
    // logs either `info!("Registered schema for X")` (single) or
    // `info!("Registered schemas for X, Y, Z")` (plural, comma-separated).
    // Those log lines are the ONLY source-of-truth inventory of which tool
    // names the conditional shape advertises — there is no central table,
    // and a denylist entry that names one of them depends on these being
    // there at source level. Same shape-aware scan as the other two:
    // match the `Registered schema` keyword, walk to ` for `, walk to the
    // closing quote, split on commas, keep snake_case tokens only.
    let mut constructor_count = 0usize;
    for (idx, _) in constructor_src.match_indices("Registered schema") {
        let rest = &constructor_src[idx..];
        let Some(for_offset) = rest.find(" for ") else {
            continue;
        };
        let after_for = &rest[for_offset + " for ".len()..];
        let Some(quote_end) = after_for.find('"') else {
            continue;
        };
        let names_str = &after_for[..quote_end];
        for part in names_str.split(',') {
            let part = part.trim();
            if !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                names.insert(part.to_string());
                constructor_count += 1;
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
    // Stable generic floor + soft cap. The exact count is an artifact of which
    // constructor blocks are unconditional today and will drift as new tools are
    // added; a magic lower bound bakes a specific toolset into the test, and a
    // missing upper bound lets a runaway `for x in tools { reg(x) }` slip through
    // as "fine" because the count would only go up. The diagnostic is not special-
    // cased here; the diagnostic-specific assertion lives in
    // `disabled_diagnostics_registration_is_guarded_by_constructor_optional` and
    // `disabled_diagnostics_has_no_unconditional_catalog_definition`.
    assert!(
        constructor_count >= 1,
        "constructor scan returned 0 — the scanner stopped matching `Registered schema` \
         and would now pass by finding nothing (got {constructor_count})"
    );
    assert!(
        constructor_count <= 32,
        "constructor scan returned {constructor_count} entries — above the 32 soft cap, \
         this suggests unconditional bulk registration swept in unintended tools. \
         Investigate before relaxing this bound."
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

/// Walk forward from `start` (which must point at `{`) to the matching `}`,
/// respecting nested braces and Rust string literals (so a `}` inside a
/// `"…"` does not terminate the block early). Returns the byte offset of
/// the matching `}` or `None` if the braces do not pair.
fn find_matching_close(src: &str, start: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth: i32 = 0;
    let mut i = start;
    let mut in_string = false;
    let mut escape = false;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            i += 1;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
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

    /// The conditional registration shape must be picked up too.
    ///
    /// The constructor's `if let Some(ref X) = config.X { … }` blocks are
    /// the third registration site (alongside `BUILTIN_TOOL_DEFINITIONS` and
    /// `reg(…)`): a tool like `capability_projection_diagnostics` lives ONLY
    /// there, because it must not be advertised unconditionally (the feature
    /// is gated on `ALEPH_CAPABILITY_DIAGNOSTICS=1`). Without this scan the
    /// census would happily report "all clear" about a denylist entry that
    /// names a tool the constructor registers but the catalog does not.
    #[test]
    fn the_census_sees_constructor_conditional_registration() {
        let advertised = advertised_tools();
        // From the constructor's single-name `Registered schema for X` line.
        assert!(
            advertised.contains("config_audit"),
            "conditional registration shape not seen in the census"
        );
        // From the constructor's plural
        // `Registered schemas for X, Y, Z` line.
        assert!(
            advertised.contains("media_understand"),
            "constructor plural-schema line not seen in the census"
        );
    }

    /// The diagnostics tool, once wired, must be in the source census.
    ///
    /// `capability_projection_diagnostics` is conditionally registered
    /// (gated on `BuiltinToolConfig.diagnostics_control.is_some()`, which
    /// startup sets only when `ALEPH_CAPABILITY_DIAGNOSTICS=1`). The census
    /// is source-level, so it sees the name in the constructor's
    /// `if let Some(ref dc) = config.diagnostics_control { … }` block
    /// regardless of the runtime config. The dispatch arm lives next to the
    /// other per-call-handle arms in `tool_registry_impl.rs`; the dispatch
    /// census (`every_advertised_builtin_tool_is_dispatchable`) catches a
    /// missing arm by name.
    #[test]
    fn enabled_diagnostics_is_in_census() {
        let advertised = advertised_tools();
        let dispatchable = dispatchable_tools();
        assert!(
            advertised.contains("capability_projection_diagnostics"),
            "capability_projection_diagnostics is registered in the constructor's \
             conditional block but the source census does not see it — the \
             `Registered schema for capability_projection_diagnostics` log \
             line is missing, or the constructor scan was removed"
        );
        assert!(
            dispatchable.contains("capability_projection_diagnostics"),
            "capability_projection_diagnostics is in the source census but has no \
             arm in `ToolRegistry::execute_tool` — every call would answer \
             \"Unknown tool: capability_projection_diagnostics\" while the \
             description is billed on every request"
        );
    }

    /// The diagnostics tool must NOT be in the **unconditional catalog** in
    /// `src/executor/builtin_registry/definitions.rs`.
    ///
    /// Scope: ONLY `BUILTIN_TOOL_DEFINITIONS` in `definitions.rs`. The
    /// constructor's `if let Some(ref dc) = config.diagnostics_control { … }`
    /// block is a SEPARATE site, covered by
    /// `disabled_diagnostics_registration_is_guarded_by_constructor_optional`
    /// and `enabled_diagnostics_is_in_census`. The dispatch arm in
    /// `tool_registry_impl.rs` is covered by
    /// `disabled_diagnostics_dispatch_arm_is_gated_by_diagnostics_control_some`.
    /// Catalog presence here would advertise the tool regardless of
    /// `ALEPH_CAPABILITY_DIAGNOSTICS` — the test name pins the scope so a
    /// future "simplification" that moves the entry from the conditional
    /// block to the unconditional catalog fails here, not by accident.
    #[test]
    fn disabled_diagnostics_has_no_unconditional_catalog_definition() {
        let catalog_src = production_source(include_str!("definitions.rs"));
        // The catalog uses `name: "foo",` rows. Same shape the census uses.
        let mut cursor = 0usize;
        let mut found = false;
        while let Some(idx) = catalog_src[cursor..].find("name:") {
            let abs = cursor + idx;
            if let Some(n) = quoted_after(&catalog_src[abs + "name:".len()..]) {
                if n == "capability_projection_diagnostics" {
                    found = true;
                    break;
                }
            }
            cursor = abs + "name:".len();
        }
        assert!(
            !found,
            "capability_projection_diagnostics is unconditionally listed in \
             BUILTIN_TOOL_DEFINITIONS — the tool would advertise regardless of \
             ALEPH_CAPABILITY_DIAGNOSTICS, defeating the conditional gate. \
             Remove the entry from definitions.rs and keep the registration in \
             the constructor's `if let Some(ref dc) = config.diagnostics_control` \
             block."
        );
    }

    /// The diagnostics tool's constructor registration must be entirely
    /// inside the `if let Some(ref dc) = config.diagnostics_control { … }`
    /// block in `src/executor/builtin_registry/builder/constructor/mod.rs`.
    ///
    /// Scope: ONLY the constructor's `if let Some(ref dc) = config
    /// .diagnostics_control` block. Catalog absence is pinned by
    /// `disabled_diagnostics_has_no_unconditional_catalog_definition`; the
    /// dispatch-arm gate is pinned by
    /// `disabled_diagnostics_dispatch_arm_is_gated_by_diagnostics_control_some`.
    /// The constructor is the only `reg(...)` registration site for this tool,
    /// so a "simplification" that pulls the registration out of the
    /// conditional block (e.g. always-call `reg(...)` and only conditionally
    /// append to `tools`) would advertise it on every startup regardless of
    /// the env var — this test reads the constructor source and asserts the
    /// literal name does not appear outside the conditional block.
    #[test]
    fn disabled_diagnostics_registration_is_guarded_by_constructor_optional() {
        let constructor_src = include_str!("builder/constructor/mod.rs");
        let needle = "capability_projection_diagnostics";

        // Locate the conditional block. The exact pattern is the only
        // registration site for the tool — if the source no longer has it
        // the tool is no longer being wired at all, which is a different
        // failure mode (caught by `enabled_diagnostics_is_in_census`).
        let guard_open = constructor_src
            .find("if let Some(ref dc) = config.diagnostics_control")
            .unwrap_or_else(|| {
                panic!(
                    "constructor's `if let Some(ref dc) = config.diagnostics_control` \
                     block is missing — the conditional registration of \
                     capability_projection_diagnostics is the entire point of the gate; \
                     the unconditional-catalog absence in \
                     `disabled_diagnostics_has_no_unconditional_catalog_definition` \
                     does not, by itself, prevent advertising"
                )
            });
        let brace_open = constructor_src[guard_open..]
            .find('{')
            .map(|i| guard_open + i)
            .expect("conditional block has no opening brace");
        let brace_close = find_matching_close(constructor_src, brace_open)
            .expect("conditional block has no matching close brace");

        // Scan the entire constructor source for every occurrence of the
        // literal. Any occurrence OUTSIDE the conditional block fails the
        // test, with the byte offset printed for diagnosis.
        let mut cursor = 0usize;
        let mut outside: Vec<usize> = Vec::new();
        while let Some(idx) = constructor_src[cursor..].find(needle) {
            let abs = cursor + idx;
            if abs < brace_open || abs > brace_close {
                outside.push(abs);
            }
            cursor = abs + needle.len();
        }
        assert!(
            outside.is_empty(),
            "capability_projection_diagnostics is registered outside the \
             `if let Some(ref dc) = config.diagnostics_control` block at byte \
             offsets {outside:?} in builder/constructor/mod.rs. The tool would \
             be advertised unconditionally; keep every registration inside the \
             conditional block. The catalog absence is checked separately by \
             `disabled_diagnostics_has_no_unconditional_catalog_definition`."
        );
    }

    /// The diagnostics tool's dispatch arm must require
    /// `BuiltinToolRegistry.diagnostics_control` to be `Some` before any
    /// handler code runs.
    ///
    /// Scope: ONLY the `"capability_projection_diagnostics" =>` arm in
    /// `src/executor/builtin_registry/registry/tool_registry_impl.rs`.
    /// The constructor's conditional is pinned by
    /// `disabled_diagnostics_registration_is_guarded_by_constructor_optional`;
    /// the catalog is pinned by
    /// `disabled_diagnostics_has_no_unconditional_catalog_definition`. The
    /// arm must reject with an error that names the missing dep so that
    /// "diagnostics enabled, but `diagnostics_control` was never installed"
    /// surfaces a coherent message instead of an opaque panic or a handler
    /// that quietly no-ops. We assert the gate shape (the
    /// `self.diagnostics_control.as_ref().ok_or_else(AlephError::tool(...))`
    /// pattern is the same one `media_understand` / `config_audit` use).
    #[test]
    fn disabled_diagnostics_dispatch_arm_is_gated_by_diagnostics_control_some() {
        let dispatch_src = production_source(include_str!("registry/tool_registry_impl.rs"));

        // Find the `"capability_projection_diagnostics" =>` arm and bound
        // its body to the matching `}` of the `Box::pin(async move { ... })`
        // block. Walking the braces is robust to body length, multi-line
        // strings, and any interior braces — the previous `find("\"")`
        // approach mistook the closing quote of the error message for the
        // next arm's opening quote and the 4 KiB cap truncated the body
        // before the asserted phrase could be reached.
        let arm_open = match dispatch_src.find("\"capability_projection_diagnostics\" =>") {
            Some(off) => off,
            None => panic!(
                "no `capability_projection_diagnostics` arm in \
                 registry/tool_registry_impl.rs — the dispatch census in \
                 `enabled_diagnostics_is_in_census` would have caught this; \
                 if you are reading this, that test is gone too"
            ),
        };
        let body_start = arm_open + "\"capability_projection_diagnostics\" =>".len();
        let async_brace_open = dispatch_src[body_start..]
            .find("async move {")
            .map(|i| body_start + i + "async move ".len())
            .unwrap_or_else(|| panic!(
                "diagnostics dispatch arm has no `async move {{` after the `=>` — \
                 the gate shape this test pins (a `Box::pin(async move {{ ... }})` \
                 returning `Result<_, AlephError>`) is no longer there. The arm body \
                 up to 4 KiB after the `=>` is:\n{}",
                &dispatch_src[body_start..(body_start + 4096).min(dispatch_src.len())]
            ));
        let async_brace_close = find_matching_close(&dispatch_src, async_brace_open)
            .unwrap_or_else(|| panic!(
                "diagnostics dispatch arm's `async move` block has no matching `}}` — \
                 the block is unterminated; either the gate was deleted or the source \
                 was edited mid-string. The arm body up to 4 KiB after the `=>` is:\n{}",
                &dispatch_src[body_start..(body_start + 4096).min(dispatch_src.len())]
            ));
        let arm_body = &dispatch_src[body_start..=async_brace_close];

        // The arm must read `self.diagnostics_control` (the same shape as
        // the `media_pipeline` / `config_audit` arms) and surface a
        // diagnostic-specific error message when the slot is None.
        assert!(
            arm_body.contains("self.diagnostics_control"),
            "diagnostics dispatch arm does not gate on `self.diagnostics_control` — \
             the tool would dispatch without the runtime enablement handle. \
             The arm body is:\n{arm_body}"
        );
        assert!(
            arm_body.contains("ok_or_else"),
            "diagnostics dispatch arm does not short-circuit on the None branch — \
             the call would either panic on `.unwrap()` or fall through to \
             `parse_request` before the gate. The arm body is:\n{arm_body}"
        );
        assert!(
            arm_body.contains("no DiagnosticControl configured"),
            "diagnostics dispatch arm's None-branch error message is missing the \
             \"no DiagnosticControl configured\" phrase — operators reading a \
             rejected call would not know which env var to set. The arm body \
             is:\n{arm_body}"
        );
    }
}
