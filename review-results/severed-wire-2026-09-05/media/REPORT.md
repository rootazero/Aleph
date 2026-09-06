# Severed-Wire Audit: `src/media`

**Date:** 2026-09-05
**Scope:** Aleph core library — `src/media/` (15 files, 4,734 LoC)
**Method:** PRODUCED–CONSUMED symbol parity via `rg` across `src/`, `bin/`, `interfaces/`, `shared/`, `desktop/`, `tests/`. Read-before-write triage per the severity/triage decision tree in the prompt.

---

## Module overview

`src/media` is the multimodal pipeline layer. It owns:

- **Type taxonomy** (`types.rs`) — `MediaType` enum + format sub-enums
- **Detection** (`detect.rs`) — magic-byte + extension matching
- **Policy** (`policy.rs`) — size and duration caps per category
- **Provider trait** (`provider.rs`) — pluggable `MediaProvider`
- **Orchestration** (`pipeline.rs`) — priority-based provider fallback with SSRF + size gating
- **Concrete providers** (`processors/{image,audio,document}.rs`) — bridge existing `VisionPipeline` / `TranscriptionService` / direct I/O onto the trait
- **Cache** (`cache.rs`) — resolves inbound `Attachment`s to local files with TOCTOU-safe open + base64 encoding for LLM injection
- **Audio transcription** (`transcription.rs`, `whisper.rs`, `resolve.rs`) — `TranscriptionService` trait + OpenAI-Whisper implementation + the canonical "which backend is configured" resolver
- **Processor** (`processor.rs`) — `MediaProcessor` is the unified attachment-to-`ContentBlock` converter wired into `ExecutionEngine`
- **Errors** (`error.rs`) — `MediaError`

The headline flow documented in `mod.rs` is intact: attachments → `MediaProcessor.process()` → `ContentBlock` (Image for vision-capable, VisionPipeline description otherwise, audio transcription, plain-text summary for everything else). Every link in that chain has a live consumer (see "Live wires" below). The findings here are about inert re-exports, never-matched error variants, format-detection arms with no provider behind them, and a pub-on-the-edge `MediaPolicy` that no config layer feeds.

---

## Live wires (CONFIRMED — already connected)

The 6 "producer–consumer" pairs that ARE wired, with the rg evidence:

