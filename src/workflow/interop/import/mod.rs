//! Parse a `.workflow.js` (or raw AWI manifest JSON) into a `WorkflowManifest`.
//!
//! Three paths, in priority order:
//! 0. **Bare manifest JSON** (starts with `{`) → exact parse, lossless.
//! 1. **Embedded block** (`/* @aleph-workflow {json} */`) → exact parse, lossless.
//! 2. **Bare `.workflow.js`** → light-weight scan of the pure-literal `meta`
//!    block + ordered `phase()` / `agent()` / `clarify()` calls; imperative
//!    constructs go into `dropped`. A `clarify("q", ["a", "b"])` step is the
//!    inverse of `export`'s `render_clarify_call`, so a header-stripped export
//!    of a workflow with clarify steps re-imports them faithfully (the bare path
//!    is symmetric for every body construct export emits, not only agents).
//!    `phase()` markers are captured and assigned to the steps that
//!    follow them, so a hand-written phased workflow keeps its phase plan. Each
//!    `agent(prompt, { opts })` call's opts object is parsed too — the literal
//!    `label`/`phase`/`model`/`isolation`/`agentType` fields and the `schema`
//!    are recovered, making the bare path symmetric with `export`'s
//!    `render_agent_call` (a header-stripped export re-imports its opts intact).
//!    A `schema` may be an inline literal *or* a `schema: NAME_SCHEMA` reference
//!    to a hoisted top-level `const` — the engineering format's convention.
//!    Both are normalised by the bounded data parser in [`super::consts`], which
//!    accepts JS-lax literals (bare keys, single quotes, trailing commas) that
//!    plain JSON parsing rejects, and *abstains* on any expression value.
//!    A `parallel([...])` block is reconstructed into sibling steps of one DAG
//!    layer (fan-out from the prior step, fan-in to the next), so the
//!    parallelisation / orchestrator-workers shape round-trips instead of being
//!    flattened into a sequential chain.
//!
//! No JS engine, no full parser (R3). The scan's limits are surfaced via
//! `dropped` — an unresolved / non-data `schema`, and a count of `agent()` calls
//! skipped for a dynamic (non-literal) prompt — never hidden (P7).
//!
//! **Layout.** [`lexer`] holds the literal readers and the two whole-source
//! rewrites; [`opts`] holds the `agent(…, { … })` opts object; [`scan`] turns a
//! body into ordered [`ScanEvent`]s. This file keeps the three entry paths and
//! the one thing that needs the whole picture — turning those events into a
//! manifest (DAG layers, phase plan, `dropped`).

mod lexer;
mod opts;
mod scan;

use crate::error::{AlephError, Result};
use crate::workflow::interop::consts::collect_consts;
use crate::workflow::interop::export::{EMBED_PREFIX, EMBED_SUFFIX};
use crate::workflow::interop::manifest::{WorkflowManifest, WorkflowManifestStep, WorkflowPhase};

