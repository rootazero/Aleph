# src/media — occams-r5 Review
## Summary
- **15 findings: 0 crit / 2 high / 5 med / 8 low**
- Files reviewed (line counts):

| File | Lines | Notes |
|------|-------|-------|
| cache.rs | 1121 | |
| processor.rs | 746 | |
| detect.rs | 513 | |
| pipeline.rs | 357 | |
| policy.rs | 336 | |
| processors/document.rs | 270 | |
| whisper.rs | 277 | |
| providers/image.rs | 240 | |
| types.rs | 208 | |
| resolve.rs | 235 | |
| providers/audio.rs | 216 | |
| provider.rs | 150 | |
| mod.rs | 166 | |
| error.rs | 39 | |
| transcription.rs | 59 | |
| processors/mod.rs | 9 | |

## Findings

### [high] `ImageMediaProvider::supported_types()` lies about its capabilities
- **Location**: `processors/image.rs:85-87`
- **Category**: API contract / misleading declaration

`supported_types()` hardcodes `vec![MediaType::Image { format: MediaImageFormat::Png }]` but the `convert_input()` impl (lines 48-68) handles **all** `MediaImageFormat` variants — PNG, Jpeg, WebP, Gif, Svg, Heic — with `to_vision_format()` translating PNG/Jpeg/WebP and returning `UnsupportedFormat` for the rest. A caller introspecting `supported_types()` gets an inaccurate answer. The pipeline's `supports()` uses the `category()` shortcut so routing is unaffected, but the Vec itself is dead code for its declared purpose.

**Risk**: If a future caller uses `supported_types()` to build a UI, configure routing, or count capabilities, it under-reports. The `#[allow(excessive_clone)]` lines 43-46 on the `convert_input` clones are justified (trait signature owns inputs), but the broader mismatch between declared and actual support surface is not self-documenting.

**Suggested fix**: Expand `supported_types()` to include `MediaImageFormat::Jpeg` and `MediaImageFormat::WebP`, the two non-PNG formats `to_vision_format()` can actually handle; document that Gif/Svg/Heic fall through to the error path.

---

### [high] `detect_document_magic()` cannot distinguish DOCX from XLSX/PPTX/JAR — but always says `DocFormat::Docx`
- **Location**: `detect.rs:164-170`
- **Category**: False positive / misclassification

The comment acknowledges the limitation, but the return is unconditionally `DocFormat::Docx`. A ZIP-based file (XLSX, PPTX, JAR, EPUB) that arrives via `_media` tool will be misclassified as a Word document. `pipeline.rs` routes by media type, so an XLSX would be offered to `TextDocumentProvider`, which rejects it (`Docx` is not in its `supported_types` list). A user uploading an XLSX gets `NoProvider` — opaque unless the error path surfaces the magic-byte classification.

**Risk**: Medium — mis-routed files produce a `NoProvider` error rather than silently degrading, so the failure is visible. But the error message won't say "this looks like a spreadsheet, not a document."

**Suggested fix**: Either (a) return `MediaType::Unknown` for PKZIP when distinguishing is impossible and let callers handle the fallback, or (b) add a second-tier heuristic (e.g., peek at `[Content_Types].xml` or ZIP entry names) to distinguish XLSX/PPTX from DOCX. Option (a) is simpler; the doc comment already flags the problem.

---

### [med] `#[allow(excessive_clone)]` used 16× — most in `download_media_item` building `Attachment` structs
- **Location**: `cache.rs:355-587` (11 occurrences), `processors/image.rs:43-66` (3), `cache.rs:583-586` (2)
- **Category**: Clone hygiene / code smell

Every `rust-doctor-disable-next-line excessive-clone` in `download_media_item` is on a field of a locally-constructed `Attachment` that immediately passes into `self.resolve()` then out as `CachedMedia`. The clones are necessary because the function signature takes `&MediaItem` but `Attachment` owns the fields. However, the frequency — 11 in one function — suggests the data flow could be restructured: `download_media_item` could accept or construct the `Attachment` at the call site instead of building and immediately consuming it.

The `image.rs` clones (lines 43-66) are on `MediaInput` → `ImageInput` conversions where the trait signature demands owned inputs; those are unavoidable.

**Suggested fix**: For `download_media_item`: consider accepting `item: MediaItem` (owned) and building `Attachment` via `Attachment::from(item)` or a `From<MediaItem>` impl. For `image.rs`: unavoidable given the trait. No behavior change.

