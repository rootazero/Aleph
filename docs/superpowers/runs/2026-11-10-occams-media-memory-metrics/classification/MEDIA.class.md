# Occam's Razor Static Pattern Classification: MEDIA Batch

**Target Subsystem**: `src/media` (16 files, ~4,959 LoC)  
**Reviewer Role**: Jev (Fast Static Pattern Scanning, Taxonomy Classification, Filtering & Routing)  
**Doctrine**: Occam's Razor (`Simplicity is the removal of unnecessary concepts, not merely line count reduction`)  
**Timestamp**: 2026-11-10  

---

## 1. Executive Summary & Routing Metrics

| Taxonomy Category | Candidates Found | FIX-NOW | FIX-CAREFULLY | DEFER | REJECT |
|---|:---:|:---:|:---:|:---:|:---:|
| **DEAD-CODE / ACCIDENTAL-COMPLEXITY** | 4 | 3 | 0 | 0 | 1 |
| **REDUNDANT-ALLOCATION** | 12 | 8 | 3 | 1 | 0 |
| **CONTROL-FLOW / INDIRECTION** | 5 | 5 | 0 | 0 | 0 |
| **PANIC-RISK / DEFENSIVE-CHECK** | 2 | 2 | 0 | 0 | 0 |
| **CORRECTNESS / INVARIANT-TIGHTENING** | 2 | 0 | 0 | 0 | 2 |
| **TOTAL** | **25** | **18** | **3** | **1** | **3** |

---

## 2. Detailed Findings by Taxonomy

### A. REDUNDANT-ALLOCATION

#### [MED-ALLOC-01] Eager `format!` in `detect_from_path` Fallback
- **File & Lines**: `src/media/detect.rs:256-260`
- **Pattern**:
  ```rust
  path.extension()
      .and_then(|e| e.to_str())
      .map(detect_by_extension)
      .unwrap_or(Err(MediaError::DetectionFailed(format!(
          "Cannot determine media type for: {}",
          path.display(),
      ))))
  ```
- **Analysis**: `.unwrap_or(...)` evaluates its argument eagerly. Every successful extension-based detection (PNG, JPEG, MP3, etc.) still evaluates `format!("Cannot determine media type for: {}", path.display())` on the heap and then discards it.
- **Remediation**: Use `.unwrap_or_else(|| Err(MediaError::DetectionFailed(format!(...))))` to defer formatting strictly to the failure path.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-02] Repeated `to_ascii_lowercase()` Heap Allocations on MIME Checks
- **File & Lines**:
  - `src/media/processor.rs:105` (`let mime = attachment.mime_type.to_ascii_lowercase();`)
  - `src/media/processor.rs:398` (`let mime_lower = mime.to_ascii_lowercase();`)
  - `src/media/processor.rs:462` (`let mime_lower = mime.to_ascii_lowercase();`)
- **Analysis**: Multiple functions allocate fresh `String`s via `to_ascii_lowercase()` simply to test prefixes (`starts_with("image/")`, `starts_with("audio/")`) or match MIME types.
- **Remediation**:
  - In `media_kind(mime)`: compare without allocation: `mime.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("image/"))` and `audio/`.
  - In `image_format_from_mime(mime)`: check formats using `eq_ignore_ascii_case` or a small stack-based matching pattern.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-03] Multi-Stage Intermediate Formatting Allocations in `media_summary`
- **File & Lines**: `src/media/processor.rs:429-436`, `src/media/processor.rs:440-455`
- **Pattern**:
  ```rust
  let details = match size {
      Some(size) => format!("{}, {}", attachment.mime_type, human_size(size)),
      None => attachment.mime_type.clone(),
  };
  match note {
      Some(note) => format!("[{kind}: {name} ({details}) — {note}]"),
      None => format!("[{kind}: {name} ({details})]"),
  }
  ```
