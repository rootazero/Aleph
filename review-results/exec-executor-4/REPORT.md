# Review Report — Batch 4: `src/exec/{leak_detector,kernel,analysis,decision,allowed_decisions,risk,mod}.rs`

**Date:** 2026-08-11
**Scope:** `src/exec/leak_detector.rs` (378 lines) + `src/exec/kernel.rs` (142 lines) +
`src/exec/analysis.rs` (197 lines) + `src/exec/decision.rs` (41 lines) +
`src/exec/allowed_decisions.rs` (43 lines) + `src/exec/risk.rs` (46 lines) +
`src/exec/mod.rs` (34 lines) — 881 lines total
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    1 |     4 |   2 |    7 |

This batch is the security kernel of the exec surface. Most of the code
here is small and well-tested. The findings are mostly about the
boundary between the builtin floor (`command_policy`) and the
user-configured floor (`SecurityKernel`), the alignment between
`LeakDetector` and `SecretMasker` (which is also a Batch 1 finding
from the patterns perspective), and a few small quality issues that
make the code easier to audit.

## Findings

### [HIGH] `kernel.rs:from_config` (line 47-72) — invalid pattern count is dropped on the floor
**Category:** security / logic
**Confidence:** High

**Description.** `SecurityKernel::from_config` walks `config.custom_blocked`
and `config.custom_danger`, compiling each via `bounded_builder`. Invalid
patterns produce a `tracing::warn!` and are silently dropped. The function
returns `Self` with no indication of how many patterns were rejected.

This is the same pattern the `install_operator_patterns` mask pattern was
guilty of (Batch 1, [HIGH] #3), and the fix is identical in shape:
- The boot path that calls `from_config` does not log the rejection count.
- A misconfigured `[security] custom_blocked` with 100 entries, 50 of
  which are typos, gives the operator 50 patterns of coverage rather
  than the 100 they think they have, and the discrepancy is invisible.

**Suggested fix.** Return the rejection count (or a `Vec<(String, Err)>`)
so the boot path can log it. The two-line change is the same shape as
`install_operator_patterns`.

### [MEDIUM] `leak_detector.rs:scan` (line 113-148) — Aho-Corasick automaton is built and run on every call but the result is dropped
**Category:** architecture
**Confidence:** High

**Description.** `scan` calls `self.ac.is_match(content)` and discards the
result. The comment at line 124-127 explains why: the automaton is a
"soft hint" because some high-value secrets carry no prefix the automaton
knows about. So the regex sweep is always run.

But: the `ac.is_match` call is O(n) over the content (Aho-Corasick's
fast-path), and the regex sweep is O(n × patterns). For 13 patterns and
a 100 KB content, this is ~1.3M regex steps + a 100K-byte Aho-Corasick
scan. The Aho-Corasick pass is on top, not as a gate.

The comment frames the Aho-Corasick call as a "soft hint" — but the
code is using the wrong word. The Aho-Corasick call is a "do-not-skip"
guard against an earlier fast-path that used to gate the regex sweep.
The current code's call to `is_match` is **dead work**: the result is
discarded, so the Aho-Corasick automaton serves no purpose. The
construction-time cost is also paid on every `new()` call.

**Suggested fix.** Either:

1. Drop the `ac` field entirely (and the `prefixes` argument to `new`).
   The Aho-Corasick automaton has no observable effect; the comment
   explains the prior gate it used to enforce.
2. Or, use the `ac.is_match` result to short-circuit a slower path
   that does additional checks (e.g. a CRYPTO_BLOB pattern set that
   only runs when a prefix matches, like a true "fast path"). This is
   a real perf/correctness win but a wider refactor.

For this pass, dropping the field is the smaller, audit-cleaner change.

### [MEDIUM] `leak_detector.rs:redact` (line 162-175) — replaces patterns in-place; N patterns × 1 alloc per pattern
**Category:** quality
**Confidence:** High

**Description.** `redact` walks every pattern with `action == Redact`,
and for each one does `regex.replace_all(&current, "***REDACTED***")`.
Each iteration allocates a new `String` for the result and reassigns
`current`. For N Redact patterns and content C, the cost is O(N × |C|)
plus N allocations.

The current N is small (1: `bearer_token`), so this is a 1× cost in
practice. But the function scales linearly, and a future pattern
addition silently doubles the cost.

**Suggested fix.** Collect matches across all patterns, then apply
all replacements in a single pass. The `regex` crate's `RegexSet`
supports set-level matching but not set-level replacement. The
practical refactor is to iterate the matches once and rewrite.

For this pass, no change — the cost is bounded by the pattern count,
and the function is called by a panel on tool results, not a hot
loop. Document the cost in the doc comment.

### [MEDIUM] `analysis.rs:CommandResolution::not_found` (line 132-145) — empty `raw` produces `executable_name: ""`
**Category:** logic
**Confidence:** High

**Description.** `CommandResolution::not_found(raw)` does
`Path::new(&raw).file_name().and_then(|n| n.to_str()).unwrap_or(&raw).to_string()`.
For an empty `raw`, `Path::new("").file_name()` returns `None`, so the
fallback is `&raw` which is `""`. The resulting `CommandResolution` has
`executable_name: ""` and `resolved_path: None`.

An empty executable name is a soft failure: the model can call the
command by name (it sees `argv[0]`), but `ExecutedCommand::executable`
returns `Some("")` for any subsequent guard. A test or audit surface
that filters on non-empty executables would silently drop these.

The path is unreachable today: `parser.rs:analyze_shell_command` only
calls `resolve_executable` with `argv[0]`, and `tokenize_segment`
never produces an empty token. The corner case is reachable only
through a future code path that calls `CommandResolution::not_found`
with an empty string.

**Suggested fix.** Guard the empty case at the constructor:

```rust
pub fn not_found(raw: impl Into<String>) -> Self {
    let raw = raw.into();
    let name = if raw.is_empty() {
        "<empty>".to_string()
    } else {
        std::path::Path::new(&raw)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&raw)
            .to_string()
    };
    Self { raw_executable: raw, resolved_path: None, executable_name: name }
}
```

### [MEDIUM] `analysis.rs:CommandSegment` — no max size on `argv`
**Category:** logic
**Confidence:** Medium

**Description.** `CommandSegment.argv` is `Vec<String>` with no upper
bound. The tokenizer at `parser.rs:tokenize_segment` produces tokens
until end-of-input; a command with 10000 tokens (e.g. a generated
script fragment) produces a 10000-element argv.

This is fine in practice: the parser's 64 KB cap (after Batch 1's
fix) bounds the byte size, and 64 KB / 1-char tokens is at most 64K
elements. But the `Vec` allocation is unobserved, and a caller that
serialises the segment to JSON for the model would inflate the
request body.

