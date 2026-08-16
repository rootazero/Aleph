# Severed-Wire Audit — `src/vision`

- **Batch:** agents-batch-6
- **Module:** `src/vision` (6 files, 1038 LOC)
- **Date:** 2026-08-16
- **Reviewer:** static (severed-wire-audit skill)

## Result counts

| Severity | Count |
|----------|-------|
| critical | 0 |
| high     | 1 |
| medium   | 0 |
| low      | 4 |
| **total**| **5** |

| Decision | Count |
|----------|-------|
| CONNECT  | 1 |
| CUT      | 4 |
| DECIDE   | 0 |

The module is otherwise well-formed: no `// TODO`/`todo!`/`unimplemented!` stubs in
production code, and the registration seam for the desktop screenshot bridge is intact
(`constructor/mod.rs` builds a `VisionPipeline` and registers `PlatformOcrProvider`).

---

## Findings

### [HIGH] src/vision/providers/platform_ocr.rs:65 — `PlatformOcrProvider` is wired to the screenshot bridge but severed from `MediaProcessor`'s image-attachment fallback

**Category:** architecture
**Decision:** CONNECT
**Related:** `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1187`, `src/media/processor.rs:226`, `src/executor/builtin_registry/builder/constructor/mod.rs:337`, `src/gateway/execution_engine/run_loop/inner.rs:548`

**Description:** Both ends of the wire are fully implemented and tested; only the startup
connection is missing. The *producer* is `PlatformOcrProvider::with_platform` (here) plus
`VisionPipeline`, and it already runs in production for the desktop `screenshot
{describe:true}` bridge (`constructor/mod.rs:337`). The *consumer* is
`MediaProcessor::describe_image_fallback` (`media/processor.rs:226`), which — when given a
`Some(Arc<VisionPipeline>)` — runs `understand_image` on an attached image and emits
`[Image: <description>]` for text-only models. The wire is cut at server startup:
`agent_init/mod.rs:1187` hardcodes `let vision: Option<Arc<VisionPipeline>> = None;` with
the comment *"VisionPipeline is not currently created at startup — pass None for now"*.

Read-first triage confirmed a **live caller**: `run_loop/inner.rs:548` invokes
`media_processor.process(attachments, supports_vision=false, ...)` for every turn served by
a text-only model. That path reaches `describe_image_fallback`, sees `vision == None`, and
degrades to `[Image: name — not viewable by this model]` — the image content is silently
lost. No error path or newer mechanism covers "describe an attached image for a text-only
model", so this is a genuinely dark feature, not a painless severed wire.

**Suggested fix:** Build a `VisionPipeline` at startup (mirroring `constructor/mod.rs`):
`PlatformOcrProvider::with_platform(Arc::clone(&desktop_platform))`, wrap in `Arc`, and pass
`Some(Arc)` into `MediaProcessor::new(transcription, vision)` instead of `None`. This adds no
new coupling — `MediaProcessor` already accepts `Option<Arc<VisionPipeline>>`.

---

### [LOW] src/vision/types.rs:75 — `Rect` type and its methods are dead scaffolding

**Category:** quality
**Decision:** CUT
**Related:** `src/vision/mod.rs:13`, `src/vision/providers/platform_ocr.rs:176`

**Description:** `Rect` plus `new`/`new_unchecked`/`is_valid`/`area` are used only inside the
type's own unit tests (`mod.rs:397-428`). A `\bRect\b` sweep of `src/` finds no production
construction, field read, or schema consumer. Its natural consumer would be OCR bounding
boxes, but `convert_platform_ocr_result` (`platform_ocr.rs:176`) drops
`aleph_desktop::OcrResult.lines`, and the vision `OcrResult` has no `lines` field — so `Rect`
is orphaned. Being `pub` and re-exported, `dead_code` lints cannot flag it.

**Suggested fix:** Delete `Rect`, its impl block, and the `rect_validation` /
`rect_new_unchecked` tests; drop `Rect` from the `pub use` in `mod.rs`.

---

### [LOW] src/vision/error.rs:21 — `VisionError::UnsupportedFormat` variant is never constructed

**Category:** quality
**Decision:** CUT
**Related:** `src/vision/providers/platform_ocr.rs:114`

**Description:** Grepping `VisionError::UnsupportedFormat` returns only the definition. The
other `UnsupportedFormat` hits are unrelated enums (`MediaError`, `SoulLoadError`). Every
format/size failure in the vision module is reported as `ImageError` (e.g.
`platform_ocr.rs:114` reports an unsupported URL image as `ImageError`). The variant lives on
a `#[non_exhaustive]` `pub` enum, so it compiles clean and is invisible to `dead_code`.

**Suggested fix:** Remove the `UnsupportedFormat` variant (or, if format-specific errors are
actually wanted, route the URL/unsupported-format cases in `resolve_png_bytes` to it — but as
written it is dead).

---

### [LOW] src/vision/types.rs:50 — `ImageFormat::mime_type()` / `extension()` have no non-test caller

**Category:** quality
**Decision:** CUT
**Related:** `src/vision/types.rs:60`

**Description:** Grepping `.mime_type()` and `.extension()` finds no call site outside the
module's own unit test `image_format_mime_and_extension` (`mod.rs:435-440`). Production users
of `ImageFormat` (`media/processor.rs:460`, `media/processors/image.rs:29`) only map variants
to/from `ImageFormat` and never call these accessors. Both are `pub const` on a `pub` enum, so
`dead_code` cannot catch them.

**Suggested fix:** Delete `mime_type()` and `extension()` (and their unit test), unless a
downstream MIME-mapping consumer is planned.

---

### [LOW] src/vision/mod.rs:39 — `VisionPipeline::provider_count()` / `capabilities()` have no non-test caller

**Category:** quality
**Decision:** CUT
**Related:** `src/vision/mod.rs:129`

**Description:** `provider_count()` is used only in `provider_count` (`mod.rs:384-392`), and
the aggregate `VisionPipeline::capabilities()` only in `aggregated_capabilities`
(`mod.rs:361,378`). No production caller: `VisionBridge` and `ImageMediaProvider` call
`understand_image`/`ocr` directly and handle `Err`. The per-provider trait method
`VisionProvider::capabilities` *is* consumed internally by the pipeline loop (`mod.rs:62,103,132`),
so it is not in scope — only the two public aggregate accessors are dead. Both are `pub`, so
`dead_code` cannot flag them.

**Suggested fix:** Delete `provider_count()` and the aggregate `capabilities()` method (keep
the trait method), and their unit tests.

---

## What I did not do / remaining caveats

- **Not verified by compilation.** This is a read-only audit; the CUTs were not applied, so
  no `cargo test --no-run` pass confirmed the deletion is safe. Before deleting `Rect`, grep
  every exported symbol (`Rect`, `ImageFormat::mime_type`, `extension`, etc.) per the skill's
  delete-a-file rule.
- **`ImageInput::Url` is a semi-dark path.** `PlatformOcrProvider::resolve_png_bytes` rejects
  URL images (`platform_ocr.rs:114`), and the `ImageMediaProvider` converts `MediaInput::Url`
  → `ImageInput::Url`. A URL image reaching OCR therefore always errors. This is a
  completeness gap rather than a severed wire and was not counted.
- **Scope discipline.** The headline CONNECT's actual one-line fix lives outside `src/vision/`
  (in `agent_init/mod.rs`); it is anchored here on the producer side as required, with the
  consumer cross-referenced in `related`.
- **No `DECIDE` candidates** — none of the severed wires required new coupling or a product
  judgment; they were unambiguous CONNECT/CUT.
