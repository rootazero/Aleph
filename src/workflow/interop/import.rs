//! Parse a `.workflow.js` (or raw AWI manifest JSON) into a `WorkflowManifest`.
//!
//! Three paths, in priority order:
//! 0. **Bare manifest JSON** (starts with `{`) → exact parse, lossless.
//! 1. **Embedded block** (`/* @aleph-workflow {json} */`) → exact parse, lossless.
//! 2. **Bare `.workflow.js`** → light-weight scan of the pure-literal `meta`
//!    block + ordered `phase()` / `agent()` calls; imperative constructs go into
//!    `dropped`. `phase()` markers are captured and assigned to the steps that
//!    follow them, so a hand-written phased workflow keeps its phase plan.
//!
//! No JS engine, no full parser (R3). The scan's limits are surfaced via
//! `dropped`, never hidden.

use crate::error::{AlephError, Result};
use crate::workflow::interop::export::{EMBED_PREFIX, EMBED_SUFFIX};
use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep, WorkflowPhase};

/// Result of importing a `.workflow.js`.
#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub manifest: WorkflowManifest,
    /// Imperative constructs the scan could not map (empty on lossless paths).
    pub dropped: Vec<String>,
}

/// Parse `src` into a manifest. See module docs for the three paths.
pub fn parse_workflow_js(src: &str) -> Result<ImportOutcome> {
    // Path 0: bare manifest JSON document.
    let trimmed = src.trim_start();
    if trimmed.starts_with('{') {
        let manifest: WorkflowManifest = serde_json::from_str(trimmed)
            .map_err(|e| AlephError::invalid_input(format!("manifest JSON parse failed: {e}")))?;
        return Ok(ImportOutcome {
            manifest,
            dropped: Vec::new(),
        });
    }

    // Path 1: embedded lossless block.
    if let Some(json) = extract_embedded(src) {
        let manifest: WorkflowManifest = serde_json::from_str(&json).map_err(|e| {
            AlephError::invalid_input(format!("embedded @aleph-workflow parse failed: {e}"))
        })?;
        return Ok(ImportOutcome {
            manifest,
            dropped: Vec::new(),
        });
    }

    // Path 2: best-effort scan of a bare .workflow.js.
    scan_bare(src)
}

/// Extract the JSON between the first `EMBED_PREFIX` and the matching
/// `EMBED_SUFFIX` that follows it.  Only the first embed block is considered.
fn extract_embedded(src: &str) -> Option<String> {
    let start = src.find(EMBED_PREFIX)? + EMBED_PREFIX.len();
    let rest = &src[start..];
    let end = rest.find(EMBED_SUFFIX)?;
    Some(rest[..end].trim().to_string())
}

