//! Drift guards for the bundled `self` skill's config documentation.
//!
//! Two assertions over the **same** `include_dir!` tree, deliberately in one
//! module rather than two:
//!
//! 1. `the_bundled_self_skill_names_no_phantom_config_keys` — no config table
//!    names a section or field that does not exist (this file's first half).
//! 2. `no_bundled_snippet_hardcodes_the_aleph_config_path` — no runnable
//!    snippet types the config path instead of deriving it from `ALEPH_HOME`
//!    (second half, with its own rationale block).
//!
//! # Why this exists
//!
//! `skills/self/SKILL.md` is embedded into `aleph-server` at compile time
//! (`include_dir!`, [`crate::bundled::BUNDLED_SKILLS`]) and is read **by the
//! model** to learn how to edit `~/.aleph/config.toml`. A section or field named
//! there that no longer exists is not a stale doc — it is text that makes the
//! model confidently write config that reaches no code, saves without error, and
//! changes nothing.
//!
//! That is not hypothetical. `[agent]` / `[cowork]` were retired on 2026-08-17
//! and were still documented as live on 2026-09-05: the 2026-09-05 correction
//! removed **10 phantom top-level sections**, added **10 live sections that were
//! missing entirely** (including `[gateway]`), and fixed **39 phantom field
//! names across 21 rows**.
//!
//! # The truth is derived, never re-typed
//!
//! A hand-maintained list of valid sections in this file would be a second
//! spelling of the same fact — the exact defect being guarded against. So:
//!
//! * **Sections** = the `properties` of `schema_for!(Config)`
//!   ∪ the roots of [`dead_keys`]'s `TOLERATED` entries with `retired: false`.
//!   The second half is load-bearing: `Config` has **no** `gateway` field by
//!   design (`GatewayConfig::load_default` reads `[gateway]` out of the same
//!   file), so deriving from `Config` alone would condemn a live, load-bearing
//!   section. `security.ssrf` is the same shape (a raw-TOML bridge in
//!   `src/config/load.rs`).
//! * **Retired sections** = single-segment `TOLERATED` entries with
//!   `retired: true`. Those parse and are tolerated but reach no code, so
//!   documenting one as writable is drift, not correctness.
//! * **Fields of a section** = the `properties` of that section's schema node,
//!   resolved through `$ref` / `anyOf` / `allOf` / `oneOf` / `items` /
//!   `additionalProperties`, ∪ any two-segment non-retired `TOLERATED` path
//!   rooted at that section.
//!
//! # Faithfulness of the derivation (why schemars, not `to_value(default)`)
//!
//! The obvious route — serialize a `Config::default()` and read the keys — is
//! **unfaithful in the dangerous direction**. `Config` uses
//! `skip_serializing_if = "Option::is_none"` on 13 sections and
//! `skip_serializing_if = "…is_empty"` on 5 more; every one of them is absent
//! from a default serialization, so the guard would report a **false phantom**
//! for a real, live section. A guard that misfires on correct documentation is
//! more expensive than no guard, because someone will `#[ignore]` it and take
//! the real signal with it.
//!
//! `schema_for!(Config)` is derived from the **type**, so presence does not
//! depend on a value. Per-attribute:
//!
//! | serde attribute | effect on `schema_for!` | consequence here |
//! |---|---|---|
//! | `skip_serializing_if` | none — the property is still emitted | correct |
//! | `default` | only moves the name out of `required` | correct |
//! | `rename = "voice"` | schemars emits the **renamed** key | correct (`voice_local` → `voice`) |
//! | `#[serde(skip)]` | property omitted entirely | correct: `presets_override` lives in `presets.toml`, not `config.toml` |
//! | `flatten` | `schemars::_private::flatten` merges `properties` (or composes `allOf`) | handled by the `allOf`/`anyOf`/`oneOf` recursion |
//! | `alias = "task_reaper"` | **not emitted** — `schemars_derive` 1.2 has no alias support | see the blind spot below |
//!
//! # Where this guard deliberately errs: under-reporting
//!
//! Every judgement call below is resolved toward *missing a real phantom*
//! rather than *flagging a correct row*:
//!
//! * **Aliases are invisible.** `tasks_reaper` carries `alias = "task_reaper"`
//!   and no derivation can see it. The doc writes the alias inside parentheses,
//!   and `strip_parens` drops every parenthesised span before field tokens are
//!   read (the same rule that stops `` `mode` (`auto`/`always_local`) `` from
//!   reporting two phantom fields). So an alias named in parentheses is neither
//!   checked nor flagged; one named **outside** parentheses would be a false
//!   phantom. That is the one over-report this guard can still produce, and it
//!   is bounded to a documented shape.
//! * **Only the first dotted segment of a field token is checked.**
//!   `memory.compression` under `[policies]` is checked as `policies.memory`.
//!   Nested phantoms are not caught.
//! * **Retired *fields* are not flagged**, only retired *sections*. A row that
//!   documents `profiles.*.system_prompt` passes if `ProfileConfig` still
//!   declares the field.
//! * **Field-name resolution unions every schema branch** (all `anyOf`
//!   variants, both sides of a `flatten`), which can only widen the accepted
//!   set.
//! * **Only two tables are read** — the `Config Sections Quick Reference` and
//!   the `Config Path Examples`. Prose, blockquotes and fenced code blocks are
//!   not checked. `skills/self/references/config-editing.md` is not checked at
//!   all: its section list lives inside a fence.
//!
//! # Subset claim, not equality
//!
//! `Key Fields` is a curated selection. The guard asserts only that every name
//! the doc mentions **exists**; a real field absent from the doc is not a
//! failure and is never reported.
//!
//! # The largest blind spot: existence is not liveness
//!
//! This predicate answers "does the type declare this name". It cannot answer
//! "does anything read it", and those are different questions — the difference
//! being the whole reason the `self` skill matters. A field that is declared,
//! parses, round-trips through serde and is then **dropped on the floor** by
//! every production reader passes this guard silently.
//!
//! It has already happened, inside the very pass that was fixing wrong rows:
//!
//! ```text
//! | `rules` | Provider routing rules | `regex`, `provider`, `preferred_model`, `rule_type` |
//! ```
//!
//! `src/config/types/routing.rs`'s module doc (audit 2026-08-26) records that
//! only `regex`, `system_prompt` and — for command rules — `is_builtin` are
//! consumed by the sole production reader
//! (`tool_metadata::registry::registration::register_custom_commands`), while
//! `provider`, `preferred_model`, `strip_prefix`, `intent_type` and `icon`
//! "parse and round-trip cleanly through serde but are silently dropped on the
//! registration path". So that row names three inert fields and **omits the one
//! field that is actually consumed** — and every name in it exists on
//! `RoutingRuleConfig`, so this guard passes it.
//!
//! **Read a green here as: every documented name exists on the type. It does
//! NOT mean the field reaches any code.**
//!
//! ## Why this is not narrowed here
//!
//! Nothing in the tree marks a field as parse-only in a form a test can read:
//!
//! * There is no attribute or wrapper type for it.
//! * [`dead_keys`]'s `TOLERATED` cannot see it *by construction*, and says so:
//!   a key the schema **declares** parses, so `serde_ignored` never fires —
//!   "parses but reaches no code" is a different question from "parses and is
//!   discarded", and only the second is mechanically visible there.
//! * [`crate::config::reload_impact`]'s `INERT_SECTIONS` is the closest thing
//!   to a register, and it is empty (`&[]`) and section-granular — it could not
//!   express "`rules[].provider` is inert while `rules[].regex` is live" even if
//!   it were populated.
//!
//! The only record is English prose in one module doc. Keying a guard on
//! phrases in doc comments would be a guard whose green covers exactly the
//! wordings it happens to recognise, which is the failure mode this module is
//! built to avoid — so the boundary is stated rather than approximated.

