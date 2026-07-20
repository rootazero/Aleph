# Module: src/workflow

- Path: `src/workflow/`
- Files scanned: 10
- Total LOC: 4672

## Summary
| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 0 |
| medium   | 3 |
| low      | 7 |
| **Total**| **10** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
```
ISSUE|src/workflow/proposal.rs:51|medium|`canonical_name` joins raw skill names without sanitization, but `already_covered` compares against `WorkflowMeta.name` which is the sanitized filename stem — the miner re-drafts the same chain every dream cycle instead of dedup'ing
ISSUE|src/workflow/interop/import.rs:97|low|Imperative-construct needle `"budget"` is too loose — matches substring `budget` anywhere (incl. identifiers/comments); false positives
ISSUE|src/workflow/interop/import.rs:98|medium|Imperative-construct needles `"for "` and `"while "` require a trailing space, missing minified `for(` / `while(` forms; loop logic silently lost on re-import
ISSUE|src/workflow/clarify.rs:98|low|`ClarifyTaskMeta::to_value` silently swallows serialisation errors with `unwrap_or(Value::Null)` — orphan `Pending` task owned by sentinel no dispatcher knows
```

### Perspective 2 — Logic & Correctness
```
ISSUE|src/workflow/interop/import.rs:282|low|`scan_meta_field` locates the field via `src.find(&key)`, returning the FIRST textual occurrence (no comment/string skipping), causing wrong/empty field reads
ISSUE|src/workflow/proposal.rs:186|low|`covered_by_step_set` sanitises the chain but compares against `manifest.steps[*].id` which are user-authored raw IDs — miner shadows an existing user workflow with a draft
ISSUE|src/workflow/compile.rs:286|low|`subject` constructed as `format!("{}:{}", def.name, step.id)`; both raw user strings — colons in name/step id produce subject with multiple `:`; downstream consumers may mis-attribute boundary
ISSUE|src/workflow/compile.rs:320|low|`cancel_partial` awaits status update for each partial task sequentially — O(N × round-trip); observer may see partial run still in `Pending`/`Blocked`
```

### Perspective 3 — Architecture Compliance
No violations. The module stays inside R1/R3/R4/R7/R10.

### Perspective 4 — Code Quality
```
ISSUE|src/workflow/interop/import.rs:1|low|`interop/import.rs` is 1658 lines — 3× the 500-line guideline; mixes lexer/scanner/parser/tests
ISSUE|src/workflow/interop/import.rs:298|low|`read_first_string_literal` allocates O(N) Vec<char> on every call to look at the first non-whitespace token
```
