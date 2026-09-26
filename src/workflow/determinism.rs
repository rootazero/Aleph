//! Static determinism audit for step prompts.
//!
//! A workflow template is a *program* a model runs over multiple invocations.
//! Any prompt that resolves to different text on different runs breaks two
//! properties users have come to expect:
//!
//! 1. **Replay equality.** Two runs of the same template with the same args
//!    and the same upstream outputs should produce the same agent context
//!    byte-for-byte. Anything the prompt references that is NOT one of those
//!    three sources (the run's `{input}` and `{{args}}`, the upstream steps'
//!    outputs handed in by `build_handoff_context`) is non-deterministic.
//! 2. **Cache equality.** A prompt cache keyed on the prompt text is silently
//!    invalidated when the prompt itself is the source of variation — the
//!    cache hit rate falls to zero not because the answers changed, but
//!    because the prompt cannot be reproduced.
//!
//! This module surfaces the static subset of those non-determinism sources:
//! placeholders in the prompt body that reference runtime values the *template
//! language* knows about but does not substitute. They are not present in
//! `WorkflowDef::referenced_vars()` (which only finds `{{name}}` placeholders)
//! because they are NOT `{{...}}` — they look like the model is being asked
//! to embed a token the harness would have to expand.
//!
//! ## Why advisory, not fatal
//!
//! This is an audit, not a refusal. The runtime cannot stop a model from
//! improvising non-determinism anyway (the model may write `"now()"` to a
//! file, or fetch a URL via the configured tools), so a hard refusal at
//! save-time would only catch the things that are easy to catch and miss
//! every form the audit would fail closed against. The contract with
//! `workflow_tool`'s save/import paths is: log the findings alongside the
//! save response, never refuse the save.
//!
//! ## Boundary discipline
//!
//! A `{` that opens nothing is left as written by [`crate::workflow::def::render_prompt`],
//! so a `{nowhere` substring must not match. Word-bounded matching prevents
//! the `random` of `randomized_var_name` from firing on the substring
//! `random` alone — the audit is a tool for the user, not a parser they
//! have to keep apologising to.



/// What one finding of [`audit_step_prompt`] is about.
///
/// The kind carries **just enough** information for a UI to render an icon
/// and a one-line summary; the exact span (`column`/`len`) and the offending
/// text (`snippet`) on [`DeterminismFinding`] carry the rest. No payload
/// field for `cost` or `severity` — the audit does not score, it surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeterminismFindingKind {
    /// A `{now}` / `{time}` / `{timestamp}` placeholder — references the wall
    /// clock, so the same run yields a different prompt every time it is
    /// rendered.
    WallClock,
    /// A `{uuid}` / `{guid}` placeholder — references a freshly-minted
    /// identifier. The same run's two replays cannot produce the same value.
    RandomId,
    /// A `{random:N}` / `{rand}` placeholder — references an unseeded random
    /// value. Any two replays differ.
    RandomValue,
}

/// One non-deterministic surface detected in a step prompt.
///
/// Findings are produced by [`audit_step_prompt`] in left-to-right scan
/// order (a `BTreeMap` would re-sort by span and lose that, so a `Vec` is the
/// correct collection). The `column` is a byte offset, matching
/// `str::find` and the convention `serde_json::Value::pointer` uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeterminismFinding {
    /// Which family the finding belongs to (so a UI can group / colour them).
    pub kind: DeterminismFindingKind,
    /// Byte offset of the placeholder in the input prompt.
    pub column: usize,
    /// Byte length of the placeholder (the `{...}` token, inclusive).
    pub len: usize,
    /// The placeholder text verbatim, e.g. `"{{now}}"`. The harness may want
    /// to render this back to the user; carrying it avoids the audit's
    /// consumer having to slice the prompt itself.
    pub snippet: String,
}