- **Analysis**: Generates up to three intermediate heap `String` allocations: `human_size` formats a String, `details` formats another String, and final `media_summary` formats a third String.
- **Remediation**: Write directly into a pre-allocated `String` using `write!(&mut s, ...)`, or format in one step.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-04] Intermediate Heap String Allocation in `fallback_text`
- **File & Lines**: `src/media/processor.rs:390`
- **Pattern**: `Some(&format!("could not be retrieved: {error}"))`
- **Analysis**: Allocates a temporary `String` via `format!`, takes a reference `&String` as `&str`, passes it to `media_summary`, and drops it immediately.
- **Remediation**: Have `media_summary` or `fallback_text` format directly, avoiding the intermediate allocation.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-05] Redundant `.to_string()` on `Cow<'_, str>` in `download_media_item`
- **File & Lines**: `src/media/cache.rs:440`
- **Pattern**: `path: Some(cached.local_path.to_string_lossy().to_string())`
- **Analysis**: `to_string_lossy()` returns `Cow<'_, str>`. Calling `.to_string()` forces another allocation even if the `Cow` is already `Owned`.
- **Remediation**: Use `cached.local_path.to_string_lossy().into_owned()`.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-06] Inefficient Option Cloning in `MediaCache::download_media_item`
- **File & Lines**: `src/media/cache.rs:331-335`
- **Pattern**:
  ```rust
  let mime = item
      .mime_type
      .as_deref()
      .map(|s| s.to_string())
      .unwrap_or_else(|| detect_mime(&item.url, &item.media_type));
  ```
- **Analysis**: `.as_deref().map(|s| s.to_string())` is a roundabout way to clone `Option<String>`.
- **Remediation**: Simplify to `item.mime_type.clone().unwrap_or_else(|| detect_mime(&item.url, &item.media_type))`.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-07] Multiple Redundant `entry.path()` Allocations in `cleanup_stale`
- **File & Lines**: `src/media/cache.rs:289, 302, 303, 305`
- **Pattern**: `entry.path()` called 4 times in the same loop iteration (once for `.join(".created_at")`, once for `remove_dir_all`, twice in log macros).
- **Analysis**: `DirEntry::path()` constructs a new `PathBuf` on each call.
- **Remediation**: Bind `let entry_path = entry.path();` once at the top of the loop.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-08] Intermediate `Vec<String>` in `MediaPipeline::process` Error Aggregation
- **File & Lines**: `src/media/pipeline.rs:145-149`
- **Pattern**:
  ```rust
  let summary = attempts
      .iter()
      .map(|(n, e)| format!("{n}: {e}"))
      .collect::<Vec<_>>()
      .join("; ");
  ```
- **Analysis**: Collects into a temporary `Vec<String>` just to call `.join("; ")`.
- **Remediation**: Join iteratively into a single `String` buffer or format directly.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-09] Unnecessary `file_name.to_string()` Allocation in `WhisperTranscription`
- **File & Lines**: `src/media/whisper.rs:178-184`
- **Pattern**:
  ```rust
  let file_name = audio
      .local_path
      .file_name()
      .and_then(|n| n.to_str())
      .unwrap_or("audio.bin")
      .to_string();
  ```
- **Analysis**: `reqwest::multipart::Part::file_name` accepts `T: Into<Cow<'static, str>>`. It accepts `&'static str` (`"audio.bin"`) or `&str` without requiring `.to_string()`.
- **Remediation**: Pass `file_name` as `&str` directly.
- **Route**: `FIX-NOW`

#### [MED-ALLOC-10] `MediaProvider::supports` Default Implementation Allocates `Vec<MediaType>` on Every Probe
- **File & Lines**: `src/media/provider.rs:24-29`, `src/media/processors/audio.rs:45-59`
- **Pattern**:
  ```rust
  fn supports(&self, media_type: &MediaType) -> bool {
      let category = media_type.category();
      self.supported_types()
          .iter()
          .any(|t| t.category() == category)
  }
  ```
- **Analysis**: `AudioMediaProvider` does not override `supports()`. Every time pipeline checks if audio is supported, `AudioMediaProvider::supported_types()` creates a 5-element `Vec<MediaType>` on the heap just to check `.category() == "audio"`.
- **Remediation**: Implement `fn supports(&self, media_type: &MediaType) -> bool { matches!(media_type, MediaType::Audio { .. }) }` in `AudioMediaProvider`.
- **Route**: `FIX-CAREFULLY` (Confirm category match semantics match `supported_types()`).

