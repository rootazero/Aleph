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
//!    `label`/`phase`/`model`/`isolation`/`agentType` fields and a JSON `schema`
//!    are recovered, making the bare path symmetric with `export`'s
//!    `render_agent_call` (a header-stripped export re-imports its opts intact).
//!    A `parallel([...])` block is reconstructed into sibling steps of one DAG
//!    layer (fan-out from the prior step, fan-in to the next), so the
//!    parallelisation / orchestrator-workers shape round-trips instead of being
//!    flattened into a sequential chain.
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
    // layer before it and fan into the step after it — the exact inverse of
    // `export`'s topo-layers → `parallel(...)` rendering. A sequential step is a
    // singleton layer (a plain chain). This recovers the parallelisation /
    // orchestrator-workers DAG shape a flat scan would otherwise linearise, so
    // re-imported siblings stay independent `Pending` tasks the dispatcher runs
    // concurrently.
    let mut prev_layer: Vec<usize> = Vec::new();
    let mut parallel_depth: u32 = 0;
    let mut parallel_group: Vec<usize> = Vec::new();
    for ev in scan_events(src) {
        match ev {
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
                    kind: crate::workflow::def::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: call.opts.review,
                    timeout_secs: call.opts.timeout_secs,
                    max_retries: call.opts.max_retries,
                });
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
        return Err(AlephError::invalid_input(
            "no agent() calls found in .workflow.js; nothing to import",
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
    let chars: Vec<char> = s.chars().collect();
    read_first_string_literal_chars(&chars)
}

/// Char-slice core of [`read_first_string_literal`]: the leading token of
/// `chars` must be a quote, else `None` (no over-reach to a later literal).
fn read_first_string_literal_chars(chars: &[char]) -> Option<String> {
    let i = first_non_ws(chars, 0);
    match chars.get(i).copied() {
        Some('\'' | '"') => read_literal_at(chars, i).map(|(lit, _)| lit),
        _ => None,
    }
}

/// Index of the first non-whitespace char at or after `start` (clamped to len).
fn first_non_ws(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// Read the quoted string literal beginning at `chars[start]` (which must be a
/// quote). Returns the decoded content and the index just past the closing
/// quote. Standard JS escapes are interpreted (`\n`/`\t`/`\r`/`\0` → the control
/// char; `\"`/`\'`/`\\`/`\/` and any other escape → the char verbatim) so a
/// `.join("\n")` separator decodes to a real newline — the round-trip inverse of
/// `export`'s `js_str`. UTF-8 safe (operates on `char`s).
fn read_literal_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let quote = *chars.get(start)?;
    let mut i = start + 1;
    let mut out = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let esc = *chars.get(i + 1)?;
            out.push(match esc {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                other => other,
            });
            i += 2;
            continue;
        }
        if c == quote {
            return Some((out, i + 1));
        }
        out.push(c);
        i += 1;
    }
    None
}

/// Read the prompt argument of an `agent(` call from `s` (the source *after* the
/// open paren). Handles the two declarative prompt shapes of the engineering
/// format:
///   * `agent("prompt", …)`              → the single string literal;
///   * `agent([ "a", "b" ].join("\n"), …)` → the array elements joined by the
///     `.join` separator (the format's signature multi-line idiom).
///
/// Returns `None` for a non-literal argument (`agent(promptVar)`, a `.map(...)`
/// expression, or an array with a non-literal element) — those are dynamic and
/// intentionally not statically importable (R7/R10); they surface elsewhere as
/// `dropped` constructs rather than being half-captured.
///
/// On success the second tuple element is the char index just past the prompt
/// argument, so the caller can continue into the `, { opts }` object.
fn read_agent_prompt(chars: &[char], start: usize) -> Option<(String, usize)> {
    let i = first_non_ws(chars, start);
    match *chars.get(i)? {
        '\'' | '"' => read_literal_at(chars, i),
        '[' => read_joined_array(chars, i),
        _ => None,
    }
}

