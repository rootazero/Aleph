//! Render an AWI manifest into a Claude-Code-compatible `.workflow.js`.
//!
//! Deterministic string rendering — no reasoning. The static dependency DAG is
//! reconstructed into the declarative `phase()` / `parallel()` / sequential
//! `agent()` skeleton; imperative control flow is never emitted (Aleph's source
//! has none). A `/* @aleph-workflow {json} */` header carries the full manifest
//! for lossless re-import.
//!
//! The `meta.phases` plan is *reconciled* with the body: any phase a step
//! references via its `phase` field is declared in `meta.phases` even when the
//! authored manifest left it out, so every body `phase()` marker has a matching
//! declaration (the `.workflow.js` convention).

use std::collections::{HashMap, HashSet};

use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep, WorkflowPhase};

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

    // Topological layers drive both the meta phase plan and the body skeleton;
    // compute once and share so the plan lists every phase the body emits.
    let levels = topo_levels(manifest);

    // 2. meta block (pure literal).
    out.push_str(&render_meta(manifest, levels.as_deref()));
    out.push('\n');

    // 3. Body: topological layers → parallel/sequential agent() skeleton.
    match &levels {
        Some(levels) => {
            let mut last_phase: Option<&str> = None;
            for layer in levels {
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

/// Compute the `meta.phases` plan: the declared phases (preserving
/// detail/model/order) plus any phase a step references but the manifest did not
/// declare, appended in topological first-occurrence order. Guarantees every
/// body `phase()` marker is also declared in `meta`, matching the `.workflow.js`
/// convention ("use the same titles in meta.phases as in phase() calls"). Pure
/// field shuffling — no reasoning (R10).
fn effective_phases(manifest: &WorkflowManifest, levels: Option<&[Vec<usize>]>) -> Vec<WorkflowPhase> {
    let mut out = manifest.phases.clone();
    let mut seen: HashSet<String> = manifest.phases.iter().map(|p| p.title.clone()).collect();
    // Visit steps in the order the body emits them so a derived phase lines up
    // with its first `phase()` marker. Fall back to list order if the DAG is
    // degenerate (never happens for a validated manifest).
    let order: Vec<usize> = match levels {
        Some(levels) => levels.iter().flatten().copied().collect(),
        None => (0..manifest.steps.len()).collect(),
    };
    for i in order {
        if let Some(ph) = manifest.steps[i].phase.as_deref() {
            if seen.insert(ph.to_string()) {
                out.push(WorkflowPhase {
                    title: ph.to_string(),
                    detail: String::new(),
                    model: None,
                });
            }
        }
    }
    out
}

/// Render the `export const meta = {...}` literal.
fn render_meta(manifest: &WorkflowManifest, levels: Option<&[Vec<usize>]>) -> String {
    let mut phases = String::new();
    for p in &effective_phases(manifest, levels) {
        // `model` is optional on a `.workflow.js` phase entry; emit it only when
        // present so model-less phases render byte-identically to before.
        match &p.model {
            Some(m) => phases.push_str(&format!(
                "    {{ title: {}, detail: {}, model: {} }},\n",
                js_str(&p.title),
                js_str(&p.detail),
                js_str(m)
            )),
            None => phases.push_str(&format!(
                "    {{ title: {}, detail: {} }},\n",
                js_str(&p.title),
                js_str(&p.detail)
            )),
        }
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
    if let Some(iso) = &step.isolation {
        opts.push(format!("isolation: {}", js_str(iso)));
    }
    if let Some(at) = &step.agent_type {
        opts.push(format!("agentType: {}", js_str(at)));
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
            isolation: None,
            agent_type: None,
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
    fn isolation_and_agent_type_opts_rendered() {
        // The two `.workflow.js` agent-opts unique to the engineering format —
        // `isolation` (e.g. worktree) and `agentType` (custom subagent) — must
        // appear verbatim in the rendered call for a faithful export.
        let mut s = step("a", &[]);
        s.isolation = Some("worktree".into());
        s.agent_type = Some("code-reviewer".into());
        let js = render_workflow_js(&manifest(vec![s]));
        assert!(js.contains("isolation: \"worktree\""), "isolation: {js}");
        assert!(
            js.contains("agentType: \"code-reviewer\""),
            "agentType: {js}"
        );
    }

    #[test]
    fn phase_entry_model_rendered_only_when_present() {
        // A phase with a model override emits `model:` in its meta entry; a
        // model-less phase stays byte-identical to the legacy two-field form.
        let m = WorkflowManifest {
            name: "wf".into(),
            description: "d".into(),
            when_to_use: String::new(),
            phases: vec![
                crate::workflow::interop::manifest::WorkflowPhase {
                    title: "Heavy".into(),
                    detail: "deep".into(),
                    model: Some("opus".into()),
                },
                crate::workflow::interop::manifest::WorkflowPhase {
                    title: "Light".into(),
                    detail: "quick".into(),
                    model: None,
                },
            ],
            steps: vec![step("a", &[])],
        };
        let js = render_workflow_js(&m);
        assert!(
            js.contains("{ title: \"Heavy\", detail: \"deep\", model: \"opus\" }"),
            "phase with model: {js}"
        );
        assert!(
            js.contains("{ title: \"Light\", detail: \"quick\" }"),
            "phase without model unchanged: {js}"
        );
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

    #[test]
    fn meta_phases_include_step_referenced_phases() {
        // A phase a step references but the manifest never declared must still
        // appear in meta.phases (every body phase() is declared), in topological
        // first-occurrence order: Audit (layer 0) before Verify (layer 1).
        let mut a = step("a", &[]);
        a.phase = Some("Audit".into());
        let mut b = step("b", &["a"]);
        b.phase = Some("Verify".into());
        let js = render_workflow_js(&manifest(vec![a, b]));
        assert!(
            js.contains("{ title: \"Audit\", detail: \"\" }"),
            "derived Audit in meta: {js}"
        );
        assert!(
            js.contains("{ title: \"Verify\", detail: \"\" }"),
            "derived Verify in meta: {js}"
        );
        let ai = js.find("title: \"Audit\"").unwrap();
        let vi = js.find("title: \"Verify\"").unwrap();
        assert!(ai < vi, "topological phase order in meta: {js}");
    }

    #[test]
    fn declared_phase_not_duplicated_by_step_reference() {
        // A declared phase keeps its detail/model and is NOT re-emitted as a bare
        // derived entry when a step also references it.
        let mut a = step("a", &[]);
        a.phase = Some("Scan".into());
        let mut m = manifest(vec![a]);
        m.phases = vec![WorkflowPhase {
            title: "Scan".into(),
            detail: "deep".into(),
            model: Some("opus".into()),
        }];
        let js = render_workflow_js(&m);
        assert!(
            js.contains("{ title: \"Scan\", detail: \"deep\", model: \"opus\" }"),
            "declared detail/model preserved: {js}"
        );
        assert_eq!(
            js.matches("title: \"Scan\"").count(),
            1,
            "no duplicate Scan entry: {js}"
        );
    }

    #[test]
    fn no_step_phase_keeps_meta_phases_byte_identical() {
        // Regression guard: with no per-step phase and no declared phases, the
        // reconciliation adds nothing — the meta block is byte-identical to the
        // pre-reconciliation output (empty phases list).
        let js = render_workflow_js(&manifest(vec![step("a", &[]), step("b", &["a"])]));
        assert!(js.contains("phases: [\n  ],"), "empty phase plan: {js}");
    }
}