---

### [med] `detect_from_path` is async but does a single `read_exact` — no I/O concurrency benefit
- **Location**: `detect.rs:242-259`
- **Category**: Unjustified async

`detect_from_path` is the only async fn in `detect.rs`. It opens a file, reads 16 bytes, and returns. No concurrent I/O, no awaits inside a loop, no `.join`, no blocking calls. The async wrapper adds stack + poll overhead for zero gain.

**Suggested fix**: Make it `fn detect_from_path(path: &Path) -> Result<MediaType, MediaError>` using `std::fs::File` directly. Move the tokio-using test (line 264-271) to a `#[tokio::test]` or separate the async test from the sync impl.

---

### [med] `pipeline.rs:process()` unwinds all providers on error — last error wins; no early bail on unrecoverable failures
- **Location**: `pipeline.rs:120-148`
- **Category**: Error semantics / design smell

The loop tries every eligible provider and surfaces all failures in a joined message only if `attempts.len() > 1`. If the first provider fails with a permanent error (e.g., `UnsupportedFormat`) and the second would also fail, the final error is the second provider's message — potentially misleading. A `NoProvider` from a non-existent format routes to `TextDocumentProvider` which rejects it, and the error becomes `TextDocumentProvider` rather than `UnsupportedFormat`.

**Risk**: Low — the code is documented and the fallback pattern is intentional. But a provider that returns a permanent error should ideally short-circuit.

**Suggested fix**: Distinguish permanent (`UnsupportedFormat`, `Refused`) vs. transient errors and short-circuit on permanent ones. Add a `is_recoverable()` helper on `MediaError`.

---

### [med] `cache.rs:write_private()` flushes on every write but `to_base64` does not — inconsistent durability
- **Location**: `cache.rs:667-692` vs `cache.rs:212-228`
- **Category**: Inconsistency / durability

`write_private` calls `file.flush().await` after `write_all` (line 692). `to_base64` opens the file read-only with no flush and uses `read_to_end`. If a crash occurs between `write_private` completing and a subsequent `to_base64` read, the file should be consistent — but `flush()` only flushes user-space buffers; `fsync` is needed for durability guarantees. On a power loss, `flush()` alone may not guarantee the data is on disk.

**Risk**: Low — this is a temp file cache; durability of temp files is not a hard requirement.

**Suggested fix**: Either (a) accept the current behavior as intentional (temp files, no fsync needed), or (b) add `file.sync_all().await` after flush for explicit durability if this is ever used for less-ephemeral data.

---

### [low] `process_one` in `processor.rs` calls `mime.to_ascii_lowercase()` then `starts_with("image/")` — O(n) per attachment
- **Location**: `processor.rs:117-134`
- **Category**: Micro-optimization / negligible

`to_ascii_lowercase()` allocates a `String` for every attachment to check one prefix. `mime.starts_with("image/")` is ASCII-safe without lowercasing; the MIME type registry (IANA) uses lowercase for all primary types. The `starts_with` check already works correctly for any casing. However, the code may be defensive against caller-supplied MIME strings with mixed case.

**Risk**: Negligible — per-attachment allocation, but attachments are typically few. The defensive lowercasing is arguably correct.

**Suggested fix**: No change required. Accept as defensive. If performance matters, use `str::eq_ignore_ascii_case` instead of lowercasing.

---

### [low] `pipeline.rs` has no `Default` for `MediaPipeline` — but `pipeline.rs:19` has `impl Default`
- **Location**: `pipeline.rs:19-21` + `pipeline.rs:145-148`
- **Category**: Inconsistency / naming

`MediaPipeline` has both `fn new()` and `impl Default`, which are functionally equivalent. `new()` creates an empty pipeline; `default()` does the same. Tests use `new()`. Having both is redundant.

**Suggested fix**: Drop `impl Default` and use `new()` everywhere, or drop `new()` in favor of `Default`.

---

### [low] `whisper.rs:MAX_AUDIO_BYTES = 25 MB` is half of `cache.rs:MAX_FILE_SIZE = 50 MB` — no explanation of the ratio
- **Location**: `whisper.rs:131` vs `cache.rs:29`
- **Category**: Magic number / undocumented relationship

The Whisper API enforces a 25 MB limit; the cache enforces 50 MB. A file between 25-50 MB will pass the cache, reach the transcription provider, and be rejected with `anyhow::bail`. This is defensible (different limits for different stages), but the 2:1 ratio is uncommented.