/// Parse `[ "a", "b", … ].join("sep")` starting at `chars[start] == '['`.
/// Every element must be a plain string literal; a non-literal element or an
/// element-level concatenation (`'a' + x`) makes the joined value not statically
/// known, so the whole read abstains (returns `None`). The separator defaults to
/// `"\n"` (the format's convention) when no explicit `.join(...)` follows.
///
/// The second tuple element is the index just past the array (and its `.join`,
/// if any), so the caller can resume scanning the agent opts.
fn read_joined_array(chars: &[char], start: usize) -> Option<(String, usize)> {
    let n = chars.len();
    let mut i = start + 1; // past '['
    let mut parts: Vec<String> = Vec::new();
    loop {
        while i < n && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        match *chars.get(i)? {
            ']' => {
                i += 1;
                break;
            }
            '\'' | '"' => {
                let (lit, next) = read_literal_at(chars, i)?;
                // `'a' + x` concatenation → joined string not statically known.
                if chars.get(first_non_ws(chars, next)) == Some(&'+') {
                    return None;
                }
                parts.push(lit);
                i = next;
            }
            // Identifier / expression element → dynamic array, abstain.
            _ => return None,
        }
    }
    // `i` now points just past `]`. An optional `.join("sep")` follows; absent
    // it, the convention separator is `"\n"` and the array ends at `]`.
    let (sep, end) = parse_join_separator(chars, i).unwrap_or_else(|| ("\n".to_string(), i));
    Some((parts.join(&sep), end))
}

/// After a `]`, read an optional `.join("sep")` and return the separator literal
/// plus the index just past the closing `)`. `None` if no well-formed
/// `.join(<string literal>)` follows.
fn parse_join_separator(chars: &[char], start: usize) -> Option<(String, usize)> {
    let i = first_non_ws(chars, start);
    // Match the `.join` identifier exactly.
    let dotjoin = ['.', 'j', 'o', 'i', 'n'];
    if (0..dotjoin.len()).any(|k| chars.get(i + k) != Some(&dotjoin[k])) {
        return None;
    }
    let i = first_non_ws(chars, i + dotjoin.len());
    if chars.get(i) != Some(&'(') {
        return None;
    }
    let i = first_non_ws(chars, i + 1);
    let (sep, next) = match *chars.get(i)? {
        '\'' | '"' => read_literal_at(chars, i)?,
        _ => return None,
    };
    // Consume the closing `)` so the returned end index is past the whole
    // `.join(...)` call, not stranded mid-expression.
    let j = first_non_ws(chars, next);
    if chars.get(j) != Some(&')') {
        return None;
    }
    Some((sep, j + 1))
}

/// Read an optional `, ["a", "b"]` choices array that follows a clarify
/// question. `start` is the index just past the prompt argument. Returns the
/// decoded choice literals, or an empty vec when no array follows (a free-text
/// clarification) — the inverse of `export`'s `render_clarify_call`. A
/// non-literal element makes the menu dynamic, so the whole read abstains
/// (returns empty) rather than half-capturing it (R7/R10), exactly like the
/// prompt readers.
fn read_clarify_choices(chars: &[char], start: usize) -> Vec<String> {
    let i = first_non_ws(chars, start);
    if chars.get(i) != Some(&',') {
        return Vec::new();
    }
    let i = first_non_ws(chars, i + 1);
    if chars.get(i) != Some(&'[') {
        return Vec::new();
    }
    let n = chars.len();
    let mut j = i + 1; // past '['
    let mut out: Vec<String> = Vec::new();
    loop {
        while j < n && (chars[j].is_whitespace() || chars[j] == ',') {
            j += 1;
        }
        match chars.get(j) {
            Some(']') | None => break,
            Some('\'' | '"') => match read_literal_at(chars, j) {
                Some((lit, next)) => {
                    out.push(lit);
                    j = next;
                }
                None => break,
            },
            // Identifier / expression element → dynamic menu, abstain entirely.
            _ => {
                out.clear();
                return out;
            }
        }
    }
    out
}