/// Light-weight scan of a hand-written `.workflow.js`.
fn scan_bare(src: &str) -> Result<ImportOutcome> {
    let name = scan_meta_field(src, "name").ok_or_else(|| {
        AlephError::invalid_input(
            "no @aleph-workflow block and no `meta.name` found; cannot import",
        )
    })?;
    let description = scan_meta_field(src, "description").unwrap_or_default();
    let when_to_use = scan_meta_field(src, "whenToUse").unwrap_or_default();

    // Imperative-construct detection must ignore string-literal *contents* so a
    // prompt like `agent('search for files if (any) exist')` does not
    // false-positive as a `for` loop / `if` conditional. Scan only the code
    // skeleton with every string body blanked out (delimiters preserved).
    let skeleton = strip_string_literals(src);
    let mut dropped = Vec::new();
    for (needle, label) in [
        (
            "pipeline(",
            "pipeline(...) — runtime item list not statically known",
        ),
        ("budget", "budget-driven control flow"),
        ("workflow(", "nested workflow() call"),
        ("for ", "for loop"),
        ("while ", "while loop"),
        ("if (", "if conditional"),
        ("if(", "if conditional"),
    ] {
        if skeleton.contains(needle) {
            dropped.push(label.to_string());
        }
    }
    if skeleton.contains("parallel(") {
        dropped.push("parallel(...) grouping approximated as a sequential chain".to_string());
    }

    // Walk the source in order, tracking the active `phase()` so each scanned
    // `agent()` step inherits it. The scan is string-aware (a prompt mentioning
    // `phase(` or `agent(` never registers as a call), so phases are reconstructed
    // only from real markers.
    let mut steps: Vec<WorkflowManifestStep> = Vec::new();
    let mut phase_titles: Vec<String> = Vec::new();
    let mut current_phase: Option<String> = None;
    for ev in scan_events(src) {
        match ev {
            ScanEvent::Phase(title) => {
                if !phase_titles.iter().any(|t| t == &title) {
                    phase_titles.push(title.clone());
                }
                current_phase = Some(title);
            }
            ScanEvent::Agent(prompt) => {
                let i = steps.len();
                steps.push(WorkflowManifestStep {
                    id: format!("step_{}", i + 1),
                    agent: "agent".to_string(),
                    prompt,
                    depends_on: if i == 0 {
                        Vec::new()
                    } else {
                        vec![format!("step_{i}")]
                    },
                    label: None,
                    model: None,
                    phase: current_phase.clone(),
                    schema: None,
                    isolation: None,
                    agent_type: None,
                });
            }
        }
    }
    if steps.is_empty() {
        return Err(AlephError::invalid_input(
            "no agent() calls found in .workflow.js; nothing to import",
        ));
    }
    let phases: Vec<WorkflowPhase> = phase_titles
        .into_iter()
        .map(|title| WorkflowPhase {
            title,
            detail: String::new(),
            model: None,
        })
        .collect();

    Ok(ImportOutcome {
        manifest: WorkflowManifest {
            name,
            description,
            when_to_use,
            phases,
            steps,
        },
        dropped,
    })
}

/// Find `<field>:` then read the next JS string literal that follows it.
fn scan_meta_field(src: &str, field: &str) -> Option<String> {
    let key = format!("{field}:");
    let pos = src.find(&key)? + key.len();
    read_first_string_literal(&src[pos..])
}

/// Read a single- or double-quoted string literal that is the *first
/// non-whitespace token* of `s` (UTF-8 safe, honours backslash escapes by
/// keeping the escaped char verbatim).
///
/// Requiring the literal to lead — rather than scanning arbitrarily far ahead —
/// keeps a non-literal argument (`agent(promptVar)`, `meta: { name: foo }`)
/// from silently capturing an *unrelated* later string elsewhere in the source.
/// Both real callers (a `meta.<field>:` value and an `agent(` first argument)
/// place the literal immediately after optional whitespace, so this is the
/// correct shape, not just a safer one.
fn read_first_string_literal(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let quote = loop {
        match chars.next()? {
            c @ ('\'' | '"') => break c,
            c if c.is_whitespace() => continue,
            // First non-whitespace token is not a string literal — give up
            // rather than over-reaching to a later, unrelated literal.
            _ => return None,
        }
    };
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(esc) = chars.next() {
                out.push(esc);
            }
            continue;
        }
        if c == quote {
            return Some(out);
        }
        out.push(c);
    }
    None
}