use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The path of the bundled doc inside [`crate::bundled::BUNDLED_SKILLS`].
const SKILL_DOC: &str = "self/SKILL.md";

/// Headings whose table is read, and whether its last cell carries field names.
const TABLES: &[(&str, bool)] = &[
    ("## Config Sections Quick Reference", true),
    ("### Config Path Examples", false),
];

/// The minimum number of table rows the parser must find before a clean run is
/// allowed to mean anything.
///
/// A renamed heading or an added fence turns this parser into the "not
/// installed" face of a guard that cannot fail: a permanent, meaningless green.
/// The corrected doc carries ~67 rows across the two tables.
const MIN_ROWS_SCANNED: usize = 10;

/// The minimum number of sections whose field set must actually resolve.
///
/// The section check can stay green while every field check silently degrades
/// to "unchecked" — a schema-shape change (schemars switching `$defs` for
/// `definitions`, say) would do exactly that. This is the second half of the
/// self-defence: the field arm has to be *doing something*.
const MIN_SECTIONS_WITH_FIELDS: usize = 20;

// ---------------------------------------------------------------------------
// Derivation
// ---------------------------------------------------------------------------

/// The authoritative view of `config.toml`'s top-level shape, derived from the
/// code that owns it.
struct Truth {
    /// Sections an operator may legitimately write.
    live: BTreeSet<String>,
    /// Sections that still parse but reach no code.
    retired: BTreeSet<String>,
    /// Per-section field names, for the sections whose schema resolved.
    fields: BTreeMap<String, BTreeSet<String>>,
}

impl Truth {
    fn derive() -> Self {
        let schema = crate::config::generate_config_schema_json();
        let empty = Map::new();
        let defs = schema
            .get("$defs")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let props = schema
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);