| Symbol | Producer (location) | Consumer (location) |
|---|---|---|
| `MediaProcessor::new` / `process` / `cleanup_stale` | `src/media/processor.rs:44,76,90` | `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1284-1286`, `src/gateway/execution_engine/run_loop/inner.rs:555`, `tests/multimodal_probe.rs:55,77,107,...` |
| `MediaCache::resolve` / `to_base64` | `src/media/cache.rs:88,162` | `MediaProcessor::process_image` (`src/media/processor.rs:166-170`); also `gateway/execution_engine/run_loop/inner.rs:1614` (one-shot use) |
| `MediaCache::download_media_item` | `src/media/cache.rs:264` | `src/tools/scoped/artifact_harvest.rs:351` |
| `MediaCache::url_only_attachment` | `src/media/cache.rs:572` | `src/tools/scoped/artifact_harvest.rs:380` |
| `MediaCache::cleanup_session` | `src/media/cache.rs:209` | `artifact_harvest.rs:246,645,875,1112`, `gateway/reply_emitter/emitter/helpers.rs:266`, plus MediaProcessor (`processor.rs:83`) |
| `MediaCache::decode_data_url` (`pub(crate)`) | `src/media/cache.rs:419` | `src/builtin_tools/media_send.rs:169` |
| `MediaCache::safe_local_media_path` (`pub(crate)`) | `src/media/cache.rs:536` | `src/builtin_tools/media_send.rs:176` |
| `crate::media::cache::MAX_FILE_SIZE` | `src/media/cache.rs:25` | `src/artifacts/store.rs:14,20` (aliased to `MAX_ARTIFACT_BYTES`) |
| `MediaPipeline::new` / `add_provider` / `process` | `src/media/pipeline.rs:21,30,42` | `src/executor/builtin_registry/builder/constructor/mod.rs:389-405`, `src/builtin_tools/media_tools/{extract,transcribe,understand}.rs` (tool struct constructors) |
| `ImageMediaProvider::new` | `src/media/processors/image.rs:22` | `builtin_registry/constructor/mod.rs:391`, `builtin_tools/media_tools/understand.rs:266` (tests) |
| `TextDocumentProvider` (unit) | `src/media/processors/document.rs:15` | `builtin_registry/constructor/mod.rs:395`, `builtin_tools/media_tools/extract.rs:151` (tests) |
| `AudioMediaProvider::new` | `src/media/processors/audio.rs:31` | `builtin_registry/constructor/mod.rs:401` (only when transcription resolves) |
| `transcription_service` + `ResolvedTranscription` | `src/media/resolve.rs:42,30` | `agent_init/mod.rs:1236` (MediaProcessor wiring) + `builtin_registry/constructor/mod.rs:1463` (MediaPipeline wiring) — both sides consume the same answer, so attachment and tool paths can never disagree |
| `WhisperTranscription::new` | `src/media/whisper.rs:38` | `crate::media::resolve::transcription_service:109` (sole live caller) |
| `TranscriptionService` (trait) | `src/media/transcription.rs:54` | `WhisperTranscription` (`whisper.rs:129`), `LocalTranscription` (`gateway/voice/local_provider.rs:60`), `MediaProcessor` (`processor.rs:147`), `AudioMediaProvider` (`processors/audio.rs:75`), tests |
| `TranscriptionConfigError` | `src/media/transcription.rs:32` | Surfaced from `transcription_service`; matched in `resolve.rs` tests |
| `LocalTranscription` | `src/gateway/voice/local_provider.rs:22` | `crate::media::resolve::transcription_service:101` (for `provider_type == LOCAL_PROVIDER_TYPE`) |
| `MediaInput::FilePath` / `Url` / `Base64` | `src/media/types.rs:96` | All three used by `MediaPipeline` (size gate + ssrf gate), `ImageMediaProvider` (all three), `TextDocumentProvider` (all three), `AudioMediaProvider` (FilePath only); tool wrappers (`builtin_tools/media_tools/{extract,transcribe,understand}.rs`) construct these |
| `MediaOutput::Text` / `Description` | `src/media/types.rs:127` | `MediaProcessor::process_image` matches both; `ImageMediaProvider` returns `Description`; `AudioMediaProvider` and `TextDocumentProvider` return `Text`; `builtin_tools/media_tools/{extract,transcribe}.rs:130` match `Text` |
| `MediaType::Image { .. }` | `src/media/types.rs:67` | Routed via `ImageMediaProvider` in builtin_registry constructor |
| `MediaType::Audio { .. }` | `src/media/types.rs:73` | Routed via `AudioMediaProvider` for `audio_transcribe` tool |
| `MediaType::Document { .. }` (Txt/Md/Html) | `src/media/types.rs:85` | Routed via `TextDocumentProvider` for `document_extract` tool |
| `MediaType::Unknown` | `src/media/types.rs:89` | Produced by `detect_by_extension` unknown arm + `MediaUnderstandTool` unknown-format branch (`builtin_tools/media_tools/understand.rs:177,178,184,185,202`) |
| `MediaPolicy::default` / `check_size` | `src/media/policy.rs:70,86` | `MediaPipeline::new` (`pipeline.rs:26`), `MediaPipeline::process` (`pipeline.rs:68,96`) — **internal-only** (see finding 1) |
| `crate::media::detect::{detect_by_extension, detect_from_path, detect_by_magic}` | `src/media/detect.rs:9,242,98` | `builtin_tools/media_tools/{extract,transcribe,understand}.rs`, `src/media/mod.rs:99` (test only) |
| `CachedMedia` (struct) | `src/media/cache.rs:53` | `MediaProcessor` (`processor.rs:29`), `WhisperTranscription` (`whisper.rs:14`), `LocalTranscription` (`gateway/voice/local_provider.rs:18`), `AudioMediaProvider` (`processors/audio.rs:15`), tests |
| `CacheError` + variants `Refused` / `TooLarge` / `Io` / `Download` | `src/media/cache.rs:79` | All produced internally; consumers (`artifact_harvest.rs`, `media_send.rs`, `processor.rs`) handle via `?` or `unwrap_err()`; `Refused` variant is the new failure path for `safe_local_media_path` refusals (was previously a silent warn! — see `media_send.rs` docstring) |