#### [MED-ALLOC-11] Multiple `id.clone()` in `MediaCache::download_media_item`
- **File & Lines**: `src/media/cache.rs:356, 393, 409, 433`
- **Pattern**: `id.clone()` called in every branch of `temp_attachment` creation and then moved into return `Attachment`. Suppressed with `// rust-doctor-disable-next-line excessive-clone`.
- **Analysis**: `id` is cloned because `id` is used in fallback filename formatting and then passed into the temporary attachment.
- **Remediation**: Construct `filename` using `&id[..8]` slice before moving `id` into `Attachment`, or restructure `download_media_item` so `id` is moved without intermediate clones.
- **Route**: `FIX-CAREFULLY`

#### [MED-ALLOC-12] `MediaPipeline::process` Collects `eligible` Vector
- **File & Lines**: `src/media/pipeline.rs:106-110`
- **Pattern**:
  ```rust
  let eligible: Vec<_> = self
      .providers
      .iter()
      .filter(|p| p.supports(media_type))
      .collect();
  ```
- **Analysis**: Allocates a `Vec<&Box<dyn MediaProvider>>` on every process invocation.
- **Remediation**: Can iterate over `self.providers.iter().filter(...)` directly without collecting into `Vec`, sizing `attempts` based on `self.providers.len()`.
- **Route**: `FIX-CAREFULLY`

---

### B. CONTROL-FLOW / INDIRECTION

#### [MED-FLOW-01] Triple Nested `if let` in `MediaCache::resolve_url`
- **File & Lines**: `src/media/cache.rs:187-196`
- **Pattern**:
  ```rust
  if let Some(content_length) = response.headers.get(reqwest::header::CONTENT_LENGTH) {
      if let Some(content_length) = content_length.to_str().ok().and_then(|s| s.parse().ok()) {
          if content_length > MAX_FILE_SIZE {
              return Err(CacheError::TooLarge { size: content_length });
          }
      }
  }
  ```
- **Analysis**: Three levels of indentation for a single parsed integer check.
- **Remediation**: Flatten using `.and_then()`:
  ```rust
  if let Some(content_length) = response
      .headers
      .get(reqwest::header::CONTENT_LENGTH)
      .and_then(|v| v.to_str().ok())
      .and_then(|s| s.parse::<u64>().ok())
  {
      if content_length > MAX_FILE_SIZE {
          return Err(CacheError::TooLarge { size: content_length });
      }
  }
  ```
- **Route**: `FIX-NOW`

#### [MED-FLOW-02] Duplicate Provider Lookup in `transcription_service`
- **File & Lines**: `src/media/resolve.rs:65-83`
- **Pattern**:
  ```rust
  if let Some(name) = gen.default_transcription_provider.as_ref() {
      if let Some(pcfg) = gen.transcription_providers.get(name) {
          if !pcfg.enabled {
              tracing::warn!(...);
          }
      }
  }
  gen.default_transcription_provider
      .as_ref()
      .and_then(|name| {
          gen.transcription_providers
              .get(name)
              .filter(|pcfg| pcfg.enabled)
              ...
      })
  ```
- **Analysis**: `gen.default_transcription_provider.as_ref()` and `gen.transcription_providers.get(name)` are looked up twice consecutively.
- **Remediation**: Collapse into a single lookup:
  ```rust
  let picked = gen
      .default_transcription_provider
      .as_deref()
      .and_then(|name| {
          let pcfg = gen.transcription_providers.get(name)?;
          if !pcfg.enabled {
              tracing::warn!(provider = %name, "default_transcription_provider is disabled; falling back to any enabled entry");
              None
          } else {
              resolve_key(name, pcfg).map(|key| (name.to_string(), key, pcfg))
          }
      })
      .or_else(|| ...);
  ```
- **Route**: `FIX-NOW`

#### [MED-FLOW-03] Repetitive Match Arms in `TextDocumentProvider::supports`
- **File & Lines**: `src/media/processors/document.rs:46-58`
- **Pattern**:
  ```rust
  matches!(
      media_type,
      MediaType::Document { format: DocFormat::Txt, .. }
      | MediaType::Document { format: DocFormat::Markdown, .. }
      | MediaType::Document { format: DocFormat::Html, .. }
  )
  ```
