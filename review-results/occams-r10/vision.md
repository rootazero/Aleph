# Module: src/vision (occams-r10 review, 2026-10-02)

## Summary

- **Files reviewed**: 6 (error.rs 28L, mod.rs 144L, provider.rs 30L, types.rs 138L, providers/mod.rs 6L, providers/platform_ocr.rs 450L) ≈ **796 LOC**
- **Total findings**: **0 critical / 0 warning / 1 informational / 0 suggested test**
- **R1 (Brain-Limb Separation)**: **PASS** — `src/vision/` imports only `aleph_desktop::{ScreenCapability, DesktopPlatform, OcrResult, NativeScreen}` traits; no `cocoa|appkit|coregraphics|objc2|metal|windows|x11|wayland` imports.
- **R3**: **PASS** — only workspace deps: `async-trait`, `base64`, `schemars`, `serde`, `thiserror`, `tokio`. No new heavy deps.
- **R7 (One core, many shells)**: **PASS** — single `PlatformOcrProvider` defers platform-specific OCR to `desktop/shared` `NativeScreen` / `aleph_desktop` traits; macOS routes through the Swift bridge per documented behavior.
- **R10 (YAGNI)**: clean.
- **r9 carry-over status** (from `vision-2026-08-29.md`):
  - [High] Base64 input no pre-decode size bound (DoS via allocation) → **FIXED** (pre-decode bound at `platform_ocr.rs:100`, `MAX_BASE64_ENCODED_SIZE` const at `types.rs:24`)
  - [Medium] `ImageInput::FilePath` reads whole file before cap → **FIXED** (metadata-first at `platform_ocr.rs:148-152`, defence-in-depth re-check post-read at `:169-173`)
  - [Medium] `ImageInput::Base64` PNG label trusted without magic-byte verification → **FIXED** (`PNG_MAGIC` const + sniff at `platform_ocr.rs:119-131` for Base64 arm and `:179-184` for FilePath arm)
  - [Info] Provider-error semantics (rate-limit vs auth-fail) → **DEFERRED** (r9 marked dormant until a 2nd provider lands; still dormant — only `PlatformOcrProvider` wired)
  - [Info] Duplicate provider registration → **DEFERRED** (callers register once at startup; same as r9)
  - [Info] `mod.rs` docstring doesn't mention wiring → **DEFERRED**
  - [Info] `PlatformOcrProvider::with_screen` is dead code (no test uses it) → **DEFERRED** (still dead — confirmed via grep)
- **NEW findings (r10)**: 0 critical / 0 warning / **1 informational observation**

## Critical

(none)

## Warning

(none)

## Suggested Test

(none — the new tests added with the r9 carry-over fixes are sufficient; see `platform_ocr.rs` test module lines ~360-460)

## Per-perspective findings

### Security

- **Three-cap defence-in-depth** for image-size DoS:
  - `Base64` arm: pre-decode `data.len()` cap (`platform_ocr.rs:100`) → post-decode `decoded.len()` cap (`:111`)
  - `FilePath` arm: pre-read `metadata.len()` cap (`:154`) → post-read `bytes.len()` cap (`:169`)
  - `MAX_BASE64_ENCODED_SIZE` formula: `((N/3)+1)*4` — sound upper bound (1-byte slack) for the standard base64 alphabet without line breaks (documented at `types.rs:9-23`).
- **Format-vs-magic-byte mismatch** rejected at boundary (`:128` for Base64, `:184` for FilePath) — mislabeled JPEG-as-PNG can no longer silently slip through.
- **No `unwrap`/`expect` in production paths** (verified via `grep -n '\.unwrap()\|\.expect(' src/vision/{mod.rs,types.rs,provider.rs,error.rs,providers/platform_ocr.rs}` — every match is inside `#[cfg(test)]` or `mod tests`).
- **No `unsafe`** anywhere in `src/vision/`.
- **Provider chain treats all errors as transient** (`mod.rs:54-58, 99-103`) — first-success-wins, last-error-surfaces. Adequate for current single-provider setup; the deferred r9 finding on rate-limit vs auth-fail distinction is acknowledged.

### Logic

- **Provider fallback chain** (`mod.rs:33-117`): correctness verified by tests
  - `no_provider_supports_capability_returns_unsupported` — capability-gate loop short-circuits with `UnsupportedCapability`
  - `skips_providers_without_capability` — capable provider not masked by earlier no-op skip
  - `all_providers_fail_returns_last_error` — last-error-surfacing
  - `empty_pipeline_returns_no_provider` — distinct from `UnsupportedCapability`
- **Distinct error variants for distinct failure modes** (`error.rs`): `NoProvider` (empty pipeline) / `UnsupportedCapability` (capability gate) / `ProviderError` (all capable failed) / `ImageError` (decode/format/size). Three of these used in pipeline; `ImageError` is `PlatformOcrProvider`-internal.
- **`ImageFormat` parse-don't-validate**: enum is the boundary, `#[non_exhaustive]` allows future formats without API break.
- **Confidence bounds**: `OcrLine.confidence: Option<f64>` is exposed in the result type but `PlatformOcrProvider::convert_platform_ocr_result` discards it (TODO comment at `platform_ocr.rs:181`). Acceptable per R10 (YAGNI); deferred.
- **Test coverage** (12+ tests): empty pipeline, single-success, fallback on failure, all-fail, capability-gating, skip-without-capability, all/none/default caps, serde round-trip, and the 4 size-cap / magic-byte tests for the r9 fix.