The producer-side payload of this audit is that **every active entry point — the four media tools (`media_understand`, `audio_transcribe`, `document_extract`, `media_send`) plus `MediaProcessor` for inbound attachments — has a live, registered dispatch and a live provider/validator behind it.** The severed-wire audit is therefore not "this tool was never wired" (the recent commit `src/executor/builtin_registry/builder/constructor/mod.rs:373-405` explicitly fixed exactly that class of bug for `MediaPipeline`); the failures here are smaller and located in the inert-detection and unused-export space.

---

## Findings

### sw-media-1 — `MediaPolicy` is pub-on-the-edge with no operator config behind it (medium)

- **Files:** `src/media/policy.rs:11-49`, `src/lib.rs:234`, `src/media/pipeline.rs:17,26`
- **Severity:** medium — inert config: defined+parsed-able, but no config binder reads it
- **Form:** 3 (inert config) — the operator cannot change the policy; the bytes-only default always wins

**Evidence — no external constructor or reader:**
```
$ rg -n "MediaPolicy" src/ bin/ --type rust | rg -v "src/media/"
src/lib.rs:234:    MediaPolicy, MediaProvider, MediaType, VideoFormat,

$ rg -n "MediaPolicy::default|MediaPolicy\s*\{" --type rust
src/media/policy.rs:11:pub struct MediaPolicy {
src/media/policy.rs:70:impl Default for MediaPolicy {
src/media/policy.rs:84:impl MediaPolicy {
... (10x in tests) ...
src/media/pipeline.rs:26:            policy: MediaPolicy::default(),

$ rg -n "media_policy|max_image_bytes|max_audio_bytes|max_video_bytes|max_document_bytes|max_unknown_bytes" src/ --type rust
src/media/policy.rs:... (only the source-of-truth definitions + pipeline.rs read)
```

The struct is `pub`, every field is `pub` and serde-tagged with a `#[serde(default = "...")]` helper, and `MediaPolicy` is re-exported at the crate root (`src/lib.rs:234`). So an operator could in principle write a config file containing `[media] policy = { max_image_bytes = 52428800 }`. But:

- Nothing in `src/config/` parses a `MediaPolicy`.
- `MediaPipeline::new` always constructs `MediaPolicy::default()` (`pipeline.rs:26`).
- `MediaPolicy::check_size` is only called from inside the pipeline (`pipeline.rs:68, 96`) — never from external code.

So the entire `pub` surface of `MediaPolicy` (constructor, all 7 fields, the `check_size` method) is invisible to operators. The defaults — 20 MB image / 100 MB audio / 500 MB video / 50 MB document / 100 MB unknown — are effectively compile-time constants. The non-zero-cost part: any future config wiring would silently have to share those exact field names, because once an operator has filled them in once, renaming them is a breaking change. The cheapest cure is one of:

1. **Decision needed:** is operator-configurable media policy a product requirement? If not, hide the struct (drop `pub` fields, remove the `#[serde]` tags, remove the `pub use` in `lib.rs`).
2. **Connect:** wire `app_config.media` (or a new `AppConfig::media_policy` field) into `MediaPipeline::with_policy(...)` and pass it from the server-startup builder.

I do not have authority to decide which is right; the audit calls it `DECIDE` because deleting the surface would foreclose a reasonable future requirement, and wiring it without an operator request adds configuration surface that nothing reads.

**Decision:** DECIDE
**Risk:** low either way; the choice is purely architectural. Currently inert, not buggy.
**Verification:** grep returns no `MediaPolicy` reader outside `src/media/`. If DECIDE→CONNECT, the missing wire is `builtin_registry/builder/constructor/mod.rs` (or `agent_init`) reading `app_config.media_policy` and passing it through a new `MediaPipeline::with_policy()` constructor.

---

### sw-media-2 — `MediaError::DetectionFailed` is a dead variant (low)

- **Files:** `src/media/error.rs:26`, `src/media/detect.rs:259`
- **Severity:** low — pure dead code
- **Form:** 1 (producer with zero callers)

