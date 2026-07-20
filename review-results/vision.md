# Module: vision

## Summary
- Path: `src/vision/` (4 top-level `.rs` files + 3 in `src/vision/providers/`, ~1,579 lines total)
- Issues found: 0 high-confidence, 1 informational observation

## Reviewers
- Security / Logic / Architecture / Quality

## High-Confidence Issues
None.

## Per-perspective findings

### Security
- `claude.rs` uses `reqwest::Client::new()` (HTTP — fine for a network-bound provider).
- Image size enforcement: `MAX_IMAGE_FILE_SIZE` cap at lines `platform_ocr.rs:86, 95, 111`. Three separate guard points (raw bytes, decoded, base64-estimate) before any I/O — defense-in-depth.
- `validate_confidence` is the centralized confidence-range validator (used at tool-result boundaries).
- No `unwrap`/`expect` in production paths.

### Logic
- All `.unwrap()` calls verified to be inside `#[cfg(test)]` blocks.
- `VisionPipeline` falls through providers in registration order; first success wins, last error surfaces.
- `ImageFormat` enum + `validate_confidence` provide parse-don't-validate at boundaries.
- Empty pipeline returns `VisionError::NoProvider`; ability mismatch returns `VisionError::UnsupportedCapability` — distinct error types prevent merging coincidental failures.

### Architecture (R1-R10)
- **R1**: clean. `src/vision/providers/platform_ocr.rs` only references the `aleph_desktop::ScreenCapability` / `aleph_desktop::DesktopPlatform` traits and the `aleph_desktop::NativeScreen` struct — all from the `desktop/shared` crate. No `cocoa|appkit|coregraphics|objc2|metal` imports inside `src/`. The `desktop/shared` crate's own module-level docstring (`desktop/shared/src/lib.rs`) explicitly states *"Real platform API calls never live here: each platform crate (`desktop-macos`, `desktop-linux`, `desktop-windows`) implements `DesktopPlatform` and reaches the OS through the `bridge` JSON-RPC IPC layer (R1 brain–limb separation)."* — confirming R1's contract.
- **R3**: `reqwest` (already a workspace dep), `base64`, `image`, `serde`, `tokio`. No heavy deps introduced.
- **R4, R8, R9, R10**: vision is a leaf capability provider. No business logic. No LLM bypass — it forwards user prompts unchanged to upstream vision APIs.

### Quality
- Smallest module surface (`~1,579` lines across 7 files).
- Trait `VisionProvider` cleanly separates abstraction from concrete impl.
- `mod.rs` documents the fallback-chain behavior.
- `providers/mod.rs` lists both providers with their capabilities in one place.

## Informational observation (no action)

### Potential default-fallback subtle behavior
- **File**: `src/vision/providers/platform_ocr.rs:54`
- **Severity**: n/a (documented behavior)
- **Evidence**: `PlatformOcrProvider::new()` defaults to `aleph_desktop::NativeScreen::new()`. Per the source comments ("`NativeScreen`'s OCR is `NotImplemented` on macOS — macOS OCR is routed through the Swift bridge"), this default fallback is a graceful no-op (`VisionError::OcrNotAvailable` on macOS). The production registry should use `with_platform()` to get the bridge-backed screen. This is correctly documented in-line. Not an R1/R4 issue.

## Conclusion
`src/vision/` is clean and respects the R1 brain–limb separation (vision in core depends only on `aleph-desktop` *traits*, the platform implementations live in the platform-specific crates accessed via IPC bridge). No changes required.
