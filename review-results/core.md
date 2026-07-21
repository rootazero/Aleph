# Module: src/core

- Path: `src/core/`
- Files scanned: 2
- Total LOC: 158
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary
| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 2 |
| medium   | 7 |
| low      | 5 |
| **Total**| **14** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
```
ISSUE|src/core/types.rs:71-78|medium|DoS vector: `MediaAttachment.data` (unbounded `String`) has no invariant tying its actual length to `size_bytes`; producer/consumer must trust the unvalidated self-report.
ISSUE|src/core/types.rs:134|medium|`similarity_score: Option<f32>` accepts NaN/Inf through serde without guard — poisons downstream sort/rank and is silently persisted.
ISSUE|src/core/types.rs:64-78|medium|Trusted-input gap: `data` is declared as Base64 or UTF8 via `encoding` but no validation runs at deserialize time, allowing non-UTF8 bytes under `encoding = Utf8` to panic later.
ISSUE|src/core/types.rs:132|low|`timestamp: i64` accepts negative/zero values with no range check, admitting nonsensical epochs.
ISSUE|src/core/types.rs:121-135|low|`Default` for `MemoryEntry` yields `id=""` + `timestamp=0`, which is indistinguishable from uninitialized state if ever leaked into API responses.
```

### Perspective 2 — Logic & Correctness
```
ISSUE|src/core/mod.rs:3-7|high|Broken public seam: `MediaAttachment.media_type` references `core::types::MediaType`, but `MediaType` is not re-exported from `crate::core` — external users cannot name the field type through the public path.
ISSUE|src/core/types.rs:108-118|medium|Name collision risk: `CompressionStats` here overlaps with `crate::gateway::handlers::memory::CompressionStats` (different shape) — future revival will silently shadow the handler type.
ISSUE|src/core/types.rs:121-135|medium|Name collision risk: `MemoryEntry` here overlaps with `crate::memory::context::MemoryEntry` and `crate::gateway::handlers::memory::MemoryEntry` (different fields/ownership) — three distinct structs share one name in the same crate.
ISSUE|src/core/types.rs:107-118|low|No invariant enforced that `CompressionStats.valid_facts <= total_facts` — producer could report a contradictory state that passes deserialization.
ISSUE|src/core/types.rs:64-78|low|`MediaAttachment` and `CapturedContext` lack `Default` while `MemoryEntry` and `CompressionStats` have it — inconsistent API surface within a 151-line file.
```

### Perspective 3 — Architecture Compliance
```
ISSUE|src/core/mod.rs:3-7|medium|All 5 re-exported types have zero consumers in the codebase (verified via `rg '\b(CapturedContext|CompressionStats|ContentEncoding|MediaAttachment|MemoryEntry)\b'`); the only reference is `src/lib.rs:271-273` re-exporting them "for backward compatibility" — dead seam hides intent.
ISSUE|src/lib.rs:270-273|medium|Misleading doc "for backward compatibility" for a module whose types have no current consumer; signals architectural rot rather than a stable API contract.
ISSUE|src/core/types.rs:93|medium|R10 leaky abstraction: doc comment `/// Captured context from active application (Swift → Rust)` ties the "core" type to a specific bridge direction — core should be language/bridge-agnostic.
ISSUE|src/core/types.rs:57-91|low|R3 borderline: `serde` + per-field `data: String` payloads push wire-format responsibilities into core; consider whether multimodal payload types belong in a bridge-specific module.
```

### Perspective 4 — Code Quality
```
ISSUE|src/core/types.rs:137-150|low|Test coverage is asymmetric: only `MemoryEntry::default()` is exercised while `CapturedContext`, `MediaAttachment`, `CompressionStats` — none of which have consumers — have zero tests.
ISSUE|src/core/mod.rs:3-7|low|Re-exports are split across 5 lines instead of a single `pub use types::{...}` group, and the group is missing `MediaType` — inconsistent and easy to drift further.
ISSUE|src/core/types.rs:27-55|low|Duplicated `Display` impl boilerplate for two enums (`MediaType`, `ContentEncoding`); identical 8-line shape repeated, fragile when variants are added.
ISSUE|src/core/types.rs:14-25|low|`pub enum MediaType` lacks `#[non_exhaustive]`, making future variant additions a breaking change for any downstream deserializer — relevant once the dead seam is revived.
ISSUE|src/core/types.rs:64-135|low|No `#[serde(deny_unknown_fields)]` on any struct — silently accepts unknown keys, weakening wire-format forward-compatibility for what is supposed to be a public core API.
```