# Occam's Razor Static Review: Batch GEN1 (`src/generation` core & types)

## Summary of Findings

| ID | File:Lines | Severity / Category | Summary |
|---|---|---|---|
| **GEN1-01** | `src/generation/registry.rs:375-389` | **High** / Parity Bug | `registry.remove()` ignores `canonical_index`: removing by canonical alias silently fails and returns `None` despite `contains()` and `get()` returning `true`. |
| **GEN1-02** | `src/generation/probe.rs:39-58,91-135,139-167` | **High** / Reliability Bug | `probe_generation_provider` lacks case/whitespace normalization; testing `"ElevenLabs"` or `"Cartesia"` falls back to `Bearer` auth resulting in false `401 Unauthorized` rejections. |
| **GEN1-03** | `src/generation/types/progress.rs:28-37` | **Medium** / Panic Risk | `f32::clamp` preserves `NaN`: uninitialized or 0/0 progress calculations propagate `NaN`, causing runtime failure when serialized to JSON via `serde_json`. |
| **GEN1-04** | `src/generation/probe.rs:68-73` | **Low** / Redundant Allocations | `probe_url` allocates 2 heap `String`s via `.map(str::to_string)` before borrowing `&str` into `resolve_base_url`. |
| **GEN1-05** | `src/generation/registry.rs:304-310,337-346,457-467` | **Medium** / Efficiency & Dispatch | Filtering calls `p.supported_types().contains(&gen_type)` allocating a `Vec` for every provider on every check instead of using `p.supports(gen_type)`; lacks short-circuiting existence check. |
| **GEN1-06** | `src/generation/error.rs:677-743` | **Medium** / Error Context Loss | `From<GenerationError> for AlephError` silently drops `resets_at`, `parameter`, `category`, `job_id`, and `url` fields, losing diagnostic fidelity across conversion boundary. |
| **GEN1-07** | `src/generation/probe.rs:124`, `src/generation/registry.rs:120-123` | **Low** / Accidental Complexity | Redundant `.unwrap_or(401)` on already-matched `Some(401 | 403)`, and redundant `.clone()` on newly allocated `String` in `register()`. |

---

## Detailed Findings

### GEN1-01: Parity Violation in `GenerationProviderRegistry::remove` for Canonical Aliases
- **File & Line**: `src/generation/registry.rs:375-389`
- **Issue**: `remove` only removes from `self.providers`, leaving alias and failing if called with canonical alias.
- **Proposed Fix**: Resolve through `canonical_index` if not in `self.providers`, clean up index.

### GEN1-02: Auth Header Mismatch and False Positives in `probe_generation_provider`
- **File & Line**: `src/generation/probe.rs:39-58, 67-79, 91-135, 139-167`
- **Issue**: Casing mismatch causes non-Bearer providers like ElevenLabs / Cartesia to fail probe.
- **Proposed Fix**: Normalize `provider_type.trim().to_ascii_lowercase()` at entry of probe.

### GEN1-03: `NaN` Propagation and JSON Serialization Failure in `GenerationProgress::new`
- **File & Line**: `src/generation/types/progress.rs:28-37`
- **Issue**: `NaN.clamp` returns `NaN`, crashing serde_json serialization.
- **Proposed Fix**: Guard with `if percentage.is_nan() { 0.0 }`.

### GEN1-04: Redundant Heap Allocations in `probe_url`
- **File & Line**: `src/generation/probe.rs:68-79`
- **Issue**: 2 heap `String` allocations before URL resolution.
- **Proposed Fix**: Keep `raw` as `&str`.

### GEN1-05: Redundant Vector Allocations and Dynamic Dispatch Smells in Registry Filtering
- **File & Line**: `src/generation/registry.rs:304-310, 337-346, 457-467`
- **Issue**: Uses `supported_types().contains(&t)` which allocates a `Vec` for every provider.
- **Proposed Fix**: Use `p.supports(gen_type)` and add `has_provider_for_type`.

### GEN1-06: Loss of Diagnostic Context in `From<GenerationError> for AlephError`
- **File & Line**: `src/generation/error.rs:677-743`
- **Issue**: Drops `resets_at`, `parameter`, `category`, `job_id`, `url`.
- **Proposed Fix**: Preserve fields into formatted error strings.

### GEN1-07: Redundant `.unwrap_or(401)` and Unnecessary String Allocation
- **File & Line**: `src/generation/probe.rs:124`, `src/generation/registry.rs:120-123`, `src/generation/types/params.rs:187-190`
- **Issue**: `.unwrap_or` on matched `Some`, redundant clone in `register()`, manual loop instead of `extend`.
- **Proposed Fix**: Direct binding, move string, and `extra.extend`.