/// Blank out the contents of every string literal (`'`, `"`, `` ` ``) so a
/// downstream keyword scan sees only the code skeleton, never prompt text.
/// Quote delimiters and surrounding code are preserved; an escaped quote
/// inside a literal does not terminate it. UTF-8 safe (iterates `chars`);
/// an unterminated literal degrades to dropping the trailing bytes.
fn strip_string_literals(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' | '`' => {
                out.push(c);
                while let Some(d) = chars.next() {
                    if d == '\\' {
                        // Skip the escaped char so e.g. \" does not close early;
                        // its body is irrelevant to the skeleton, so drop it.
                        chars.next();
                        continue;
                    }
                    if d == c {
                        out.push(d); // keep the closing delimiter
                        break;
                    }
                    // literal body intentionally dropped
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// One ordered call recovered from a bare `.workflow.js` scan.
enum ScanEvent {
    /// A `phase("title")` marker — the title of its string-literal argument.
    Phase(String),
    /// An `agent("prompt", ...)` call — the prompt from its first string literal.
    Agent(String),
}

/// Scan `src` for `phase(...)` and `agent(...)` calls in source order, string-
/// aware so a prompt mentioning `phase(`/`agent(` never registers as a call.
///
/// A single forward pass tokenises identifiers and skips string-literal bodies
/// wholesale; only a bare `phase`/`agent` identifier immediately followed by `(`
/// counts, so `subagent(` / `useragent(` and the like never over-match. Calls
/// whose first argument is not a string literal (e.g. `agent(promptVar)`) yield
/// no event. Catches both top-level calls and `() => agent(` inside
/// `parallel([...])`.
fn scan_events(src: &str) -> Vec<ScanEvent> {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut events = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        // Skip string-literal bodies so their contents are never tokenised.
        if c == '\'' || c == '"' || c == '`' {
            i += 1;
            while i < n {
                let d = chars[i];
                if d == '\\' {
                    i += 2; // skip the escaped char so \" / \' does not close early
                    continue;
                }
                i += 1;
                if d == c {
                    break;
                }
            }
            continue;
        }
        // Identifier? Consume it, then check for a following `(`.
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            if ident == "phase" || ident == "agent" {
                let mut j = i;
                while j < n && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < n && chars[j] == '(' {
                    let after: String = chars[j + 1..].iter().collect();
                    if let Some(lit) = read_first_string_literal(&after) {
                        match ident.as_str() {
                            "phase" => events.push(ScanEvent::Phase(lit)),
                            "agent" => events.push(ScanEvent::Agent(lit)),
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::interop::export::render_workflow_js;
    use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep};

    fn sample_manifest() -> WorkflowManifest {
        WorkflowManifest {
            name: "rep".into(),
            description: "demo".into(),
            when_to_use: "use it".into(),
            phases: vec![],
            steps: vec![
                WorkflowManifestStep {
                    id: "a".into(),
                    agent: "researcher".into(),
                    prompt: "research {input}".into(),
                    depends_on: vec![],
                    label: Some("audit:a".into()),
                    model: None,
                    phase: None,
                    schema: None,
                    // Exercise the new agent-opts on the lossless roundtrip path.
                    isolation: Some("worktree".into()),
                    agent_type: Some("Explore".into()),
                },
                WorkflowManifestStep {
                    id: "b".into(),
                    agent: "writer".into(),
                    prompt: "write".into(),
                    depends_on: vec!["a".into()],
                    label: None,
                    model: None,
                    phase: None,
                    schema: None,
                    isolation: None,
                    agent_type: None,
                },
            ],
        }
    }

    #[test]
    fn embedded_block_roundtrips_losslessly() {
        let original = sample_manifest();
        let js = render_workflow_js(&original);
        let outcome = parse_workflow_js(&js).expect("parse rendered js");
        assert_eq!(outcome.manifest, original, "embedded block is lossless");
        assert!(outcome.dropped.is_empty());
    }

    #[test]
    fn bare_manifest_json_parses() {
        let json = serde_json::to_string(&sample_manifest()).unwrap();
        let outcome = parse_workflow_js(&json).expect("parse bare json");
        assert_eq!(outcome.manifest, sample_manifest());
    }

    #[test]
    fn bare_js_extracts_meta_and_agents() {
        let src = r#"
export const meta = {
  name: 'hand-written',
  description: 'a manual workflow',
  whenToUse: 'when testing',
}
await agent('first step')
await agent('second step')
"#;
        let outcome = parse_workflow_js(src).expect("scan bare js");
        assert_eq!(outcome.manifest.name, "hand-written");
        assert_eq!(outcome.manifest.description, "a manual workflow");
        assert_eq!(outcome.manifest.when_to_use, "when testing");
        assert_eq!(outcome.manifest.steps.len(), 2);
        assert_eq!(outcome.manifest.steps[0].prompt, "first step");
        assert_eq!(
            outcome.manifest.steps[1].depends_on,
            vec!["step_1".to_string()]
        );
    }

    #[test]
    fn imperative_constructs_recorded_in_dropped() {
        let src = r#"
export const meta = { name: 'loopy' }
for (const x of items) {
  await agent('do thing')
}
const r = await pipeline(items, s1, s2)
"#;
        let outcome = parse_workflow_js(src).expect("scan");
        assert!(outcome.dropped.iter().any(|d| d.contains("for loop")));
        assert!(outcome.dropped.iter().any(|d| d.contains("pipeline")));
    }

    #[test]
    fn bare_js_without_name_errors() {
        let src = "await agent('x')";
        assert!(parse_workflow_js(src).is_err());
    }

    #[test]
    fn bare_js_without_agents_errors() {
        let src = "export const meta = { name: 'empty' }";
        assert!(parse_workflow_js(src).is_err());
    }

    #[test]
    fn embed_block_roundtrips_prompt_containing_comment_terminator() {
        // A prompt containing ` */` (glob/regex/C-comment) must NOT truncate the
        // embed block; export escapes it as `*\/`, import parses it back.
        let original = WorkflowManifest {
            name: "scan".into(),
            description: "look in src/**/*.rs */ etc".into(),
            when_to_use: String::new(),
            phases: vec![],
            steps: vec![WorkflowManifestStep {
                id: "a".into(),
                agent: "scanner".into(),
                prompt: "scan src/**/*.rs */ then stop".into(),
                depends_on: vec![],
                label: None,
                model: None,
                phase: None,
                schema: None,
                isolation: None,
                agent_type: None,
            }],
        };
        let js = render_workflow_js(&original);
        let outcome = parse_workflow_js(&js).expect("parse js with */ in prompt");
        assert_eq!(outcome.manifest, original, "embed block stays lossless");
        assert!(outcome.dropped.is_empty());
    }

    #[test]
    fn imperative_needles_ignore_prompt_text() {
        // A prompt that merely *mentions* loop/conditional keywords (and even a
        // literal `pipeline(`) must NOT be reported as a dropped construct — the
        // needles only apply to the code skeleton, not string contents.
        let src = "export const meta = { name: 'wf' }\n\
                   await agent('search for files if (any) exist; run pipeline(x) while waiting')";
        let outcome = parse_workflow_js(src).expect("scan bare js");
        assert!(
            outcome.dropped.is_empty(),
            "prompt text must not trip imperative needles, got: {:?}",
            outcome.dropped
        );
        assert_eq!(outcome.manifest.steps.len(), 1);
    }

    #[test]
    fn strip_string_literals_blanks_bodies_keeps_skeleton() {
        // Bodies gone, delimiters + code kept; escaped quote does not close early.
        assert_eq!(
            strip_string_literals("for (x) agent('a b')"),
            "for (x) agent('')"
        );
        assert_eq!(strip_string_literals(r#"f("a \" b")"#), r#"f("")"#);
        assert_eq!(strip_string_literals("agent(`tpl text`)"), "agent(``)");
    }

    #[test]
    fn real_imperative_constructs_still_detected_after_strip() {
        // Regression guard: stripping bodies must not blind the skeleton scan to
        // genuine code-level constructs.
        let src = "export const meta = { name: 'loopy' }\n\
                   for (const x of items) { await agent('do thing') }\n\
                   const r = await pipeline(items, s1, s2)";
        let outcome = parse_workflow_js(src).expect("scan");
        assert!(outcome.dropped.iter().any(|d| d.contains("for loop")));
        assert!(outcome.dropped.iter().any(|d| d.contains("pipeline")));
    }

    #[test]
    fn agent_with_non_literal_arg_does_not_capture_unrelated_string() {
        // `agent(promptVar)` has no leading string literal. The scanner must NOT
        // reach forward and adopt the next unrelated literal (`'unrelated'`) as
        // this step's prompt; that call yields no importable prompt.
        let src = "export const meta = { name: 'wf' }\n\
                   await agent(promptVar)\n\
                   const note = 'unrelated string'\n\
                   await agent('real prompt')";
        let outcome = parse_workflow_js(src).expect("scan bare js");
        assert_eq!(
            outcome.manifest.steps.len(),
            1,
            "only the string-literal agent() counts, got: {:?}",
            outcome
                .manifest
                .steps
                .iter()
                .map(|s| s.prompt.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(outcome.manifest.steps[0].prompt, "real prompt");
    }

    #[test]
    fn scan_ignores_subagent_identifier() {
        // `subagent(` must not over-match the `agent(` needle on the bare path.
        let src = "export const meta = { name: 'wf' }\n\
                   await subagent('noise')\n\
                   await agent('real')";
        let outcome = parse_workflow_js(src).expect("scan bare js");
        assert_eq!(
            outcome.manifest.steps.len(),
            1,
            "only the real agent() counts"
        );
        assert_eq!(outcome.manifest.steps[0].prompt, "real");
    }

    #[test]
    fn bare_js_captures_phase_markers_and_assigns_steps() {
        // `phase()` markers are reconstructed into manifest.phases (in order) and
        // each following step inherits the active phase — the headline bare-path
        // fidelity fix.
        let src = r#"
export const meta = { name: 'phased' }
phase('Audit')
await agent('audit the code')
phase('Fix')
await agent('fix the bug')
await agent('fix more')
"#;
        let outcome = parse_workflow_js(src).expect("scan phased js");
        let m = outcome.manifest;
        assert_eq!(
            m.phases.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
            vec!["Audit", "Fix"],
            "phase plan reconstructed in order"
        );
        assert_eq!(m.steps[0].phase.as_deref(), Some("Audit"));
        assert_eq!(m.steps[1].phase.as_deref(), Some("Fix"));
        assert_eq!(
            m.steps[2].phase.as_deref(),
            Some("Fix"),
            "phase carries forward until the next marker"
        );
    }

    #[test]
    fn phase_in_prompt_text_is_not_a_marker() {
        // A prompt that merely mentions `phase(` must NOT create a phantom phase —
        // the scan is string-aware.
        let src = "export const meta = { name: 'wf' }\n\
                   await agent('discuss the next phase(2) of the project')";
        let outcome = parse_workflow_js(src).expect("scan");
        assert!(
            outcome.manifest.phases.is_empty(),
            "no phantom phase from prompt text: {:?}",
            outcome.manifest.phases
        );
        assert_eq!(outcome.manifest.steps.len(), 1);
        assert!(outcome.manifest.steps[0].phase.is_none());
    }

    #[test]
    fn bare_phased_js_round_trips_phase_plan_through_export() {
        // import a phased bare workflow → export → re-import: the phase plan and
        // per-step phase survive (export's embed block makes the second hop
        // lossless, proving the import captured the phases correctly).
        let src = "export const meta = { name: 'rt' }\n\
                   phase('Scan')\n\
                   await agent('scan it')\n\
                   phase('Report')\n\
                   await agent('report it')";
        let first = parse_workflow_js(src).expect("first import");
        assert_eq!(first.manifest.phases.len(), 2);
        let js = render_workflow_js(&first.manifest);
        let second = parse_workflow_js(&js).expect("re-import exported js");
        assert_eq!(
            second.manifest, first.manifest,
            "phase plan survives export round-trip"
        );
    }
}