- **Analysis**: Verbose repetitive match arms.
- **Remediation**:
  ```rust
  matches!(
      media_type,
      MediaType::Document {
          format: DocFormat::Txt | DocFormat::Markdown | DocFormat::Html,
          ..
      }
  )
  ```
- **Route**: `FIX-NOW`

#### [MED-FLOW-04] Substring Check Repeated 3 Times in `decode_data_url`
- **File & Lines**: `src/media/cache.rs:469, 482, 498`
- **Pattern**: `header.contains("base64")` evaluated 3 separate times on the same header slice.
- **Analysis**: `contains()` repeatedly scans `header`.
- **Remediation**: Bind `let is_base64 = header.contains("base64");` once.
- **Route**: `FIX-NOW`

#### [MED-FLOW-05] Redundant Sequential `if let` in `detect_by_extension`
- **File & Lines**: `src/media/detect.rs:11-23`
- **Pattern**: Four sequential `if let Some(ty) = ... { return Ok(ty); }` statements.
- **Analysis**: Inconsistent with `detect_by_magic` in the same file, which uses `.or_else(...)`.
- **Remediation**: Chain via `detect_image_extension(ext_clean).or_else(...).or_else(...).ok_or_else(...)`.
- **Route**: `FIX-NOW`

---

### C. DEAD-CODE / ACCIDENTAL-COMPLEXITY

#### [MED-DEAD-01] By-Reference Parameter on `Copy` Enum in `ImageMediaProvider::to_vision_format`
- **File & Lines**: `src/media/processors/image.rs:29`
- **Pattern**: `const fn to_vision_format(fmt: &MediaImageFormat) -> Option<VisionImageFormat>`
- **Analysis**: `MediaImageFormat` derives `Copy`. Passing it by reference adds pointer indirection for a 1-byte enum.
- **Remediation**: Change signature to `const fn to_vision_format(fmt: MediaImageFormat) -> Option<VisionImageFormat>`.
- **Route**: `FIX-NOW`

#### [MED-DEAD-02] Duplicate Vision Error Mapping Branches
- **File & Lines**: `src/media/processors/image.rs:120-127` and `133-140`
- **Pattern**:
  ```rust
  Err(VisionError::NoProvider) => Err(MediaError::NoProvider {
      media_type: "image".to_string(),
  }),
  Err(e) => Err(MediaError::ProviderError {
      provider: "vision-pipeline".to_string(),
      message: e.to_string(),
  }),
  ```
- **Analysis**: Copied verbatim between `understand_image` and `ocr` arms.
- **Remediation**: Extract a shared closure `let map_err = |e| match e { ... };`.
- **Route**: `FIX-NOW`

#### [MED-DEAD-03] Hardcoded Provider Name String Allocations
- **File & Lines**: `src/media/processors/document.rs:90, 99, 128, 135, 152, 164, 168, 174`
- **Pattern**: Repeated `"text-document".into()` and `"text-document".to_string()`.
- **Analysis**: Accidental duplication across 8 error call sites.
- **Remediation**: Define `const NAME: &str = "text-document";` (matching `audio.rs:21`).
- **Route**: `FIX-NOW`

#### [MED-DEAD-04] Redundant String Clone in `AudioMediaProvider::process`
- **File & Lines**: `src/media/processors/audio.rs:101`
- **Pattern**: `local_path: path.clone()`
- **Analysis**: `path` is `&PathBuf`. `CachedMedia` needs an owned `PathBuf`. This clone is necessary to construct `CachedMedia`.
- **Remediation**: Retain as-is.
- **Route**: `REJECT`

---

### D. PANIC-RISK / DEFENSIVE-CHECK

#### [MED-DEF-01] TOCTOU Filesystem Race in `MediaCache::cleanup_session`
- **File & Lines**: `src/media/cache.rs:239-244`
- **Pattern**:
  ```rust
  let dir = session_dir(session_id)?;
  if dir.exists() {
      std::fs::remove_dir_all(&dir)?;
      debug!(session_id, "cleaned up media cache");
  }
  ```