/// Literal opts recovered from an `agent(prompt, { … })` call. Every field is
/// optional; a key whose value is not a static literal (an identifier, a
/// computed expression, a non-JSON schema) is left unset rather than guessed
/// (R7/R10), exactly mirroring the prompt readers' abstain-on-dynamic policy.
#[derive(Default)]
struct AgentOpts {
    label: Option<String>,
    phase: Option<String>,
    model: Option<String>,
    schema: Option<serde_json::Value>,
    isolation: Option<String>,
    agent_type: Option<String>,
    /// Reasoning-effort tier (`effort: "high"`). Interchange-only, recovered on
    /// the bare path so a header-stripped `.workflow.js` round-trips its effort
    /// hint instead of silently losing it (was dropped as an unknown key before).
    effort: Option<String>,
    /// Lead-review gate (`review: true`). Recovered on the bare path so a
    /// header-stripped round-trip cannot silently drop an oversight gate
    /// (a step meant to park in WaitingReview would auto-complete).
    review: bool,
    /// Per-step timeout — parsed from `timeoutSecs: <n>`.
    timeout_secs: Option<u64>,
    /// Per-step retry ceiling — parsed from `maxRetries: <n>`.
    max_retries: Option<u32>,
}

/// Read the optional `, { label: "…", phase: "…", model: "…", schema: {…},
/// isolation: "…", agentType: "…" }` opts object that follows an agent prompt.
///
/// `start` is the index just past the prompt argument. Returns defaults when no
/// `, {` opts object follows. String-valued keys decode via [`read_literal_at`];
/// `schema` is captured as raw `{…}` text and JSON-parsed (a non-JSON object is
/// skipped, leaving `schema` unset). Unknown keys and non-literal values are
/// skipped without aborting the rest of the object — the inverse of `export`'s
/// `render_agent_call`, so a header-stripped export round-trips its opts.
fn read_agent_opts(chars: &[char], start: usize) -> AgentOpts {
    let mut opts = AgentOpts::default();
    let mut i = first_non_ws(chars, start);
    if chars.get(i) != Some(&',') {
        return opts;
    }
    i = first_non_ws(chars, i + 1);
    if chars.get(i) != Some(&'{') {
        return opts;
    }
    i += 1; // past '{'
    let n = chars.len();
    loop {
        i = first_non_ws(chars, i);
        match chars.get(i) {
            None | Some('}') => break,
            Some(',') => {
                i += 1;
                continue;
            }
            _ => {}
        }
        // Read a bare identifier key (label / phase / model / schema / …).
        let key_start = i;
        while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        if i == key_start {
            // Not an identifier where a key was expected — give up on the rest
            // of the object rather than spin.
            break;
        }
        let key: String = chars[key_start..i].iter().collect();
        i = first_non_ws(chars, i);
        if chars.get(i) != Some(&':') {
            break;
        }
        i = first_non_ws(chars, i + 1);
        match chars.get(i) {
            Some('\'' | '"') => match read_literal_at(chars, i) {
                Some((lit, next)) => {
                    assign_string_opt(&mut opts, &key, lit);
                    i = next;
                }
                None => break,
            },
            Some('{') => match read_brace_object(chars, i) {
                Some((raw, next)) => {
                    if key == "schema" {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                            opts.schema = Some(v);
                        }
                    }
                    i = next;
                }
                None => break,
            },
            // Bare (non-string, non-object) value: capture the raw token so
            // the executable-core opts export emits as bare literals —
            // `review: true`, `timeoutSecs: 1800`, `maxRetries: 0` — round-trip
            // instead of being silently dropped. Anything unrecognised is
            // still skipped, not guessed.
            _ => {
                let end = skip_value(chars, i);
                let raw: String = chars[i..end.min(chars.len())].iter().collect();
                assign_bare_opt(&mut opts, &key, raw.trim());
                i = end;
            }
        }
    }
    opts
}

