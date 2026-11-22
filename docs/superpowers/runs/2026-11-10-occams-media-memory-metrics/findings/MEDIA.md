# Occam's Razor Findings & Remediation: Batch MEDIA

**Module**: `src/media/` (16 files, ~4,959 LOC)  
**Date**: 2026-11-10  
**Doctrine**: `/rust-occams-razor`

---

## 1. Executive Summary

Static review of `src/media/` targeted caching, media detection, processor dispatch, format conversion, and external provider integration.
The following Occam's Razor remediations were executed:

1. **Zero-Allocation MIME and Extension Classification (`src/media/detect.rs`, `src/media/processor.rs`)**:
   - Replaced `.unwrap_or(...)` in `detect_from_path` with `.unwrap_or_else(...)`, avoiding eager `format!` allocations on every successful detection.
   - Chained sequential `if let` blocks in `detect_by_extension` via `.or_else(...)`.
   - Replaced repeated `.to_ascii_lowercase()` string allocations in `media_kind`, `process_one`, and `image_format_from_mime` with zero-allocation ASCII case-insensitive comparisons (`mime.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("image/"))` and `eq_ignore_ascii_case`).
2. **Intermediate Formatting Elimination (`src/media/processor.rs`, `src/media/pipeline.rs`)**:
   - Consolidated 3-stage intermediate string formatting in `media_summary` into a single-pass `format!` expression.
   - Streamed provider error summaries in `MediaPipeline::process` into a pre-allocated buffer with `write!` instead of intermediate `Vec<String>` and `.join()`.
3. **Filesystem Defense & Race Prevention (`src/media/cache.rs`)**:
   - Eliminated TOCTOU race in `cleanup_session` by removing `dir.exists()` guard and handling `io::ErrorKind::NotFound` directly.
   - Bound `entry_path = entry.path()` once per iteration in `cleanup_stale` to prevent 4x redundant `PathBuf` allocations, and eliminated double `stat` by replacing `created.exists()` with direct `metadata(&created)`.
   - Converted `to_string_lossy().to_string()` to `.into_owned()`.
   - Reused `is_base64` flag in `decode_data_url` instead of scanning the header 3 times.
4. **Control Flow & Error Duplication Consolidation (`src/media/resolve.rs`, `src/media/processors/`)**:
   - Deduplicated duplicate `gen.default_transcription_provider` lookups in `transcription_service`.
   - Deduplicated repeated `VisionError` mapping arms in `ImageMediaProvider::process` with a shared closure.
   - Consolidated verbose match arms in `TextDocumentProvider::supports`.
   - Eliminated redundant `file_name.to_string()` in `WhisperTranscription::transcribe`.

All media type definitions, public contracts, and symlink/security boundaries (`O_NOFOLLOW`) remain intact.
