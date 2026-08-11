# Review Report — Batch 1: `src/exec/parser.rs` + `src/exec/masker.rs` + `src/exec/secret_patterns.rs`

**Date:** 2026-08-11
**Scope:** `src/exec/parser.rs` (639 lines) + `src/exec/masker.rs` (259 lines) + `src/exec/secret_patterns.rs` (264 lines) — 1162 lines total
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    3 |     4 |   3 |   10 |

This batch covers the only two layers between user input and an outbound message:
the **command parser** that decides what a shell line is allowed to do, and the
**secret masker** that decides what text gets shipped to the LLM. Both are
single-source defences: a bypass either silently runs an arbitrary command, or
silently prints a credential. The `LeakDetector` and `SecretMasker` patterns are
shared, and the patterns list is the only place those two consumers' coverage
either diverges or agrees — so this is also a `R10` / "single source" check.

## Findings

### [HIGH] `parser.rs:contains_unquoted_subshell` (line 17-50) — `$<open_brace>` and `\\<open_paren>` slip through
**Category:** security / logic
**Confidence:** High

**Description.** The detector matches `$(`, `<(`, `>(` only when the OPEN paren
is the trigger character. A user who types `echo $( echo hi )` — with arbitrary
whitespace, comments, or string interpolation between the dollar and the paren
— relies on bash's actual tokenization, which collapses both forms. The detector
must reject any **dollar followed by a paren after only "interpolatable"
whitespace**, otherwise:

- `echo $ (echo hi)` — bash would error, but our detector allows it (parens
  not adjacent to `$`). This is benign today, but…
- `echo $; (echo hi)` — semicolon is fine in our chain parser, then we get to
  ` (echo hi)` which is a subshell. The detector scans the whole string and
  never sees `$(`, so it allows a command that BASH will execute as
  `echo $; ( echo hi )` → a literal `$;` followed by a fresh subshell.

The actual dangerous one is `echo $  ( whoami )` (whitespace and a comment
between `$` and `(`) — bash will complain in interactive mode but in
`set +H` / a script it accepts. The defensive answer is the same: require
**the dollar and the open-paren to be tokenized as one construct**, not merely
adjacent bytes.

**Suggested fix.** Either add a pre-pass that joins `\$\s*\(` into `$(`
before the linear scan, or change the match arm to permit intervening
whitespace and shell comment markers (`#` to EOL):