**Evidence:**
```
$ rg -n "DetectionFailed" --type rust
src/media/detect.rs:259:        .unwrap_or(Err(MediaError::DetectionFailed(format!(
src/media/error.rs:26:    DetectionFailed(String),
```

The variant is constructed in exactly one place: `detect_from_path` when neither magic-byte detection nor extension detection succeeds (`detect.rs:259`). No code path matches on `MediaError::DetectionFailed` — every consumer wraps the error via `format!("Format detection failed: {e}")` (`builtin_tools/media_tools/{extract,transcribe}.rs`) which uses `Display`, never `match`.

The format-detection logic still produces a useful error message; the variant just isn't load-bearing for matching. Either:

1. Remove the variant from `MediaError` and have `detect_from_path` return `Err(MediaError::UnsupportedFormat(...))` instead (the existing message format "Cannot determine media type for: ..." is more specific, but `UnsupportedFormat` already exists for unknown extensions).
2. Keep it as a future-proof distinction — the constructor call is rare and cheap.

**Decision:** DECIDE (low) — keep if anyone intends to use the distinction in operator UI later, otherwise CUT.
**Risk:** removing it makes the public error enum smaller; the constructor site collapses to `UnsupportedFormat` and `?` continues to work.
**Verification:** every test in `policy.rs` covers `MediaType::Unknown` paths but not the `DetectionFailed` arm. No caller distinguishes.

---

### sw-media-3 — `MediaType::Video` detection has no provider behind it (medium)

- **Files:** `src/media/types.rs:79`, `src/media/detect.rs:54,230`, `src/media/policy.rs:112-130`
- **Severity:** medium — inert-but-meaningful surface; the policy code is real but unreachable in production
- **Form:** 2 (stub far-end — except the detection arm and policy arm ARE written, the missing piece is the provider)

**Evidence — detection + policy implemented, no provider:**
```
$ rg -n "MediaType::Video" src/media/processors/ src/media/pipeline.rs
src/media/processors/image.rs:53:                        // Any other category (Audio, Video, Document,
(no provider)

$ rg -n "VideoFormat::Mp4|VideoFormat::WebM|VideoFormat::Mov|VideoFormat::WebM" src/media/
src/media/detect.rs:56-58  (detect_video_extension — maps extensions to MediaType::Video)
src/media/detect.rs:213-235 (detect_video_magic + detect_ftyp — maps ftyp boxes to MediaType::Video)
src/media/policy.rs:112-130 (size + duration policy for MediaType::Video)
(no provider)
```

`detect_by_extension("mp4")`, `detect_by_magic(<ftyp box>)`, and `MediaPolicy::check_size` for `MediaType::Video { duration_secs, .. }` are all fully implemented. But no `MediaProvider::supported_types()` ever returns `MediaType::Video`:

```
$ rg -nA2 "fn supported_types" src/media/processors/
src/media/processors/image.rs:85-89  → returns MediaType::Image only
src/media/processors/document.rs:27-40 → returns MediaType::Document only
src/media/processors/audio.rs:45-58 → returns MediaType::Audio only
(no file returns MediaType::Video)
```

End-to-end consequence: an attachment with `mime: video/mp4` reaches `MediaProcessor::process_one` (`processor.rs:104-118`) — none of the three branches (`image/`, `audio/`, `else`) matches `video/*`, so it falls into the `else` branch and the user gets `[Attachment: <name> (video/mp4, …)]` — a placeholder text, **not** a transcription or description. If the model then calls `media_understand` on a video file, the pipeline returns `MediaError::NoProvider { media_type: "video" }` (`pipeline.rs:107-117`). The policy arm for video (500 MB + 30-min duration caps) is correct but never reached for a video attachment through the `process` call.

**Decision:** DECIDE
**Rationale:** video understanding is a meaningful product gap, but adding a provider is a significant chunk of work (frame extraction → multimodal LLM, or scene-level captions). The detection code is harmless and the policy is correct if/when a provider arrives. Delete the detection arm only if the product decision is "no video", otherwise leave it as scaffolding for the next iteration.
**Risk:** deleting `MediaType::Video` + `VideoFormat` + `detect_video_*` is ~80 lines of working code that would have to be reimplemented. Keeping it: dead arm in three call sites.
**Verification:** any video attachment currently flows through the `process_one` "else" placeholder path (`processor.rs:110-118`). To prove the gap, attach an mp4 in tests and assert `MediaProcessor::process` produces a `[Attachment: …]` text block (which is exactly what `test_process_unsupported_mime` shows).

