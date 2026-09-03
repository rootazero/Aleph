//! The `agent(prompt, { … })` opts object: what the bare scan can recover from
//! it, and what it says out loud when it cannot.
//!
//! `export::render_agent_call` writes this object; [`read_agent_opts`] reads it
//! back. The reader is not a JS parser, so two failures are structural rather
//! than incidental — a `schema:` it cannot resolve, and a tail it cannot finish
//! (a spread, a computed key). Both are carried out in [`AgentOpts`] as notes
//! rather than silently absent fields, because the keys that live past a stop
//! (`review`, `requireGrounding`) default to OFF: a silent stop downgrades a
//! step authored to park for lead review into one that auto-completes (P7).

use crate::workflow::interop::consts::{parse_js_data, ConstTable};

use super::lexer::{first_non_ws, read_literal_at, skip_value};

/// Literal opts recovered from an `agent(prompt, { … })` call. Every field is
/// optional; a key whose value is not a static literal (an identifier, a
/// computed expression, a non-JSON schema) is left unset rather than guessed
/// (R7/R10), exactly mirroring the prompt readers' abstain-on-dynamic policy.
#[derive(Default)]
pub(super) struct AgentOpts {
    pub(super) label: Option<String>,
    pub(super) phase: Option<String>,
    pub(super) model: Option<String>,
    pub(super) schema: Option<serde_json::Value>,
    pub(super) isolation: Option<String>,
    pub(super) agent_type: Option<String>,
    /// Reasoning-effort tier (`effort: "high"`). Interchange-only, recovered on
    /// the bare path so a header-stripped `.workflow.js` round-trips its effort
    /// hint instead of silently losing it (was dropped as an unknown key before).
    pub(super) effort: Option<String>,
    /// Lead-review gate (`review: true`). Recovered on the bare path so a
    /// header-stripped round-trip cannot silently drop an oversight gate
    /// (a step meant to park in WaitingReview would auto-complete).
    pub(super) review: bool,
    /// Grounding demand (`requireGrounding: true`) — the review gate's anchor
    /// requirement. Same reasoning as `review`: a round trip that drops it
    /// silently downgrades a gate that must touch reality into one that takes
    /// the model's word.
    pub(super) require_grounding: bool,
    /// Tolerant fan-in (`tolerateFailedDeps: true`). Recovered on the bare path
    /// for the same reason as the two above — it changes whether a step runs at
    /// all after an upstream failure, so a header-stripped round trip that
    /// dropped it would silently turn a deliberately fault-tolerant synthesis
    /// step back into one the first failure kills.
    pub(super) tolerate_failed_deps: bool,
    /// Per-step timeout — parsed from `timeoutSecs: <n>`.
    pub(super) timeout_secs: Option<u64>,
    /// Per-step retry ceiling — parsed from `maxRetries: <n>`.
    pub(super) max_retries: Option<u32>,
    /// A `schema:` value that could not be captured — an unknown const
    /// reference or an object literal holding non-data (expression) values.
    /// Carried up so `scan_bare` can surface it in `dropped` (P7 honesty) rather
    /// than the schema vanishing silently.
    pub(super) schema_dropped: Option<String>,
    /// The opts object stopped being parseable partway through, so every key
    /// AFTER that point was abandoned.
    ///
    /// The scanner is a small hand-rolled reader, not a JS parser, so it has to
    /// stop somewhere — a spread (`{ ...BASE_OPTS, review: true }`), a computed
    /// key, a template literal. Stopping is fine; stopping SILENTLY is not, and
    /// this is the field that makes the difference. What lives past the stop is
    /// the oversight half of the format: `review` (park in `WaitingReview` for
    /// lead review) and `requireGrounding` (that review must touch reality).
    /// Both default to `false`, so an abandoned tail downgrades a gated step
    /// into an auto-completing one and `dropped` came back empty — the import
    /// reported itself lossless. Same honesty rule as `schema_dropped`; this is
    /// the case that rule did not cover.
    pub(super) opts_abandoned: Option<String>,
}

/// How much of an abandoned opts tail to quote back to the user.
const ABANDON_SNIPPET_CHARS: usize = 60;

/// Describe what stopped the opts reader and what it therefore did not read.
///
/// Naming the surviving text matters more than naming the cause: the author
/// needs to see which keys were skipped, and "everything from here" is only
/// actionable if "here" is shown.
fn abandon_note(chars: &[char], at: usize, why: &str) -> String {
    let tail: String = chars
        .get(at..)
        .unwrap_or_default()
        .iter()
        .take(ABANDON_SNIPPET_CHARS)
        .collect();
    let tail = tail.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "agent opts: stopped at `{tail}` ({why}) — every key after this point was NOT imported \
         (this includes `review` / `requireGrounding`, which default to off)"
    )
}