        let mut live: BTreeSet<String> = props.keys().cloned().collect();
        let mut retired = BTreeSet::new();
        let mut fields = BTreeMap::new();

        for (name, node) in props {
            let mut names = BTreeSet::new();
            collect_field_names(node, defs, 8, &mut names);
            if !names.is_empty() {
                fields.insert(name.clone(), names);
            }
        }

        // The other half of the truth: sections `Config` does not declare but
        // something else reads (or once read) out of the same file.
        for (path, is_retired) in crate::config::dead_keys::tolerated_roots() {
            let mut segments = path.split('.');
            let Some(root) = segments.next() else {
                continue;
            };
            let rest: Vec<&str> = segments.collect();
            if is_retired {
                // Only whole-section retirements name a section. A nested
                // retired key (`desktop.presence`) is a field-level fact this
                // guard deliberately does not police.
                if rest.is_empty() {
                    retired.insert(root.to_string());
                }
                continue;
            }
            live.insert(root.to_string());
            // A foreign-owned nested path is a legitimate field of its section
            // even though serde cannot see it — `[security.ssrf]` is the live
            // example. Adding it can only widen the accepted set.
            if rest.len() == 1 && rest[0] != "*" {
                fields
                    .entry(root.to_string())
                    .or_default()
                    .insert(rest[0].to_string());
            }
        }

        Self {
            live,
            retired,
            fields,
        }
    }
}

/// Collect the property names a JSON-Schema node can legitimately carry.
///
/// Follows the composition keywords rather than assuming a shape, because the
/// same section can arrive as a bare `$ref` (`[general]`), an `anyOf` with a
/// null branch (`Option<GuardrailsToml>`), an array (`[[personas]]`) or a map
/// (`[providers.*]`). Every branch is unioned: this can only widen the accepted
/// set, which is the direction this guard errs in on purpose.
fn collect_field_names(
    node: &Value,
    defs: &Map<String, Value>,
    depth: u8,
    out: &mut BTreeSet<String>,
) {
    let Some(depth) = depth.checked_sub(1) else {
        return;
    };
    let Some(obj) = node.as_object() else { return };

    if let Some(name) = obj
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix("#/$defs/"))
    {
        if let Some(target) = defs.get(name) {
            collect_field_names(target, defs, depth, out);
        }
    }
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        out.extend(props.keys().cloned());
    }
    for key in ["allOf", "anyOf", "oneOf"] {
        for branch in obj.get(key).and_then(Value::as_array).into_iter().flatten() {
            collect_field_names(branch, defs, depth, out);
        }
    }
    for key in ["items", "additionalProperties", "unevaluatedProperties"] {
        if let Some(inner) = obj.get(key).filter(|v| v.is_object()) {
            collect_field_names(inner, defs, depth, out);
        }
    }
}

// ---------------------------------------------------------------------------
// Markdown reading
// ---------------------------------------------------------------------------

/// Drop fenced code blocks, everywhere. A fence is illustrative TOML, not a
/// claim about the schema.
fn strip_code_fences(doc: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in doc.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            out.push(line);
        }
    }
    out
}

/// The `|`-rows of the table under `heading`, or `None` when the heading is
/// gone. The table ends at the next heading **of any level**.
///
/// "Any level" is load-bearing and was got wrong once. Keying the terminator on
/// `## ` alone means a `### ` heading does not stop the scan, so the section's
/// rows run on into the *next* table — and since the Quick Reference's last cell
/// is field-scanned, the following table's prose column gets read as field
/// names. In the doc as it stands the two tables happen to sit in the order that
/// hides this; reordering them would have produced false phantoms out of
/// ordinary prose. `the_audit_reports_drift_when_it_is_present` pins the fix by
/// putting them in the exposing order.
fn table_rows<'a>(lines: &[&'a str], heading: &str) -> Option<Vec<&'a str>> {
    let start = lines.iter().position(|l| l.starts_with(heading))?;
    let mut rows = Vec::new();
    for line in &lines[start + 1..] {
        if line.starts_with('#') {
            break;
        }
        if line.starts_with('|') {
            rows.push(*line);
        }
    }
    (!rows.is_empty()).then_some(rows)
}

fn cells(row: &str) -> Option<Vec<&str>> {
    let parts: Vec<&str> = row.split('|').collect();
    (parts.len() >= 3).then(|| parts[1..parts.len() - 1].iter().map(|c| c.trim()).collect())
}

