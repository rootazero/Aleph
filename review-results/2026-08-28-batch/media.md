# Media Module Review (2026-08-28)

**Scope:** `src/media/` (~4584 lines; 14 .rs files including `processors/`)
**Reviewer:** static, subagent
**Files covered:**
- `src/media/mod.rs` (165L), `types.rs` (208L), `error.rs` (27L)
- `src/media/detect.rs` (491L), `resolve.rs` (220L), `policy.rs` (251L)
- `src/media/cache.rs` (1066L), `pipeline.rs` (304L), `processor.rs` (726L)
- `src/media/provider.rs` (150L), `transcription.rs` (59L), `whisper.rs` (247L)
- `src/media/processors/{mod.rs,image.rs,audio.rs,document.rs}` (670L)

## Summary

- **P0:** 0
- **P1:** 6
- **P2:** 5
- **Total findings:** 11
- **Architecture compliance:** R1 ✓, R3 ✓, R4 ✓

The media module is **well-defended** for a security-sensitive surface. `cache.rs` carries the strongest security posture — `safe_local_media_path` does canonicalize + ownership check + root containment; `safe_fetch` enforces DNS pinning and SSRF policy; data-URL decoding has both pre-decode and post-decode size caps; `write_private` creates owner-only files via `mode(0o600)`. `whisper.rs` is fail-closed on plaintext bearer tokens over non-loopback HTTP, and `resolve.rs` cleanly distinguishes "no provider" from "provider refused". The pipeline has defense-in-depth (SSRF validated both in `pipeline.rs` and inside `safe_fetch`). Errors are well-modeled (`MediaError`, `CacheError`, `TranscriptionConfigError`).

Most issues are **classification precision** (magic-byte heuristics being broader than they could be) and **defense-in-depth gaps** (size checks not applied uniformly across input variants). No actual exploitable vulnerabilities were found at high confidence; the cache's local-path guard is a model of explicit threat-model documentation and is the load-bearing piece reviewers should preserve.

---

## Findings

### [P1] `src/media/detect.rs:135-142` — `detect_by_magic` returns `DocFormat::Docx` for any ZIP-based file (XLSX, PPTX, JAR, EPUB, DOCX itself)