/// Audit one step prompt for non-deterministic placeholders.
///
/// Returns one [`DeterminismFinding`] per match, in left-to-right scan order.
/// Duplicates (the same placeholder appearing twice) are returned twice — the
/// prompt author almost certainly meant both, and a count of "you have two
/// `now`s" is more useful than "you have one `now`" because both must be
/// dealt with to make the prompt deterministic.
///
/// **Advisory only** (see module docs): callers must never refuse a save
/// based on the returned vector. The contract with `workflow_tool`'s
/// save/import paths is to surface findings to the user, not to gate on them.
#[must_use]
pub fn audit_step_prompt(prompt: &str) -> Vec<DeterminismFinding> {
    // The pattern set is hand-written rather than a single regex because
    // each family has its own boundary rule:
    //   - `{now}` / `{time}` / `{timestamp}` — `{`-bounded, identifier-style
    //     suffix `{{name}}` is unrelated (the name would NOT be one of these
    //     three keywords);
    //   - `{uuid}` / `{guid}` — same shape;
    //   - `{random}` / `{rand}` — same shape, but `{random:8}` and `{random:N}`
    //     are also detected, so the suffix is `([:][0-9]+)?` not `}}`.
    // A single regex with all three rule sets expressed as alternations
    // would obscure the boundary rule for each; the manual walk keeps each
    // one inspectable.
    let mut findings = Vec::new();

    // Time family: {now}, {time}, {timestamp}. The double-brace `{{name}}`
    // shape is the named-placeholder form — `name` would be `"now"` etc.,
    // but those are valid `{{...}}` placeholders whose value comes from the
    // run's `args` map, NOT non-deterministic at this layer. The audit
    // targets the **{single-brace** runtime-variable form, which is what a
    // templating engine would expand if it had a `now` helper. Single-brace
    // means: the `{` is followed by exactly one identifier word, then `}`.
    for keyword in ["now", "time", "timestamp"] {
        scan_single_brace_keyword(prompt, keyword, &mut findings, DeterminismFindingKind::WallClock);
    }
    // Random-id family: {uuid}, {guid}. Same shape, different domain — the
    // value is a freshly-minted identifier rather than a wall-clock sample.
    for keyword in ["uuid", "guid"] {
        scan_single_brace_keyword(prompt, keyword, &mut findings, DeterminismFindingKind::RandomId);
    }
    // Random-value family: {random}, {rand}, and the suffixed forms
    // {random:N} / {rand:8}. The optional `:N` is a length hint, NOT an
    // argument — the value is still random either way.
    scan_random_family(prompt, "random", &mut findings);
    scan_random_family(prompt, "rand", &mut findings);

    findings.sort_by_key(|f| f.column);
    findings
}

/// Single-brace `{<keyword>}` scan with strict word-boundary semantics.
///
/// A `{` opens a placeholder only when followed by `[A-Za-z_][A-Za-z0-9_]*`
/// and a closing `}`. A `{nowhere` opens nothing — the audit must not match
/// the substring `{now`. The boundary check is `is_word_char`, not a regex
/// word-boundary, so the `{` in `{now {time} }` opens at the first `{` and
/// the next walk resumes from the next position; `now {time}` therefore
/// matches only `{time}`, not `{now` (which is followed by a space).
///
/// Also rejects `{{keyword}}`: the byte immediately BEFORE the `{` must not
/// be another `{`. Without this check, the substring `{now}` inside the
/// `{{name}}` named-arg placeholder would be flagged as non-deterministic
/// even though the run's args map supplies the value. A run-time arg named
/// `now` is legitimate; the audit targets the *single-brace* runtime form
/// only.
fn scan_single_brace_keyword(
    prompt: &str,
    keyword: &str,
    out: &mut Vec<DeterminismFinding>,
    kind: DeterminismFindingKind,
) {
    let needle = format!("{{{keyword}}}");
    let mut start = 0usize;
    while let Some(rel) = prompt[start..].find(&needle) {
        let at = start + rel;
        // Word-boundary on the LEFT: the byte before `{` must not be a word
        // char, otherwise `{now` is part of `prefix{now}` and the user's
        // intent is ambiguous (the audit errs on the side of catching it,
        // and the user can decide; matching only the bare `{now}` form would
        // miss `prefix{now}` entirely, which is the worse failure mode).
        let left_ok = at == 0
            || !prompt[..at]
                .chars()
                .next_back()
                .is_some_and(is_word_char);
        // `{{keyword}}` (named-arg) guard: the byte before `{` must not be
        // another `{`. Without it, `{{now}}` matches the `{now}` substring
        // starting at byte 1.
        let not_double_brace = at == 0 || prompt.as_bytes()[at - 1] != b'{';
        // Word-boundary on the RIGHT: the byte after `}` must not be a word
        // char, otherwise `{now}s` is a word that starts with the
        // placeholder (matching it would suggest renaming `now` to `nowstamp`
        // fixes a non-issue).
        let end = at + needle.len();
        let right_ok = end >= prompt.len() || !is_word_char_in_str_at(prompt, end);
        if left_ok && not_double_brace && right_ok {
            out.push(DeterminismFinding {
                kind,
                column: at,
                len: needle.len(),
                snippet: needle.clone(),
            });
        }
        start = at + 1;
    }
}

