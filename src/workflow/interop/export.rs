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

use crate::workflow::def::WorkflowStepKind;
use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep, WorkflowPhase};

/// Embedded round-trip marker. `import` reads the JSON between prefix/suffix.
pub const EMBED_PREFIX: &str = "/* @aleph-workflow ";
pub const EMBED_SUFFIX: &str = " */";

/// Render `manifest` as a `.workflow.js` source string.
#[must_use]
pub fn render_workflow_js(manifest: &WorkflowManifest) -> String {
    // Escape `*/` as `*\/` (a legal JSON `/` escape) so a string field
    // containing `*/` (glob/regex/C-comment) cannot terminate the embed block
    // early; `import`'s serde_json parse reads `\/` back transparently.
    let manifest_json = serde_json::to_string(manifest)
        .unwrap_or_else(|e| {
            tracing::error!(%e, "WorkflowManifest serialization failed");
            "{}".to_string()
        })
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

    // 2b. Disclose the one thing the body skeleton cannot say (P7). Rendered
    // before the body so a reader hits it before the code it qualifies.
    if let Some(note) = partial_fan_in_disclosure(manifest, levels.as_deref()) {
        out.push_str(&note);
        out.push('\n');
    }

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
                        render_step_call(&manifest.steps[layer[0]])
                    ));
                } else {
                    out.push_str("await parallel([\n");
                    for &i in layer {
                        out.push_str(&format!(
                            "  () => {},\n",
                            render_step_call(&manifest.steps[i])
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
                out.push_str(&format!("await {}\n", render_step_call(step)));
            }
        }
    }

    out
}