```rust
'$' if !in_single => {
    // Look ahead: if the next non-whitespace, non-comment char is `(`,
    // this is a command substitution. The current `prev == '$'` check
    // misses `$( ` (with a space), which bash still parses as `$(`.
    let mut peek = chars.clone();
    let mut parens_seen = false;
    while let Some(&p) = peek.peek() {
        match p {
            ' ' | '\t' => { peek.next(); }
            '#' => break, // comment until EOL — not a subshell
            '(' => { parens_seen = true; break; }
            _ => break,
        }
    }
    if parens_seen { return true; }
}
```
(The actual implementation should also handle nested `$(` inside double
quotes — currently the in_double branch reads only `(` after `$`, and an
`echo "$( echo hi )"` is rejected only because the *outer* `$` and `(` are
adjacent, with the same `prev == '$'` rule.)

### [HIGH] `parser.rs:analyze_shell_command` (line 88-93) — 64 KiB cap is bytes, not chars; multi-byte input passes it cleanly while O(n) passes still burn
**Category:** logic / DoS
**Confidence:** High

**Description.** The DoS guard is `command.len() > 64 * 1024`, which is bytes
in the Rust sense. A 64 KiB-of-**bytes** command in pure CJK is ~21k
characters — well below the cap when read as chars. The three downstream
passes (subshell / redirect / chain split) iterate `command.chars()`, so the
O(n) per pass is the same as the byte-length check. That's not the DoS vector
the cap was added for.

The real DoS vector is **byte length**: a 64 KiB string of 4-byte UTF-8 chars
forces ~256 KiB of allocation in `split_command_chain` (it builds
`current: String` chunk-by-chunk via `chars().next()` and `push(ch)`). That's
the case the byte cap catches. But a 64 KiB-of-**chars** string of pure ASCII
is the same allocation cost. So the byte cap protects against UTF-8 of
mostly-1-byte, but a 64 KiB-of-4-byte string is **half** the cap. That is
inconsistent.

**Suggested fix.** Switch to `command.len()` and add a SECOND guard on
`command.chars().count()` so neither dimension can blow past the intended
memory cost:

```rust
const MAX_COMMAND_BYTES: usize = 64 * 1024;
const MAX_COMMAND_CHARS: usize = 64 * 1024;
if command.len() > MAX_COMMAND_BYTES || command.chars().count() > MAX_COMMAND_CHARS {
    return CommandAnalysis::error("command exceeds maximum analyzable length");
}
```

### [HIGH] `masker.rs:OPERATOR_PATTERNS` (line 18-24) — process-global + unbounded count + compilation is one of three failure modes
**Category:** security / logic
**Confidence:** High

**Description.** `OPERATOR_PATTERNS` is a process-global `RwLock<Arc<Vec<...>>>`.
The header comment says this is intentional ("process-global on purpose" so
every `SecretMasker::new()` site picks it up). But:

1. There is no upper bound on how many patterns an operator can install.
   `install_operator_patterns` accepts `impl IntoIterator<Item = (&str, &str)>`
   and pushes every valid pair. A config typo that points at a 1000-entry
   file (or a future tool that lets users define per-prompt redactions) makes
   every `mask()` call run 1000 regex passes over its input — and a redacting
   emitter runs `mask()` on every outbound string leaf of every JSON payload.
2. Each pattern is compiled with `bounded_builder`, which means a pathological
   regex (catastrophic backtracking) hits the same DoS surface for every
   `mask()` call. The bounded builder is the mitigation, but the call is in a
   hot path; the cost of compiling once is paid by every request.
3. `OPERATOR_PATTERNS` is process-global but `install_operator_patterns` is
   called exactly once at boot. **No code path resets the patterns on a config
   reload.** A `self_config update` of `[security] mask_patterns` therefore
   never takes effect without a daemon restart — a fact the operator cannot
   discover from logs and which silently leaves credentials unredacted.

**Suggested fix.** Three independent small fixes:

1. Add a `const MAX_OPERATOR_PATTERNS: usize = 64;` and reject at install
   time. A misconfigured `mask_patterns` is the more common failure mode than
   a legitimate use of 65+ patterns.
2. Expose `installed_count()` and surface it in `doctor` / `config_audit` so
   the operator can see "patterns installed: N" without grepping logs.
3. Re-call `install_operator_patterns` on a `[security]` config reload (the
   same hook `self_config`'s broadcaster already uses for other sections) so
   patterns track config, not boot.

### [MEDIUM] `parser.rs:resolve_executable` (line 327-333) — `path.exists()` is a TOCTOU race that also reads the host filesystem
**Category:** security / R1 violation
**Confidence:** Medium

**Description.** The comment on `resolve_executable` says it must use only the
injected env map (sandbox-aware) and never `std::env::var("PATH")` — that part
is correct. But the existence check is `path.exists()`, which calls into
`std::fs::metadata` — that reads the **host** filesystem, not the sandbox's.

Concretely, in a containerised/sandboxed run:
- The injected `env.PATH` lists sandbox bin dirs (`/sandbox/bin`).
- The host filesystem has `/sandbox/bin/rm` — the metadata call succeeds.
- The LLM sees `rm` resolved to `/sandbox/bin/rm` and writes a command that
  names that path.
- The actual exec happens inside the sandbox, which may not have that
  mount — so the call either fails (visible: "command not found", OK) or
  succeeds against a host path the LLM doesn't know it has access to.

This is an R1 violation: `src` is reading the host filesystem on behalf of
the sandbox.

**Suggested fix.** Gate `path.exists()` behind the same sandbox boundary that
the eventual `exec` call uses. Until that exists, drop the existence check —
the dispatch layer (`BashExecTool`) re-resolves at exec time and reports
`not found` more authoritatively. The `CommandResolution::not_found` path is
the truthful answer; the existing `found` path lies about reachability.

### [MEDIUM] `parser.rs:tokenize_segment` (line 265-303) — unclosed-quote returns `None` instead of an error analysis
**Category:** logic
**Confidence:** High

**Description.** When the tokenizer hits an unclosed quote or trailing escape,
it returns `None` and the caller maps that to
`CommandAnalysis::error("unable to parse command segment")`. But the same
condition in `split_command_chain` and `split_pipeline` (lines 138 and 226)
returns `Err("unclosed quote or trailing escape in pipeline")`. Two
inconsistent error surfaces for the same condition; downstream code has to
string-match on different phrases.

**Suggested fix.** Promote the three error sites to a single
`ParseError::UnclosedQuote` (or `UnclosedEscape`) enum and have
`analyze_shell_command` translate the enum into a `CommandAnalysis::error`
with a stable `reason` string. The dispatcher can then string-match
reliably for telemetry and the model can rely on a consistent error.

### [MEDIUM] `masker.rs:install_operator_patterns` (line 38-55) — invalid regex is reported but caller never logs it
**Category:** logic
**Confidence:** High

**Description.** `install_operator_patterns` returns
`(usize, Vec<(String, regex::Error)>)` so the caller can log rejected
patterns. **No caller does.** `grep -rn "install_operator_patterns" src/`
shows the function is called once (boot); the `(installed, rejected)` tuple
is discarded. The comment in `masker.rs:42-46` says "The caller logs the
rejects", but the caller is the only producer of the secret-patterns
library, and the `_rejected` arm is silenced.

**Suggested fix.** Either log inside the function (at `warn!` level,
matching the kernel's pattern-drop behaviour) or document that the caller
takes responsibility. The current "caller logs" comment is a false contract.

### [MEDIUM] `secret_patterns.rs:secret_masker_patterns` (line 18-130) — patterns list is mostly duplicated against `leak_detector_assets`
**Category:** architecture
**Confidence:** High

**Description.** `secret_masker_patterns()` and `leak_detector_assets()` both
list the same OpenAI / Anthropic / Google / AWS / GitHub / Slack / Discord /
GitHub PAT patterns. The test `openai_pattern_in_both` already exists
specifically to assert the two lists do not drift. But:

- The masker carries extra patterns the detector doesn't:
  `aws_secret_access_key = ...`, `(?i)bearer...`, `(?i)-u/--user` style
  curl passwords, `://user:pass@`, generic `password=` lines, and the
  private-key block. The detector's `bearer_token` is a `Redact` (not
  `Block`).
- The detector's `github_fine_grained_pat` is in both, but the masker's
  `github_pat_` is also in both; the test only asserts that the regex
  strings match, not the action. If the detector ever flips `github_token`
  to `Redact` (mirror of `bearer_token`), the masker still `Block`s the
  same pattern — they will diverge in BEHAVIOUR while passing every
  test.

**Suggested fix.** Refactor to a single `SecretPattern` type that carries
`(name, regex, action, replacement)` and have the masker + detector both
source from it. The existing `LeakDetectorAssets { prefixes, patterns }`
and the `Vec<SecretPattern>` (masker) become one type, with the detector
filtering on `action != Redact` (since the detector's job is finding
findings, the masker's is finding+replacing). This is R10 in action:
"intelligence lives in the prompt" — the patterns list IS the policy.

### [LOW] `parser.rs` — `actual_path` defaulting to empty string on missing `env.PATH`
**Category:** quality
**Confidence:** High

**Description.** `let actual_path = env.and_then(|e| e.get("PATH")).cloned().unwrap_or_default();`
means a missing `PATH` becomes `""`. The downstream `split(':')` on empty
yields one empty entry which is `continue`d. So a missing PATH is a silent
no-op. This is the right behaviour, but the empty-string default is implicit
— a reader has to follow three steps to know why "no PATH" doesn't crash.

**Suggested fix.** Rename `actual_path` to `path_env_value: String` and add
a one-line `// empty PATH → split yields one empty entry → skipped below` comment.

### [LOW] `masker.rs:60-66` — `contains_secrets` is a separate pass over both lists
**Category:** quality
**Confidence:** Medium

**Description.** `mask()` walks every pattern in `SECRET_PATTERNS` then every
in `operator_patterns()`. `contains_secrets()` does the same. Each
`is_match(text)` call is a fresh regex scan. For a 100 KB content, that's
2N regex passes per public method.

**Suggested fix.** This is a 2N perf nit, not a correctness issue. Leave
unless profiler points here. (Document the cost in the doc comment so
future readers know.)

### [LOW] `secret_patterns.rs:53-54` — `AIza[a-zA-Z0-9_\-]{35}` has no word boundary
**Category:** logic
**Confidence:** Medium

**Description.** The Google API key pattern starts with `AIza` and the next
35 chars are `[a-zA-Z0-9_-]`. There is no `\b` anchor, so `sAIzaXXX…XXX` in
the middle of a string matches. In practice that's a 39-char false positive,
which is unlikely enough that the leak-detector would rather over-match than
miss. But the openai pattern (`\bsk-[a-zA-Z0-9]{20,}`) IS anchored, and the
inconsistency is the kind of thing that bites when someone changes the
google pattern in isolation.

**Suggested fix.** Add `\b` to the AIza pattern, OR add a one-line comment
that the deliberate asymmetry (openai = anchor, google = no anchor) is on
purpose (because the body in the google pattern already requires `_` or
`-` separators and the false-positive rate is negligible).

## Cross-References

- `parser.rs:resolve_executable:341` — `path.exists()` reads the host filesystem.
  The bash/code_exec tools re-resolve at exec time under the sandbox; the parser
  is the one place that resolves eagerly, and the answer is wrong whenever the
  sandbox view differs from the host view. See `src/sandbox/command_policy`
  for the authoritative resolution surface.
- `masker.rs:OPERATOR_PATTERNS:18` — process-global, set once at boot, never
  re-set on config reload. The `self_config` tool's broadcaster pattern is the
  hook to call `install_operator_patterns` again; this is the natural fix
  surface for finding [HIGH] #3.
- `secret_patterns.rs:github_pat_[A-Za-z0-9_]{50,}` — the github_fine_grained_pat
  is in BOTH `secret_masker_patterns()` and `leak_detector_assets()`. The
  existing test (`github_token_pattern_in_both`) ensures the regex strings
  match. It does NOT ensure the actions match. The architectural fix in
  [MEDIUM] #7 closes that gap.
- `parser.rs:160` — `try_split_chain_operator` is the only place that rejects
  the bare `&` background operator. It returns `Err("background operator (&) not
  allowed")`. The same error string is not tested in `parser.rs`'s test
  module. Lowest priority — the existing test `test_background_operator_rejected`
  does assert `result.is_err()`.