- **Analysis**: `dir.exists()` followed by `remove_dir_all(&dir)` has a classic TOCTOU race condition if another thread/process cleans up the directory concurrently, returning an unhandled `io::Error`.
- **Remediation**: Call `std::fs::remove_dir_all(&dir)` directly and ignore `io::ErrorKind::NotFound`:
  ```rust
  match std::fs::remove_dir_all(&dir) {
      Ok(()) => debug!(session_id, "cleaned up media cache"),
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(e.into()),
  }
  ```
- **Route**: `FIX-NOW`

#### [MED-DEF-02] Redundant `created.exists()` Check Before `metadata()` in `cleanup_stale`
- **File & Lines**: `src/media/cache.rs:290-296`
- **Pattern**:
  ```rust
  let created = entry.path().join(".created_at");
  let timestamp = if created.exists() {
      std::fs::metadata(&created)
          .ok()
          .and_then(|m| m.modified().ok())
  } else {
      meta.modified().ok()
  };
  ```
- **Analysis**: Double stat: `created.exists()` calls `metadata()` under the hood, and then `std::fs::metadata(&created)` calls it again immediately.
- **Remediation**: Remove `created.exists()`:
  ```rust
  let timestamp = std::fs::metadata(&created)
      .ok()
      .and_then(|m| m.modified().ok())
      .or_else(|| meta.modified().ok());
  ```
- **Route**: `FIX-NOW`

---

### E. CORRECTNESS / INVARIANT-TIGHTENING

#### [MED-INV-01] Retention of Public API Contracts in `src/media/types.rs`
- **File & Lines**: `src/media/types.rs:108-133`
- **Analysis**: `MediaInput` and `MediaOutput` use owned `String` and `PathBuf` fields. While borrowing could theoretically reduce allocations, these types are exposed via `schemars::JsonSchema` and `serde::Serialize/Deserialize` across external tool boundaries and protocol definitions.
- **Remediation**: Do not modify public contracts.
- **Route**: `REJECT`

#### [MED-INV-02] Retention of P1 Security Trust-Root Bounds
- **File & Lines**:
  - `src/media/cache.rs:536-562` (`safe_local_media_path`)
  - `src/media/processors/audio.rs:85-90`
  - `src/media/processors/document.rs:79-84`
- **Analysis**: Checked for any potential "simplification" that might weaken path validation or symlink protection (`O_NOFOLLOW`). All guards are verified essential and must remain strictly intact.
- **Remediation**: Preserve all trust-root assertions and symlink checks.
- **Route**: `REJECT`

---

## 3. Prioritized Action Plan for Minimax-M3 / Deep Reasoning Tier

1. **Phase 1: Zero-Risk Hot Path Allocations (`FIX-NOW`)**
   - [x] Fix eager `format!` in `detect_from_path` (`src/media/detect.rs:256-260`)
   - [x] Eliminate `to_ascii_lowercase()` allocations on MIME classification (`src/media/processor.rs:105, 398, 462`)
   - [x] Streamline `media_summary` formatting (`src/media/processor.rs:429-436`)
   - [x] Fix `.to_string_lossy().to_string()` to `.into_owned()` (`src/media/cache.rs:440`)
   - [x] Optimize `MediaCache::cleanup_session` & `cleanup_stale` filesystem calls (`src/media/cache.rs:239-244, 290-296`)

2. **Phase 2: Control Flow & Duplication Simplifications (`FIX-NOW`)**
   - [x] Flatten triple-nested `if let` in `MediaCache::resolve_url` (`src/media/cache.rs:187-196`)
   - [x] Deduplicate provider lookup in `transcription_service` (`src/media/resolve.rs:65-83`)
   - [x] Deduplicate vision error mapping in `ImageMediaProvider::process` (`src/media/processors/image.rs:120-140`)
   - [x] Unify `matches!` pattern in `TextDocumentProvider::supports` (`src/media/processors/document.rs:46-58`)
   - [x] Pass `MediaImageFormat` by value in `to_vision_format` (`src/media/processors/image.rs:29`)

3. **Phase 3: Trait & Provider Allocations (`FIX-CAREFULLY`)**
   - [ ] Override `supports()` in `AudioMediaProvider` to avoid dynamic `Vec<MediaType>` allocation (`src/media/processors/audio.rs:45-59`)
   - [ ] Refactor `MediaCache::download_media_item` to eliminate `id.clone()` across match arms (`src/media/cache.rs:356, 393, 409`)