**Suggested fix.** No change — the byte cap is the right bound. Add a
one-line doc comment on `CommandSegment.argv` noting the cap is
upstream in the parser.

### [LOW] `risk.rs:RiskLevel` — only `Blocked` has a predicate; `Safe` / `Caution` / `Danger` are unreachable
**Category:** architecture
**Confidence:** High

**Description.** `RiskLevel` is a 4-level enum. The kernel's
`assess_custom` only ever returns `Blocked` or `Danger` (or `None`).
`Safe` and `Caution` are never produced. The order tests
(`RiskLevel::Safe < Caution < Danger < Blocked`) are fine, but the
`is_blocked` predicate covers only one variant.

The MEDIUM is a documented design choice (the comment at line 5-9
says "Only Blocked / Danger are produced today"). The LOW is that
the unused variants are a future refactor risk: a future
"Caution" producer would have no predicate, and a future
"is_safe_or_caution" check would have to be hand-written.

**Suggested fix.** Either:
1. Add `is_safe()`, `is_caution()`, `is_danger()` predicates for
   symmetry.
2. Or, document the "only Blocked and Danger are produced"
   contract on the enum with `#[non_exhaustive]` so a future
   producer cannot quietly add a variant.

For this pass, no change — the comment is enough.

### [LOW] `decision.rs` — `ExecApprovalRequest::command` is `String` with no max length
**Category:** quality
**Confidence:** Medium

**Description.** `ExecApprovalRequest::command` is a `String` with no
documented cap. The parser's 64 KB cap is the upstream bound, so this
is bounded in practice. But a future caller that constructs a request
without going through the parser (e.g. a test) could submit a 4 GB
command, and the manager's `display_line` would `chars().take(120)`
truncate it (OK), but the channel bridge's `action.summary` (which
carries the command) is unbounded in `ApprovalAction::summary` (see
Batch 3 [HIGH] #2 — that fix truncates at 1000 chars; OK).

**Suggested fix.** No change — the parser's cap is the upstream
bound. Add a one-line doc comment.

## Cross-References

- `kernel.rs:from_config:47-72` — invalid pattern count is dropped.
  Same shape as `masker.rs::install_operator_patterns` (Batch 1,
  [HIGH] #3). The fix is identical: return the rejected count so the
  boot path can log it.
- `leak_detector.rs:scan:113-148` — the Aho-Corasick `is_match` call
  is dead work. The comment at line 124-127 is right that the
  fast-path used to gate the regex sweep; the fix is to drop the
  field. See `secret_patterns.rs:leak_detector_assets` (Batch 1) for
  the prefix list the automaton was built over.
- `analysis.rs:CommandResolution::not_found:132-145` — empty `raw`
  produces `executable_name: ""`. The fix is a one-line guard. The
  caller is `parser.rs:resolve_executable:resolve_executable` (Batch
  1), which never produces an empty `executable` today.
- `leak_detector.rs:redact:162-175` — N-pattern × per-pattern
  replacement. The `bearer_token` pattern is the only `Redact` action
  in the default set, so the cost is 1× in practice. See
  `secret_patterns.rs:leak_detector_assets` (Batch 1) for the
  pattern list and `LeakAction` enum.
- `risk.rs:RiskLevel` — only `Blocked` and `Danger` are produced.
  The catalog of `Blocked` patterns lives in
  `[security].custom_blocked` (parsed at `kernel.rs:from_config`).