/// The contents of every `` `backticked` `` span, in order.
fn backticked(cell: &str) -> Vec<&str> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Blank out every parenthesised span.
///
/// The known false positive of scanning backticks is that enum **values** look
/// exactly like field names. The doc's convention is that values are
/// parenthesised — `` `mode` (`auto`/`always_local`/`always_cloud`) `` — so
/// suppressing parens is what keeps that row from reporting three phantoms.
/// It is also what makes `` (alias `task_reaper`) `` invisible; see the module
/// doc.
fn strip_parens(cell: &str) -> String {
    let mut out = String::with_capacity(cell.len());
    let mut depth = 0usize;
    for c in cell.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// `snake_case` name, optionally dotted, as the docs write config paths.
fn is_field_token(token: &str) -> bool {
    let mut segments = token.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    let head_ok = first
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && first
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    head_ok
        && segments.all(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '*')
        })
}

/// The section a row's first cell names: the first backticked token, rooted.
fn section_of(cell: &str) -> Option<String> {
    let name = backticked(cell).first().copied()?;
    let root = name.split('.').next()?.split(' ').next()?;
    is_field_token(root).then(|| root.to_string())
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Finding {
    kind: &'static str,
    what: String,
    fix: String,
}

struct Report {
    findings: Vec<Finding>,
    rows_scanned: usize,
    tables_found: usize,
    /// Sections named by the doc whose field set could not be resolved. Not a
    /// failure — but never a pass either, so it is printed.
    unchecked: BTreeSet<String>,
}

fn audit(doc: &str, truth: &Truth) -> Report {
    let lines = strip_code_fences(doc);
    let mut report = Report {
        findings: Vec::new(),
        rows_scanned: 0,
        tables_found: 0,
        unchecked: BTreeSet::new(),
    };

    for (heading, has_fields) in TABLES {
        let Some(rows) = table_rows(&lines, heading) else {
            continue;
        };
        report.tables_found += 1;
        for row in rows {
            let Some(cells) = cells(row) else { continue };
            let Some(section) = section_of(cells[0]) else {
                continue;
            };
            report.rows_scanned += 1;

            if truth.retired.contains(&section) {
                report.findings.push(Finding {
                    kind: "RETIRED SECTION",
                    what: format!("[{section}]"),
                    fix: format!(
                        "`{section}` is a TOLERATED {{ retired: true }} entry in \
                         src/config/dead_keys.rs: it still parses but reaches no code. Either \
                         drop the row from the doc, or — if the section was revived — flip its \
                         `retired` flag and give `Config` the field."
                    ),
                });
                continue;
            }
            if !truth.live.contains(&section) {
                report.findings.push(Finding {
                    kind: "PHANTOM SECTION",
                    what: format!("[{section}]"),
                    fix: format!(
                        "`Config` has no `{section}` field and nothing in \
                         src/config/dead_keys.rs::TOLERATED claims it. Correct the doc; if the \
                         section really is read by another parser out of the same file (as \
                         `[gateway]` is), add a TOLERATED {{ retired: false }} entry naming that \
                         reader; if it was retired, add TOLERATED {{ retired: true }}."
                    ),
                });
                continue;
            }

            if !has_fields || cells.len() < 3 {
                continue;
            }
            let Some(known) = truth.fields.get(&section) else {
                report.unchecked.insert(section);
                continue;
            };
            let last = strip_parens(cells[cells.len() - 1]);
            for token in backticked(&last) {
                if !is_field_token(token) {
                    continue;
                }
                let Some(head) = token.split('.').next() else {
                    continue;
                };
                if !known.contains(head) {
                    report.findings.push(Finding {
                        kind: "PHANTOM FIELD",
                        what: format!("{section}.{token}"),
                        fix: format!(
                            "the type behind `[{section}]` declares no `{head}`. Correct the \
                             `Key Fields` cell; if the key was retired, add a TOLERATED entry in \
                             src/config/dead_keys.rs and drop it from the doc."
                        ),
                    });
                }
            }
        }
    }
    report.findings.sort();
    report
}

fn render(doc_name: &str, report: &Report) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "\n{} names {} config key(s) that do not exist.\n\
         This doc is embedded into aleph-server via include_dir! and is read BY THE MODEL: a \
         wrong row here makes it write config that saves cleanly and reaches no code.\n\n",
        doc_name,
        report.findings.len()
    );
    for f in &report.findings {
        let _ = writeln!(out, "  {:<15} {}\n{:<19}{}", f.kind, f.what, "", f.fix);
    }
    let _ = writeln!(
        out,
        "  (scanned {} rows across {} tables; sections with no resolvable field set: {:?})",
        report.rows_scanned, report.tables_found, report.unchecked
    );
    let _ = writeln!(
        out,
        "  The authoritative set is DERIVED, not typed: `schema_for!(Config)`'s properties UNION\n  \
         src/config/dead_keys.rs::TOLERATED {{ retired: false }}. Fix the doc, or fix the code\n  \
         that owns the fact — never this test's expectations."
    );
    out
}

