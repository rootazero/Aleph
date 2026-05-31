//! Render an AWI manifest into a Claude-Code-compatible `.workflow.js`.
//!
//! Deterministic string rendering — no reasoning. The static dependency DAG is
//! reconstructed into the declarative `phase()` / `parallel()` / sequential
//! `agent()` skeleton; imperative control flow is never emitted (Aleph's source
//! has none). A `/* @aleph-workflow {json} */` header carries the full manifest
//! for lossless re-import.

use std::collections::HashMap;

use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep};

/// Embedded round-trip marker. `import` reads the JSON between prefix/suffix.
pub const EMBED_PREFIX: &str = "/* @aleph-workflow ";
pub const EMBED_SUFFIX: &str = " */";

/// Render `manifest` as a `.workflow.js` source string.
pub fn render_workflow_js(manifest: &WorkflowManifest) -> String {
    // Escape `*/` as `*\/` (a legal JSON `/` escape) so a string field
    // containing `*/` (glob/regex/C-comment) cannot terminate the embed block
    // early; `import`'s serde_json parse reads `\/` back transparently.
    let manifest_json = serde_json::to_string(manifest)
        .unwrap_or_else(|_| "{}".to_string())
        .replace("*/", "*\\/");
    let mut out = String::new();

    // 1. Lossless round-trip header.
    out.push_str(EMBED_PREFIX);
    out.push_str(&manifest_json);
    out.push_str(EMBED_SUFFIX);
    out.push('\n');

    // 2. meta block (pure literal).
    out.push_str(&render_meta(manifest));
    out.push('\n');

    // 3. Body: topological layers → parallel/sequential agent() skeleton.
    match topo_levels(manifest) {
        Some(levels) => {
            let mut last_phase: Option<&str> = None;
            for layer in &levels {
                // The phase marker reflects only the layer's first step. Mixed
                // per-step phases within one parallel layer are NOT all rendered
                // in the body, but every step's `phase` is preserved losslessly
                // via the embed-block manifest.
                if let Some(&first) = layer.first() {
                    if let Some(ph) = manifest.steps[first].phase.as_deref() {
                        if last_phase != Some(ph) {
                            out.push_str(&format!("phase({})\n", js_str(ph)));
                            last_phase = Some(ph);
                        }
                    }
                }
                if layer.len() == 1 {
                    out.push_str(&format!(
                        "await {}\n",
                        render_agent_call(&manifest.steps[layer[0]])
                    ));
                } else {
                    out.push_str("await parallel([\n");
                    for &i in layer {
                        out.push_str(&format!(
                            "  () => {},\n",
                            render_agent_call(&manifest.steps[i])
                        ));
                    }
                    out.push_str("])\n");
                }
            }
        }
        None => {
            // Cycle / unknown dep — should not happen for a validated manifest.
            // Degrade to a flat sequence rather than panicking.
            for step in &manifest.steps {
                out.push_str(&format!("await {}\n", render_agent_call(step)));
            }
        }
    }

    out
}

/// Render the `export const meta = {...}` literal.
fn render_meta(manifest: &WorkflowManifest) -> String {
    let mut phases = String::new();
    for p in &manifest.phases {
        phases.push_str(&format!(
            "    {{ title: {}, detail: {} }},\n",
            js_str(&p.title),
            js_str(&p.detail)
        ));
    }
    format!(
        "export const meta = {{\n  name: {},\n  description: {},\n  whenToUse: {},\n  phases: [\n{}  ],\n}}\n",
        js_str(&manifest.name),
        js_str(&manifest.description),
        js_str(&manifest.when_to_use),
        phases
    )
}

/// Render a single `agent(prompt, { opts })` call.
fn render_agent_call(step: &WorkflowManifestStep) -> String {
    let mut opts: Vec<String> = Vec::new();
    if let Some(l) = &step.label {
        opts.push(format!("label: {}", js_str(l)));
    }
    if let Some(p) = &step.phase {
        opts.push(format!("phase: {}", js_str(p)));
    }
    if let Some(m) = &step.model {
        opts.push(format!("model: {}", js_str(m)));
    }
    if let Some(sc) = &step.schema {
        let schema_json = serde_json::to_string(sc).unwrap_or_else(|_| "{}".to_string());
        opts.push(format!("schema: {schema_json}"));
    }
    if opts.is_empty() {
        format!("agent({})", js_str(&step.prompt))
    } else {
        format!("agent({}, {{ {} }})", js_str(&step.prompt), opts.join(", "))
    }
}

