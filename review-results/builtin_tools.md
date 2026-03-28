All fixes compile successfully. Here's the report:

---

# Module: builtin_tools

## Summary
- Files reviewed: 108
- Issues found: 15
- Issues fixed: 12 (3 noted for future improvement)

## Fixes Applied

### Critical (Security/Correctness)

1. **`pdf_generate/browser_engine.rs:73`** Temp file race condition — concurrent PDF calls used same `aleph_pdf_{pid}.html` → Added `AtomicU64` counter for unique filenames per call

2. **`code_exec.rs:188-213`** Path security bypass — `canonicalize` fallback to raw user input allowed path traversal; `create_dir_all` error silently swallowed → Reordered: create dir first, then canonicalize (fail on error), then validate boundary

3. **`clawhub.rs:432`** Unsanitized skill_name in Update path — `slug.split('/').next_back()` without `sanitize_skill_name()` could yield `..` as skill_name → Now calls `sanitize_skill_name(&slug)?`

4. **`clawhub.rs:266`** SKILL.md validation too permissive — `ends_with("/SKILL.md")` matched deeply nested files, silently accepting broken skill packages → Changed to exact `== "SKILL.md"` (root-level only)

### Warning (Robustness)

5. **`generation/image_generate.rs:125`** Lock poisoning propagated permanently — `.read().map_err()` → `.read().unwrap_or_else(|e| e.into_inner())`

6. **`generation/video_generate.rs:89`** Same lock poisoning fix

7. **`generation/speech_generate.rs:131`** Same lock poisoning fix

8. **`generation/audio_generate.rs:72`** Same lock poisoning fix

9. **`browser_tools/type_text.rs:70`** `args.text.len()` (bytes) reported as "chars" → `.chars().count()`

10. **`browser_tools/evaluate.rs:57`** `args.script.len()` (bytes) reported as "chars" → `.chars().count()`

11. **`scratchpad.rs:123`** Path traversal via null byte or dot-prefix — Added `\0` and `.` prefix checks to `project_id` validation

12. **`file_ops/ops.rs:321`** Delete count inflated by I/O errors — `entries.count()` counted `Err` results → `.filter(|e| e.is_ok()).count()`

13. **`pdf_generate/mod.rs:107`** `home_dir().unwrap_or_default()` silently produced empty path → `.ok_or_else(|| ToolError::Execution(...))?`

14. **`file_ops/path_utils.rs:97`** `.unwrap()` on `strip_prefix("~")` could panic for edge-case paths → `.unwrap_or_else(|_| Path::new(""))`

## Noted (Not Fixed — Design-Level)

- **`sessions/spawn_tool.rs:435`** Returns `Accepted` but never spawns execution — stub implementation, needs full executor handoff
- **`sessions/send_tool.rs:389`** Hardcoded Chinese string matching for truncation detection (violates R8) — needs structured protocol field
- **`browser_tools/tabs.rs:108`** `TabAction::Switch` returns `success: true` as a no-op — misleading for agents

## Compilation
- `cargo check -p alephcore --lib` — **PASS** (0 errors, 15 pre-existing warnings)
