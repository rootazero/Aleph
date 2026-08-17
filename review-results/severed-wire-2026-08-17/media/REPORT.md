# Severed-Wire Audit — `src/media`

- **Audit**: severed-wire-audit (PRODUCED–CONSUMED symbol parity)
- **Date**: 2026-08-17
- **Module**: `src/media` (16 files, 4242 LOC)
- **Tree**: `.worktrees/review-fix-2026-08-17`
- **Method**: `rg -n "<symbol>" src/ src/bin/ interfaces/ shared/`; read-all-files first; no cargo, no edits.
- **Result**: 3 findings (0 critical, 0 high, 2 medium, 1 low). Decisions: 3× DECIDE, 0× CUT, 0× CONNECT.

---

## Wiring summary (verified, not severed)

The module is **externally connected on both entry paths** — nothing in the
module's core spine is orphaned:

| Producer | Production consumers (path:line) |
|---|---|
| `MediaProcessor` (processor.rs:34) | `engine.rs:86,391-393` (field + `with_media_processor`); run_loop `inner.rs:495-548` (`process`), `inner.rs:1437,1450,1513,1530` (`cleanup`); `agent_init/mod.rs:1168-1216` (construction, `cleanup_stale` at 1211); integration test `tests/multimodal_probe.rs` |
| `MediaPipeline` (pipeline.rs:15) | `builtin_registry/builder/constructor/mod.rs:379-392` (builds pipeline with 3 providers); dispatch `tool_registry_impl.rs:1495-1510`; tools `media_tools/{understand,extract,transcribe}.rs` |
| `MediaProvider` trait (provider.rs:16) | implemented by `AudioMediaProvider`, `ImageMediaProvider`, `TextDocumentProvider`; all three registered at constructor/mod.rs:380-390 |
| `MediaCache` (cache.rs:68) | `processor.rs` (resolve/to_base64), `run_loop/inner.rs:1593` (`archive_inbound_attachments`), `media_send.rs:156,163`, `artifact_harvest.rs:337,351,380` |
| `detect_*` (detect.rs:7,85,220) | `media_tools/understand.rs:109,131,138`, `extract.rs:97`, `transcribe.rs:96`; `detect_by_magic` via `detect_from_path` (detect.rs:227) |
| `transcription_service` / `ResolvedTranscription` (resolve.rs:33,19) | `agent_init/mod.rs:1179`, `constructor/mod.rs:1426-1430` |
| `WhisperTranscription` (whisper.rs:23) | `resolve.rs:81` — reachable when `[generation]` config resolves a non-local transcription provider |
| `CachedMedia`/`CacheError` | `processor.rs`, `whisper.rs`, `processors/audio.rs:64`, `gateway/voice/local_provider.rs:18,63` |

**Question asked in the brief — `transcription.rs` (30 LOC) a stub?** No. It is
the trait contract + result type (`TranscriptionService`, `TranscriptionResult`),
consumed by `processor.rs:39` (field type), `resolve.rs`, `whisper.rs`,
`processors/audio.rs`, `gateway/voice/local_provider.rs`. Form-2 does not apply.

**Question asked — is the whisper path superseded by
`generation/providers/openai_whisper`?** No. `OpenAiWhisperProvider` is a
`GenerationProvider` serving explicit generation/RPC requests
(`generation/providers/openai_whisper/mod.rs:8-17` states the layering itself);
`media::whisper::WhisperTranscription` serves the attachment auto-transcription
path via `resolve.rs`. Both are wired; neither is dead.

---

## Findings

### sw-me-1 — `MediaPolicy` pub config surface is inert and unconsumable (form 6 / 5)

- **Severity**: medium
- **Form**: 6 (orphaned pub API: serde+JsonSchema surface re-exported, never configured) with form-5 drift (docs/serde imply config-readability nothing exercises)
- **Files**: `src/media/policy.rs:11-54,56-67`, `src/media/pipeline.rs:15-26`, `src/lib.rs:232`