/// The doc as it actually ships: the `include_dir!` snapshot compiled into the
/// binary, not the working tree. Guarding the working tree would leave the
/// artifact unguarded whenever the submodule pointer and the checkout disagree.
fn bundled_skill_doc() -> &'static str {
    crate::bundled::BUNDLED_SKILLS
        .get_file(SKILL_DOC)
        .unwrap_or_else(|| {
            panic!(
                "{SKILL_DOC} is not in the embedded skills tree. Either the file moved (update \
                 SKILL_DOC) or the `skills/` submodule is not checked out — in which case this \
                 guard is not running at all."
            )
        })
        .contents_utf8()
        .expect("the bundled self skill is UTF-8")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The guard. Every section and field the bundled `self` skill names must exist.
///
/// # What a green here does and does not mean
///
/// **Does:** every name the config tables mention is declared by `Config` (or
/// is a non-retired `TOLERATED` path). Nothing in those tables is a phantom.
///
/// **Does not:** that any of those fields reaches code. This predicate is
/// existence, not liveness, and the two come apart — `[rules]`'s row documents
/// `provider` / `preferred_model`, which `routing.rs`'s module doc records as
/// parsed and then silently dropped, and omits `system_prompt`, which is one of
/// the three fields actually consumed. That row passes this test. See the
/// module doc for why the gap is stated rather than closed.
#[test]
fn the_bundled_self_skill_names_no_phantom_config_keys() {
    let truth = Truth::derive();
    let report = audit(bundled_skill_doc(), &truth);

    // Self-defence first: a clean report from a parser that found nothing is
    // the "not installed" face of a guard that cannot fail.
    assert_eq!(
        report.tables_found,
        TABLES.len(),
        "the parser lost a table in {SKILL_DOC}: found {} of {}. The headings it keys on are \
         {:?} — one was renamed, or the table was wrapped in a code fence. This is NOT a pass; \
         fix the parser or the heading before reading anything else here.",
        report.tables_found,
        TABLES.len(),
        TABLES.iter().map(|(h, _)| *h).collect::<Vec<_>>()
    );
    assert!(
        report.rows_scanned > MIN_ROWS_SCANNED,
        "the parser found the headings in {SKILL_DOC} but only {} row(s) (expected > {}). The \
         table shape changed under it — a clean run now proves nothing.",
        report.rows_scanned,
        MIN_ROWS_SCANNED
    );
    assert!(
        truth.fields.len() >= MIN_SECTIONS_WITH_FIELDS,
        "only {} of {} sections resolved to a field set (expected >= {}). The field arm of this \
         guard has silently stopped checking: `schema_for!(Config)` no longer has the shape \
         collect_field_names walks (schemars renaming `$defs`, for instance).",
        truth.fields.len(),
        truth.live.len(),
        MIN_SECTIONS_WITH_FIELDS
    );

    assert!(report.findings.is_empty(), "{}", render(SKILL_DOC, &report));
}

/// The derivation itself, on the three serde attributes that distort it.
///
/// This is the assertion that keeps the module doc's faithfulness table honest:
/// if schemars ever starts emitting `voice_local`, or stops emitting a section
/// that `skip_serializing_if` hides from a default serialization, the guard's
/// accepted set has silently moved and the doc becomes the thing that looks
/// wrong.
#[test]
fn the_derived_section_set_survives_the_serde_attributes_that_distort_it() {
    let truth = Truth::derive();

    // `rename = "voice"` — the TOML key, not the Rust field name.
    assert!(truth.live.contains("voice"), "sections: {:?}", truth.live);
    assert!(!truth.live.contains("voice_local"));

    // `skip_serializing_if = "Option::is_none"` / `"…is_empty"`: absent from a
    // serialized default, present in the schema. These are the sections a
    // `to_value(Config::default())` derivation would have called phantoms.
    for hidden in ["guardrails", "stability", "moa", "profiles", "personas"] {
        assert!(
            truth.live.contains(hidden),
            "`{hidden}` vanished from the derived section set — the derivation regressed to \
             something value-based; it would now report correct documentation as drift"
        );
    }

    // `#[serde(skip)]` — `presets_override` is not a `config.toml` key at all.
    assert!(!truth.live.contains("presets_override"));

    // The foreign-owned half. Without it, `[gateway]` reads as a phantom.
    assert!(
        truth.live.contains("gateway"),
        "the TOLERATED half of the derivation is not wired: `Config` has no `gateway` field by \
         design, so deriving from the schema alone condemns a live section"
    );
    assert!(
        truth
            .fields
            .get("security")
            .is_some_and(|f| f.contains("ssrf")),
        "`security.ssrf` is foreign-owned (a raw-TOML bridge), so it must be an accepted field \
         of `[security]` even though `ShellSecurityConfig` has no such field"
    );

    // Whole-section retirements are known as retirements, not as live keys.
    for retired in ["agent", "cowork", "secret_providers"] {
        assert!(
            truth.retired.contains(retired),
            "retired: {:?}",
            truth.retired
        );
        assert!(!truth.live.contains(retired));
    }
}