/// `{random}` / `{random:N}` / `{rand}` / `{rand:N}` — optional `:N` length
/// hint after the keyword. Word-bounded like [`scan_single_brace_keyword`],
/// but the suffix character class is digit-only (no alphanumeric suffix is
/// legal — `{random5}` is a typo, not a length hint).
///
/// Same `{{keyword}}` named-arg guard as the keyword scanner: the byte
/// immediately before the `{` must not be another `{`, or `{{random}}`
/// would be falsely flagged as non-deterministic (the value comes from
/// `args` at render time, deterministic per-run).
fn scan_random_family(prompt: &str, keyword: &str, out: &mut Vec<DeterminismFinding>) {
    let prefix = '{';
    let mut idx = 0usize;
    while let Some(rel) = prompt[idx..].find(prefix) {
        let at = idx + rel;
        let after_brace = at + 1;
        // Check the keyword starts at `after_brace`.
        if !prompt[after_brace..].starts_with(keyword) {
            idx = at + 1;
            continue;
        }
        let kw_end = after_brace + keyword.len();
        let suffix_start = kw_end;
        // Determine the end of the placeholder: optional `:N` of digits, then `}`.
        let (placeholder_end, body_end) = match &prompt.as_bytes()[suffix_start..] {
            [b'}', ..] => (suffix_start + 1, suffix_start + 1),
            [b':', rest @ ..] => {
                // Collect digits after the colon. An empty `:}` is a malformed
                // placeholder — skip it; the prompt author intended a length
                // and got the parser, not the model.
                let digits_end = rest
                    .iter()
                    .take_while(|&&b| b.is_ascii_digit())
                    .count();
                if digits_end == 0 || rest.get(digits_end) != Some(&b'}') {
                    idx = at + 1;
                    continue;
                }
                (suffix_start + 1 + digits_end + 1, suffix_start + 1 + digits_end)
            }
            _ => {
                idx = at + 1;
                continue;
            }
        };
        // Word-boundary checks identical to the keyword case, plus the
        // `{{keyword}}` guard (the byte before `{` must not be another `{`).
        let left_ok = at == 0
            || !prompt[..at]
                .chars()
                .next_back()
                .is_some_and(is_word_char);
        let not_double_brace = at == 0 || prompt.as_bytes()[at - 1] != b'{';
        let right_ok = placeholder_end >= prompt.len()
            || !is_word_char_in_str_at(prompt, placeholder_end);
        if left_ok && not_double_brace && right_ok {
            let snippet = prompt[at..placeholder_end].to_string();
            out.push(DeterminismFinding {
                kind: DeterminismFindingKind::RandomValue,
                column: at,
                len: snippet.len(),
                snippet,
            });
        }
        idx = body_end.max(at + 1);
    }
}

const fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `is_word_char(prompt.as_bytes()[i])` expressed in char terms. The byte at
/// `i` may be the middle byte of a multi-byte UTF-8 sequence (we walk by
/// bytes for speed), in which case the function returns `false` (it is not
/// an ASCII word char), which is the conservative answer — non-ASCII bytes
/// never count as word chars in the audit's boundary check.
fn is_word_char_in_str_at(s: &str, i: usize) -> bool {
    // SAFETY: the caller guarantees `i < s.len()` (the only call site checks
    // against `prompt.len()` before invoking). A mid-codepoint byte cannot
    // match an ASCII alphanum; this returns false for it, which is the right
    // boundary answer.
    s.as_bytes()[i].is_ascii_alphanumeric() || s.as_bytes()[i] == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(prompt: &str) -> Vec<DeterminismFindingKind> {
        audit_step_prompt(prompt)
            .into_iter()
            .map(|f| f.kind)
            .collect()
    }

    #[test]
    fn a_clean_prompt_finds_nothing() {
        // The common case: a prompt without any runtime-variable references
        // audits to an empty vector. The audit is a tool for the user; an
        // empty result must not be reported as a finding.
        assert!(audit_step_prompt("research the topic and write a report").is_empty());
        assert!(audit_step_prompt("audit {{region}} for {{env}}, then refresh").is_empty());
        assert!(audit_step_prompt("").is_empty());
    }

    #[test]
    fn detects_wall_clock_placeholders() {
        // All three keywords are caught, in any case of repetition, with the
        // correct kind tagged so a UI can render them as "time" warnings.
        let findings = audit_step_prompt("at {now} write {timestamp}");
        assert_eq!(
            kinds("at {now} write {timestamp}"),
            vec![DeterminismFindingKind::WallClock, DeterminismFindingKind::WallClock],
        );
        // Snippet + column are carried so the audit consumer does not have
        // to re-slice the prompt.
        assert_eq!(findings[0].snippet, "{now}");
        assert_eq!(findings[0].column, 3);
        assert_eq!(findings[0].len, 5);
        assert_eq!(findings[1].snippet, "{timestamp}");
    }

    #[test]
    fn detects_random_id_placeholders() {
        assert_eq!(
            kinds("name: {uuid}, alt: {guid}"),
            vec![DeterminismFindingKind::RandomId, DeterminismFindingKind::RandomId],
        );
    }

    #[test]
    fn detects_random_value_placeholders_with_and_without_length() {
        // `{random}` is the base form; `{random:8}` adds a length hint —
        // the placeholder is still non-deterministic, the length is a hint
        // to whatever would expand it.
        let findings = audit_step_prompt("token={random}, id={rand}, salt={random:16}");
        assert_eq!(findings.len(), 3);
        assert!(findings.iter().all(|f| f.kind == DeterminismFindingKind::RandomValue));
        assert_eq!(findings[0].snippet, "{random}");
        assert_eq!(findings[0].len, 8);
        assert_eq!(findings[1].snippet, "{rand}");
        assert_eq!(findings[1].len, 6);
        assert_eq!(findings[2].snippet, "{random:16}");
        assert_eq!(findings[2].len, 11, "{{random:16}} is 11 chars including braces");
    }

    #[test]
    fn ignores_word_fragments_not_placeholders() {
        // `{nowhere` is a `{` that opens nothing — a literal in the prompt.
        // `random` inside `randomized_var` is not a placeholder. `nows` is
        // not `{now}`. The audit must distinguish the user's prose from the
        // would-be placeholders, or it surfaces noise that authors stop
        // reading.
        assert!(audit_step_prompt("{nowhere").is_empty());
        assert!(audit_step_prompt("the variable is random").is_empty());
        assert!(audit_step_prompt("{now}s elapsed").is_empty());
        assert!(audit_step_prompt("prefix{now}").is_empty(),
            "word-boundary on the left: `{{now}}` after a word char is part of a larger identifier");
        assert!(audit_step_prompt("{random_5}").is_empty(),
            "underscore-suffixed keyword is not the base form");
    }

    #[test]
    fn ignores_double_brace_placeholders_with_deterministic_names() {
        // `{{name}}` placeholders derive from the run's `args` map at
        // render time — they are NOT non-deterministic at this layer, even
        // if the name happens to be `now` or `uuid`. The audit's domain is
        // the **{single-brace** runtime-variable form only.
        assert!(audit_step_prompt("{{now}}").is_empty());
        assert!(audit_step_prompt("{{uuid}}").is_empty());
        assert!(audit_step_prompt("{{random}}").is_empty());
        assert!(audit_step_prompt("a {{now}} b {{uuid}} c").is_empty());
    }

    #[test]
    fn mixed_finds_are_returned_in_left_to_right_order() {
        // Multiple findings must come back in scan order so a UI can render
        // them inline without re-sorting.
        let findings =
            audit_step_prompt("start {now} middle {uuid} end {random:4} done {guid}");
        assert_eq!(findings.len(), 4);
        assert_eq!(findings[0].column, 6);
        assert_eq!(findings[1].column, 19);
        assert_eq!(findings[2].column, 30);
        assert_eq!(findings[3].column, 46);
        let kinds_seen: Vec<_> = findings.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds_seen,
            vec![
                DeterminismFindingKind::WallClock,
                DeterminismFindingKind::RandomId,
                DeterminismFindingKind::RandomValue,
                DeterminismFindingKind::RandomId,
            ],
        );
    }

    #[test]
    fn repeated_placeholder_yields_multiple_findings() {
        // A prompt that says `{now} ... {now}` has TWO `now`s — both must be
        // reported, not deduped, because the user has to fix both to make
        // the prompt deterministic.
        let findings = audit_step_prompt("{now} and {now}");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].column, 0);
        assert_eq!(findings[1].column, 10);
    }

    #[test]
    fn malformed_random_length_hint_is_skipped() {
        // `{random:}` (empty length) and `{random:abc}` (non-digit length)
        // are malformed placeholders — the audit's domain is the `{...}` form,
        // and a `:}` or `:abc}` does not match. Skipping, rather than
        // matching, keeps the audit honest about what it found.
        assert!(audit_step_prompt("{random:}").is_empty());
        assert!(audit_step_prompt("{random:abc}").is_empty());
        assert!(audit_step_prompt("{rand:x}").is_empty());
    }

    #[test]
    fn audit_is_byte_position_correct_in_utf8() {
        // The prompt contains a non-ASCII character before the placeholder.
        // The audit's `column` is a BYTE offset (the same unit `str::find`
        // returns), so the placeholder's byte position must account for the
        // multi-byte char. A char-offset audit would silently shift the
        // highlight left in any UI that renders columns.
        let prompt = "前缀 {now} 后缀";
        let findings = audit_step_prompt(prompt);
        assert_eq!(findings.len(), 1);
        // `前` is 3 bytes, `缀` is 3 bytes, the space is 1 → `{` at byte 7.
        assert_eq!(findings[0].column, 7);
        assert_eq!(findings[0].snippet, "{now}");
    }

    #[test]
    fn single_brace_open_with_no_close_does_not_crash() {
        // A `{` that opens nothing is left as written. A trailing `{now`
        // with no `}` must not panic the scanner; it must simply produce no
        // finding.
        assert!(audit_step_prompt("{now").is_empty());
        assert!(audit_step_prompt("text {").is_empty());
        assert!(audit_step_prompt("{random").is_empty());
    }

    #[test]
    fn finding_kind_display_helpers_exist_for_logging() {
        // The kind enum's `Debug` output is the surface every log line
        // renders — assert the names are stable so log greps do not break
        // when someone adds a variant.
        assert_eq!(format!("{:?}", DeterminismFindingKind::WallClock), "WallClock");
        assert_eq!(format!("{:?}", DeterminismFindingKind::RandomId), "RandomId");
        assert_eq!(
            format!("{:?}", DeterminismFindingKind::RandomValue),
            "RandomValue"
        );
    }

    #[test]
    fn the_btreemap_use_does_not_leak_into_crate_ordering() {
        // Defensive: the module does NOT sort findings through a BTreeMap
        // — a previous draft did, and a BTreeMap would silently re-sort by
        // `(kind, column)`, dropping the left-to-right ordering callers
        // rely on. The audit's contract is scan-order; keep it that way by
        // asserting the order directly.
        let prompt = "{guid} {random} {uuid} {now}";
        let findings = audit_step_prompt(prompt);
        let snippets: Vec<_> = findings.iter().map(|f| f.snippet.as_str()).collect();
        assert_eq!(snippets, vec!["{guid}", "{random}", "{uuid}", "{now}"]);
    }
}