use self::lexer::{blank_comments, read_first_string_literal, strip_string_literals};
use self::scan::{contains_call_like_keyword, scan_events, ScanEvent};

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
fn scan_bare(raw: &str) -> Result<ImportOutcome> {
    // Every read below works on the comment-blanked copy so offsets stay
    // mutually consistent; string literals (prompts) are untouched.
    let blanked = blank_comments(raw);
    let src: &str = &blanked;
    // Hoisted top-level `const NAME = <data-literal>` declarations, so a bare
    // `schema: AUDIT_SCHEMA` reference in the body resolves to its object —
    // and so `meta` itself is read as a parsed object rather than by grepping
    // the file for a `name:` substring.
    let consts = collect_consts(src);
    let meta_obj = consts.get("meta").and_then(serde_json::Value::as_object);
    // Prefer the parsed `meta` object; fall back to the positional scan only
    // when `meta` is not a pure data literal (it may hold an expression).
    //
    // The positional scan alone was wrong in both directions: it took the FIRST
    // `<field>:` anywhere in the raw source, so hoisting a schema const above
    // the meta block — the engineering format's own convention — made
    // `name:` land on a schema property whose next token is `{`, aborting the
    // whole import; and a `description:` inside a schema could supply the
    // workflow's description instead of the author's.
    // The fallback is per-SOURCE, not per-FIELD. A parsed `meta` is the
    // authority on what the workflow is called AND on what it deliberately does
    // not say: `export const meta = { name: 'audit', phases: [] }` omitting
    // `description` is an answer, not a gap. Falling back key-by-key let the
    // positional scan answer for those omissions, and the first `description:`
    // in a file that follows the format's own hoist-your-schema-consts
    // convention belongs to a JSON Schema property — so the workflow got
    // "PASS or FAIL" as its description. Only an unparseable `meta` (one
    // holding an expression) hands the question back to the scan.
    let meta_field = |field: &str| -> Option<String> {
        match meta_obj {
            Some(m) => m
                .get(field)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty()),
            None => scan_meta_field(src, field),
        }
    };
    let name = meta_field("name").ok_or_else(|| {
        AlephError::invalid_input(
            "no @aleph-workflow block and no `meta.name` found; cannot import",
        )
    })?;
    let description = meta_field("description").unwrap_or_default();
    let when_to_use = meta_field("whenToUse").unwrap_or_default();

    // Imperative-construct detection must ignore string-literal *contents* so a
    // prompt like `agent('search for files if (any) exist')` does not
    // false-positive as a `for` loop / `if` conditional. Scan only the code
    // skeleton with every string body blanked out (delimiters preserved).
    let skeleton = strip_string_literals(src);
    let mut dropped = Vec::new();
    // Imperative-construct detection matches MACHINE FORMATS (R8): a call
    // signature (`pipeline(`, `workflow(`) — the paren is the structural
    // marker, so identifiers like `pipeline_var` do not trip the needle.
    // A bare-word needle like `"budget"` would match natural-language use
    // (`// tune the budget`, a `BUDGET_LIMIT` const) without a structural
    // signature, violating R8; control-flow intent is already covered by the
    // `for` / `while` / `if` / `switch` keywords below, so no information is
    // lost by omitting it.
    for (needle, label) in [
        (
            "pipeline(",
            "pipeline(...) — runtime item list not statically known",
        ),
        ("workflow(", "nested workflow() call"),
    ] {
        if skeleton.contains(needle) {
            dropped.push(label.to_string());
        }
    }
    // Control-flow KEYWORDS are matched structurally, not by spelling. JS puts
    // any amount of whitespace (or none) between the keyword and its `(`, so a
    // spelling list answers "was this file written with a space" rather than
    // "does this file branch". It had `if (` and `if(` but only `for ` and
    // `while ` — so `for(const t of TARGETS) { await agent(...) }` imported its
    // loop body as ONE step and reported `dropped: []`, i.e. claimed a lossless
    // import of a file whose per-target fan-out had been collapsed.
    for (keyword, label) in [
        ("for", "for loop"),
        ("while", "while loop"),
        ("if", "if conditional"),
        ("switch", "switch statement"),
    ] {
        if contains_call_like_keyword(&skeleton, keyword) {
            dropped.push(label.to_string());
        }
    }
    // Array fan-out is a METHOD call, not a statement keyword, so it cannot ride
    // on the list above: `contains_call_like_keyword("for", …)` deliberately
    // rejects `forEach(` (the leading-boundary rule is what keeps `iffy(` and
    // `switcher` from false-positiving), and that rejection is correct there.
    // But `.forEach(...)` / `.map(...)` ARE the engineering format's fan-out
    // idiom. The dynamic-prompt counter only catches the fan-out whose prompt is
    // built per item; a LITERAL prompt inside the closure
    // (`TARGETS.forEach(() => agent("audit this target"))`) imported as ONE step
    // with `dropped: []` — an import reported lossless for a file whose N-way
    // fan-out had been collapsed. Matched with the leading `.` so a local
    // identifier named `map` or a `Map(` constructor does not trip it.
    if skeleton.contains(".forEach(") || skeleton.contains(".map(") {
        dropped.push(
            "array fan-out (.forEach/.map) — runtime item list not statically known".to_string(),
        );
    }
    // NOTE: `parallel([...])` is NO LONGER dropped — the scan reconstructs its
    // sibling steps as a DAG layer (see the `ParallelStart`/`ParallelEnd`
    // handling below), so the parallelisation structure round-trips faithfully
    // instead of being linearised.

    // Walk the source in order, tracking the active `phase()` so each scanned
    // `agent()` step inherits it. The scan is string-aware (a prompt mentioning
    // `phase(` or `agent(` never registers as a call), so phases are reconstructed
    // only from real markers.
    let mut steps: Vec<WorkflowManifestStep> = Vec::new();
    let mut phase_titles: Vec<String> = Vec::new();
    let mut current_phase: Option<String> = None;
    // Dependency reconstruction tracks the previous DAG "layer" so a
    // `parallel([...])` block re-imports as sibling steps that fan out from the
    // layer before it and fan into the step after it. A sequential step is a
    // singleton layer (a plain chain). This recovers the parallelisation /
    // orchestrator-workers DAG shape a flat scan would otherwise linearise, so
    // re-imported siblings stay independent `Pending` tasks the dispatcher runs
    // concurrently.
    //
    // NOT the exact inverse of `export`'s topo-layers → `parallel(...)`
    // rendering — this comment used to claim that, and it was only true for
    // adjacent layers that are complete-bipartite in the original DAG. "Depend
    // on every step of the preceding layer" WIDENS a partial fan-in: `a`, `b`
    // independent and `c` depending on `a` alone round-trips into `c` depending
    // on `a` AND `b`, so a failing `b` makes `c` `Unsatisfiable` where the
    // original template ran it. The layer shape simply does not carry the edge
    // set, so nothing here can recover it; the loss is disclosed on the far end
    // instead — `export::partial_fan_in_disclosure` writes a `//` note into any
    // rendered file whose DAG this rule cannot reproduce, and the embedded
    // `@aleph-workflow` header (read before this scan ever runs) is what makes a
    // round trip lossless.
    let mut prev_layer: Vec<usize> = Vec::new();
    let mut parallel_depth: u32 = 0;
    let mut parallel_group: Vec<usize> = Vec::new();
    // Count of `agent()`/`clarify()` calls skipped for a dynamic (non-literal)
    // prompt — reported in `dropped` after the walk so the loss is never silent.
    let mut dynamic_prompts: usize = 0;
    for ev in scan_events(src, &consts) {
        match ev {
            ScanEvent::DynamicPrompt => dynamic_prompts += 1,
            ScanEvent::Phase(title) => {
                if !phase_titles.iter().any(|t| t == &title) {
                    phase_titles.push(title.clone());
                }
                current_phase = Some(title);
            }
            ScanEvent::ParallelStart => {
                // Only the outermost block defines a layer; nested parallels just
                // keep accumulating into the same sibling group (all concurrent).
                if parallel_depth == 0 {
                    parallel_group.clear();
                }
                parallel_depth += 1;
            }
            ScanEvent::ParallelEnd => {
                parallel_depth = parallel_depth.saturating_sub(1);
                // Closing the outermost block: its members become the layer the
                // next sequential step fans in from.
                if parallel_depth == 0 && !parallel_group.is_empty() {
                    prev_layer = std::mem::take(&mut parallel_group);
                }
            }
            ScanEvent::Agent(call) => {
                let call = *call;
                let i = steps.len();
                // An explicit `phase:` opt on the agent() call wins over the
                // active `phase()` marker: the engineering format gives per-agent
                // phases inside `parallel([...])` / `pipeline(...)` stages where no
                // top-level marker precedes them. Register a phase seen only via an
                // opt so it still appears in the reconstructed phase plan.
                if let Some(ph) = &call.opts.phase {
                    if !phase_titles.iter().any(|t| t == ph) {
                        phase_titles.push(ph.clone());
                    }
                }
                let phase = call.opts.phase.or_else(|| current_phase.clone());
                // Depend on every step in the preceding layer: one for a
                // sequential predecessor, N for a fan-in after a parallel block.
                let depends_on: Vec<String> = prev_layer
                    .iter()
                    .map(|&p| format!("step_{}", p + 1))
                    .collect();
                steps.push(WorkflowManifestStep {
                    id: format!("step_{}", i + 1),
                    agent: "agent".to_string(),
                    prompt: call.prompt,
                    depends_on,
                    label: call.opts.label,
                    model: call.opts.model,
                    phase,
                    schema: call.opts.schema,
                    isolation: call.opts.isolation,
                    agent_type: call.opts.agent_type,
                    effort: call.opts.effort,
                    require_grounding: call.opts.require_grounding,
                    tolerate_failed_deps: call.opts.tolerate_failed_deps,
                    kind: crate::workflow::def::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: call.opts.review,
                    timeout_secs: call.opts.timeout_secs,
                    max_retries: call.opts.max_retries,
                });
                // Surface a `schema:` that could not be captured (unknown const
                // ref or a non-data literal) — the step imports, but say the
                // schema was lost rather than leave it silently absent (P7).
                if let Some(note) = call.opts.schema_dropped {
                    dropped.push(note);
                }
                // Same rule, the case it did not cover: an opts object the
                // reader could not finish. Silence here reads as "lossless
                // import" while `review` / `requireGrounding` came back off.
                if let Some(note) = call.opts.opts_abandoned {
                    dropped.push(note);
                }
                // A sibling inside a parallel block extends the current group; a
                // sequential step becomes the next singleton layer.
                if parallel_depth > 0 {
                    parallel_group.push(i);
                } else {
                    prev_layer = vec![i];
                }
            }
            ScanEvent::Clarify(call) => {
                // A `clarify("q", [choices])` step is the exact inverse of
                // `export`'s `render_clarify_call` — recover it so the bare path
                // is symmetric for every body construct export emits (agent,
                // parallel, phase, opts, AND clarify), not only the embed header.
                // It joins the DAG with the same layer logic as an agent step:
                // it can follow the prior layer and be depended on by the next.
                let i = steps.len();
                let depends_on: Vec<String> = prev_layer
                    .iter()
                    .map(|&p| format!("step_{}", p + 1))
                    .collect();
                steps.push(WorkflowManifestStep {
                    id: format!("step_{}", i + 1),
                    // A clarify step is owned by the sentinel, not a team member,
                    // so its `agent` is intentionally empty (matches `from_def`).
                    agent: String::new(), // rust-doctor-disable-line unnecessary-allocation
                    prompt: call.prompt,
                    depends_on,
                    label: None,
                    model: None,
                    // A clarify step has no opts object, so it inherits the active
                    // `phase()` marker just like a sequential agent step would.
                    phase: current_phase.clone(),
                    schema: None,
                    isolation: None,
                    agent_type: None,
                    effort: None,
                    kind: crate::workflow::def::WorkflowStepKind::Clarify,
                    choices: call.choices,
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_secs: None,
                    max_retries: None,
                });
                if parallel_depth > 0 {
                    parallel_group.push(i);
                } else {
                    prev_layer = vec![i];
                }
            }
        }
    }
    if steps.is_empty() {
        // Distinguish "nothing there" from "everything there was dynamic": a
        // parameterised engineering file (per-item `buildPrompt(u)`, `.map`)
        // has agent() calls but none statically importable — say so.
        return Err(AlephError::invalid_input(if dynamic_prompts > 0 {
            format!(
                "no statically-importable agent() calls in .workflow.js \
                 ({dynamic_prompts} had dynamic (non-literal) prompts); nothing to import"
            )
        } else {
            "no agent() calls found in .workflow.js; nothing to import".to_string()
        }));
    }
    // Report agent()/clarify() calls skipped for a dynamic prompt so a partly
    // static file discloses exactly what the scan could not capture (P7).
    if dynamic_prompts > 0 {
        dropped.push(format!(
            "{dynamic_prompts} agent()/clarify() call(s) with dynamic (non-literal) \
             prompts not imported"
        ));
    }
    // Honesty at the boundary (P7): the bare scan cannot know which team
    // member owns each call, so every agent step gets the placeholder owner
    // "agent". Say so — otherwise a subsequent `run` fails team preflight on
    // an owner the import invented, with no hint where it came from.
    if steps
        .iter()
        .any(|s| s.kind == crate::workflow::def::WorkflowStepKind::Agent && s.agent == "agent")
    {
        dropped.push(
            "agent owners unknown — steps assigned placeholder owner 'agent'; retarget the \
             agents (edit + save) before running"
                .to_string(),
        );
    }
    let phases = phase_plan(meta_obj, phase_titles);

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