### Architecture (R1-R10)

- **R1 verified clean**. `src/vision/` imports only `crate::vision::*` and `aleph_desktop::{ScreenCapability, DesktopPlatform, OcrResult, NativeScreen}`. The `NativeScreen` is the shared `desktop/shared` cross-platform impl; macOS-specific / Linux-specific / Windows-specific implementations live in `desktop-macos`, `desktop-linux`, `desktop-windows` and are reached via the bridge JSON-RPC IPC. **No** platform-API imports. Verified via `grep -rn "cocoa|appkit|coregraphics|objc2|metal|windows|x11|wayland" src/vision/` → 0 hits.
- **R3 verified clean**. No new heavy deps.
- **R4 verified clean**. Vision is a leaf capability provider; no business logic.
- **R5/R8/R9/R10**: not applicable.
- **Trait ergonomics**: `VisionProvider` has 4 methods (`understand_image`, `ocr`, `capabilities`, `name`) — minimal, async-trait, `Send + Sync`. Docstrings name the orchestrator so readers can find it.
- **`with_screen` constructor** (`platform_ocr.rs:54`) is the natural injection seam for tests; currently unused (no test in this file uses it, and `builtin_tools/desktop/native.rs:2695-2811` uses a hand-rolled `FixedOcrProvider` instead). Documented "for testing"; deferred (same as r9).

### Code Quality

- **`mod.rs` module-level docstring**: 3 lines, covers the trait and the pipeline. Could mention the production wiring and size cap, but not blocking. Deferred (r9 marked).
- **`provider.rs`**: clean trait.
- **`types.rs`**: exposes only what's needed. `MAX_IMAGE_FILE_SIZE` is `pub` so other modules can reference the cap consistently. `MAX_BASE64_ENCODED_SIZE` carries a 4-line comment explaining the formula and the integer-division rationale.
- **`error.rs`**: thiserror with `#[non_exhaustive]` on the enum. `Clone` derived (needed for `Send + Sync` across `Arc<dyn VisionProvider>` boundaries). Reasonable.
- **Error messages** consistent in shape: `"<thing> exceeds maximum size of {} MB"`, `"<thing> does not start with PNG magic signature"`, `"base64 image exceeds maximum encoded size of {} KB"`.

## Informational observation (no action)

### I-1 [quality] `ImageInput::Url` variant is uninstantiable — no path in `src/vision/` constructs or matches on it
- **Location**: `src/vision/types.rs:46-48` (`ImageInput::Url { url: String }`)
- **Evidence**: `grep -rn 'ImageInput::Url' src/vision/` returns only the type definition itself; no constructor, no match arm in `mod.rs` pipeline, no arm in `platform_ocr.rs::resolve_png_bytes`. The pipeline's `understand_image` and `ocr` pass `&ImageInput` to the provider unchanged; the only provider (`PlatformOcrProvider`) has no URL handling. The variant exists in the public schema (`#[non_exhaustive]`) but no code path can construct it server-side.
- **Why this is informational, not a finding**: `ImageInput` is the boundary type — `#[non_exhaustive]` is the standard escape hatch for "future variants". The URL variant likely exists as a placeholder for a future remote-image provider (multimodal LLM that fetches URLs itself). Per R10, adding the variant now is cheap; not adding the consumer yet is correct.
- **Suggested fix if acted on**: either (a) remove the variant (breaks any consumer that has `JsonSchema` reflected on it) or (b) add a `match` arm in `resolve_png_bytes` that returns `VisionError::UnsupportedCapability("remote_url")` (deferred until a real consumer lands).

## Conclusion

### Net delta from r9

- **What holds**: All 3 r9 high-confidence findings (Base64 pre-decode bound, FilePath metadata-first, PNG magic byte) **FIXED** with proper tests. The carry-over comment chains (constant definition, docstrings referencing the rationale) make the fixes greppable.
- **What changed**: None material. `MAX_BASE64_ENCODED_SIZE` is a new constant (`types.rs:24`) added as part of the r9 fix; the conservative integer formula `((N/3)+1)*4` is sound and matches the r9 rationale.
- **What didn't regress**: R1/R3/R4/R7 all still PASS. No new critical or warning findings.

### Fix order proposal (this module only)

**No fixes required.** Module is **shape-up**.

(If the dead-`Url`-variant observation is acted on in a future round, it would be a 5-minute change — either delete the variant or add a `match` arm that returns `UnsupportedCapability`. Out of scope for r10.)

---

**Module verdict: shape-up.** All r9 carry-over items resolved; r10 surfaces zero new findings worth fixing.