/// The audit reports drift when drift is present.
///
/// Without this, `the_bundled_self_skill_names_no_phantom_config_keys` has
/// never been seen red and its green means only "nothing threw". The fixture
/// exercises every arm: a retired section, a phantom section, a phantom field,
/// the code-fence skip, and the paren suppression that keeps enum values and
/// the invisible `alias` from being reported.
#[test]
fn the_audit_reports_drift_when_it_is_present() {
    let truth = Truth {
        live: ["general", "memory", "gateway", "route", "tasks_reaper"]
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        retired: ["agent"].iter().map(|s| (*s).to_string()).collect(),
        fields: [
            ("general", vec!["default_provider", "language"]),
            ("memory", vec!["enabled"]),
            ("route", vec!["mode"]),
            ("tasks_reaper", vec!["enabled"]),
        ]
        .into_iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                v.into_iter().map(str::to_string).collect::<BTreeSet<_>>(),
            )
        })
        .collect(),
    };

    let doc = "\
## Config Sections Quick Reference

| Section | Purpose | Key Fields |
|---|---|---|
| `general` | Core | `default_provider`, `language` |
| `memory` | Mem | `enabled`, `auto_save` |
| `route` | Routing | `mode` (`auto`/`always_local`) |
| `tasks_reaper` (alias `task_reaper`) | Reaper | `enabled` |
| `agent` | Retired | `enabled` |
| `smart_flow` | Bogus | `enabled` |
| `gateway` | Foreign-owned | `host` |

```toml
| `not_a_row` | inside a fence | `nope` |
```

### Config Path Examples

| Path | Meaning |
|---|---|
| `general` | Prefer the `route_status` action to view this |
| `ghost` | Bogus |
";

    let report = audit(doc, &truth);
    let got: Vec<(&str, &str)> = report
        .findings
        .iter()
        .map(|f| (f.kind, f.what.as_str()))
        .collect();

    assert_eq!(
        got,
        vec![
            ("PHANTOM FIELD", "memory.auto_save"),
            ("PHANTOM SECTION", "[ghost]"),
            ("PHANTOM SECTION", "[smart_flow]"),
            ("RETIRED SECTION", "[agent]"),
        ],
        "audit arms changed; full report:\n{}",
        render("fixture", &report)
    );

    // `mode`'s enum values are parenthesised and must not read as fields; the
    // fenced row must not be seen at all; `gateway` has no field truth, so it
    // is UNCHECKED rather than clean.
    assert!(!doc_mentions_finding(&report, "not_a_row"));
    assert!(!doc_mentions_finding(&report, "always_local"));
    assert!(!doc_mentions_finding(&report, "task_reaper"));

    // The Quick Reference is deliberately placed *before* the Path Examples
    // here — the order the live doc does not use. With a `## `-only terminator
    // the first table's scan runs into the second, and `route_status` (prose in
    // a `Meaning` cell, never a field name) is reported as `general.route_status`.
    // This is the over-report the guard must never produce.
    assert!(
        !doc_mentions_finding(&report, "route_status"),
        "the Quick-Reference field scan bled into the following table's prose"
    );
    assert_eq!(report.unchecked, BTreeSet::from(["gateway".to_string()]));
    assert_eq!(report.tables_found, 2);
    assert_eq!(report.rows_scanned, 9);
}

fn doc_mentions_finding(report: &Report, needle: &str) -> bool {
    report.findings.iter().any(|f| f.what.contains(needle))
}