/// The phase plan for a bare import: a parsed `meta.phases` is the authority,
/// body `phase()` markers only supply what it does not declare.
///
/// `export::render_meta` writes each `meta.phases` entry WITH its `detail` and,
/// when set, its `model`. Rebuilding the plan from body markers alone threw both
/// away on every header-stripped round trip — with `dropped` empty, on a path
/// whose module doc says every loss lands there.
///
/// This is the same "meta is authoritative, including about what it does not
/// say" rule the string fields already follow, applied per-SOURCE: a parsed
/// `meta` answers for the plan; titles seen only as body markers are appended
/// (a hand-written file may carry `phase()` calls and no `meta.phases` at all),
/// and a `meta` that is not a pure data literal leaves markers as the only
/// source, exactly as before.
fn phase_plan(
    meta_obj: Option<&serde_json::Map<String, serde_json::Value>>,
    marker_titles: Vec<String>,
) -> Vec<WorkflowPhase> {
    let mut out: Vec<WorkflowPhase> = Vec::new();
    if let Some(arr) = meta_obj
        .and_then(|m| m.get("phases"))
        .and_then(serde_json::Value::as_array)
    {
        for entry in arr {
            // An entry that is not an object, or carries no usable `title`, is
            // not a phase this plan can name — skip it rather than mint a
            // blank-titled phase (a wrong label costs more than a missing one).
            let Some(obj) = entry.as_object() else {
                continue;
            };
            let title = obj
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if title.is_empty() || out.iter().any(|p| p.title == title) {
                continue;
            }
            out.push(WorkflowPhase {
                title: title.to_string(),
                detail: obj
                    .get("detail")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                model: obj
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            });
        }
    }
    for title in marker_titles {
        if !out.iter().any(|p| p.title == title) {
            out.push(WorkflowPhase {
                title,
                detail: String::new(), // rust-doctor-disable-line unnecessary-allocation
                model: None,
            });
        }
    }
    out
}