**Suggested fix**: Add a comment explaining why 25 MB vs 50 MB, e.g. `// Whisper API limit is 25 MB; cache allows 50 MB so other processors (vision, document) can handle larger files.`

---

### [low] `MediaCache::safe_local_media_path` canonicalizes `temp_dir()` on every call — could be cached
- **Location**: `cache.rs:528-531`
- **Category**: Repeated I/O / minor perf

`safe_local_media_path` calls `tokio::fs::canonicalize(std::env::temp_dir())` on every invocation (line 530). The temp directory doesn't change at runtime. On a system with many media items, this is N redundant syscalls.

**Suggested fix**: Compute once at startup or use `LazyLock`/`OnceCell` to cache the canonical temp path.

---

### [low] `expand_tilde` has a subtle bug: `path == "~"` arm does NOT check `components()`
- **Location**: `cache.rs:741-750`
- **Category**: Subtle logic gap

`expand_tilde` handles `~/...` (checks components for safety) and `~` (just expands to home) separately. The `~/...` branch validates that `rest` has no `..` or `Component::ParentDir` before joining. The `~` arm does not call `components()` at all. But `~` alone is trivially safe — there are no components to validate. This is correct but asymmetric; a future reader might incorrectly add component validation to the `~` arm.

**Risk**: Negligible. Correct as written.

**Suggested fix**: Add a comment: `// "~" alone is always safe — no path components to validate.`

---

### [low] `provider.rs` `MockProvider` test impl doesn't override `priority()` — default is 100
- **Location**: `provider.rs:109-119`
- **Category**: Test hygiene

The test `MockProvider` in `provider.rs` doesn't override `priority()`, so it uses the trait default (100). The test `provider_default_priority` (line 123-128) tests a separate `DefaultPrio` struct rather than `MockProvider`. This is fine but the separation is not obviously motivated.

**Suggested fix**: No change needed. Accept as deliberate separation of concerns.

---

### [low] `transcription.rs` is 59 lines — trivially small, no issues
- **Category**: N/A

---

### [low] `rust-doctor-disable excessive-clone` in `image.rs:66` is on `data.clone()` — the only unavoidable one
- **Location**: `processors/image.rs:66`
- **Category**: Clone hygiene

The `Base64` arm of `convert_input` clones `data` into `ImageInput::Base64 { data: data.clone(), format }`. The clone is required because `ImageInput` owns `data` and `convert_input` takes `&MediaInput`. The `#[allow]` is justified.

---

### [low] `media_summary` constructs `String` with nested `Option` unwrapping — readable but verbose
- **Location**: `processor.rs:412-430`
- **Category**: Readability / style

The `media_summary` function uses `.or_else(|| ...)` chains and `match note` with string concatenation. This is readable and well-commented but could use `format_args!` or a small helper. Not worth changing.

---

## Cross-cutting

- **Security posture**: Strong. `O_NOFOLLOW` applied consistently across `cache.rs`, `whisper.rs`, `document.rs`. MED-03/04/05/06/07/08 are all addressed and regression-tested. No lock-across-await patterns found.
- **Policy**: `MediaPolicy` is config-driven via serde with sensible defaults. `const fn` defaults prevent accidental drift. `debug_assert` in `new()` pins the `max_unknown_bytes <= max_image_bytes` invariant.
- **Error taxonomy**: `MediaError` is clean — 7 variants, all named after failure mode not component. `CacheError` is separate with policy/refusal distinction correctly separating "cannot read" from "will not read."
- **Test coverage**: High. Every security fix has a regression test. `pipeline.rs` tests priority ordering, fallback, empty-pipeline, and SSRF gating. `cache.rs` has ~15 integration tests covering inline/path/URL/data-url/error paths.
- **`#[allow]` inventory**: 3 total — `clippy::too_many_arguments` (policy, justified), `clippy::cast_precision_loss` (processor display only, justified). Zero unconditional allows.

## Out of scope

- `src/media/` public API surface vs. callers outside `src/` (reviewed callers for context only)
- `graphify-out/2026-09-30/GRAPH_REPORT.md` caller graph — not present at time of review
- `src/gateway/media.rs`, `src/gateway/execution_engine/`, `src/builtin_tools/media_send.rs`, `src/tools/scoped/artifact_harvest.rs` — outside `src/media/` boundary
- Behavioral changes to `MediaProcessor` / `MediaPipeline` ownership and caller lifecycles