/// Read the optional `, { label: "…", phase: "…", model: "…", schema: {…},
/// isolation: "…", agentType: "…" }` opts object that follows an agent prompt.
///
/// `start` is the index just past the prompt argument. Returns defaults when no
/// `, {` opts object follows. String-valued keys decode via [`read_literal_at`];
/// `schema` is normalised by [`parse_js_data`] whether it is an inline literal
/// or a `schema: NAME` reference into `consts` (a hoisted top-level const), so a
/// foreign engineering file's JS-lax schema imports instead of vanishing; an
/// unresolved or non-data schema is recorded in [`AgentOpts::schema_dropped`].
/// Other unknown keys and non-literal values are skipped without aborting the
/// rest of the object — the inverse of `export`'s `render_agent_call`, so a
/// header-stripped export round-trips its opts.
pub(super) fn read_agent_opts(chars: &[char], start: usize, consts: &ConstTable) -> AgentOpts {
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
        // Read the key: a bare identifier (label / phase / model / schema / …)
        // or a quoted one. Quoted keys are ordinary JS and the interchange
        // format nowhere forbids them, but hitting one used to abandon the
        // WHOLE opts object at that point with no diagnostic — so a single
        // `{ "phase": "Ship", review: true }` silently dropped the review gate
        // (a safety control) along with everything after it.
        let key: String = if matches!(chars.get(i), Some('\'' | '"')) {
            match read_literal_at(chars, i) {
                Some((lit, next)) => {
                    i = next;
                    lit
                }
                None => {
                    opts.opts_abandoned = Some(abandon_note(chars, i, "unterminated quoted key"));
                    break;
                }
            }
        } else {
            let key_start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if i == key_start {
                // Not a key at all — give up on the rest of the object rather
                // than spin. This is the spread arm (`{ ...BASE, review: true }`)
                // and every other construct this reader is not a parser for;
                // record it, because what follows is usually the oversight half.
                opts.opts_abandoned = Some(abandon_note(chars, i, "not a key"));
                break;
            }
            chars[key_start..i].iter().collect()
        };
        i = first_non_ws(chars, i);
        if chars.get(i) != Some(&':') {
            opts.opts_abandoned = Some(abandon_note(chars, i, "missing ':' after key"));
            break;
        }
        i = first_non_ws(chars, i + 1);
        match chars.get(i) {
            Some('\'' | '"') => match read_literal_at(chars, i) {
                Some((lit, next)) => {
                    if key == "schema" {
                        // A string-valued schema is rare but `schema` is
                        // `Option<Value>`; capture it verbatim so a
                        // header-stripped export round-trips it instead of the
                        // string arm silently dropping it (P7).
                        opts.schema = Some(serde_json::Value::String(lit));
                    } else {
                        assign_string_opt(&mut opts, &key, lit);
                    }
                    i = next;
                }
                None => {
                    opts.opts_abandoned = Some(abandon_note(chars, i, "unterminated string value"));
                    break;
                }
            },
            // An object / array literal value. Only `schema` carries one; it is
            // normalised via the bounded data parser, which accepts JS-lax
            // literals (bare keys, single quotes, trailing commas) that plain
            // JSON parsing rejects — so a foreign engineering file's
            // `schema: { type: 'object', … }` imports instead of vanishing. A
            // literal holding expression values is not pure data → recorded
            // dropped, never guessed (R3/R7).
            Some('{' | '[') => {
                if key == "schema" {
                    match parse_js_data(chars, i) {
                        Some((v, next)) => {
                            opts.schema = Some(v);
                            i = next;
                        }
                        None => {
                            opts.schema_dropped = Some(
                                "inline schema literal holds non-data (expression) values — \
                                 not imported"
                                    .to_string(),
                            );
                            i = skip_value(chars, i);
                        }
                    }
                } else {
                    // No other opt is object/array-valued; skip it wholesale.
                    i = skip_value(chars, i);
                }
            }
            // Bare (non-string, non-object) value: capture the raw token. The
            // executable-core opts export emits as bare literals —
            // `review: true`, `timeoutSecs: 1800`, `maxRetries: 0` — round-trip
            // via their raw literal. A bare `schema: SOME_SCHEMA` is a hoisted
            // const reference: resolve it against the collected const table, or
            // record it dropped. Anything else unrecognised is skipped, not
            // guessed.
            _ => {
                let end = skip_value(chars, i);
                let raw: String = chars[i..end.min(chars.len())].iter().collect();
                let raw = raw.trim();
                if key == "schema" {
                    resolve_schema_ref(&mut opts, raw, consts);
                } else {
                    assign_bare_opt(&mut opts, &key, raw);
                }
                i = end;
            }
        }
    }
    opts
}

/// Resolve a bare `schema: IDENT` reference against the collected top-level
/// const table. A const bound to an object/array data literal becomes the
/// step's schema; an unknown name (or a const that abstained as non-data) is
/// recorded in `schema_dropped` so the loss is surfaced, never silent (P7).
fn resolve_schema_ref(opts: &mut AgentOpts, raw: &str, consts: &ConstTable) {
    match consts.get(raw) {
        Some(v @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => {
            opts.schema = Some(v.clone());
        }
        // The name resolves, but not to an object/array — not a usable schema.
        Some(_) => {
            opts.schema_dropped = Some(format!(
                "const '{raw}' is not an object/array schema — not imported"
            ));
        }
        // A bare non-object literal (`schema: 42` / `true` / `null`) versus a
        // genuine unknown identifier — distinct diagnostics either way (P7).
        None => {
            let is_literal = raw.parse::<f64>().is_ok() || matches!(raw, "true" | "false" | "null");
            opts.schema_dropped = Some(if is_literal {
                format!("non-object schema literal '{raw}' not imported")
            } else {
                format!(
                    "schema reference '{raw}' unresolved (no top-level data-literal \
                     const '{raw}') — not imported"
                )
            });
        }
    }
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
        "requireGrounding" => {
            if raw == "true" {
                opts.require_grounding = true;
            }
        }
        "tolerateFailedDeps" => {
            if raw == "true" {
                opts.tolerate_failed_deps = true;
            }
        }
        // `timeoutSecs: 0` is passed through so the shared `validate()` produces
        // the one authoritative error ("omit the field for the global
        // default"). Silently rewriting it to "unset" here made the UNTRUSTED
        // boundary the most permissive of the three import paths: the author's
        // (mistaken) "no timeout" became the dispatcher's global budget with no
        // word to anyone.
        "timeoutSecs" => opts.timeout_secs = raw.parse::<u64>().ok(),
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
