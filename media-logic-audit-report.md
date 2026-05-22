# Logic Review Report
**Module**: media
**Scope**: All files under `src/media/` (16 files, ~3,681 lines)
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Warning] URL download loads entire response into memory before size check
- **Location**: `src/media/cache.rs:130-140`
- **Risk**: A malicious server can OOM the process by streaming an arbitrarily large file. The old code called `resp.bytes().await` which buffers the full response body into RAM, then checked size *after* allocation.
- **Current impact**: High (DoS vector)
- **Suggestion**: Stream response to file with incremental size check. Fixed — now uses `resp.bytes_stream()` with chunk-by-chunk write and early abort if `MAX_FILE_SIZE` exceeded. Also pre-checks `Content-Length` header for fast rejection.

### [Warning] Data URL prefix check is case-sensitive
- **Location**: `src/media/cache.rs:249`
- **Risk**: Per RFC 2397, `DATA:` and `Data:` are valid data-URL schemes. The old code used `.starts_with("data:")` which would misclassify non-lowercase schemes as HTTP URLs, causing unexpected download attempts or wrong fallback paths.
- **Current impact**: Medium (RFC non-compliance, incorrect routing)
- **Suggestion**: Use case-insensitive comparison. Fixed — now uses `.to_ascii_lowercase().starts_with("data:")`.

### [Warning] MIME type prefix checks are case-sensitive
- **Location**: `src/media/processor.rs:99, 365-368` and `src/media/placeholder.rs:60-63`
- **Risk**: MIME types are case-insensitive per RFC 2045. An attachment with `MIME="IMAGE/PNG"` would fall through to the "other" branch instead of being processed as an image.
- **Current impact**: Medium (incorrect media classification)
- **Suggestion**: Normalize MIME type to lowercase before prefix matching. Fixed in both `process_one()`, `fallback_text()`, and `MediaRegistry::register()`.

### [Warning] Tilde expansion does not handle bare "~"
- **Location**: `src/media/cache.rs:393-401`
- **Risk**: A local path of exactly `~` (without trailing slash) is not expanded to the home directory, causing a file-not-found error when resolving attachments.
- **Current impact**: Low (edge case in path handling)
- **Suggestion**: Handle bare `~` in `expand_tilde()`. Fixed — added explicit `path == "~"` branch.

### [Suggested Test] Concurrent download with same filename
```rust
#[tokio::test]
async fn concurrent_resolve_same_filename() {
    let cache = MediaCache::new();
    let mut att1 = empty_attachment();
    att1.filename = Some("same.txt".into());
    att1.data = Some(vec![1, 2, 3]);
    let mut att2 = empty_attachment();
    att2.filename = Some("same.txt".into());
    att2.data = Some(vec![4, 5, 6]);

    let (r1, r2) = tokio::join!(
        cache.resolve(&att1, "test-concurrent"),
        cache.resolve(&att2, "test-concurrent"),
    );
    // Both should succeed; file content may vary due to race
    assert!(r1.is_ok());
    assert!(r2.is_ok());
}
```

### [Suggested Test] Data URL exceeding size limit
```rust
#[tokio::test]
async fn test_resolve_data_url_too_large() {
    let cache = MediaCache::new();
    let mut att = empty_attachment();
    // Base64 of 51MB of zeros ≈ 68MB string — should fail
    let huge_data = vec![0u8; 51 * 1024 * 1024];
    att.data = Some(huge_data);
    let result = cache.resolve(&att, "test-huge-data").await;
    assert!(matches!(result.unwrap_err(), CacheError::TooLarge { .. }));
}
```

### [Suggested Test] URL with misleading Content-Length
```rust
#[tokio::test]
async fn test_resolve_url_content_length_too_large() {
    // Requires a mock HTTP server that returns Content-Length > 50MB
    // but actual body is small. Should fail early without downloading.
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 4 (all fixed) |
| Suggested Test | 3 |

## Fixes Applied
1. **cache.rs**: Replaced `resp.bytes().await` with streaming download (`bytes_stream()` + incremental size check) + `Content-Length` pre-check.
2. **cache.rs**: Made data URL scheme detection case-insensitive.
3. **processor.rs + placeholder.rs**: Made MIME type prefix matching case-insensitive.
4. **cache.rs**: Added bare `~` handling in `expand_tilde()`.