/// Compute the `meta.phases` plan: the declared phases (preserving
/// detail/model/order) plus any phase a step references but the manifest did not
/// declare, appended in topological first-occurrence order. Guarantees every
/// body `phase()` marker is also declared in `meta`, matching the `.workflow.js`
/// convention ("use the same titles in meta.phases as in `phase()` calls"). Pure
/// field shuffling — no reasoning (R10).
fn effective_phases(
    manifest: &WorkflowManifest,
    levels: Option<&[Vec<usize>]>,
) -> Vec<WorkflowPhase> {
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
                    detail: String::new(), // rust-doctor-disable-line unnecessary-allocation
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

/// Step indices whose exact dependency set the body skeleton cannot express.
///
/// The body renders topological *layers*: a layer with more than one member
/// becomes `parallel([...])`. The bare-scan importer can derive exactly one
/// edge rule from that shape — "depend on every step of the preceding layer".
/// That is the inverse of this rendering **only** when each step's
/// `depends_on` is precisely its whole preceding layer.
///
/// Two shapes break it, and both used to break it silently:
/// - **partial fan-in** — `a` and `b` independent, `c` depends on `a` only. A
///   header-stripped round trip gives `c` `depends_on: [a, b]`, so a failing
///   `b` makes `c` `Unsatisfiable` where the original template ran it fine.
/// - **skip edges** — an edge reaching back *past* the preceding layer is not
///   recoverable from the layer shape at all.
///
/// Returned in body order (layer by layer, list order within a layer).
fn partial_fan_in_steps(manifest: &WorkflowManifest, levels: Option<&[Vec<usize>]>) -> Vec<usize> {
    // No topo order means the renderer already degraded to a flat sequence;
    // there is no layer shape to compare an edge set against.
    let Some(levels) = levels else {
        return Vec::new();
    };
    let index_of: HashMap<&str, usize> = manifest
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let mut offenders = Vec::new();
    let mut prev: HashSet<usize> = HashSet::new();
    for layer in levels {
        for &i in layer {
            let actual: HashSet<usize> = manifest.steps[i]
                .depends_on
                .iter()
                .filter_map(|d| index_of.get(d.as_str()).copied())
                .collect();
            if actual != prev {
                offenders.push(i);
            }
        }
        prev = layer.iter().copied().collect();
    }
    offenders
}

/// One `"<step id> depends on: <deps>"` line per step whose edge set the body
/// skeleton cannot express; empty when the render is the exact inverse of a
/// bare re-import.
///
/// The same predicate the in-file `//` disclosure uses, exposed for the tool's
/// export MESSAGE — a caller who exports through the `workflow` tool reads the
/// message, not the rendered file, so the two faces of one fact must share the
/// derivation instead of each carrying their own (criterion 9).
#[must_use]
pub(crate) fn partial_fan_in_notes(manifest: &WorkflowManifest) -> Vec<String> {
    let levels = topo_levels(manifest);
    partial_fan_in_notes_at(manifest, levels.as_deref())
}

/// [`partial_fan_in_notes`] against layers the caller already computed, so the
/// in-file disclosure and the tool message render the identical line text.
fn partial_fan_in_notes_at(manifest: &WorkflowManifest, levels: Option<&[Vec<usize>]>) -> Vec<String> {
    partial_fan_in_steps(manifest, levels)
        .into_iter()
        .map(|i| {
            let step = &manifest.steps[i];
            let deps = if step.depends_on.is_empty() {
                "nothing".to_string()
            } else {
                step.depends_on.join(", ")
            };
            format!("{} depends on: {deps}", step.id)
        })
        .collect()
}

/// The `//` comment block warning that this file's body is a lossy encoding of
/// its DAG, or `None` when the skeleton *is* the exact inverse.
///
/// Rendered into the file rather than only into the tool's message because the
/// population this protects is precisely the reader who ends up holding the
/// body without the header (P7 / criterion 17: a missing label is cheaper than
/// a wrong one, and "no note" here reads as "lossless").
fn partial_fan_in_disclosure(
    manifest: &WorkflowManifest,
    levels: Option<&[Vec<usize>]>,
) -> Option<String> {
    let offenders = partial_fan_in_notes_at(manifest, levels);
    if offenders.is_empty() {
        return None;
    }
    let mut out = String::from(
        "// NOTE - the body below is NOT a lossless encoding of this workflow's DAG.\n\
         // A parallel([...]) layer only says \"these run together\"; re-importing a\n\
         // header-stripped copy of this file makes each following step depend on ALL\n\
         // of the preceding layer. These steps depend on only part of it:\n",
    );
    for note in offenders {
        out.push_str(&format!("//   - {note}\n"));
    }
    out.push_str(
        "// Keep the @aleph-workflow header at the top of this file - it is what\n\
         // makes a re-import lossless.\n",
    );
    Some(out)
}

/// Render a step's body call, dispatching on kind: a clarify step becomes a
/// `clarify(prompt, [choices])` call (an Aleph extension to the `.workflow.js`
/// vocabulary), every other step an `agent(...)` call. The embedded
/// `@aleph-workflow` header is the canonical lossless round-trip; this body call
/// keeps the rendered source readable and re-importable on the bare-scan path.
fn render_step_call(step: &WorkflowManifestStep) -> String {
    match step.kind {
        WorkflowStepKind::Clarify => render_clarify_call(step),
        WorkflowStepKind::Agent => render_agent_call(step),
    }
}

/// Render a `clarify("question", ["a", "b"])` call. Choices are omitted for a
/// free-text clarification.
fn render_clarify_call(step: &WorkflowManifestStep) -> String {
    if step.choices.is_empty() {
        format!("clarify({})", render_prompt_arg(&step.prompt))
    } else {
        let choices: Vec<String> = step.choices.iter().map(|c| js_str(c)).collect();
        format!(
            "clarify({}, [{}])",
            render_prompt_arg(&step.prompt),
            choices.join(", ")
        )
    }
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
    if let Some(ef) = &step.effort {
        opts.push(format!("effort: {}", js_str(ef)));
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
    // Executable-core opts beyond the Claude-Code set: without these the
    // header-stripped (bare-scan) round-trip silently loses a human-review
    // safety gate and the per-step execution budgets — the wrong side to
    // fail on. `read_agent_opts` parses the bare literals back.
    if step.review {
        // The lead-review gate is an oversight attribute — silently dropping
        // it on a header-stripped re-import would auto-complete steps that
        // were meant to park in WaitingReview. Omitted when false (serde
        // `skip_serializing_if` parity keeps ungated steps byte-identical).
        opts.push("review: true".to_string());
    }
    if step.require_grounding {
        // Same reasoning as `review`: the grounding demand is an oversight
        // attribute, so it must survive a header-stripped round trip.
        opts.push("requireGrounding: true".to_string());
    }
    if step.tolerate_failed_deps {
        // Whether a step still runs after an upstream failure is the opposite
        // of decoration: dropping it on a header-stripped re-import turns a
        // fault-tolerant synthesis step back into a structurally dead one.
        opts.push("tolerateFailedDeps: true".to_string());
    }
    if let Some(t) = step.timeout_secs {
        opts.push(format!("timeoutSecs: {t}"));
    }
    if let Some(r) = step.max_retries {
        opts.push(format!("maxRetries: {r}"));
    }
    if opts.is_empty() {
        format!("agent({})", render_prompt_arg(&step.prompt))
    } else {
        format!(
            "agent({}, {{ {} }})",
            render_prompt_arg(&step.prompt),
            opts.join(", ")
        )
    }
}

/// Render a step prompt as the `agent()` first argument.
///
/// A single-line prompt renders as a plain string literal (byte-identical to
/// the legacy output). A multi-line prompt renders as the engineering format's
/// signature idiom — a `[ "line", ... ].join("\n")` array — so an exported
/// `.workflow.js` reads natively and stays editable line-by-line instead of
/// becoming one unreadable mega-string. The split/join is exact: `import`'s
/// array-join reader reconstructs the identical prompt (round-trip safe even on
/// the header-less bare-scan path).
fn render_prompt_arg(prompt: &str) -> String {
    if !prompt.contains('\n') {
        return js_str(prompt);
    }
    let mut out = String::from("[\n");
    for line in prompt.split('\n') {
        out.push_str("    ");
        out.push_str(&js_str(line));
        out.push_str(",\n");
    }
    // `.join("\n")` recombines the lines; the separator is itself a JS literal
    // (`js_str("\n")` → `"\n"`) so the source stays parseable.
    out.push_str("  ].join(");
    out.push_str(&js_str("\n"));
    out.push(')');
    out
}

/// Render a Rust string as a safe double-quoted JS string literal (handles all
/// escaping via `serde_json` — avoids the raw-backtick / quote-escape traps).
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
    let mut next: Vec<usize> = Vec::new();
    while !current.is_empty() {
        placed += current.len();
        next.clear();
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
        current = std::mem::take(&mut next);
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
            effort: None,
            kind: WorkflowStepKind::Agent,
            choices: vec![],
            review: false,
            require_grounding: false,
            tolerate_failed_deps: false,
            timeout_secs: None,
            max_retries: None,
        }
    }

    fn clarify_step(
        id: &str,
        question: &str,
        choices: &[&str],
        deps: &[&str],
    ) -> WorkflowManifestStep {
        WorkflowManifestStep {
            id: id.into(),
            agent: String::new(),
            prompt: question.into(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            label: None,
            model: None,
            phase: None,
            schema: None,
            isolation: None,
            agent_type: None,
            effort: None,
            kind: WorkflowStepKind::Clarify,
            choices: choices.iter().map(|s| s.to_string()).collect(),
            review: false,
            require_grounding: false,
            tolerate_failed_deps: false,
            timeout_secs: None,
            max_retries: None,
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
    fn effort_opt_rendered_and_roundtrips_via_bare_scan() {
        // `effort` is the reasoning-effort agent-opt from the current dynamic
        // `.workflow.js` vocabulary — it must render as a bare `effort: "…"` and
        // survive even the header-stripped bare-scan re-import (the strictest
        // symmetry check, since the embed header would otherwise mask a gap).
        use crate::workflow::interop::import::parse_workflow_js;
        let mut s = step("a", &[]);
        s.effort = Some("xhigh".into());
        let js = render_workflow_js(&manifest(vec![s]));
        assert!(js.contains("effort: \"xhigh\""), "effort rendered: {js}");
        // Strip the lossless embed header → force the bare scan path.
        let bare: String = js.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(!bare.contains(EMBED_PREFIX), "header stripped: {bare}");
        let back = parse_workflow_js(&bare).expect("bare re-import").manifest;
        assert_eq!(
            back.steps[0].effort.as_deref(),
            Some("xhigh"),
            "effort survives header-stripped round-trip: {bare}"
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
    fn multiline_prompt_renders_as_join_array() {
        // A prompt with newlines renders as the engineering format's
        // `[ "line", ... ].join("\n")` idiom, one element per line, instead of
        // a single unreadable mega-string.
        let mut s = step("a", &[]);
        s.prompt = "You are auditing X.\nRead the files.\nReport gaps.".into();
        let js = render_workflow_js(&manifest(vec![s]));
        assert!(js.contains("agent([\n"), "array opener: {js}");
        assert!(js.contains("\"You are auditing X.\","), "first line: {js}");
        assert!(js.contains("\"Read the files.\","), "middle line: {js}");
        assert!(js.contains("\"Report gaps.\","), "last line: {js}");
        assert!(js.contains("].join(\"\\n\")"), "join separator: {js}");
        // A single-line prompt stays a plain literal (no array).
        let plain = render_workflow_js(&manifest(vec![step("b", &[])]));
        assert!(
            !plain.contains(".join("),
            "single-line stays plain: {plain}"
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
    fn clarify_step_renders_clarify_call() {
        // A clarify step renders as `clarify(question, [choices])`, not agent(...).
        let js = render_workflow_js(&manifest(vec![clarify_step(
            "ask",
            "Deploy where?",
            &["staging", "prod"],
            &[],
        )]));
        assert!(js.contains("await clarify("), "clarify call: {js}");
        assert!(js.contains("\"Deploy where?\""), "question: {js}");
        assert!(
            js.contains("[\"staging\", \"prod\"]"),
            "choices array: {js}"
        );
        assert!(
            !js.contains("agent("),
            "no agent() for a clarify step: {js}"
        );
    }

    #[test]
    fn clarify_step_without_choices_renders_no_array() {
        let js = render_workflow_js(&manifest(vec![clarify_step(
            "ask",
            "Which file?",
            &[],
            &[],
        )]));
        assert!(
            js.contains("await clarify(\"Which file?\")"),
            "free-text clarify: {js}"
        );
    }

    #[test]
    fn clarify_step_roundtrips_via_embedded_header() {
        // The embedded @aleph-workflow header is the lossless path: export then
        // re-import reconstructs the clarify kind + choices exactly.
        use crate::workflow::interop::import::parse_workflow_js;
        let original = manifest(vec![clarify_step("ask", "Pick env", &["a", "b"], &[])]);
        let js = render_workflow_js(&original);
        let back = parse_workflow_js(&js).expect("import").manifest;
        assert_eq!(back.steps[0].kind, WorkflowStepKind::Clarify);
        assert_eq!(back.steps[0].choices, vec!["a", "b"]);
    }

    #[test]
    fn no_step_phase_keeps_meta_phases_byte_identical() {
        // Regression guard: with no per-step phase and no declared phases, the
        // reconciliation adds nothing — the meta block is byte-identical to the
        // pre-reconciliation output (empty phases list).
        let js = render_workflow_js(&manifest(vec![step("a", &[]), step("b", &["a"])]));
        assert!(js.contains("phases: [\n  ],"), "empty phase plan: {js}");
    }

    #[test]
    fn partial_fan_in_is_disclosed_in_the_rendered_body() {
        // a, b independent; c depends on a ONLY. topo_levels → [[a,b],[c]] and
        // the body renders `parallel([a, b])` then `await c` — a shape a
        // header-stripped re-import can only read as "c depends on a AND b".
        let js = render_workflow_js(&manifest(vec![
            step("a", &[]),
            step("b", &[]),
            step("c", &["a"]),
        ]));
        assert!(
            js.contains("NOT a lossless encoding"),
            "partial fan-in must be disclosed: {js}"
        );
        assert!(
            js.contains("//   - c depends on: a"),
            "the disclosure names the step and its real edges: {js}"
        );
        assert!(
            js.contains("@aleph-workflow header"),
            "the disclosure points at the lossless path: {js}"
        );
    }

    #[test]
    fn complete_bipartite_fan_in_is_not_disclosed() {
        // a, b independent; c and d each depend on BOTH. Every step's
        // depends_on is exactly its whole preceding layer, so the skeleton IS
        // the exact inverse and there is nothing to warn about.
        let js = render_workflow_js(&manifest(vec![
            step("a", &[]),
            step("b", &[]),
            step("c", &["a", "b"]),
            step("d", &["a", "b"]),
        ]));
        assert!(
            !js.contains("NOT a lossless encoding"),
            "a lossless skeleton must not carry the warning: {js}"
        );
    }

    #[test]
    fn linear_chain_is_not_disclosed() {
        // Singleton layers throughout: each step depends on exactly its
        // predecessor, which is its whole preceding layer.
        let js = render_workflow_js(&manifest(vec![
            step("a", &[]),
            step("b", &["a"]),
            step("c", &["b"]),
        ]));
        assert!(!js.contains("NOT a lossless encoding"), "{js}");
    }

    #[test]
    fn a_skip_edge_past_the_preceding_layer_is_disclosed() {
        // a → b → c plus a direct a → c edge. `c` sits one layer after `b`, so
        // the layer rule reconstructs `depends_on: [b]` and the a → c edge is
        // simply gone — the second shape the skeleton cannot carry.
        let js = render_workflow_js(&manifest(vec![
            step("a", &[]),
            step("b", &["a"]),
            step("c", &["a", "b"]),
        ]));
        assert!(js.contains("//   - c depends on: a, b"), "{js}");
    }

    #[test]
    fn the_disclosure_is_a_comment_and_does_not_alter_the_import() {
        // The note is a `//` comment: `blank_comments` erases it before the
        // scan, so it can never be mistaken for a step or a meta field. The
        // embedded header still drives this import, so the DAG is exact.
        use crate::workflow::interop::import::parse_workflow_js;
        let original = manifest(vec![step("a", &[]), step("b", &[]), step("c", &["a"])]);
        let js = render_workflow_js(&original);
        let back = parse_workflow_js(&js).expect("import").manifest;
        assert_eq!(back.steps.len(), 3, "no phantom step from the note");
        assert_eq!(back.steps[2].depends_on, vec!["a".to_string()]);
    }
}