// ===========================================================================
// Second assertion: runnable snippets must derive the config path, not type it
// ===========================================================================
//
// # Why this is here and not in a module of its own
//
// It walks the same `BUNDLED_SKILLS` tree for the same reason — the embedded
// snapshot is what ships to the model — and two modules walking one tree would
// be the duplication this whole round has been removing.
//
// # What went wrong
//
// `~/.aleph/config.toml` handed to Python's `open()` expands neither `~` nor
// `$HOME`: the read raises `FileNotFoundError`, and the paired write creates a
// literal `./~/.aleph/` tree in the cwd. Worse, every snippet hardcoded
// `~/.aleph` and ignored `ALEPH_HOME`, which per `shared/protocol/src/paths.rs`
// points **at** the `.aleph` directory rather than being a parent to join onto:
// on a host with it set, the model edits a file the daemon never reads and
// reports success.
//
// The `~`-expansion half had already been fixed once, on 2026-07-18 in the
// sibling repo, with the parent pointer deliberately left unbumped "until the
// next sync". That sync landed this week and delivered the **unfixed** text: a
// re-import erased the fix and nothing noticed. Which is precisely why this
// guard anchors to `BUNDLED_SKILLS` — a guard reading `skills/` on disk through
// a stale pointer would have stayed green through the exact failure that just
// happened.
//
// # The claim is positive, not a blacklist
//
// A blacklist only catches the bad spellings someone thought of. The property
// the fix actually holds is: *a runnable snippet that names the config file
// derives its directory from `ALEPH_HOME`*. So the assertion is that every
// fenced block mentioning `.aleph/config.toml` also mentions `ALEPH_HOME`.
//
// Note what that needle does and does not match. The canonical spellings —
// `"${ALEPH_HOME:-$HOME/.aleph}/config.toml"` and Python's
// `os.path.join(os.path.expanduser(os.environ.get("ALEPH_HOME", "~/.aleph")),
// "config.toml")` — do not contain the contiguous string at all, because the
// directory and the filename are joined at runtime. The needle therefore fires
// on exactly the hardcoded-literal shape, and `ALEPH_HOME` in the same fence is
// the escape hatch for a snippet that shows both.
//
// # Scope: fenced, runnable content only
//
// Prose and directory diagrams that mention `~/.aleph` are correct as they
// stand — there the path's conventional location is the thing being described.
// Only fenced blocks are scanned, and fences are found by their ``` markers
// regardless of info string. That last part is load-bearing:
// `self/references/generation-providers.md` smuggles Python through a **bash**
// fence as `bash(python3 -c "…")`, and it is one of the two files that carried
// the bug. A scan keyed on `python`/`py` info strings would not have seen it.

/// The literal shape a snippet must not contain on its own.
const HARDCODED_CONFIG_PATH: &str = ".aleph/config.toml";
/// The environment variable a correct snippet derives the directory from.
const ALEPH_HOME_VAR: &str = "ALEPH_HOME";

/// Canonical spellings, quoted verbatim into the failure message.
const CANONICAL_BASH: &str = r#""${ALEPH_HOME:-$HOME/.aleph}/config.toml""#;
const CANONICAL_PYTHON: &str =
    r#"os.path.join(os.path.expanduser(os.environ.get("ALEPH_HOME", "~/.aleph")), "config.toml")"#;

/// The minimum number of fenced blocks the walk must find.
///
/// The bundled tree carries a few hundred. If the fence parser stops finding
/// them — a nested fence, a `~~~` block, a tree that failed to embed — the
/// config-path claim degrades to a permanent, meaningless green. This is the
/// same "not installed" face as `MIN_ROWS_SCANNED`, on the other walk.
const MIN_FENCES_SCANNED: usize = 50;

/// One fenced block from a bundled file.
struct Fence {
    /// Path inside the embedded tree.
    file: String,
    /// 1-based line of the opening ``` marker.
    line: usize,
    /// The info string after the opening backticks (`bash`, `python`, or empty).
    info: String,
    /// Everything between the markers.
    body: String,
}

/// Every fenced block in `text`, keyed by ``` markers rather than by language.
fn fenced_blocks(file: &str, text: &str) -> Vec<Fence> {
    let mut out = Vec::new();
    let mut open: Option<(usize, String, String)> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            match open.take() {
                None => open = Some((index + 1, rest.trim().to_string(), String::new())),
                Some((start, info, body)) => out.push(Fence {
                    file: file.to_string(),
                    line: start,
                    info,
                    body,
                }),
            }
            continue;
        }
        if let Some((_, _, body)) = open.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    // An unterminated fence is not silently dropped: whatever it holds is still
    // content the model reads, and dropping it would be a hole in the scan.
    if let Some((start, info, body)) = open {
        out.push(Fence {
            file: file.to_string(),
            line: start,
            info,
            body,
        });
    }
    out
}