---

### sw-media-4 — `MediaType::Document::Pdf/Docx/Xlsx` have detection but no provider (medium)

- **Files:** `src/media/types.rs:59-65`, `src/media/detect.rs:69-74,141-156`, `src/media/processors/document.rs:27-55`
- **Severity:** medium — inert detection arm
- **Form:** 2 (stub far-end — detection runs, no provider handles)

**Evidence:**
```
$ rg -n "DocFormat::Pdf|DocFormat::Docx|DocFormat::Xlsx|DocFormat::Txt|DocFormat::Markdown|DocFormat::Html" src/media/processors/document.rs
src/media/processors/document.rs:27-40  supported_types returns only Txt, Markdown, Html
src/media/processors/document.rs:48-55  supports() returns false for Pdf/Docx/Xlsx
src/media/processors/document.rs:150-168  tests assert Pdf/Docx unsupported

$ rg -n "pdf|docx|xlsx" src/media/processors/document.rs
(no provider for these)
```

The `document_extract` tool's description (`builtin_tools/media_tools/extract.rs:81`) advertises: `"Supports: txt, md, html (native), pdf, docx, xlsx (via plugins)."` There are no plugins registered; the description is a forward-looking claim. End-to-end behavior: `document_extract` on a `.pdf` returns `Document extraction failed: No media provider available for document`. `MediaProcessor::process_one` for a `application/pdf` attachment falls into the same `[Attachment: report.pdf (application/pdf)]` placeholder.

This is the same shape as finding 3 but for documents. Same call: detection is harmless, deletion is fine only if the product explicitly drops PDF/DOCX/XLSX extraction.

**Decision:** DECIDE
**Rationale:** PDF extraction in particular is a high-value addition (the LLM can answer "summarize the invoice" once it can read PDFs). The document provider's module-level docstring (`processors/document.rs:1-5`) explicitly defers to "plugins (P4)" — i.e. the gap is acknowledged and scheduled. Delete detection only if the deferral is canceled.
**Risk:** deletion costs ~40 lines; reimplementation is non-trivial (PDF parsing, DOCX ZIP-extraction).
**Verification:** same as finding 3 — send a PDF to `document_extract` and observe `MediaError::NoProvider { media_type: "document" }`.

---

### sw-media-5 — `MediaImageFormat::Gif/Svg/Heic` detected but never processed (low)

- **Files:** `src/media/types.rs:21`, `src/media/detect.rs:31-33,127-135,221-227`, `src/media/processors/image.rs:29-44`
- **Severity:** low — partially inert, but each format has a documented reason
- **Form:** 2 (stub far-end)

**Evidence:**
```
$ rg -n "MediaImageFormat::Gif|MediaImageFormat::Svg|MediaImageFormat::Heic" src/media/
src/media/detect.rs:31-33  → detect_image_extension maps .gif/.svg/.heic
src/media/detect.rs:127-135 → detect_image_magic detects GIF magic
src/media/detect.rs:221-227 → detect_ftyp detects HEIC brands
src/media/processors/image.rs:35-44 → to_vision_format returns None for Gif/Svg/Heic
src/media/processors/image.rs:236 → test asserts Gif returns None
src/media/processor.rs:461-465 → image_format_from_mime returns None for svg/heic
```

These three formats have detection logic and at least one of them (SVG, HEIC) is **deliberately** excluded in `image_format_from_mime` with an explanatory comment (`processor.rs:464-465`: "SVG and HEIC are not raster formats — vision APIs typically cannot process them directly"). GIF is technically raster but lost during conversion to a vision-API-compatible format.

Concretely:
- An attached `.gif` reaches `MediaProcessor::process_image`, falls into `describe_image_fallback`, and gets `media_summary("Image", …, Some("format not supported for description"))` (`processor.rs:244-253`).
- A `.svg` / `.heic` attachment goes through the same path.
- A `media_understand` call on these formats returns `MediaError::UnsupportedFormat` (raised by `ImageMediaProvider::convert_input` at `processors/image.rs:50`).