/// Assign a bare-literal opt value (boolean / number) by `.workflow.js` key.
/// Unparseable values leave the field unset — never guessed.
fn assign_bare_opt(opts: &mut AgentOpts, key: &str, raw: &str) {
    match key {
        "review" => {
            if raw == "true" {
                opts.review = true;
            }
        }
        "timeoutSecs" => opts.timeout_secs = raw.parse::<u64>().ok().filter(|t| *t > 0),
        "maxRetries" => opts.max_retries = raw.parse::<u32>().ok(),
        _ => {}
    }
}

/// Assign a decoded string literal to its opts field by `.workflow.js` key name.
/// Unknown keys are ignored (forward-compatible with format additions).
fn assign_string_opt(opts: &mut AgentOpts, key: &str, val: String) {
    match key {
        "label" => opts.label = Some(val),
        "phase" => opts.phase = Some(val),
        "model" => opts.model = Some(val),
        "isolation" => opts.isolation = Some(val),
        "agentType" => opts.agent_type = Some(val),
        "effort" => opts.effort = Some(val),
        _ => {}
    }
}

/// Read a balanced `{ … }` object starting at `chars[start] == '{'`, returning
/// its raw source text (braces included) and the index just past the close.
/// Brace depth is tracked string-aware so a `}` inside a nested string never
/// closes early. `None` if the object is unterminated.
fn read_brace_object(chars: &[char], start: usize) -> Option<(String, usize)> {
    let n = chars.len();
    let mut i = start;
    let mut depth: i32 = 0;
    let mut raw = String::new();
    while i < n {
        let c = chars[i];
        raw.push(c);
        match c {
            '\'' | '"' | '`' => {
                i += 1;
                while i < n {
                    let d = chars[i];
                    raw.push(d);
                    if d == '\\' {
                        if let Some(&e) = chars.get(i + 1) {
                            raw.push(e);
                        }
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if d == c {
                        break;
                    }
                }
            }
            '{' => {
                depth += 1;
                i += 1;
            }
            '}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some((raw, i));
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// Skip a value at the current opts cursor, stopping at the next top-level `,` or
/// the object's closing `}`. Nested `{}`/`[]`/`()` and string literals are
/// skipped wholesale so an inner separator never ends the value early. Returns
/// the index of the stopping delimiter.
fn skip_value(chars: &[char], start: usize) -> usize {
    let n = chars.len();
    let mut i = start;
    let mut depth: i32 = 0;
    while i < n {
        let c = chars[i];
        match c {
            '\'' | '"' | '`' => {
                i += 1;
                while i < n {
                    let d = chars[i];
                    if d == '\\' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if d == c {
                        break;
                    }
                }
            }
            '{' | '[' | '(' => {
                depth += 1;
                i += 1;
            }
            '}' | ']' | ')' => {
                if depth == 0 {
                    return i; // the opts object's closing brace
                }
                depth -= 1;
                i += 1;
            }
            ',' if depth == 0 => return i,
            _ => i += 1,
        }
    }
    i
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
    /// An `agent("prompt", { opts })` call — the prompt plus any recovered opts.
    Agent(AgentCall),
    /// A `clarify("question", ["a", "b"])` call (an Aleph extension to the
    /// `.workflow.js` vocabulary) — the question plus any literal choices.
    Clarify(ClarifyCall),
    /// The opening of a `parallel([...])` block — the agents up to the matching
    /// [`ParallelEnd`](ScanEvent::ParallelEnd) are siblings (same DAG layer).
    ParallelStart,
    /// The close of the innermost open `parallel([...])` block.
    ParallelEnd,
}

/// A recovered `agent()` call: its prompt and the literal opts that followed.
struct AgentCall {
    prompt: String,
    opts: AgentOpts,
}

/// A recovered `clarify()` call: its question and the literal choice menu
/// (empty for a free-text clarification — the inverse of `render_clarify_call`).
struct ClarifyCall {
    prompt: String,
    choices: Vec<String>,
}

/// Scan `src` for `phase(...)` and `agent(...)` calls in source order, string-
/// aware so a prompt mentioning `phase(`/`agent(` never registers as a call.
///
/// A single forward pass tokenises identifiers and skips string-literal bodies
/// wholesale; only a bare `phase`/`agent`/`parallel` identifier immediately
/// followed by `(` counts, so `subagent(` / `useragent(` and the like never
/// over-match. Calls whose first argument is not a string literal (e.g.
/// `agent(promptVar)`) yield no event. Catches both top-level calls and
/// `() => agent(` inside `parallel([...])`.
///
/// Parenthesis depth is tracked (string contents excluded) so a `parallel(`
/// block's matching `)` is found: the agents between the emitted `ParallelStart`
/// and `ParallelEnd` are siblings of one DAG layer.
fn scan_events(src: &str) -> Vec<ScanEvent> {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut events = Vec::new();
    let mut i = 0;
    // Depth of `(` nesting in code (not strings). `parallel_watch` records the
    // depth at each open `parallel(` so its close emits `ParallelEnd`.
    let mut paren_depth: i32 = 0;
    let mut parallel_watch: Vec<i32> = Vec::new();
    while i < n {
        let c = chars[i];
        // Skip string-literal bodies so their contents are never tokenised
        // (parens inside a prompt must not perturb the depth count).
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
            if ident == "phase" || ident == "agent" || ident == "parallel" || ident == "clarify" {
                let mut j = i;
                while j < n && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < n && chars[j] == '(' {
                    // The call's argument list, as a char slice (paren consumed).
                    let after = &chars[j + 1..];
                    match ident.as_str() {
                        // A phase title is always a plain literal.
                        "phase" => {
                            if let Some(lit) = read_first_string_literal_chars(after) {
                                events.push(ScanEvent::Phase(lit));
                            }
                        }
                        // An agent prompt may be a literal or a `[...].join()`
                        // array; the trailing `{ opts }` object is parsed from
                        // where the prompt ends.
                        "agent" => {
                            if let Some((prompt, end)) = read_agent_prompt(after, 0) {
                                let opts = read_agent_opts(after, end);
                                events.push(ScanEvent::Agent(AgentCall { prompt, opts }));
                            }
                        }
                        // A clarify question is a plain/`join`-array literal (the
                        // `read_agent_prompt` shape), optionally followed by a
                        // `["a", "b"]` choices array — the inverse of export's
                        // `render_clarify_call`.
                        "clarify" => {
                            if let Some((prompt, end)) = read_agent_prompt(after, 0) {
                                let choices = read_clarify_choices(after, end);
                                events.push(ScanEvent::Clarify(ClarifyCall { prompt, choices }));
                            }
                        }
                        // The block opens at the `(` the main loop is about to
                        // count; watch for the `)` that returns to this depth.
                        "parallel" => {
                            parallel_watch.push(paren_depth);
                            events.push(ScanEvent::ParallelStart);
                        }
                        _ => {}
                    }
                }
            }
            continue;
        }
        // Track code paren depth so `parallel(`'s close can be located. An
        // `agent(...)` / `() =>` call's own parens balance out at a deeper level
        // and never spuriously close the watched block.
        if c == '(' {
            paren_depth += 1;
            i += 1;
            continue;
        }
        if c == ')' {
            paren_depth -= 1;
            if parallel_watch.last() == Some(&paren_depth) {
                parallel_watch.pop();
                events.push(ScanEvent::ParallelEnd);
            }
            i += 1;
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
                    effort: Some("high".into()),
                    kind: crate::workflow::def::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
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
}