/// Render a Rust string as a safe double-quoted JS string literal (handles all
/// escaping via serde_json — avoids the raw-backtick / quote-escape traps).
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Group step indices into dependency layers: layer 0 = no-dep steps, layer k =
/// steps whose deps are all in layers < k. Within-layer order follows manifest
/// list order. Returns `None` on cycle/unknown dep (mirrors `WorkflowDef::topo_order`).
fn topo_levels(manifest: &WorkflowManifest) -> Option<Vec<Vec<usize>>> {
    let index_of: HashMap<&str, usize> = manifest
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let n = manifest.steps.len();
    let mut indegree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, s) in manifest.steps.iter().enumerate() {
        for d in &s.depends_on {
            let j = *index_of.get(d.as_str())?;
            dependents[j].push(i);
            indegree[i] += 1;
        }
    }

    let mut placed = 0usize;
    let mut levels: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    while !current.is_empty() {
        placed += current.len();
        let mut next: Vec<usize> = Vec::new();
        for &i in &current {
            for &c in &dependents[i] {
                indegree[c] -= 1;
                if indegree[c] == 0 {
                    next.push(c);
                }
            }
        }
        next.sort_unstable(); // keep deterministic list order within a layer
        levels.push(current);
        current = next;
    }
    if placed != n {
        return None;
    }
    Some(levels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep};

    fn step(id: &str, deps: &[&str]) -> WorkflowManifestStep {
        WorkflowManifestStep {
            id: id.into(),
            agent: "ag".into(),
            prompt: format!("do {id}"),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            label: None,
            model: None,
            phase: None,
            schema: None,
        }
    }

    fn manifest(steps: Vec<WorkflowManifestStep>) -> WorkflowManifest {
        WorkflowManifest {
            name: "wf".into(),
            description: "d".into(),
            when_to_use: String::new(),
            phases: vec![],
            steps,
        }
    }

    #[test]
    fn header_and_meta_present() {
        let js = render_workflow_js(&manifest(vec![step("a", &[])]));
        assert!(js.starts_with(EMBED_PREFIX), "embedded header first");
        assert!(js.contains("export const meta = {"));
        assert!(js.contains("name: \"wf\""));
    }

    #[test]
    fn linear_chain_renders_sequential_agents() {
        let js = render_workflow_js(&manifest(vec![step("a", &[]), step("b", &["a"])]));
        // Two single-step layers → two sequential awaits, no parallel.
        assert_eq!(js.matches("await agent(").count(), 2);
        assert!(!js.contains("parallel("));
    }

    #[test]
    fn sibling_steps_render_parallel() {
        // a, b both depend on nothing → same layer → parallel([...]).
        let js = render_workflow_js(&manifest(vec![step("a", &[]), step("b", &[])]));
        assert!(js.contains("await parallel(["));
        assert_eq!(js.matches("() => agent(").count(), 2);
    }

    #[test]
    fn phase_marker_emitted_for_phased_step() {
        let mut s = step("a", &[]);
        s.phase = Some("Audit".into());
        let js = render_workflow_js(&manifest(vec![s]));
        assert!(js.contains("phase(\"Audit\")"));
    }

    #[test]
    fn opts_rendered_when_present() {
        let mut s = step("a", &[]);
        s.label = Some("audit:a".into());
        s.model = Some("haiku".into());
        s.schema = Some(serde_json::json!({"type": "object"}));
        let js = render_workflow_js(&manifest(vec![s]));
        assert!(js.contains("label: \"audit:a\""));
        assert!(js.contains("model: \"haiku\""));
        assert!(js.contains("schema: {\"type\":\"object\"}"));
    }

    #[test]
    fn prompt_with_quotes_is_escaped() {
        let mut s = step("a", &[]);
        s.prompt = "say \"hi\"\nnewline".into();
        let js = render_workflow_js(&manifest(vec![s]));
        // serde_json escaping keeps the source parseable — the literal contains
        // the escaped quote and \n, never a raw newline inside the call.
        assert!(js.contains("say \\\"hi\\\""));
        assert!(js.contains("\\n"));
    }
}