GIF is the most user-facing of the three (animated GIFs as inputs to chat). This is documented behavior, not a bug.

**Decision:** DECIDE
**Rationale:** the detection is fine (avoids "unknown media" if/when a provider is added); the unsupported-format error in the processor is informative. CUT only if the product decides "we don't accept GIF/SVG/HEIC", which would mean deleting three enum variants + three detection branches (~25 lines). Keep otherwise.
**Risk:** low. Either path is non-breaking.
**Verification:** the path is reachable end-to-end via `test_process_unsupported_mime`-style test using `image/gif` mime.

---

### sw-media-6 — Inert re-exports of `AudioFormat` / `DocFormat` / `MediaImageFormat` / `VideoFormat` (low)

- **Files:** `src/lib.rs:233-234`
- **Severity:** low — inert-but-meaningful public surface
- **Form:** 3 (defined+parsed-able, but external consumers never touch them)

**Evidence — declared, never matched outside the module:**
```
$ rg -n "AudioFormat::|DocFormat::|MediaImageFormat::|VideoFormat::" --type rust | rg -v "src/media/"
(no output)

$ rg -n "AudioFormat|DocFormat|MediaImageFormat|VideoFormat" --type rust | rg -v "src/media/" | rg -v "//"
src/lib.rs:233:    AudioFormat, DocFormat, MediaError, MediaImageFormat, MediaInput, MediaOutput, MediaPipeline,
src/lib.rs:234:    MediaPolicy, MediaProvider, MediaType, VideoFormat,
```

These four format enums are exported at the crate root but used exclusively inside `src/media/`. External code receives them only as fields of `MediaType` (e.g. `MediaType::Image { format: MediaImageFormat::Png }` flows through serde JSON in tool output and inbound attachments), so the variants must remain serializable. The enum *types* themselves don't need to be re-exported because no external crate pattern-matches on `MediaImageFormat::Png` — they `serde_json::from_value` the `MediaType` and treat it opaquely.

This is not technically dead — the types are reachable through `MediaType` — but the re-export at lib.rs adds to the public surface without any external caller. Removing the re-exports would not break anything except a downstream crate that imports `alephcore::AudioFormat` (none observed).

**Decision:** DECIDE
**Rationale:** low-cost either way. Keep if any future external crate is anticipated; cut if the goal is to keep lib.rs minimal. The audit notes that the binary `tests/multimodal_probe.rs` and the local `crates` in the workspace do not import these directly.
**Verification:** `rg -n "use alephcore::AudioFormat|use alephcore::DocFormat|use alephcore::MediaImageFormat|use alephcore::VideoFormat"` returns nothing in this worktree.

---

### sw-media-7 — `AudioMediaProvider` rejects `MediaInput::Url` and `MediaInput::Base64` — deliberate but undocumented in tool docs (low / DECIDE)