`MediaPolicy` (policy.rs:11) exposes six `pub` serde fields
(`max_image_bytes` 14, `max_audio_bytes` 18, `max_video_duration` 22,
`max_video_bytes` 26, `max_document_bytes` 30, `max_document_pages` 34) each with
a `#[serde(default = …)]` default fn (37-54), plus `JsonSchema`. `check_size`
(policy.rs:71) itself **is** production-reachable (pipeline.rs:60, from the three
media tools' `FilePath` inputs), so the *logic* is live — only the
configuration surface is severed.

Evidence:
```
$ rg -n "MediaPolicy" src/ src/bin/ interfaces/ shared/
src/lib.rs:232:    MediaPolicy, MediaProvider, MediaType, VideoFormat,
src/media/policy.rs:11:pub struct MediaPolicy {
src/media/policy.rs:56:impl Default for MediaPolicy {
src/media/policy.rs:69:impl MediaPolicy {
...policy.rs tests (161-246: MediaPolicy::default())...
src/media/pipeline.rs:4:use super::policy::MediaPolicy;
src/media/pipeline.rs:17:    policy: MediaPolicy,
src/media/pipeline.rs:26:            policy: MediaPolicy::default(),
src/media/mod.rs:25://! - [`MediaPolicy`] — size and lifecycle enforcement
src/media/mod.rs:44:pub use policy::MediaPolicy;

$ rg -n "max_image_bytes|max_audio_bytes|max_video_duration|max_video_bytes|max_document_bytes|max_document_pages" src/ src/bin/ interfaces/ shared/ --glob '!src/media/policy.rs'
(no output — no reader outside policy.rs, and no config loader references the names)
```

No code anywhere reads these fields or constructs a non-default `MediaPolicy`:
`MediaPipeline::new()` (pipeline.rs:23-29) hardcodes `MediaPolicy::default()`
(26) and there is **no setter** on `MediaPipeline` (`add_provider` at 31 is the
only mutator). The serde/`JsonSchema` derivation promises a config story
("serialize/deserialize me") that has zero producers and zero consumers.

- **Decision**: DECIDE — public lib API re-export (lib.rs:232), external
  consumers (shells/plugins) can't be ruled out, so no concrete deletion.
- **Options**:
  1. CUT the config surface: make fields private, drop `serde`/`JsonSchema`/default fns, keep the in-memory defaults (removes the false config promise; smallest diff keeps `check_size` intact).
  2. CONNECT: add a real config knob (e.g., `[media]` section in config loader) + a `MediaPipeline::with_policy(...)` setter, then read it in `agent_init`/constructor — the knobs stop being inert.
  3. Leave as-is (documented default-only policy; deliberate).
- **Risk of change**: low for option 1 (no in-repo reader of the fields or the
  serde form; `JsonSchema` output for `MediaPolicy` is not part of any schema
  export — `export_desktop_bridge_schema` covers `aleph_protocol`, not alephcore).
- **Verification**: `rg -n "MediaPolicy" src/ src/bin/ interfaces/ shared/` →
  no production consumer outside policy.rs tests and pipeline.rs default use
  (see evidence above); after change, `rg -n "max_video_duration" .` returns
  only policy.rs.

### sw-me-2 — `VideoFormat` produced but never read; video capability dead-ends at `NoProvider` (form 1 / 5)

- **Severity**: medium
- **Form**: 1 (symbol with zero production readers) + 5 (tool/type surface describes video support that no longer resolves)
- **Files**: `src/media/types.rs:43-49` (`VideoFormat`), `src/media/types.rs:76-79` (`MediaType::Video`), `src/media/detect.rs:56-58,180-208,210-217` (video detection), `src/media/policy.rs:97` (sole production reader of the variant, ignores `format`), `src/builtin_tools/media_tools/understand.rs:84` (advertises video)

`VideoFormat::{Mp4,WebM,Mov}` and `MediaType::Video` are *produced* by
`detect_video_extension` (detect.rs:56-58), `detect_ftyp_magic` (180-208), and
`detect_video_magic` (210-217). No production code ever **reads** the
`VideoFormat` value, and no provider can process a video:

```
$ rg -n "VideoFormat" src/ src/bin/ interfaces/ shared/
src/lib.rs:232: (re-export)
src/media/detect.rs:56-58, 192, 203, 212   (construction only)
src/media/detect.rs:337,344,351,457        (tests)
src/media/policy.rs:202,212                (tests)
src/media/types.rs:43, 80, 152             (enum decl, field type, test)
src/media/mod.rs:49                        (re-export)

$ rg -n "MediaType::Video" src/ src/bin/ | rg -v "test|tests"
src/media/detect.rs:61,191,202,211          (construction)
src/media/policy.rs:97                       (MediaType::Video { duration_secs, .. } — format unread)
```

Routing trace: `media_understand` (understand.rs:84 DESCRIPTION: "images,
audio, **video**, documents") → `detect_from_path` → `MediaType::Video` →
`pipeline.process` (pipeline.rs:37) → eligible providers by category: the three
registered providers are image/document/audio only
(constructor/mod.rs:380-390) → `MediaError::NoProvider { media_type: "video" }`
→ tool error "Media processing failed: No media provider available for video".
So every video `media_understand` call in production is a guaranteed failure,
and the `VideoFormat` enum is dead data — produced by detection, read nowhere.

- **Decision**: DECIDE — the surface is intentional scaffolding (detection
  exists, tool advertises it), so no deletion without product judgment.
- **Options**:
  1. CONNECT: add a video provider (e.g., frame-extract → existing `VisionPipeline` / `ImageMediaProvider`) so `media_understand` video stops failing.
  2. CUT: drop video detection (`detect_video_extension`, `detect_ftyp_magic` video branch, `detect_video_magic`, `VideoFormat`, `MediaType::Video`) and scrub "video" from the tool description (understand.rs:84).
  3. Leave (forward-looking; each failure is self-describing).
- **Risk**: option 1 is additive; option 2 changes `detect_by_extension("mp4")`
  from `Ok(Video)` to `Err(UnsupportedFormat)` — the tool already maps that to a
  readable error, and no other code matches on `MediaType::Video`'s format data.
- **Verification**: `rg -n "VideoFormat" src/ src/bin/` (no production reader);
  `rg -n "MediaType::Video" src/ | rg -v test` (only detect construction +
  policy.rs:97 unread-format branch). After a fix, a video `media_understand`
  call either succeeds (CONNECT) or yields `UnsupportedFormat` (CUT).
- **Related observation (not a separate finding)**: `document_extract`
  (extract.rs:31 DESCRIPTION) advertises "pdf, docx, xlsx (via plugins)", but
  `TextDocumentProvider::supports` (document.rs:43-55) deliberately refuses
  them and no plugin provider is registered in production — same
  advertised-but-unprocessable class. The src/media symbols involved
  (`DocFormat::Pdf/Docx/Xlsx`) **are** read in production (the refusal + policy
  document branch), so this is tool-description drift, not severed code inside
  the module.

### sw-me-3 — `lib.rs` media re-export block has zero in-repo consumers (form 6)

- **Severity**: low
- **Form**: 6 (orphaned pub API surface)
- **Files**: `src/lib.rs:230-233` (re-exports `AudioFormat, DocFormat, MediaError,
  MediaImageFormat, MediaInput, MediaOutput, MediaPipeline, MediaPolicy,
  MediaProvider, MediaType, VideoFormat`), plus the source symbols in `src/media/`

Every in-repo consumer reaches the module by full path
(`crate::media::…` / `alephcore::media::…`), never through the crate-root
re-exports. The `alephcore`-dependent workspace crates
(`interfaces/cli`, `interfaces/tui`, `shared/client`) and the desktop shells use
`aleph_protocol::desktop_bridge::methods::media` (their own protocol types), not
alephcore's media API:

```
$ rg -n "alephcore::Media|alephcore::media|::media::\{" src/bin/ interfaces/ shared/ desktop/
desktop/macos/src/lib.rs:220,246,278,...  use aleph_protocol::desktop_bridge::methods::media::{...
(no alephcore crate-root media import anywhere)

$ rg -n "MediaError" src/ src/bin/ interfaces/ shared/ --glob '!src/media/**'
src/lib.rs:231    (re-export only)
```

- **Decision**: DECIDE — this is a library-crate public surface; external
  consumers of `alephcore` (plugins, future shells) can't be verified
  statically, so CUT carries API-compat risk.
- **Options**: (1) keep as the intended public API for external shells (add a
  doc comment stating the contract); (2) trim to the actually-referenced types
  (`MediaPipeline`, `MediaInput`, `MediaOutput`, `MediaType` + the format enums
  if shells need them), dropping `MediaPolicy`/`MediaProvider`/`MediaError`
  until something uses them.
- **Risk**: pure re-export removal is source-compatible for in-repo code; only
  hypothetical external users break.
- **Verification**: `rg -n "alephcore::Media" src/ src/bin/ interfaces/ shared/ desktop/` →
  empty; after trimming, `cargo check -p alephcore` would confirm no in-repo
  breakage (not run here per constraints).

---

## Verified-not-severed (explicit checks that came back clean)

- **`transcription.rs`** — trait + result struct, not a stub (see Wiring summary).
- **`whisper.rs`** — reachable from production via `resolve.rs:81`
  (config-gated, not orphaned; not superseded by `generation/providers/openai_whisper`).
- **`MediaProcessor::cleanup` / `cleanup_stale`** — both have production callers
  (run_loop inner.rs:1437,1450,1513,1530; agent_init/mod.rs:1211).
- **`MediaCache::{decode_data_url, safe_local_media_path, url_only_attachment}`**
  (pub(crate)) — consumed by `media_send.rs:156,163` and `artifact_harvest.rs:380`.
- **`detect_by_magic`** — pub re-export at mod.rs:41, but has a production
  consumer (`detect_from_path`, detect.rs:227), so not dead.
- **`MediaError` variants** — all produced in production paths (`NoProvider`
  pipeline.rs:74; `SizeLimitExceeded` policy.rs; `UnsupportedFormat`
  detect.rs:20/image.rs:84; `DetectionFailed` detect.rs:232; `ProviderError`
  pipeline.rs:49).

## Skipped / not assessed

- No `cargo check`/`clippy`/`test` runs (protocol constraint); wiring claims are
  `rg`-parity only, not compile-verified.
- Behavior of the vision layer (`src/vision`), `gateway/media.rs` (outbound
  `MediaItem`), `generation/providers/openai_whisper`, and the desktop/media
  protocol bridge — referenced only as consumers/producers, not audited.
- `tests/multimodal_probe.rs` classified as integration test (non-production).
- Style/clippy noise not reported.
- Pre-existing `TODO(windows)` in cache.rs:858 noted, not a severed wire.