/// Every UTF-8 file in the embedded skills tree, as `(path, contents)`.
fn bundled_text_files() -> Vec<(String, &'static str)> {
    fn walk(dir: &'static include_dir::Dir<'static>, out: &mut Vec<(String, &'static str)>) {
        for file in dir.files() {
            if let Some(text) = file.contents_utf8() {
                out.push((file.path().display().to_string(), text));
            }
        }
        for sub in dir.dirs() {
            walk(sub, out);
        }
    }
    let mut out = Vec::new();
    walk(&crate::bundled::BUNDLED_SKILLS, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Does this fence run Python? Info string first, then the smuggled shape.
fn looks_like_python(fence: &Fence) -> bool {
    let info = fence.info.to_ascii_lowercase();
    info.starts_with("python")
        || info.starts_with("py")
        || fence.body.contains("python3 -c")
        || fence.body.contains("python -c")
}

/// Fences that name the config file without deriving its directory.
fn hardcoded_config_paths(files: &[(String, &'static str)]) -> (Vec<Finding>, usize) {
    let mut findings = Vec::new();
    let mut scanned = 0usize;
    for (path, text) in files {
        for fence in fenced_blocks(path, text) {
            scanned += 1;
            if !fence.body.contains(HARDCODED_CONFIG_PATH) || fence.body.contains(ALEPH_HOME_VAR) {
                continue;
            }
            let canonical = if looks_like_python(&fence) {
                format!("python3: {CANONICAL_PYTHON}")
            } else {
                format!("bash: {CANONICAL_BASH}")
            };
            findings.push(Finding {
                kind: "HARDCODED PATH",
                what: format!("{}:{} (```{})", fence.file, fence.line, fence.info),
                fix: format!(
                    "this runnable snippet names `{HARDCODED_CONFIG_PATH}` without deriving the \
                     directory from `{ALEPH_HOME_VAR}`. `{ALEPH_HOME_VAR}` points AT the .aleph \
                     directory (it is not a parent to join `.aleph` onto), so on a host with it \
                     set this edits a file the daemon never reads — and reports success. Python \
                     additionally expands neither `~` nor `$HOME`, so a literal read raises \
                     FileNotFoundError and a literal write creates a `./~/.aleph/` tree in the \
                     cwd. Use {canonical}"
                ),
            });
        }
    }
    findings.sort();
    (findings, scanned)
}

/// Every runnable snippet in the bundled skills derives the config path.
#[test]
fn no_bundled_snippet_hardcodes_the_aleph_config_path() {
    let files = bundled_text_files();
    let (findings, scanned) = hardcoded_config_paths(&files);

    // Self-defence, same shape as the doc-table walk: a clean report from a
    // parser that found nothing is not a pass.
    assert!(
        files.len() > 10,
        "only {} file(s) in BUNDLED_SKILLS — the `skills/` submodule is not checked out, so this \
         guard is not running at all",
        files.len()
    );
    assert!(
        scanned > MIN_FENCES_SCANNED,
        "the fence parser found only {scanned} fenced block(s) across {} bundled file(s) \
         (expected > {MIN_FENCES_SCANNED}). It has lost the blocks it is supposed to read — this \
         is NOT a pass.",
        files.len()
    );

    assert!(
        findings.is_empty(),
        "{}",
        render_config_path(&findings, scanned, files.len())
    );
}

fn render_config_path(findings: &[Finding], scanned: usize, files: usize) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "\n{} bundled snippet(s) hardcode the Aleph config path.\nThese ship inside aleph-server \
         via include_dir! and the model COPIES AND RUNS them.\n\n",
        findings.len()
    );
    for f in findings {
        let _ = writeln!(out, "  {:<15} {}\n{:<19}{}", f.kind, f.what, "", f.fix);
    }
    let _ = writeln!(
        out,
        "\n  (scanned {scanned} fenced block(s) across {files} bundled file(s))\n  \
         Fix the snippet in the `skills` submodule and bump the parent pointer — a fix left \n  \
         in the sibling repo with the pointer unbumped is how this defect came back."
    );
    out
}

/// The config-path scan reports a violation when one is present.
///
/// Both shapes that actually shipped: a `python` fence with the literal path,
/// and the smuggled `bash(python3 -c "…")` form whose info string says `bash`.
/// A fence that mentions the path *and* `ALEPH_HOME` is the escape hatch and
/// must stay silent, and prose outside any fence must never be flagged.
#[test]
fn the_config_path_scan_reports_a_hardcoded_snippet() {
    let doc = "\
Prose may say `~/.aleph/config.toml` freely — that is the conventional location.

```python
import toml
config = toml.load(open(\"~/.aleph/config.toml\"))
```

```
bash(python3 -c \"
with open('$HOME/.aleph/config.toml', 'r') as f:
    content = f.read()
\")
```

```bash
cat \"${ALEPH_HOME:-$HOME/.aleph}/config.toml\"
```

```bash
grep x ~/.aleph/config.toml   # named alongside ALEPH_HOME, so tolerated
```
";
    let files = vec![("self/FIXTURE.md".to_string(), doc)];
    let (findings, scanned) = hardcoded_config_paths(&files);

    assert_eq!(scanned, 4, "fence parser lost a block");
    let got: Vec<&str> = findings.iter().map(|f| f.what.as_str()).collect();
    assert_eq!(
        got,
        vec!["self/FIXTURE.md:3 (```python)", "self/FIXTURE.md:8 (```)"],
        "full report:\n{}",
        render_config_path(&findings, scanned, files.len())
    );

    // The smuggled shape is recognised as Python even though its fence says
    // nothing — otherwise its failure message would hand the reader the bash
    // spelling for a Python snippet.
    assert!(
        findings[1].fix.contains(CANONICAL_PYTHON),
        "`bash(python3 -c …)` must be told the python spelling: {}",
        findings[1].fix
    );
    assert!(findings[0].fix.contains(CANONICAL_PYTHON));
}