- **Category:** logic
- **Confidence:** High
- **Description:** The `PK\x03\x04` signature triggers an unconditional `DocFormat::Docx` classification. The function author flagged this as a known limitation in a code comment ("Cannot distinguish DOCX from XLSX/PPTX/ZIP by magic bytes alone; precise detection requires parsing the ZIP's `[Content_Types].xml`. Default to Docx as the most common document format."). This conflates **DOCX** with **XLSX**, **PPTX**, **JAR**, **EPUB**, **ODT**, **ODP**, and any other ZIP-based container. Downstream routing by `MediaType::Document { format, .. }` can then send an XLSX to a DOCX-only handler or vice versa.
- **Impact:** A model call asking for "the spreadsheet" gets a DOCX-shaped answer; an XLSX file detected via magic is silently routed away from any spreadsheet-aware provider; a JAR/EPUB attached as a document is misclassified as DOCX and the policy `max_document_bytes` cap is applied (rather than the correct format's cap, when one exists). In `builtin_tools/media_tools/understand.rs:177,184`, `detect_by_extension` is called with the file's extension — for an `.xlsx` file, extension detection yields `DocFormat::Xlsx` correctly; for an uploaded `.bin` file with XLSX magic bytes, the pipeline gets `DocFormat::Docx` and may reject or misroute.
- **Suggested fix:**
  1. Either return `MediaType::Unknown` for ZIP signatures and require extension-based classification as a fallback (which is what `detect_from_path` already does), OR
  2. Add a lightweight ZIP central-directory scan for the `[Content_Types].xml` override part (parsing `<Override PartName="..."/>` entries against known namespaces `wordprocessingml` → DOCX, `spreadsheetml` → XLSX, `presentationml` → PPTX).
  3. Document the contract clearly so callers (the three `builtin_tools/media_tools/*` sites) know they must prefer `detect_from_path` (which already chains magic → extension) over `detect_by_magic` for untrusted inputs.
- **Related:** `src/media/detect.rs:208-216` (`detect_video_magic`), `builtin_tools/media_tools/understand.rs:177,184`, `builtin_tools/media_tools/{extract,transcribe}.rs:97,96`

---

### [P1] `src/media/detect.rs:106-110` — JPEG detection accepts any file beginning with `FF D8 FF`

- **Category:** logic
- **Confidence:** High
- **Description:**
  ```rust
  // JPEG: FF D8 FF
  if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
      return Some(MediaType::Image { format: MediaImageFormat::Jpeg });
  }
  ```
  The third byte is **not validated**. The legitimate third-byte values are `0xE0` (JFIF), `0xE1` (EXIF/XMP), `0xE2` (ICC), `0xDB` (raw quantised-DCT scan). Real-world files also use `0xE3-0xEF` (various APP markers) and `0xFE` (comment) before a SOI/EOI structure. The current code matches **JFIF**, **EXIF**, **JPEG 2000 JP2** (`FF D8 FF 4F`), **Motion JPEG** in AVI containers, embedded JPEG thumbnails in RAW files, and JPEG-XL codestream-with-JPEG-wrapper (experimental). It does not match **JPEG XL native** (`FF 0A`) or **WebP** (`RIFF…WEBP`).
- **Impact:** Detection reports `MediaType::Image { format: Jpeg }` for any `FF D8 FF` blob. Downstream `processor.rs:image_format_from_mime` keys off `attachment.mime_type` (which a channel adapter may supply correctly) or `cached.mime_type`. A misdetected `image/jpeg` flows into `ContentBlock::Image { data, mime_type: "image/jpeg" }` for vision-capable models; the LLM provider receives an unreadable blob and silently fails. For text-only models, `image_format_from_mime` returns `Some(Jpeg)` and `VisionPipeline::understand_image` is invoked with a JPEG it cannot decode.
- **Suggested fix:** Validate the third byte against known APP markers and route to the right category. Specifically:
  ```rust
  if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
      let marker = bytes[3];
      // APP0..APP15 are valid; everything else is suspicious but still likely JPEG.
      // JP2 uses FF D8 FF 4F; treat it as JPEG family and let the downstream
      // provider refuse the unsupported subformat.
      if matches!(marker, 0xE0..=0xEF | 0xDB | 0xC0..=0xC3 | 0xC4 | 0x4F) {
          return Some(MediaType::Image { format: MediaImageFormat::Jpeg });
      }
      // Fall through to other detectors; do NOT return Jpeg on an unknown marker.
  }
  ```
  Or accept the broader check but document the false-positive risk in the function's doc comment.
- **Related:** `src/media/detect.rs:95-120` (image magic detection cluster)

---

### [P1] `src/media/detect.rs:148-153` — MP3 detection accepts any MPEG audio frame sync

- **Category:** logic
- **Confidence:** High
- **Description:**
  ```rust
  // MP3: ID3 tag or sync word
  if bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0) {
      return Some(MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None });
  }
  ```
  The sync-word check `bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0` matches every MPEG audio layer (MP1 sync `0xFF FC`, MP2 sync `0xFF FD`, MP3 sync `0xFF FB`/`0xFF FA`/`0xFF F2`/`0xFF F3`, **AAC ADTS** sync `0xFF F1`/`0xFF F9`). An AAC ADTS file attached as `audio/mp3` flows into the `AudioMediaProvider` → `WhisperTranscription::transcribe` with a multipart `Content-Type: audio/mpeg`, which Whisper rejects (`400 unsupported file format`).
- **Impact:** An AAC or MP2 file is misclassified as MP3, fails Whisper with a hard 400 error, and the channel delivers `[Audio: foo.aac (audio/mp3, N B) — transcription failed]` to the model. The user sees a confusing error instead of the actual format. The model cannot recover because the mime type is wrong, not the bytes.
- **Suggested fix:** Use the MPEG audio layer bits: `bytes[2] >> 5 == 0b111` and either `((bytes[2] >> 1) & 0b11) == 0b11` (MP3) or check for ID3v2 explicitly. Alternatively, leave the broad check but log the false-positive and re-detect from extension when the magic classification succeeds. Cleanest fix: distinguish `ID3` (always MP3) from sync-word and require the layer bits.
- **Related:** `src/media/detect.rs:147-176` (audio magic detection cluster)

---

### [P1] `src/media/pipeline.rs:62-68` — `MediaPolicy::check_size` only fires for `FilePath` inputs; `Url` and `Base64` bypass it

- **Category:** logic / defense-in-depth
- **Confidence:** High
- **Description:**
  ```rust
  // 1. Policy check (file size if path)
  if let MediaInput::FilePath { path } = input {
      if tokio::fs::try_exists(path).await.unwrap_or(false) {
          if let Ok(metadata) = tokio::fs::metadata(path).await {
              self.policy.check_size(media_type, metadata.len())?;
          }
      }
  }
  ```
  The size policy is gated on `FilePath` only. For `MediaInput::Url { url }` and `MediaInput::Base64 { data, media_type }` the `policy.check_size` is **never invoked**. The audio provider has its own 25 MB hard cap (`whisper.rs:170-171`), but **the image provider has no such cap**. A 500 MB URL or a 500 MB base64 image goes straight to `VisionPipeline::understand_image` and the model provider's request times out / OOMs at the upstream.
- **Impact:** Resource exhaustion vector against the LLM provider (large image upload bills or rate-limit hits) and against the local process (large base64 string materialised by `MediaCache::to_base64`). The `cache.rs::resolve_url` and `cache.rs::resolve_inline` paths both enforce `MAX_FILE_SIZE = 50 MB` (cache layer), and `whisper.rs::transcribe` enforces 25 MB (audio layer), but `MediaPipeline::process` for **images** via Url/Base64 has no enforcement point. An oversized Base64 image attachment is never rejected at the pipeline boundary.
- **Suggested fix:** Apply `policy.check_size` uniformly. For `Base64`, use the encoded length pre-check that `processors/document.rs` and `cache.rs::decode_data_url` already use (`data.len().saturating_mul(3) / 4` for base64, raw length otherwise). For `Url`, do a `HEAD` request (or `GET` with `Range: bytes=0-0`) to fetch `Content-Length` and refuse before the provider is invoked — or simply rely on the cache layer's `safe_fetch(... with_max_body_bytes(MAX_FILE_SIZE))` and add a `policy.check_size(media_type, response.body.len())` after the fetch returns. The current asymmetry is not a security bug (the cache protects the URL path), but the API surface is inconsistent and a future caller can route around it.
- **Related:** `src/media/policy.rs:54-150`, `src/media/whisper.rs:170-171`, `src/media/cache.rs:25-26`

---

### [P1] `src/media/cache.rs:1018-1023` — `is_owned_by_current_process` returns `true` unconditionally on non-Unix (Windows)

- **Category:** security (defense weakening, acknowledged)
- **Confidence:** High
- **Description:**
  ```rust
  #[cfg(not(unix))]
  {
      let _ = meta;
      true
  }
  ```
  The function doc-comment acknowledges the gap: "On Windows, ACLs rather than uid make this check meaningless as currently written; until a per-platform ACL check is added, Windows falls back to 'any file under temp_dir()' — which is the same overly-permissive behavior the original code had, but only on the one platform where per-user temp directories are the convention and the broader risk is lower."
- **Impact:** On Windows, `safe_local_media_path` accepts **any file** under `%TEMP%` regardless of which user owns it. `%TEMP%` is per-user on modern Windows (the Win32 API guarantees this since Vista), but **secondary temp directories** (`C:\Windows\Temp`, `TMP` environment variable override) and shared/legacy `%TEMP%` paths can be readable by multiple accounts. A model-supplied URL pointing at another user's `%TEMP%\sensitive.docx` could be attached and delivered outbound as a `MediaItem`. This is the arbitrary-file-exfiltration vector that `safe_local_media_path` was designed to close.
- **Suggested fix:** Implement ACL-based ownership on Windows via the `windows-sys` or `acl` crate (the same approach `seatbelt.rs` and `permission.rs` use elsewhere in the repo). Cross-reference `src/desktop/windows/src/permission.rs` for the existing pattern. Alternatively, gate the Windows fallback behind an opt-in feature flag and refuse the file when the check is unsupported (fail-closed is preferable to fail-open for a security predicate).
- **Related:** `src/media/cache.rs:443-468` (the doc-comment justifying the gap), `src/desktop/windows/src/permission.rs` (ACL patterns)

---

### [P1] `src/media/processors/image.rs:54-75` — `convert_input` silently routes non-Image `MediaInput::Base64` to `VisionImageFormat::Png`

- **Category:** logic
- **Confidence:** High
- **Description:**
  ```rust
  MediaInput::Base64 { data, media_type } => {
      let format = match media_type {
          MediaType::Image { format, .. } => Self::to_vision_format(format)
              .ok_or_else(|| MediaError::UnsupportedFormat(format!("{:?}", format)))?,
          _ => VisionImageFormat::Png,
      };
      Ok(ImageInput::Base64 { data: data.clone(), format })
  }
  ```
  If `MediaInput::Base64` arrives with `media_type` set to `Audio`, `Video`, `Document`, or `Unknown`, the function silently substitutes `VisionImageFormat::Png`. The downstream `VisionPipeline` then attempts to decode the bytes as a PNG image — which will fail, returning `VisionError::NoProvider` or a decode error. The actual `media_type` is discarded.
- **Impact:** In the current routing pipeline (`builtin_tools/media_tools/understand.rs`), `MediaPipeline::process` filters providers by `supports(media_type)` (which checks `category()`, not exact match), so an audio `MediaType` routed to `ImageMediaProvider` is theoretically reachable only if a bug elsewhere sets up an Audio category with an image provider — a misuse rather than a normal path. However, the silent fallback is a code smell that hides misuse. If a future caller adds an audio/image hybrid provider, the fallback will misroute without any compiler or runtime warning.
- **Suggested fix:** Replace the silent fallback with an explicit error:
  ```rust
  _ => return Err(MediaError::UnsupportedFormat(format!(
      "ImageMediaProvider cannot handle base64 input of media_type {:?}",
      media_type
  ))),
  ```
  This surfaces misuse immediately rather than at the provider boundary where the error message is less informative.
- **Related:** `src/media/processors/image.rs:51-71`, `src/media/pipeline.rs:81-94`

---

### [P2] `src/media/pipeline.rs:90-106` — `last_err` is overwritten on each provider failure; the first (often most diagnostic) error is lost

- **Category:** error-handling
- **Confidence:** High
- **Description:**
  ```rust
  for provider in &eligible {
      match provider.process(input, media_type, prompt).await {
          Ok(output) => return Ok(output),
          Err(e) => {
              tracing::warn!(provider = provider.name(), error = %e, "Media provider failed, trying next");
              last_err = e;
          }
      }
  }
  Err(last_err)
  ```
  Each provider's error is logged at WARN level, but the returned error to the caller is the **last** provider's failure. If the first provider failed with "API key invalid" and the second failed with "rate limited", the caller sees "rate limited" and the actual auth misconfiguration is hidden from any operator-facing error summary.
- **Impact:** Operator troubleshooting gets harder when multiple providers fail. Logs preserve all errors, but any UI/surface that only shows the propagated error hides the chain.
- **Suggested fix:** Aggregate errors. Either:
  1. Define `MediaError::AllProvidersFailed { attempts: Vec<(String, String)> }` and return that with all provider names + messages, OR
  2. Return the **first** error (provider order is by priority, so the first is the operator's chosen backend), with a debug-log hint that subsequent providers were attempted, OR
  3. Use `tracing::error!` for the first failure (not WARN) so it stands out in logs.
- **Related:** `src/media/error.rs:8-26`

---

### [P2] `src/media/whisper.rs:163-200` — No retry/backoff on 5xx or 429 from Whisper API

- **Category:** error-handling / resilience
- **Confidence:** High
- **Description:** The `transcribe` method makes a single `client.post(...).send().await?` call and bails on any non-success status. There is no retry on `429 Too Many Requests` or `5xx Server Error`. The 120s timeout is hardcoded and cannot be configured per-provider.
- **Impact:** Transient upstream failures (OpenAI 5xx, network blips, rate-limit windows) cause hard transcription failures. The `MediaProcessor::process_audio` catches this and emits `[Audio: foo (audio/mp3, N B) — transcription failed]` to the model, which is correct behavior but unhelpful for the user when a 2-second retry would have succeeded.
- **Suggested fix:** Add a `retry` helper using `tokio_retry::Retry` or a manual exponential-backoff loop for `429`/`5xx`/`reqwest::Error` of kind `Request` or `Connect`. Bound retries at 3 with delays `500ms, 1s, 2s`. Honor the `Retry-After` header on 429.
- **Related:** `src/media/whisper.rs:163-200`, `src/media/transcription.rs:54-58` (trait contract)

---

### [P2] `src/media/resolve.rs:60-72` — Disabled named `default_transcription_provider` silently falls through to any enabled entry

- **Category:** logic / observability
- **Confidence:** High
- **Description:**
  ```rust
  .and_then(|name| {
      gen.transcription_providers
          .get(name)
          .filter(|pcfg| pcfg.enabled)  // <-- silent skip when disabled
          .and_then(|pcfg| resolve_key(name, pcfg).map(|key| (name.clone(), key, pcfg)))
  })
  ```
  When `default_transcription_provider = Some("openai")` is set but the `openai` entry has `enabled: false`, the code silently skips to any enabled entry. The `skips_disabled_entries` test documents this as intentional design, but the operator's explicit choice (the named default) is ignored without a log line.
- **Impact:** Operator configures `openai` as the default, disables it for a temporary migration to `byo`, and the next run silently uses `byo`. No startup log says "the named default was disabled; falling back to <name>". The `transcription_service` returns `Ok(Some(...))` with a `label` that does not mention the override.
- **Suggested fix:** Emit a `tracing::warn!` when the named default is found but disabled:
  ```rust
  let named = gen.default_transcription_provider.as_ref()
      .and_then(|name| gen.transcription_providers.get(name));
  if let Some(named) = named {
      if !named.enabled {
          tracing::warn!(
              provider = %named_name,
              "default_transcription_provider is disabled; falling back to any enabled entry"
          );
      }
  }
  ```
- **Related:** `src/media/resolve.rs:43-87`, `src/media/transcription.rs:25-39` (the `TranscriptionConfigError` design — "refused is not absent" applies here too, but only for the key-refusal path)

---

### [P2] `src/media/cache.rs:42-43` — `MAX_FILE_SIZE` duplicated between `media/cache.rs` and `artifacts/store.rs`

- **Category:** code-quality / divergence risk
- **Confidence:** High
- **Description:**
  ```rust
  // src/media/cache.rs
  const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
  ```
  And `src/artifacts/store.rs:16` carries a comment: "Largest blob the store accepts, mirroring `media::cache::MAX_FILE_SIZE`". The two constants are not linked by any test or compile-time check. A future change to one will silently leave the other out of sync.
- **Impact:** Drift between the media-cache cap and the artifact-store cap could lead to confusing "uploaded to cache but rejected at artifact harvest" errors. Not a security bug — both caps are upper bounds in the same direction — but a maintenance hazard.
- **Suggested fix:** Move the constant to a shared `crate::media::consts` module (or `utils::size_limits`) and import from both call sites. The `artifacts/store.rs` doc-comment should be removed once the import is in place.
- **Related:** `src/media/cache.rs:42-43`, `src/artifacts/store.rs:16`

---

### [P2] `src/media/resolve.rs:91-95` — When `default_transcription_provider` names a keyless non-local provider, `vault_lookup` failure produces `Ok(None)` indistinguishable from "no provider configured"

- **Category:** error-handling / observability
- **Confidence:** Medium-High
- **Description:**
  ```rust
  pub fn transcription_service(
      gen: &GenerationConfig,
      vault_lookup: &dyn Fn(&str) -> Option<String>,
  ) -> Result<Option<ResolvedTranscription>, TranscriptionConfigError> {
  ```
  The closure `vault_lookup` is the **only** way to retrieve the key when `pcfg.api_key` is `None` (because `api_key` is `#[serde(skip)]` on the config struct, per the `falls_back_to_vault_for_the_key` test). If the vault returns `None` for a configured provider, the function returns `Ok(None)` ("nothing configured") — but the operator did configure `transcription_providers.openai.enabled = true`. They get a `MediaProcessor` with `transcription: None` and audio attachments degrade to `[Audio: … — transcription unavailable]`. The startup log shows no transcription-enabled line; the `agent_init` block has no error path for this.
- **Impact:** Operator's `openai` provider exists, key lives in the vault under `gen:openai`, vault lookup returns `None` (misconfiguration, secret rotation race, or transient vault backend failure). The system silently runs without transcription. The `agent_init` block at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1244-1259` matches `Ok(None)` and sets `transcription = None` without any warn/error log.
- **Suggested fix:** Distinguish three states in the return type:
  ```rust
  pub enum TranscriptionResolution {
      NoneConfigured,
      ConfiguredButKeyUnavailable { provider: String, reason: String },
      Resolved(ResolvedTranscription),
  }
  ```
  Then `agent_init` can emit a `tracing::warn!` for `ConfiguredButKeyUnavailable` and the operator sees the configuration mismatch on startup.
- **Related:** `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1233-1262`

---

## Cross-cutting observations

- **Security posture is strong and well-documented.** `cache.rs::safe_local_media_path` is the standout: 35 lines of doc-comment explaining the threat model (per-account temp squatting on shared Linux), the canonicalization strategy, and the ownership check. `write_private` creates files at `mode(0o600)` rather than chmod-ing after the write (closing the 0644 window), matching the discipline used elsewhere (`secrets::vault`, `config::save`). `safe_local_media_path`'s `pub(crate)` exposure with the "don't duplicate the predicate" warning at line 442-445 prevents a class of drift bugs. This is a model for other security predicates in the codebase.

- **Polyglot file detection is the weakest point of the module.** The ZIP → DOCX fallback in `detect.rs:135-142` is documented but not bridged; the JPEG/MP3 broadness is not documented; the ftyp-brand fallback (Mp4 for unknown brands) silently absorbs `ftyp 3gp5`, `ftyp avc1`, etc. Callers that need exact format classification (the three `builtin_tools/media_tools/*` sites) should always prefer `detect_from_path` (which already chains magic → extension) and treat `detect_by_magic` alone as insufficient for untrusted inputs.

- **Pipeline defense-in-depth is consistent but has one asymmetry.** SSRF is validated both in `pipeline.rs:48-55` and inside `safe_fetch`. File-size is validated in `cache.rs` for all input variants (URL → `safe_fetch(..., with_max_body_bytes)`; inline → pre-decode check; data URL → pre-decode check), but `pipeline.rs::check_size` is FilePath-only. The cache layer is the load-bearing enforcement; the pipeline layer is documentation. The asymmetry is not exploitable today (the cache protects the URL path; the audio provider has its own 25 MB cap) but it would be if a new provider is added without an internal size check.

- **Error model is consistent and well-typed.** `MediaError`, `CacheError`, `TranscriptionConfigError` all use `thiserror` with descriptive variants. `TranscriptionConfigError` is particularly well-designed: the `provider` field names the offending entry, the `field` field names the setting, and `detail` quotes the offending value — exactly the structure an operator needs to fix their config.

- **Test coverage is strong on the security-critical paths.** `cache.rs` has tests for: the `private_temp_root` containment, `unique_filename` separator-stripping, the `..` filename fallback, Windows path encoding in `session_dir`, local-path rejection outside temp root, owner-only file mode, data URL base64 + percent-decoded paths, and the unreachable-URL error legibility. These are not just smoke tests — they encode the threat model.

- **Code-quality minor:** Multiple `// rust-doctor-disable-next-line excessive-clone` comments indicate the code is being scanned by `rust-doctor` for excessive cloning. The `clone()` calls are deliberate (each comment marks one). Not a finding, but worth noting as a codebase-wide pattern.

- **Dependency footprint is minimal.** `cache.rs` uses `reqwest`, `base64`, `tokio`, `tracing`, `serde`, `percent-encoding`, `uuid`, `tempfile`, `libc`, `dirs`. No native image processing, no OCR libraries, no ffmpeg. R3 (core minimalism) is upheld.

---

## Architecture compliance

- **R1 — Core never calls platform APIs:** **clean.** No AppKit, Vision, CoreGraphics, Win32, or X11 calls anywhere in `src/media/`. The `#[cfg(unix)]` / `#[cfg(not(unix))` gates are limited to `libc::geteuid` for ownership checks, which is POSIX not platform-specific. ✓
- **R3 — Core minimalism:** **clean.** No heavy image/audio processing deps. The vision bridge (`processors/image.rs`) delegates to `crate::vision::VisionPipeline` rather than re-implementing vision; the transcription bridge (`processors/audio.rs`) delegates to a `TranscriptionService` trait. The cache uses `reqwest` directly for download rather than wrapping `media-tools`. ✓
- **R4 — Interface layers are pure I/O:** **clean.** `MediaProcessor` (in core) has business logic — fallback selection, error wrapping, summary formatting — but `MediaProcessor` is **core**, not interface. The interface consumers (`gateway/execution_engine`, `builtin_tools/media_tools/*`) are pure dispatchers. ✓

---

## State the negative

- I did **not** run `cargo check` or `cargo clippy`; the review is static-only per the constraint. A clippy run would surface additional lints (`arc_with_non_send_sync`, redundant `clone()`s, etc.) but those are out of scope for a static review.
- I did **not** evaluate the runtime behavior of `safe_fetch`'s DNS pinning against IPv6 rebinding or TOCTOU attacks on the `resolve_and_validate` path — that would require a network-level test harness.
- I did **not** review `src/gateway/media.rs` (208 lines) or `src/gateway/voice/local_provider.rs` end-to-end, only spot-checked the predicates they expose (`is_data_url`, `is_local_media_path`, `is_remote_fetch_url`, `detect_mime`) which are consumed by the media module.
- I did **not** review the `crate::vision::VisionPipeline` consumer (used by `processors/image.rs`); only the bridge interface.
- The **Windows ACL gap** (`is_owned_by_current_process`) is documented and accepted in the source. I surfaced it as P1 because the doc-comment says "until a per-platform ACL check is added" — the fix exists elsewhere in the codebase (`src/desktop/windows/src/permission.rs`) and the cross-platform unification is a follow-up.
- The **polyglot ZIP detection** is documented as a known limitation. I surfaced it as P1 because no test covers the misclassification behavior (XLSX attached with `.bin` extension → routed as DOCX → rejected). A regression test would be valuable.
- I found **no P0 findings**. The module does not contain exploitable security bugs at high confidence; the cache, the SSRF layer, and the bearer-token guard are all solid.
