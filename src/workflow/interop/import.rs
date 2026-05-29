//! Parse a `.workflow.js` (or raw AWI manifest JSON) into a `WorkflowManifest`.
//!
//! Three paths, in priority order:
//! 0. **Bare manifest JSON** (starts with `{`) → exact parse, lossless.
//! 1. **Embedded block** (`/* @aleph-workflow {json} */`) → exact parse, lossless.
//! 2. **Bare `.workflow.js`** → light-weight scan of the pure-literal `meta`
//!    block + `agent()` prompts; imperative constructs go into `dropped`.
//!
//! No JS engine, no full parser (R3). The scan's limits are surfaced via
//! `dropped`, never hidden.

use crate::error::{AlephError, Result};
use crate::workflow::interop::export::{EMBED_PREFIX, EMBED_SUFFIX};
use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep};

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
        return Ok(ImportOutcome { manifest, dropped: Vec::new() });
    }

    // Path 1: embedded lossless block.
    if let Some(json) = extract_embedded(src) {
        let manifest: WorkflowManifest = serde_json::from_str(&json).map_err(|e| {
            AlephError::invalid_input(format!("embedded @aleph-workflow parse failed: {e}"))
        })?;
        return Ok(ImportOutcome { manifest, dropped: Vec::new() });
    }

    // Path 2: best-effort scan of a bare .workflow.js.
    scan_bare(src)
}

/// Extract the JSON between `EMBED_PREFIX` and `EMBED_SUFFIX`, if present.
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

    let mut dropped = Vec::new();
    for (needle, label) in [
        ("pipeline(", "pipeline(...) — runtime item list not statically known"),
        ("budget", "budget-driven control flow"),
        ("workflow(", "nested workflow() call"),
        ("for ", "for loop"),
        ("while ", "while loop"),
        ("if (", "if conditional"),
        ("if(", "if conditional"),
    ] {
        if src.contains(needle) {
            dropped.push(label.to_string());
        }
    }
    if src.contains("parallel(") {
        dropped.push("parallel(...) grouping approximated as a sequential chain".to_string());
    }

    let prompts = scan_agent_prompts(src);
    if prompts.is_empty() {
        return Err(AlephError::invalid_input(
            "no agent() calls found in .workflow.js; nothing to import",
        ));
    }
    let steps: Vec<WorkflowManifestStep> = prompts
        .iter()
        .enumerate()
        .map(|(i, p)| WorkflowManifestStep {
            id: format!("step_{}", i + 1),
            agent: "agent".to_string(),
            prompt: p.clone(),
            depends_on: if i == 0 { Vec::new() } else { vec![format!("step_{i}")] },
            label: None,
            model: None,
            phase: None,
            schema: None,
        })
        .collect();

    Ok(ImportOutcome {
        manifest: WorkflowManifest {
            name,
            description,
            when_to_use,
            phases: Vec::new(),
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

/// Read the first single- or double-quoted string literal in `s` (UTF-8 safe,
/// honours backslash escapes by keeping the escaped char verbatim).
fn read_first_string_literal(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let quote = loop {
        match chars.next()? {
            c @ ('\'' | '"') => break c,
            _ => continue,
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

/// Collect the first string-literal argument of each `agent(` call, in order.
/// Catches both top-level `agent(` and `() => agent(` inside `parallel([...])`.
///
/// Only accepts `agent(` at an identifier boundary so `subagent(` /
/// `useragent(` and similar do not over-match.
fn scan_agent_prompts(src: &str) -> Vec<String> {
    let needle = "agent(";
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(pos) = rest.find(needle) {
        let after = &rest[pos + needle.len()..];
        // Reject matches embedded in a larger identifier (e.g. `subagent(`):
        // the preceding char must be absent or a non-identifier char.
        let boundary = rest[..pos]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if boundary {
            if let Some(lit) = read_first_string_literal(after) {
                out.push(lit);
            }
        }
        rest = after;
    }
    out
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
        assert_eq!(outcome.manifest.steps[1].depends_on, vec!["step_1".to_string()]);
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
            }],
        };
        let js = render_workflow_js(&original);
        let outcome = parse_workflow_js(&js).expect("parse js with */ in prompt");
        assert_eq!(outcome.manifest, original, "embed block stays lossless");
        assert!(outcome.dropped.is_empty());
    }

    #[test]
    fn scan_ignores_subagent_identifier() {
        // `subagent(` must not over-match the `agent(` needle on the bare path.
        let src = "export const meta = { name: 'wf' }\n\
                   await subagent('noise')\n\
                   await agent('real')";
        let outcome = parse_workflow_js(src).expect("scan bare js");
        assert_eq!(outcome.manifest.steps.len(), 1, "only the real agent() counts");
        assert_eq!(outcome.manifest.steps[0].prompt, "real");
    }
}
