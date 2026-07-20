# Logic Review Report
**Module**: bundled
**Scope**: Full static audit of `src/bundled/` (mod.rs, manifest.rs, extractor.rs)
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Warning] Path separator validation insufficient in `extract_skills`
- **Location**: `extractor.rs:110`
- **Risk**: While `file_name()` theoretically returns only the final path component, a bundled directory with an embedded path separator in its name could cause unexpected directory creation behavior via `Path::join()`.
- **Current impact**: low (bundled content is compile-time embedded and trusted)
- **Suggestion**: Add explicit checks for `/` and `\\` in skill names. **FIXED** — added `|| name.contains('/') || name.contains('\\')` to the validation.

### [Warning] `extract_plugins` temp directory cleanup follows symlinks
- **Location**: `extractor.rs:157-162`
- **Risk**: `tmp_dir.exists()` follows symlinks, and `remove_dir_all()` recursively deletes the target of a symlink. If an attacker pre-creates `~/.aleph/plugins/cache/aleph-official.tmp` as a symlink to a sensitive directory (e.g., `~/.aleph/config`), the cleanup could delete files outside the intended scope.
- **Current impact**: low (requires write access to `~/.aleph/plugins/cache/`)
- **Suggestion**: Use `symlink_metadata()` to verify the path is a real directory before deletion. **FIXED** — replaced `exists()` + `remove_dir_all()` with `symlink_metadata()` check that only removes true directories.

### [Warning] Temp file leak on atomic write failure
- **Location**: `extractor.rs:268-270`
- **Risk**: In `extract_dir_contents`, `std::fs::write()` writes to a temp file, then `std::fs::rename()` moves it to the destination. If `rename()` fails (e.g., destination is locked, permission denied), the temp file is left behind with no cleanup.
- **Current impact**: low (temp files are hidden dot-files; on server restart, new extractions use different temp names)
- **Suggestion**: Clean up the temp file on rename failure. **FIXED** — added `remove_file()` in the error branch before returning the error.

### [Warning] Coarse-grained version tracking causes unnecessary re-extraction
- **Location**: `extractor.rs:48-82`
- **Risk**: `bundled_version` is a single string for both skills and plugins. If plugin extraction fails but skill extraction succeeds, `bundled_version` is not updated. On next startup, skills will be re-extracted even though they already succeeded, potentially overwriting user modifications to official skills.
- **Current impact**: low (users should not modify official skills, but in practice this happens)
- **Suggestion**: Consider splitting version tracking into `bundled_skills_version` and `bundled_plugins_version` for independent granularity. This is a design-level change beyond the scope of a quick fix.

### [Suggested Test] Manifest corruption recovery
```rust
#[test]
fn manifest_corruption_triggers_recreate() {
    let tmp = tempfile::tempdir().unwrap();
    let manifest_path = tmp.path().join("manifest.json");
    std::fs::write(&manifest_path, "not valid json").unwrap();
    
    let result = InstallRegistry::load(tmp.path());
    assert!(result.is_none(), "Corrupt manifest should return None");
}
```

### [Suggested Test] Symlink safety during plugin cache cleanup
```rust
#[test]
fn plugin_cache_cleanup_does_not_follow_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("cache");
    let tmp_dir = cache_dir.with_extension("tmp");
    let sensitive = tmp.path().join("sensitive");
    std::fs::create_dir_all(&sensitive).unwrap();
    std::fs::write(sensitive.join("important.txt"), "data").unwrap();
    
    // Create tmp_dir as symlink to sensitive directory
    #[cfg(unix)]
    std::os::unix::fs::symlink(&sensitive, &tmp_dir).unwrap();
    
    // The cleanup should NOT delete files in `sensitive`
    // (Implementation detail: this test would need to run the actual extract_plugins logic
    // or extract the tmp_dir cleanup into a testable function.)
}
```

### [Suggested Test] Atomic write failure cleans up temp file
```rust
#[test]
fn atomic_write_failure_cleans_temp() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("dest.txt");
    
    // Create a read-only directory to force rename failure
    let ro_dir = tmp.path().join("readonly");
    std::fs::create_dir_all(&ro_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    
    // Attempt atomic write into read-only dir
    // Verify no temp files are left behind
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 4 |
| Suggested Test | 3 |

## Fixes Applied

1. **`extractor.rs:110`** — Added path separator validation (`/` and `\\`) to skill name checks.
2. **`extractor.rs:156-169`** — Replaced `tmp_dir.exists()` + `remove_dir_all()` with `symlink_metadata()`-based check to prevent following malicious symlinks during cleanup.
3. **`extractor.rs:270-274`** — Added temp file cleanup (`remove_file`) when `rename()` fails during atomic file extraction.