/// Find `<field>:` then read the next JS string literal that follows it.
fn scan_meta_field(src: &str, field: &str) -> Option<String> {
    let key = format!("{field}:");
    let pos = src.find(&key)? + key.len();
    read_first_string_literal(&src[pos..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bare scanner had no notion of comments, and two ordinary files broke
    /// it silently: an apostrophe in a `//` comment opened a phantom string
    /// literal that swallowed the rest of the file (every step vanished, the
    /// user was told "no agent() calls found"), and a commented-out `agent()`
    /// imported as a live step.
    #[test]
    fn a_comment_apostrophe_does_not_swallow_the_rest_of_the_file() {
        let src = r#"
export const meta = { name: 'wf', description: 'demo' }
// don't forget to update the schema
await agent('gather the sources')
await agent('write the brief')
"#;
        let out = scan_bare(src).expect("import must succeed");
        let ids: Vec<&str> = out.manifest.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "both steps survive the comment: {ids:?}");
        assert!(out.manifest.steps[0].prompt.contains("gather the sources"));
    }

    #[test]
    fn a_commented_out_agent_call_is_not_imported_as_a_step() {
        let src = r#"
export const meta = { name: 'wf' }
// await agent('the old first pass')
await agent('the real step')
/* await agent('an even older pass') */
"#;
        let out = scan_bare(src).expect("import must succeed");
        assert_eq!(out.manifest.steps.len(), 1, "only the live call is a step");
        assert!(out.manifest.steps[0].prompt.contains("the real step"));
    }

    /// `meta.name` used to be found by grepping the whole raw source for
    /// `"name:"`, so hoisting a schema const above the meta block — the
    /// engineering format's own convention — made the scan land on a schema
    /// property and abort the entire import.
    #[test]
    fn a_hoisted_schema_const_does_not_hijack_meta_name() {
        let src = r#"
const REPORT_SCHEMA = { type: 'object', properties: { name: { type: 'string' } } }
export const meta = { name: 'audit', description: 'the real description' }
await agent('do the thing', { schema: REPORT_SCHEMA })
"#;
        let out = scan_bare(src).expect("import must succeed");
        assert_eq!(out.manifest.name, "audit");
        assert_eq!(out.manifest.description, "the real description");
    }

    /// A quoted object key is ordinary JS; hitting one used to abandon the whole
    /// opts object at that point, silently dropping the `review` safety gate and
    /// everything after it.
    #[test]
    fn a_quoted_opts_key_does_not_drop_the_review_gate() {
        let src = r#"
export const meta = { name: 'wf' }
await agent('deploy to prod', { "phase": "Ship", review: true, timeoutSecs: 900 })
"#;
        let out = scan_bare(src).expect("import must succeed");
        let step = &out.manifest.steps[0];
        assert!(step.review, "the review gate must survive a quoted key");
        assert_eq!(step.timeout_secs, Some(900));
        assert_eq!(step.phase.as_deref(), Some("Ship"));
    }

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
                    effort: Some("high".into()),
                    kind: crate::workflow::def::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_secs: None,
                    max_retries: None,
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
                    effort: None,
                    kind: crate::workflow::def::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_secs: None,
                    max_retries: None,
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
        let src = r"
export const meta = {
  name: 'hand-written',
  description: 'a manual workflow',
  whenToUse: 'when testing',
}
await agent('first step')
await agent('second step')
";
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
        let src = r"
export const meta = { name: 'loopy' }
for (const x of items) {
  await agent('do thing')
}
const r = await pipeline(items, s1, s2)
";
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
                effort: None,
                kind: crate::workflow::def::WorkflowStepKind::Agent,
                choices: vec![],
                review: false,
                require_grounding: false,
                tolerate_failed_deps: false,
                timeout_secs: None,
                max_retries: None,
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
        // The bare path always notes its placeholder owners (an honesty note,
        // not an imperative-construct report) — filter it before asserting no
        // needle fired on the prompt text.
        let imperative: Vec<&String> = outcome
            .dropped
            .iter()
            .filter(|d| !d.contains("placeholder owner"))
            .collect();
        assert!(
            imperative.is_empty(),
            "prompt text must not trip imperative needles, got: {imperative:?}"
        );
        assert_eq!(outcome.manifest.steps.len(), 1);
    }

    #[test]
    fn bare_scan_notes_placeholder_owners_once() {
        let src = "export const meta = { name: 'wf' }\n\
                   await agent('a')\nawait agent('b')";
        let outcome = parse_workflow_js(src).expect("scan bare js");
        assert_eq!(
            outcome
                .dropped
                .iter()
                .filter(|d| d.contains("placeholder owner"))
                .count(),
            1,
            "exactly one honesty note regardless of step count"
        );
    }

    #[test]
    fn review_and_budget_opts_survive_bare_roundtrip() {
        // The executable-core opts (review / timeoutSecs / maxRetries) must
        // survive a header-STRIPPED export → import — losing a human-review
        // safety gate silently is the wrong side to fail on.
        let src = "export const meta = { name: 'wf' }\n\
                   await agent('do it', { review: true, timeoutSecs: 1800, maxRetries: 0 })";
        let outcome = parse_workflow_js(src).expect("scan bare js");
        let step = &outcome.manifest.steps[0];
        assert!(step.review, "review flag parsed from bare literal");
        assert_eq!(step.timeout_secs, Some(1800));
        assert_eq!(step.max_retries, Some(0));
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
        let src = r"
export const meta = { name: 'phased' }
phase('Audit')
await agent('audit the code')
phase('Fix')
await agent('fix the bug')
await agent('fix more')
";
        let outcome = parse_workflow_js(src).expect("scan phased js");
        let m = outcome.manifest;
        assert_eq!(
            m.phases
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>(),
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

    #[test]
    fn bare_js_imports_join_array_prompt() {
        // The engineering format's signature multi-line idiom: a
        // `[ 'a', 'b' ].join('\n')` prompt array must import as the joined
        // string (the separator literal decodes to a real newline).
        let src = "export const meta = { name: 'arr' }\n\
                   await agent([ 'first line', 'second line' ].join('\\n'))";
        let outcome = parse_workflow_js(src).expect("scan array prompt");
        assert_eq!(outcome.manifest.steps.len(), 1);
        assert_eq!(
            outcome.manifest.steps[0].prompt, "first line\nsecond line",
            "array elements joined by the decoded separator"
        );
    }

    #[test]
    fn multiline_export_reimports_through_bare_scan() {
        // Export a multi-line prompt, drop the lossless embed header, and prove
        // the bare scanner reconstructs the EXACT prompt from the rendered
        // `[...].join("\n")` array — export and import are symmetric even
        // without the header.
        let m = WorkflowManifest {
            name: "ml".into(),
            description: String::new(),
            when_to_use: String::new(),
            phases: vec![],
            steps: vec![WorkflowManifestStep {
                id: "a".into(),
                agent: "agent".into(),
                prompt: "Line one.\nLine two.\nLine three.".into(),
                depends_on: vec![],
                label: None,
                model: None,
                phase: None,
                schema: None,
                isolation: None,
                agent_type: None,
                effort: None,
                kind: crate::workflow::def::WorkflowStepKind::Agent,
                choices: vec![],
                review: false,
                require_grounding: false,
                tolerate_failed_deps: false,
                timeout_secs: None,
                max_retries: None,
            }],
        };
        let js = render_workflow_js(&m);
        // Skip the first line (the `/* @aleph-workflow {...} */` embed header) so
        // import is forced down the bare-scan path.
        let bare: String = js.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(
            !bare.contains("@aleph-workflow"),
            "embed header stripped: {bare}"
        );
        let outcome = parse_workflow_js(&bare).expect("bare scan of multi-line export");
        assert_eq!(outcome.manifest.steps.len(), 1);
        assert_eq!(
            outcome.manifest.steps[0].prompt, "Line one.\nLine two.\nLine three.",
            "exact prompt reconstructed from the join array"
        );
    }

    #[test]
    fn join_array_with_nonliteral_element_abstains() {
        // A `GROUND_TRUTH` identifier inside the array makes the joined value
        // dynamic → the array agent is not captured (R7/R10); only the adjacent
        // plain-literal agent becomes a step.
        let src = "export const meta = { name: 'dyn' }\n\
                   await agent([ 'intro', GROUND_TRUTH ].join('\\n'))\n\
                   await agent('plain real prompt')";
        let outcome = parse_workflow_js(src).expect("scan");
        assert_eq!(
            outcome.manifest.steps.len(),
            1,
            "dynamic array abstains; only the literal agent counts"
        );
        assert_eq!(outcome.manifest.steps[0].prompt, "plain real prompt");
    }

    #[test]
    fn join_array_with_concatenation_abstains() {
        // Element-level concatenation (`'prefix ' + name`) is also dynamic.
        let src = "export const meta = { name: 'cat' }\n\
                   await agent([ 'prefix ' + name ].join('\\n'))\n\
                   await agent('real')";
        let outcome = parse_workflow_js(src).expect("scan");
        assert_eq!(outcome.manifest.steps.len(), 1);
        assert_eq!(outcome.manifest.steps[0].prompt, "real");
    }

    #[test]
    fn bare_js_captures_agent_opts() {
        // A hand-written agent() with a full opts object recovers every literal
        // field plus the JSON schema — the bare path is no longer prompt-only.
        let src = "export const meta = { name: 'opts' }\n\
                   await agent('do it', { label: \"audit:a\", model: \"haiku\", \
                   isolation: \"worktree\", agentType: \"Explore\", \
                   schema: {\"type\":\"object\"} })";
        let outcome = parse_workflow_js(src).expect("scan opts");
        let s = &outcome.manifest.steps[0];
        assert_eq!(s.prompt, "do it");
        assert_eq!(s.label.as_deref(), Some("audit:a"));
        assert_eq!(s.model.as_deref(), Some("haiku"));
        assert_eq!(s.isolation.as_deref(), Some("worktree"));
        assert_eq!(s.agent_type.as_deref(), Some("Explore"));
        assert_eq!(s.schema, Some(serde_json::json!({"type": "object"})));
    }

    #[test]
    fn agent_opts_survive_header_stripped_export_roundtrip() {
        // The headline symmetry: export a step carrying every agent opt, drop the
        // lossless `@aleph-workflow` embed header, and prove the bare scanner
        // recovers each opt. Before this, a header-stripped export re-imported as
        // a prompt-only skeleton — the persisted manifest silently lost label /
        // model / phase / schema / isolation / agentType.
        let m = WorkflowManifest {
            name: "sym".into(),
            description: String::new(),
            when_to_use: String::new(),
            phases: vec![],
            steps: vec![WorkflowManifestStep {
                id: "a".into(),
                agent: "agent".into(),
                prompt: "Audit the module.\nReport gaps.".into(),
                depends_on: vec![],
                label: Some("audit:mod".into()),
                model: Some("opus".into()),
                phase: Some("Audit".into()),
                schema: Some(serde_json::json!({"type": "object", "required": ["x"]})),
                isolation: Some("worktree".into()),
                agent_type: Some("code-reviewer".into()),
                effort: Some("high".into()),
                kind: crate::workflow::def::WorkflowStepKind::Agent,
                choices: vec![],
                review: true,
                require_grounding: false,
                tolerate_failed_deps: true,
                timeout_secs: None,
                max_retries: None,
            }],
        };
        let js = render_workflow_js(&m);
        let bare: String = js.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(
            !bare.contains("@aleph-workflow"),
            "embed header stripped: {bare}"
        );
        let outcome = parse_workflow_js(&bare).expect("bare scan of opts export");
        let s = &outcome.manifest.steps[0];
        assert_eq!(s.prompt, "Audit the module.\nReport gaps.");
        assert_eq!(s.label.as_deref(), Some("audit:mod"));
        assert_eq!(s.model.as_deref(), Some("opus"));
        assert_eq!(s.phase.as_deref(), Some("Audit"));
        assert_eq!(s.isolation.as_deref(), Some("worktree"));
        assert_eq!(s.agent_type.as_deref(), Some("code-reviewer"));
        assert_eq!(s.effort.as_deref(), Some("high"));
        assert_eq!(
            s.schema,
            Some(serde_json::json!({"type": "object", "required": ["x"]}))
        );
        // The oversight gate must survive a header-stripped round-trip — a
        // silently dropped `review: true` auto-completes a step that was meant
        // to park in WaitingReview for lead approval.
        assert!(s.review, "lead-review gate lost in bare round-trip");
        // Same class of loss on the other side of the ledger: dropping
        // `tolerateFailedDeps` turns a step authored to survive an upstream
        // failure back into one the first failure kills.
        assert!(
            s.tolerate_failed_deps,
            "tolerant fan-in lost in bare round-trip"
        );
    }

    #[test]
    fn agent_phase_opt_registers_and_assigns_without_marker() {
        // Inside a parallel([...]) stage the format gives the phase only as an
        // opt, with no preceding phase() marker. The opt must both set the step's
        // phase and register the title in the reconstructed phase plan.
        let src = "export const meta = { name: 'p' }\n\
                   await parallel([\n\
                     () => agent('review bugs', { phase: \"Review\" }),\n\
                   ])";
        let outcome = parse_workflow_js(src).expect("scan");
        let m = outcome.manifest;
        assert_eq!(m.steps[0].phase.as_deref(), Some("Review"));
        assert_eq!(
            m.phases
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Review"],
            "phase plan reconstructed from the opt"
        );
    }

    #[test]
    fn agent_phase_opt_overrides_active_marker() {
        // A `phase()` marker is active, but the agent carries a different `phase:`
        // opt — the explicit opt wins (mirrors export, which writes the per-step
        // phase faithfully).
        let src = "export const meta = { name: 'ov' }\n\
                   phase('Outer')\n\
                   await agent('x', { phase: \"Inner\" })";
        let outcome = parse_workflow_js(src).expect("scan");
        assert_eq!(outcome.manifest.steps[0].phase.as_deref(), Some("Inner"));
    }

    #[test]
    fn agent_opts_skip_nonliteral_value_keep_others() {
        // A computed opt value (a bare identifier) is left unset rather than
        // guessed (R7/R10), but literal siblings on either side still bind.
        let src = "export const meta = { name: 'mix' }\n\
                   await agent('go', { label: \"L\", schema: SOME_SCHEMA, model: \"haiku\" })";
        let outcome = parse_workflow_js(src).expect("scan");
        let s = &outcome.manifest.steps[0];
        assert_eq!(s.label.as_deref(), Some("L"));
        assert!(
            s.schema.is_none(),
            "identifier schema left unset, not guessed"
        );
        assert_eq!(s.model.as_deref(), Some("haiku"));
    }

    #[test]
    fn bare_js_resolves_hoisted_schema_const() {
        // The engineering format hoists schemas as a top-level JS object literal
        // (bare keys, single quotes, trailing comma) and references it by name.
        // The scan resolves the reference to its normalised JSON.
        let src = "export const meta = { name: 'sc' }\n\
                   const AUDIT_SCHEMA = { type: 'object', required: ['lens'], }\n\
                   await agent('audit it', { phase: \"Audit\", schema: AUDIT_SCHEMA })";
        let outcome = parse_workflow_js(src).expect("scan schema const");
        let s = &outcome.manifest.steps[0];
        assert_eq!(
            s.schema,
            Some(serde_json::json!({"type": "object", "required": ["lens"]})),
            "hoisted const schema resolved and normalised to JSON"
        );
        assert!(
            !outcome.dropped.iter().any(|d| d.contains("schema")),
            "resolved schema is not reported dropped: {:?}",
            outcome.dropped
        );
    }

    #[test]
    fn bare_js_resolves_inline_js_literal_schema() {
        // An inline schema written in JS-lax syntax (single quotes, bare keys)
        // now normalises to JSON — before, plain JSON parsing rejected it and the
        // schema vanished silently.
        let src = "export const meta = { name: 'in' }\n\
                   await agent('go', { schema: { type: 'string', enum: ['a', 'b'] } })";
        let outcome = parse_workflow_js(src).expect("scan inline js schema");
        assert_eq!(
            outcome.manifest.steps[0].schema,
            Some(serde_json::json!({"type": "string", "enum": ["a", "b"]}))
        );
    }

    #[test]
    fn bare_js_unresolved_schema_ref_recorded_in_dropped() {
        // A `schema:` referencing a name with no top-level data-literal const is
        // surfaced in dropped (P7); the schema stays unset — never guessed.
        let src = "export const meta = { name: 'un' }\n\
                   await agent('go', { schema: MISSING_SCHEMA })";
        let outcome = parse_workflow_js(src).expect("scan");
        assert!(outcome.manifest.steps[0].schema.is_none());
        assert!(
            outcome
                .dropped
                .iter()
                .any(|d| d.contains("MISSING_SCHEMA") && d.contains("unresolved")),
            "unresolved schema ref surfaced: {:?}",
            outcome.dropped
        );
    }

    /// The opts reader is not a JS parser, so it must stop at a spread — but
    /// stopping SILENTLY loses the oversight half of the format. `review` and
    /// `requireGrounding` both default to `false`, so an abandoned tail turns a
    /// step that was authored to park in `WaitingReview` into one that
    /// auto-completes, while `dropped` came back empty.
    #[test]
    fn bare_js_abandoned_opts_tail_is_disclosed() {
        let src = "export const meta = { name: 'sp' }\n\
                   await agent('deploy', { ...BASE_OPTS, review: true, requireGrounding: true })";
        let outcome = parse_workflow_js(src).expect("scan");
        assert!(
            !outcome.manifest.steps[0].review,
            "the spread really does hide the rest — this is the premise"
        );
        assert!(
            outcome
                .dropped
                .iter()
                .any(|d| d.contains("agent opts") && d.contains("review")),
            "an abandoned opts tail must be disclosed: {:?}",
            outcome.dropped
        );
    }

    /// A parsed `meta` is the authority on what it does NOT say. Falling back
    /// per-field let the first `description:` in the file answer — and the
    /// format's own convention hoists schema consts above `meta`.
    #[test]
    fn a_hoisted_schema_const_does_not_hijack_the_description() {
        let src = "const REPORT_SCHEMA = {\n\
                   \x20 type: 'object',\n\
                   \x20 properties: { verdict: { type: 'string', description: 'PASS or FAIL' } },\n\
                   }\n\
                   export const meta = { name: 'audit', whenToUse: 'after changes' }\n\
                   await agent('run the audit')";
        let outcome = parse_workflow_js(src).expect("scan");
        assert_ne!(
            outcome.manifest.description, "PASS or FAIL",
            "a schema property must not supply the workflow description"
        );
        assert_eq!(outcome.manifest.name, "audit");
    }

    /// JS puts any amount of whitespace — or none — between a keyword and its
    /// `(`. A spelling list answers "was this written with a space", not "does
    /// this branch": `for(` collapsed a per-target loop into ONE step and
    /// reported a lossless import.
    #[test]
    fn a_no_space_loop_is_disclosed_like_a_spaced_one() {
        for src in [
            "export const meta = { name: 'sweep' }\n\
             for(const t of TARGETS) { await agent('audit one target') }",
            "export const meta = { name: 'sweep' }\n\
             for (const t of TARGETS) { await agent('audit one target') }",
        ] {
            let outcome = parse_workflow_js(src).expect("scan");
            assert!(
                outcome.dropped.iter().any(|d| d.contains("for loop")),
                "a collapsed loop must be disclosed however it is spelled: {:?}",
                outcome.dropped
            );
        }
    }

    /// The boundary check that makes the no-space match safe.
    #[test]
    fn a_keyword_inside_an_identifier_is_not_a_construct() {
        let src = "export const meta = { name: 'ok' }\n\
                   const r = items.forEach(x => x)\n\
                   await agent('go')";
        let outcome = parse_workflow_js(src).expect("scan");
        assert!(
            !outcome.dropped.iter().any(|d| d.contains("for loop")),
            "`forEach(` is not a for loop: {:?}",
            outcome.dropped
        );
    }

    #[test]
    fn bare_js_non_data_inline_schema_recorded_in_dropped() {
        // An inline schema literal holding an expression value is not pure data →
        // it abstains and is reported, never half-captured (R3/R7).
        let src = "export const meta = { name: 'nd' }\n\
                   await agent('go', { schema: { items: buildItems() } })";
        let outcome = parse_workflow_js(src).expect("scan");
        assert!(outcome.manifest.steps[0].schema.is_none());
        assert!(
            outcome.dropped.iter().any(|d| d.contains("non-data")),
            "non-data inline schema surfaced: {:?}",
            outcome.dropped
        );
    }

    #[test]
    fn bare_js_dynamic_prompt_calls_counted_in_dropped() {
        // A mix of a static agent and dynamic-prompt agents: the static one
        // imports, and the count of skipped dynamic calls is disclosed (P7).
        let src = "export const meta = { name: 'mix' }\n\
                   await agent('static one')\n\
                   await agent(buildPrompt(u))\n\
                   await agent(promptVar)";
        let outcome = parse_workflow_js(src).expect("scan");
        assert_eq!(
            outcome.manifest.steps.len(),
            1,
            "only the static agent imports"
        );
        assert!(
            outcome
                .dropped
                .iter()
                .any(|d| d.contains("2 agent()") && d.contains("dynamic")),
            "dynamic-prompt count disclosed: {:?}",
            outcome.dropped
        );
    }

    #[test]
    fn bare_js_all_dynamic_prompts_errors_with_count() {
        // A fully parameterised file (every prompt dynamic) has nothing to import
        // — the error names the dynamic count so the failure is legible.
        let src = "export const meta = { name: 'allDyn' }\n\
                   await agent(buildPrompt(a))\n\
                   await agent(buildPrompt(b))";
        let err = parse_workflow_js(src).expect_err("all-dynamic errors");
        let msg = err.to_string();
        assert!(
            msg.contains('2') && msg.contains("dynamic"),
            "error names the count: {msg}"
        );
    }

    #[test]
    fn aleph_exported_json_schema_reimports_on_bare_path() {
        // Aleph renders schema inline as compact JSON. Strip the embed header and
        // prove the bounded data parser reads Aleph's own JSON output back on the
        // bare path (JSON is a subset of the accepted grammar) — no regression.
        let m = WorkflowManifest {
            name: "js".into(),
            description: String::new(),
            when_to_use: String::new(),
            phases: vec![],
            steps: vec![WorkflowManifestStep {
                id: "a".into(),
                agent: "agent".into(),
                prompt: "go".into(),
                depends_on: vec![],
                label: None,
                model: None,
                phase: None,
                schema: Some(
                    serde_json::json!({"type": "object", "properties": {"n": {"type": "integer"}}}),
                ),
                isolation: None,
                agent_type: None,
                effort: None,
                kind: crate::workflow::def::WorkflowStepKind::Agent,
                choices: vec![],
                review: false,
                require_grounding: false,
                tolerate_failed_deps: false,
                timeout_secs: None,
                max_retries: None,
            }],
        };
        let js = render_workflow_js(&m);
        let bare: String = js.lines().skip(1).collect::<Vec<_>>().join("\n");
        let outcome = parse_workflow_js(&bare).expect("bare scan of exported schema");
        assert_eq!(
            outcome.manifest.steps[0].schema, m.steps[0].schema,
            "JSON schema round-trips on the bare path"
        );
    }

    #[test]
    fn string_valued_schema_survives_bare_roundtrip() {
        // `schema` is Option<Value>; a string-valued schema renders as
        // `schema: "…"` and must survive a header-stripped re-import via the
        // string arm rather than vanishing (the residual P7 hole, now closed).
        let m = WorkflowManifest {
            name: "ss".into(),
            description: String::new(),
            when_to_use: String::new(),
            phases: vec![],
            steps: vec![WorkflowManifestStep {
                id: "a".into(),
                agent: "agent".into(),
                prompt: "go".into(),
                depends_on: vec![],
                label: None,
                model: None,
                phase: None,
                schema: Some(serde_json::Value::String("opaque-ref".into())),
                isolation: None,
                agent_type: None,
                effort: None,
                kind: crate::workflow::def::WorkflowStepKind::Agent,
                choices: vec![],
                review: false,
                require_grounding: false,
                tolerate_failed_deps: false,
                timeout_secs: None,
                max_retries: None,
            }],
        };
        let js = render_workflow_js(&m);
        let bare: String = js.lines().skip(1).collect::<Vec<_>>().join("\n");
        let outcome = parse_workflow_js(&bare).expect("bare scan");
        assert_eq!(
            outcome.manifest.steps[0].schema,
            Some(serde_json::Value::String("opaque-ref".into())),
            "string-valued schema survives the bare path"
        );
    }

    #[test]
    fn agent_opts_brace_in_string_value_does_not_close_early() {
        // A `}` inside a string opt value must not end the opts object early —
        // the brace/string tracking keeps the following key readable.
        let src = "export const meta = { name: 'br' }\n\
                   await agent('x', { label: \"a } b\", model: \"haiku\" })";
        let outcome = parse_workflow_js(src).expect("scan");
        let s = &outcome.manifest.steps[0];
        assert_eq!(s.label.as_deref(), Some("a } b"));
        assert_eq!(s.model.as_deref(), Some("haiku"));
    }

    /// A manifest step with a literal prompt and named dependencies — for the
    /// structural round-trip tests below.
    fn mstep(id: &str, deps: &[&str]) -> WorkflowManifestStep {
        WorkflowManifestStep {
            id: id.into(),
            agent: "agent".into(),
            prompt: format!("do {id}"),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            label: None,
            model: None,
            phase: None,
            schema: None,
            isolation: None,
            agent_type: None,
            effort: None,
            kind: crate::workflow::def::WorkflowStepKind::Agent,
            choices: vec![],
            review: false,
            require_grounding: false,
            tolerate_failed_deps: false,
            timeout_secs: None,
            max_retries: None,
        }
    }

    #[test]
    fn bare_js_parallel_block_reconstructs_siblings() {
        // A hand-written `parallel([...])` block re-imports as sibling steps:
        // both fan out from the prior step and the next step fans in from both.
        // The parallelisation structure is recovered, NOT linearised.
        let src = "export const meta = { name: 'par' }\n\
                   await agent('root')\n\
                   await parallel([\n\
                     () => agent('left'),\n\
                     () => agent('right'),\n\
                   ])\n\
                   await agent('merge')";
        let outcome = parse_workflow_js(src).expect("scan parallel");
        let m = &outcome.manifest;
        assert_eq!(m.steps.len(), 4);
        assert!(m.steps[0].depends_on.is_empty(), "root has no deps");
        assert_eq!(
            m.steps[1].depends_on,
            vec!["step_1".to_string()],
            "left fans out from root"
        );
        assert_eq!(
            m.steps[2].depends_on,
            vec!["step_1".to_string()],
            "right fans out from root"
        );
        assert_eq!(
            m.steps[3].depends_on,
            vec!["step_2".to_string(), "step_3".to_string()],
            "merge fans in from BOTH siblings"
        );
        // No longer reported as a dropped sequential approximation.
        assert!(
            !outcome.dropped.iter().any(|d| d.contains("parallel")),
            "parallel reconstructed, not dropped: {:?}",
            outcome.dropped
        );
    }

    #[test]
    fn diamond_structure_survives_header_stripped_export_roundtrip() {
        // A diamond (a → {b,c} → d) exports as agent / parallel([b,c]) / agent.
        // Stripping the embed header forces the bare scan, which must rebuild the
        // fan-out + fan-in edges — export and import are now symmetric on the DAG
        // *shape*, not just the prompts. Before this, the bare path collapsed the
        // diamond into a 4-step linear chain, silently serialising the workflow.
        let m = WorkflowManifest {
            name: "dia".into(),
            description: String::new(),
            when_to_use: String::new(),
            phases: vec![],
            steps: vec![
                mstep("a", &[]),
                mstep("b", &["a"]),
                mstep("c", &["a"]),
                mstep("d", &["b", "c"]),
            ],
        };
        let js = render_workflow_js(&m);
        let bare: String = js.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(!bare.contains("@aleph-workflow"), "header stripped: {bare}");
        let back = parse_workflow_js(&bare)
            .expect("bare scan diamond")
            .manifest;
        assert_eq!(back.steps.len(), 4);
        // Renumbered ids step_1..4 follow source order a, b, c, d.
        assert!(back.steps[0].depends_on.is_empty(), "a is the root");
        assert_eq!(back.steps[1].depends_on, vec!["step_1".to_string()], "b←a");
        assert_eq!(back.steps[2].depends_on, vec!["step_1".to_string()], "c←a");
        assert_eq!(
            back.steps[3].depends_on,
            vec!["step_2".to_string(), "step_3".to_string()],
            "d fans in from both b and c"
        );
    }

    #[test]
    fn parallel_block_with_agent_opts_keeps_both_structure_and_opts() {
        // The two recoveries compose: agents inside a parallel block keep their
        // opts (here a per-agent phase) AND become siblings.
        let src = "export const meta = { name: 'po' }\n\
                   await parallel([\n\
                     () => agent('x', { phase: \"Review\", label: \"a\" }),\n\
                     () => agent('y', { phase: \"Review\", label: \"b\" }),\n\
                   ])";
        let m = parse_workflow_js(src).expect("scan").manifest;
        assert_eq!(m.steps.len(), 2);
        assert!(
            m.steps[0].depends_on.is_empty(),
            "both are roots in the layer"
        );
        assert!(m.steps[1].depends_on.is_empty());
        assert_eq!(m.steps[0].label.as_deref(), Some("a"));
        assert_eq!(m.steps[1].label.as_deref(), Some("b"));
        assert_eq!(m.steps[0].phase.as_deref(), Some("Review"));
        assert_eq!(
            m.phases
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Review"]
        );
    }

    // ---- Clarify steps on the bare path -----------------------------------

    #[test]
    fn bare_js_imports_clarify_with_choices() {
        // A hand-written `clarify("q", ["a", "b"])` re-imports as a Clarify step
        // carrying its choices — the inverse of export's render_clarify_call.
        let src = "export const meta = { name: 'deploy' }\n\
                   await clarify('Deploy where?', ['staging', 'prod'])\n\
                   await agent('deploy it')";
        let outcome = parse_workflow_js(src).expect("scan clarify");
        let m = &outcome.manifest;
        assert_eq!(m.steps.len(), 2);
        assert_eq!(
            m.steps[0].kind,
            crate::workflow::def::WorkflowStepKind::Clarify
        );
        assert_eq!(m.steps[0].prompt, "Deploy where?");
        assert_eq!(m.steps[0].choices, vec!["staging", "prod"]);
        assert!(m.steps[0].agent.is_empty(), "clarify owns no team member");
        // The agent step fans in from the clarify step (DAG wiring shared).
        assert_eq!(m.steps[1].depends_on, vec!["step_1".to_string()]);
        // clarify is no longer reported as a dropped construct.
        assert!(
            !outcome.dropped.iter().any(|d| d.contains("clarify")),
            "clarify captured, not dropped: {:?}",
            outcome.dropped
        );
    }

    #[test]
    fn bare_js_imports_free_text_clarify() {
        // A `clarify("q")` with no choices array → a free-text Clarify step.
        let src = "export const meta = { name: 'wf' }\n\
                   await clarify('Which file?')";
        let m = parse_workflow_js(src).expect("scan").manifest;
        assert_eq!(m.steps.len(), 1);
        assert_eq!(
            m.steps[0].kind,
            crate::workflow::def::WorkflowStepKind::Clarify
        );
        assert_eq!(m.steps[0].prompt, "Which file?");
        assert!(m.steps[0].choices.is_empty(), "free-text → no choices");
    }

    #[test]
    fn clarify_with_dynamic_choices_abstains() {
        // A non-literal choices element makes the menu dynamic; the read abstains
        // (empty choices) rather than half-capturing it (R7/R10). The step is
        // still recovered as a free-text clarify.
        let src = "export const meta = { name: 'wf' }\n\
                   await clarify('Pick', [ENVS, 'prod'])";
        let m = parse_workflow_js(src).expect("scan").manifest;
        assert_eq!(m.steps.len(), 1);
        assert!(
            m.steps[0].choices.is_empty(),
            "dynamic menu abstains, not guessed: {:?}",
            m.steps[0].choices
        );
    }

    #[test]
    fn clarify_in_prompt_text_is_not_a_call() {
        // A prompt that merely mentions `clarify(` must NOT register a phantom
        // clarify step — the scan is string-aware.
        let src = "export const meta = { name: 'wf' }\n\
                   await agent('please clarify(the spec) before coding')";
        let m = parse_workflow_js(src).expect("scan").manifest;
        assert_eq!(m.steps.len(), 1);
        assert_eq!(
            m.steps[0].kind,
            crate::workflow::def::WorkflowStepKind::Agent,
            "string-mention of clarify( is not a call"
        );
    }

    #[test]
    fn clarify_workflow_survives_header_stripped_export_roundtrip() {
        // The headline symmetry: export a workflow whose first step is a clarify
        // gate, drop the lossless `@aleph-workflow` embed header, and prove the
        // bare scanner rebuilds the clarify kind + choices + the dependency edge.
        // Before this, the bare path silently dropped the clarify step, so a
        // header-stripped export re-imported as a different (gate-less) workflow.
        let m = WorkflowManifest {
            name: "deploy".into(),
            description: String::new(),
            when_to_use: String::new(),
            phases: vec![],
            steps: vec![
                WorkflowManifestStep {
                    id: "ask".into(),
                    agent: String::new(),
                    prompt: "Deploy where?".into(),
                    depends_on: vec![],
                    label: None,
                    model: None,
                    phase: None,
                    schema: None,
                    isolation: None,
                    agent_type: None,
                    effort: None,
                    kind: crate::workflow::def::WorkflowStepKind::Clarify,
                    choices: vec!["staging".into(), "prod".into()],
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_secs: None,
                    max_retries: None,
                },
                WorkflowManifestStep {
                    id: "run".into(),
                    agent: "deployer".into(),
                    prompt: "deploy".into(),
                    depends_on: vec!["ask".into()],
                    label: None,
                    model: None,
                    phase: None,
                    schema: None,
                    isolation: None,
                    agent_type: None,
                    effort: None,
                    kind: crate::workflow::def::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_secs: None,
                    max_retries: None,
                },
            ],
        };
        let js = render_workflow_js(&m);
        let bare: String = js.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(!bare.contains("@aleph-workflow"), "header stripped: {bare}");
        let back = parse_workflow_js(&bare)
            .expect("bare scan of clarify export")
            .manifest;
        assert_eq!(back.steps.len(), 2);
        assert_eq!(
            back.steps[0].kind,
            crate::workflow::def::WorkflowStepKind::Clarify,
            "clarify kind survives the bare round-trip"
        );
        assert_eq!(back.steps[0].prompt, "Deploy where?");
        assert_eq!(back.steps[0].choices, vec!["staging", "prod"]);
        assert_eq!(
            back.steps[1].depends_on,
            vec!["step_1".to_string()],
            "the gated agent step still fans in from the clarify step"
        );
    }

    // ---- F8: `meta.phases` is the authority on the phase plan ----

    #[test]
    fn bare_import_keeps_meta_phase_detail_and_model() {
        // A header-stripped export used to rebuild the phase plan from body
        // `phase()` markers alone, so `detail` and the per-phase `model` — both
        // of which `export::render_meta` writes — came back empty/None with
        // `dropped: []`, i.e. a loss reported as a lossless import.
        let src = r#"
export const meta = {
  name: 'audit',
  description: 'demo',
  phases: [
    { title: 'Analyze', detail: 'read the sources', model: 'sonnet' },
    { title: 'Report', detail: 'write it up' },
  ],
}
phase("Analyze")
await agent('read the sources')
phase("Report")
await agent('write it up')
"#;
        let out = scan_bare(src).expect("import must succeed");
        let ph = &out.manifest.phases;
        assert_eq!(ph.len(), 2, "two declared phases: {ph:?}");
        assert_eq!(ph[0].title, "Analyze");
        assert_eq!(ph[0].detail, "read the sources", "detail survives");
        assert_eq!(
            ph[0].model.as_deref(),
            Some("sonnet"),
            "per-phase model survives"
        );
        assert_eq!(ph[1].title, "Report");
        assert_eq!(ph[1].detail, "write it up");
        assert_eq!(ph[1].model, None, "a phase without a model gets none");
    }

    #[test]
    fn meta_phases_round_trip_through_a_header_stripped_export() {
        // End to end over the two faces: render, drop the header line, re-scan.
        use crate::workflow::interop::export::render_workflow_js;
        let m = WorkflowManifest {
            name: "wf".into(),
            description: "d".into(),
            when_to_use: "always".into(),
            phases: vec![WorkflowPhase {
                title: "Analyze".into(),
                detail: "look hard".into(),
                model: Some("opus".into()),
            }],
            steps: vec![WorkflowManifestStep {
                id: "s1".into(),
                agent: "ag".into(),
                prompt: "do it".into(),
                depends_on: vec![],
                label: None,
                model: None,
                phase: Some("Analyze".into()),
                schema: None,
                isolation: None,
                agent_type: None,
                effort: None,
                kind: crate::workflow::def::WorkflowStepKind::Agent,
                choices: vec![],
                review: false,
                require_grounding: false,
                tolerate_failed_deps: false,
                timeout_secs: None,
                max_retries: None,
            }],
        };
        let js = render_workflow_js(&m);
        let bare: String = js.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(!bare.contains("@aleph-workflow"), "header stripped");
        let back = parse_workflow_js(&bare).expect("bare scan").manifest;
        assert_eq!(back.phases.len(), 1);
        assert_eq!(back.phases[0].detail, "look hard");
        assert_eq!(back.phases[0].model.as_deref(), Some("opus"));
    }

    #[test]
    fn a_body_marker_meta_does_not_declare_is_appended() {
        // `meta` is the authority, not a replacement: a hand-written file whose
        // body marks a phase `meta.phases` never lists still keeps it.
        let src = r#"
export const meta = {
  name: 'audit',
  phases: [ { title: 'Analyze', detail: 'read' } ],
}
phase("Analyze")
await agent('read the sources')
phase("Ship")
await agent('ship it')
"#;
        let out = scan_bare(src).expect("import must succeed");
        let titles: Vec<&str> = out
            .manifest
            .phases
            .iter()
            .map(|p| p.title.as_str())
            .collect();
        assert_eq!(
            titles,
            vec!["Analyze", "Ship"],
            "declared first, then marker"
        );
        assert_eq!(out.manifest.phases[0].detail, "read");
        assert_eq!(out.manifest.phases[1].detail, "", "a marker has no detail");
    }

    #[test]
    fn a_file_without_meta_phases_still_builds_the_plan_from_markers() {
        // The pre-existing behaviour, unchanged when `meta` declares nothing.
        let src = r#"
export const meta = { name: 'audit' }
phase("Analyze")
await agent('read the sources')
"#;
        let out = scan_bare(src).expect("import must succeed");
        assert_eq!(out.manifest.phases.len(), 1);
        assert_eq!(out.manifest.phases[0].title, "Analyze");
    }

    // ---- F9: array fan-out is disclosed ----

    #[test]
    fn array_fan_out_with_a_literal_prompt_is_reported_dropped() {
        // `.forEach(` is deliberately rejected by `contains_call_like_keyword`
        // (the leading-boundary rule), and the dynamic-prompt counter only sees
        // NON-literal prompts. A literal prompt inside the closure therefore
        // imported as ONE step with `dropped: []` — a lossless-looking import of
        // a collapsed N-way fan-out.
        let src = r#"
export const meta = { name: 'audit' }
TARGETS.forEach(() => agent("audit this target"))
"#;
        let out = scan_bare(src).expect("import must succeed");
        assert!(
            out.dropped.iter().any(|d| d.contains("array fan-out")),
            "fan-out must be disclosed: {:?}",
            out.dropped
        );
    }

    #[test]
    fn map_fan_out_is_reported_dropped() {
        let src = r#"
export const meta = { name: 'audit' }
const rs = TARGETS.map(() => agent("audit this target"))
"#;
        let out = scan_bare(src).expect("import must succeed");
        assert!(
            out.dropped.iter().any(|d| d.contains("array fan-out")),
            "{:?}",
            out.dropped
        );
    }

    #[test]
    fn a_script_without_fan_out_gets_no_fan_out_note() {
        let src = r#"
export const meta = { name: 'audit' }
await agent('read the sources')
await agent('write the brief')
"#;
        let out = scan_bare(src).expect("import must succeed");
        assert!(
            !out.dropped.iter().any(|d| d.contains("array fan-out")),
            "no fan-out in this file: {:?}",
            out.dropped
        );
    }

    #[test]
    fn a_prompt_mentioning_map_does_not_trip_the_fan_out_note() {
        // String bodies are blanked before the needle check, so prose wins no
        // false positive (the same rule the control-flow keywords follow).
        let src = r#"
export const meta = { name: 'audit' }
await agent('draw the sitemap and .map( the routes')
"#;
        let out = scan_bare(src).expect("import must succeed");
        assert!(
            !out.dropped.iter().any(|d| d.contains("array fan-out")),
            "{:?}",
            out.dropped
        );
    }
}
