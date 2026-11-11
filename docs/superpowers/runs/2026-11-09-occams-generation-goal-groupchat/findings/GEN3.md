# Static Occam's Razor Review: Batch GEN3 (src/generation/providers video & polling)

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-gen-goal-gc`  
**Subsystems Reviewed**:
- `src/generation/providers/google_veo/*` (5 files)
- `src/generation/providers/midjourney/*` (5 files)
- `src/generation/providers/replicate/*` (8 files)
- `src/generation/providers/suno/*` (3 files)
**Total Files**: 21 | **Total LOC**: 4,555

---

## 1. Executive Summary & Findings Table

| ID | File:Line | Category | Severity / Priority | One-Line Summary |
|---|---|---|---|---|
| **GEN3-01** | `replicate/builder.rs:7`<br>`replicate/provider.rs:5` | Cyclic Coupling / Complexity | High / FIX-NOW | 2-file circular dependency (`builder.rs <-> provider.rs`) due to cross-file builder extraction. |
| **GEN3-02** | `midjourney/submit_polling.rs:18-39`<br>`midjourney/provider.rs:17` | Accidental Indirection | Medium / FIX-NOW | `trait SubmitPolling` has exactly one implementor; creates redundant abstraction & boilerplate. |
| **GEN3-03** | `midjourney/provider.rs:206` | Redundant Allocation | High / FIX-NOW | Full media payload cloned (`bytes.clone()`) solely to call `.len()` 9 lines later. |
| **GEN3-04** | `replicate/provider.rs:196` | Redundant Allocation | High / FIX-NOW | Full media payload cloned (`bytes.clone()`) solely to call `.len()` 8 lines later. |
| **GEN3-05** | `replicate/prediction.rs:99`<br>`midjourney/submit_polling.rs:137`<br>`suno/mod.rs:160` | Inefficient Polling Loop | Medium / FIX-NOW | Uncached `format!("Bearer ...")` allocated on every single iteration inside async polling loops (up to 300x). |
| **GEN3-06** | `midjourney/provider.rs:228-238` | Excessive Serde Allocation | Low / FIX-CAREFULLY | Intermediate `Vec<String>` and double cloning of button labels when writing metadata actions. |
| **GEN3-07** | `replicate/provider.rs:164-192` | Redundant Allocation | Low / FIX-CAREFULLY | Output URL converted to owned `String` instead of borrowed `&str` when dispatched to `fetch_output`. |
| **GEN3-08** | `suno/mod.rs:358-372`<br>`suno/types.rs:89-94` | Redundant Allocation / Clone | Low / FIX-CAREFULLY | Alternative clips JSON clones all string fields; `SunoError::best_message(&self)` clones on owned error. |
| **GEN3-09** | `google_veo/provider.rs:119`<br>`replicate/builder.rs:52,72` | URL Malformation Risk | Medium / FIX-NOW | Missing trailing slash trim on custom endpoints produces malformed double-slash URLs (`//`). |
| **GEN3-10** | `replicate/prediction.rs:86` | Defensive Inconsistency | Medium / FIX-NOW | Upstream `prediction_id` spliced directly into URL path without validation (Shape 16 relapse). |
| **GEN3-11** | `google_veo/provider.rs:237,270,283`<br>`google_veo/helpers.rs:9-31` | Redundant Logic / Divergence | Low / FIX-CAREFULLY | `helpers.rs` validation functions bypassed in favor of duplicated raw slice checks in `provider.rs`. |

---

## 2. Detailed Findings

### GEN3-01: Circular Module Coupling & Accidental Complexity in `replicate/` (`builder.rs` <-> `provider.rs`)
- **File:Line**: `replicate/builder.rs:7`, `replicate/provider.rs:5`
- **Issue**: Mutual file-level circular import and cross-module field exposure.
- **Proposed Fix**: Move `ReplicateProviderBuilder` into `replicate/provider.rs`, re-export or keep facade, eliminate cyclic dependency.

### GEN3-02: Unnecessary Trait Indirection in `midjourney/submit_polling.rs`
- **File:Line**: `midjourney/submit_polling.rs:18-39`, `midjourney/provider.rs:17`
- **Issue**: `trait SubmitPolling` has exactly 1 implementor, adds boilerplate async trait return signatures.
- **Proposed Fix**: Make `submit_imagine`, `poll_task`, `download_image` inherent methods on `MidjourneyProvider`.

### GEN3-03: Redundant Multi-Megabyte Payload Cloning in `midjourney/provider.rs`
- **File:Line**: `midjourney/provider.rs:205-216`
- **Issue**: `let data = GenerationData::bytes(bytes.clone())` clones multi-megabyte image buffer solely to read `bytes.len()` later.
- **Proposed Fix**: Extract `let size_bytes = bytes.len() as u64;` first, then move `bytes` into `GenerationData::bytes(bytes)`.

### GEN3-04: Redundant Multi-Megabyte Payload Cloning in `replicate/provider.rs`
- **File:Line**: `replicate/provider.rs:194-205`
- **Issue**: `let data = GenerationData::bytes(bytes.clone())` clones media bytes solely to read `bytes.len()` later.
- **Proposed Fix**: Extract `let size_bytes = bytes.len() as u64;` first, then move `bytes` into `GenerationData::bytes(bytes)`.

### GEN3-05: Repeated Authorization Header Allocation Inside Async Polling Loops
- **File:Line**: `replicate/prediction.rs:99`, `midjourney/submit_polling.rs:137`, `suno/mod.rs:160`
- **Issue**: `format!("Bearer ...")` allocated on every single iteration inside polling loops (up to 300 times).
- **Proposed Fix**: Hoist `let auth_header = format!("Bearer ...");` outside the polling loop and pass `&auth_header`.

### GEN3-06: Double Allocation and Cloning in `midjourney` Task Buttons Metadata
- **File:Line**: `midjourney/provider.rs:228-238`
- **Issue**: Intermediate `Vec<String>` and double cloning of button labels into JSON values.
- **Proposed Fix**: Consume `task.buttons.into_iter()` directly.

### GEN3-07: Unnecessary String Allocation for `output_url` in `replicate` Provider
- **File:Line**: `replicate/provider.rs:164-192`
- **Issue**: Allocates a heap `String` for the output URL even though `fetch_output` takes `&str`.
- **Proposed Fix**: Borrow `&str` directly from `output_value` and pass to `fetch_output`.

### GEN3-08: Redundant String Cloning in `suno` Alternative Clips Serialization & Error Handling
- **File:Line**: `suno/mod.rs:358-372`, `suno/types.rs:89-94`
- **Issue**: Clones all clip strings when building JSON; `best_message(&self)` clones on owned error.
- **Proposed Fix**: Move clip strings via `into_iter()`; take ownership in `parse_error`.

### GEN3-09: Missing Trailing Slash Normalization in `google_veo` and `replicate`
- **File:Line**: `google_veo/provider.rs:119`, `replicate/builder.rs:52, 72`
- **Issue**: Missing trailing slash trim creates double slashes `//v1/...`.
- **Proposed Fix**: Trim trailing slashes on custom endpoints.

### GEN3-10: Unchecked Upstream Prediction ID in `replicate` Polling URL Construction
- **File:Line**: `replicate/prediction.rs:86`
- **Issue**: Upstream `prediction_id` spliced directly into URL path without validation.
- **Proposed Fix**: Validate character set and length before formatting URL.

### GEN3-11: Inconsistent Duration and Aspect Ratio Helper Usage in `google_veo`
- **File:Line**: `google_veo/helpers.rs:9-31`, `google_veo/provider.rs:237, 270, 283`
- **Issue**: Inconsistent direct slice checks vs `helpers.rs` functions.
- **Proposed Fix**: Use `is_valid_aspect_ratio` and `is_valid_veo3_duration`.