- **Files:** `src/media/processors/audio.rs:64-72`, `src/builtin_tools/media_tools/transcribe.rs:48`
- **Severity:** low — minor contract asymmetry, not a bug
- **Form:** 5 (partial: provider works for FilePath only, while the tool's advertised surface only feeds FilePath so the wire is intact)

**Evidence:**
```
$ rg -n "MediaInput::Url|MediaInput::Base64" src/media/processors/audio.rs
(no — audio.rs only matches MediaInput::FilePath at line 67; Url/Base64 fall through to the error arm at line 70-74)

$ rg -n "MediaInput::Url|MediaInput::Base64" src/builtin_tools/media_tools/transcribe.rs
src/builtin_tools/media_tools/transcribe.rs:111:        let input = MediaInput::FilePath { path };  (only FilePath constructed)
```

The `audio_transcribe` tool only accepts `file_path` (`transcribe.rs:48`), so it always builds `MediaInput::FilePath`. The `AudioMediaProvider`'s rejection of `Url` and `Base64` is therefore unreachable in practice. But `MediaPipeline` itself is advertised (in `mod.rs:13-23`) as accepting all three input variants, and `ImageMediaProvider` / `TextDocumentProvider` do accept all three. So this asymmetry is real for any future caller that goes through `MediaPipeline` directly with an audio `Url` or `Base64`.

This is **not** a severed wire — the `audio_transcribe` tool's call site is connected. It's a contract note: `AudioMediaProvider` is FilePath-only because transcription backends read the file twice (once via `tokio::fs::read` in `WhisperTranscription::transcribe:139` and once via `local_provider::transcribe`), and the cache's resolution layer isn't on the `AudioMediaProvider` path. Connecting URL/Base64 would require routing through `MediaCache::resolve` first.

**Decision:** DECIDE
**Rationale:** the asymmetry is documented in the test `rejects_non_file_input` (`processors/audio.rs:189-203`) which calls it out. Closing the gap is a product decision (do we want `audio_transcribe` to accept URL/data-URL audio?). If yes, route through `MediaCache`; if no, document the limitation in the tool description.
**Risk:** routing through `MediaCache` adds a 50 MB cap and SSRF gate that the current `AudioMediaProvider` doesn't apply — that's an improvement, not a regression, but it changes the failure surface.
**Verification:** `rg -n "AudioMediaProvider\|MediaInput::Url" src/media/processors/audio.rs` traces the `Err(...)` arm.

---

### sw-media-8 — `MediaCache::cleanup_session` error path is silently dropped in `artifact_harvest.rs:645, 875, 1112` (low)

- **Files:** `src/tools/scoped/artifact_harvest.rs:645, 875, 1112`
- **Severity:** low — silent best-effort cleanup; no data loss
- **Form:** 4 (consumer swallows the error path)

**Evidence:**
```
$ rg -n "MediaCache::cleanup_session" src/
src/media/cache.rs:209:    pub fn cleanup_session(session_id: &str) -> Result<(), CacheError>
src/media/processor.rs:83:        if let Err(e) = MediaCache::cleanup_session(session_id) {
src/tools/scoped/artifact_harvest.rs:246:        if let Err(e) = MediaCache::cleanup_session(&media_session) {
src/tools/scoped/artifact_harvest.rs:645:        let _ = MediaCache::cleanup_session(&session);
src/tools/scoped/artifact_harvest.rs:875:        let _ = MediaCache::cleanup_session("run-resolved");
src/tools/scoped/artifact_harvest.rs:1112:        let _ = MediaCache::cleanup_session(session);
src/gateway/reply_emitter/emitter/helpers.rs:266:        if let Err(e) = crate::media::cache::MediaCache::cleanup_session(&self.run_id) {
```

Two of the five callers (`artifact_harvest.rs:246` and `processor.rs:83` plus `reply_emitter/helpers.rs:266`) propagate or log the error. Three callers (`artifact_harvest.rs:645, 875, 1112`) `let _ =` it. Given the function returns `Result<(), CacheError>` where the `Err` arm is `Io` (couldn't `remove_dir_all`), silent best-effort cleanup on a temp directory is the standard pattern — but if the cleanup ever fails persistently (e.g. a holding process or a permission bit set by another account on a shared host), there is no log line. The defensive `private_temp_root` migration (finding in `cache.rs::base_dir` docstring) already mitigates the cross-account case, so this is essentially defense-in-depth.

**Decision:** DECIDE
**Rationale:** keep silent is acceptable for fire-and-forget cleanup; the `cleanup_stale` sweep on next startup catches anything leaked. The non-silent call sites log the failure already. Adding `tracing::warn!` on the `let _ =` lines is a one-line patch that brings symmetry, but it's not load-bearing.
**Verification:** `rg -n "let _ = MediaCache::cleanup_session" src/` finds exactly the three sites above.

---

## Summary table

| ID | Form | Severity | Decision | Headline |
|---|---|---|---|---|
| sw-media-1 | 3 | medium | DECIDE | `MediaPolicy` re-exported but no operator config feeds it |
| sw-media-2 | 1 | low | DECIDE | `MediaError::DetectionFailed` defined and constructed, never matched |
| sw-media-3 | 2 | medium | DECIDE | `MediaType::Video` detection + policy with no provider |
| sw-media-4 | 2 | medium | DECIDE | `DocFormat::Pdf/Docx/Xlsx` detection with no provider (plugins deferred) |
| sw-media-5 | 2 | low | DECIDE | `MediaImageFormat::Gif/Svg/Heic` detected but vision provider skips them |
| sw-media-6 | 3 | low | DECIDE | `AudioFormat` / `DocFormat` / `MediaImageFormat` / `VideoFormat` exported at lib.rs but never used outside `src/media/` |
| sw-media-7 | 5 | low | DECIDE | `AudioMediaProvider` is FilePath-only (intentional, undocumented) |
| sw-media-8 | 4 | low | DECIDE | `MediaCache::cleanup_session` errors silently swallowed in 3 sites |

**Totals:** 8 findings. 0 CUT, 0 CONNECT, 8 DECIDE.

The 0-CUT tally is intentional — every "cut" candidate here is a forward-looking scaffolding (video, PDF/DOCX, configurable policy) that the surrounding comments explicitly defer. Cutting them forecloses future product decisions; the cheapest audit action is to surface the gaps and let the operator / product owner pick.

---

## Negative findings (explicitly checked, NOT severed)

These were checked and are NOT severed wires — included so a follow-up audit doesn't re-investigate:

- `MediaProcessor` ↔ `ExecutionEngine` (`run_loop/inner.rs:555`) — wired in `agent_init/mod.rs:1284-1289`.
- `MediaCache::download_media_item` ↔ `artifact_harvest.rs:351` — wired, error path is surfaced to caller (no silent fallback per the `cache.rs:264-273` docstring contract).
- `MediaCache::decode_data_url` / `safe_local_media_path` ↔ `media_send.rs:169, 176` — wired through `pub(crate)` (deliberate, see `cache.rs:418-419` docstring).
- `MediaCache::MAX_FILE_SIZE` ↔ `artifacts/store.rs:14` — wired as `MAX_ARTIFACT_BYTES`.
- `crate::media::resolve::transcription_service` ↔ `agent_init/mod.rs:1236` (MediaProcessor) AND `builtin_registry/constructor/mod.rs:1463` (MediaPipeline) — both consumers hit the same function so they can never disagree.
- `WhisperTranscription` ↔ `crate::media::resolve::transcription_service:109` — sole caller, deliberately so (the constructor's panic-rejection was converted to `Result` per `whisper.rs:54-62`).
- `LocalTranscription` (gateway/voice) ↔ `crate::media::resolve::transcription_service:101` — wired for `provider_type == LOCAL_PROVIDER_TYPE`.
- `ImageMediaProvider` / `TextDocumentProvider` / `AudioMediaProvider` ↔ `builtin_registry/constructor/mod.rs:389-405` — wired.
- `MediaPipeline` ↔ `MediaUnderstandTool` / `AudioTranscribeTool` / `DocumentExtractTool` — wired through `definitions.rs:1186-1205` and `tool_registry_impl.rs:1611-1630`.
- `CacheError::Refused` — wired, replaces the previous silent `warn!` failure mode (see `media_send.rs:121-128` docstring).
- `MediaProcessor::cleanup_stale` ↔ `agent_init/mod.rs:1286` — wired at server startup.

---

## Files audited

`src/media/mod.rs` (165), `src/media/cache.rs` (1095), `src/media/detect.rs` (513), `src/media/error.rs` (27), `src/media/pipeline.rs` (357), `src/media/policy.rs` (270), `src/media/processor.rs` (726), `src/media/provider.rs` (150), `src/media/resolve.rs` (235), `src/media/transcription.rs` (59), `src/media/types.rs` (208), `src/media/whisper.rs` (247), `src/media/processors/audio.rs` (204), `src/media/processors/document.rs` (230), `src/media/processors/image.rs` (239). Total 4,734 LoC.

Cross-module scan covered `src/`, `src/bin/`, `src/executor/`, `src/gateway/`, `src/builtin_tools/`, `src/tools/`, `src/artifacts/`, `tests/`, `interfaces/`, `shared/`, `desktop/` — every `crate::media::*`, `alephcore::media::*`, `use crate::media::`, `use alephcore::media::` reference was checked for live-vs-test classification.
