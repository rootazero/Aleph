# Module: src/discovery

- Path: `src/discovery/`
- Files scanned: 4
- Total LOC: 1482
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary
| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 2 |
| medium   | 5 |
| low      | 14 |
| **Total**| **21** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
```
ISSUE|src/discovery/scanner.rs:226-269|medium|discover_component follows symlinks via path.is_dir()/is_file(); a symlink in ~/.aleph/skills (or commands/agents/plugins) pointing outside the expected tree is enumerated and treated as a discovered skill/command/agent.
ISSUE|src/discovery/scanner.rs:340-391|medium|scan_plugin_parent follows symlinks in ~/.aleph/plugins via path.is_dir() and read_dir, allowing a symlinked "plugin" directory to expose manifests from outside the intended scope (e.g. /etc/aleph.plugin.toml).
ISSUE|src/discovery/scanner.rs:235-256|low|is_hidden() checks the symlink's own file_name (path.file_name()), not the canonical target, so a symlink named "visible" pointing to a hidden dir is included while a symlink named ".hidden" is silently dropped — inconsistent with user expectations.
ISSUE|src/discovery/scanner.rs:90|low|find_git_root uses current.join(".git").exists() which follows symlinks; a `.git` symlink to an arbitrary directory would cause any ancestor dir to be mis-reported as a git root.
ISSUE|src/discovery/paths.rs:184|low|validate_path_component rejects any name containing ".." substring, producing false positives for legitimate filenames like foo..bar.md or v1..v2.jsonc.
ISSUE|src/discovery/scanner.rs:113|low|priority = 20u32.saturating_add(i as u32) truncates on 64-bit systems when enumerate() index exceeds u32::MAX; theoretical but the cast is implicit.
ISSUE|src/discovery/mod.rs:80|low|DiscoveryConfig::default swallows std::env::current_dir() failure with unwrap_or_else(|_| PathBuf::from(".")), masking the I/O error and silently using a relative working_dir that changes behavior under chdir.
ISSUE|src/discovery/scanner.rs:218-219|low|TOCTOU between scan_dir.exists()/component_dir.exists()/is_dir() and the subsequent read_dir; an attacker who can swap directories between checks could redirect discovery results (low impact since the parent dirs are user-owned).
```

### Perspective 2 — Logic & Correctness
```
ISSUE|src/discovery/scanner.rs:298-309|high|discover_plugins scans only ~/.aleph/plugins/ and silently ignores project-level plugin dirs; users calling the simpler API will get an incomplete plugin list unless they know to call discover_plugins_with_extra.
ISSUE|src/discovery/paths.rs:75-102|high|find_git_root duplicates utils::paths::find_git_root (src/utils/paths.rs:267) with diverging behavior (this one canonicalizes first and bounds depth at 100, the other uses PathBuf::pop with no bound); two truths in one codebase.
ISSUE|src/discovery/scanner.rs:185-208|medium|discover_component duplicates the validation logic that already exists as validate_path_component in paths.rs (same checks for empty, length, '/', '\\', '..'); drift risk when one side is updated.
ISSUE|src/discovery/scanner.rs:226-275|medium|broken symlinks (is_dir()/is_file() return false) and unreadable subdirectories are silently skipped at debug log level; users will not understand why their plugin/skill is not being discovered.
ISSUE|src/discovery/paths.rs:193-206|medium|find_file_upward and find_dir_upward perform canonicalize() on every upward step (paths.rs:157-161), causing O(depth^2) syscalls on deep trees.
ISSUE|src/discovery/paths.rs:107-169|low|find_upward's current_canonicalized tracking is subtle: if start fails to canonicalize, the whole walk skips canonicalization thereafter, mixing canonical and non-canonical paths in the same traversal.
```

### Perspective 3 — Architecture Compliance
```
ISSUE|src/discovery/paths.rs:75-102|medium|R3 duplication: two find_git_root implementations (here and utils/paths.rs:267) with no shared abstraction; should consolidate.
ISSUE|src/discovery/paths.rs:19-22|low|R3 surface bloat: pub const SKILLS_DIR and COMMANDS_DIR are unused anywhere in the repo; dead public constants.
ISSUE|src/discovery/paths.rs:46-48|low|R3 surface bloat: pub fn home_dir is only called internally by claude_home_dir(); dead public API.
ISSUE|src/discovery/paths.rs:193-222|low|R3 surface bloat: pub fn find_file_upward and find_dir_upward are only invoked from within scanner.rs; the public visibility adds API surface with no consumers.
ISSUE|src/discovery/paths.rs:225-233|low|R3 surface bloat: pub fn ensure_dir has zero callers; workflow/store.rs, canvas_io.rs, etc. each roll their own variant instead.
ISSUE|src/discovery/mod.rs:135-137|low|R3 surface bloat: get_scan_directories is only invoked from within the module (scanner.rs:80); the public delegation in DiscoveryManager has no external callers.
ISSUE|src/discovery/types.rs:29-55|low|R3 surface bloat: ScanDirectory type is exported but never named externally; consumers use the return type implicitly. Same for DiscoveredPath in many cases.
ISSUE|src/discovery/mod.rs:62-108|low|R9 (configurability exposed as tools): DiscoveryConfig fields like scan_claude_dirs and scan_project_dirs are pure startup config with no runtime tool-driven toggle; arguably OK but inconsistent with the R9 principle.
```

### Perspective 4 — Code Quality
```
ISSUE|src/discovery/scanner.rs:1-856|low|scanner.rs is 856 lines (above the 500-line guideline); production code is ~440 lines but the file still mixes ~415 lines of tests with implementation.
ISSUE|src/discovery/types.rs:74-77|low|DiscoveredPath::new derives name via path.file_name() falling back to path.to_string_lossy().into_owned(), which leaks full absolute paths as the display name when file_name() is None (root, ..).
ISSUE|src/discovery/scanner.rs:235-238|low|is_hidden is checked twice with identical logic (lines 236-238 and 256-258) for dir and file branches; extract or invert the branch.
ISSUE|src/discovery/scanner.rs:422-441|low|has_plugin_manifest enumerates 11 candidate paths linearly via Vec::iter().any(); a match-style dispatch table would be clearer about supported formats and easier to extend.
ISSUE|src/discovery/paths.rs:107-169|low|find_upward's stop-path-canonicalization branch (lines 128-134) is hard to read; the two-track logic for current_canonicalized.is_some() vs None could be a single early-return helper.
ISSUE|src/discovery/paths.rs:173-190|low|validate_path_component uses String allocations for every error path (format!/to_string); for a hot path on every discovered entry, a static error enum would be cheaper and more typed.
ISSUE|src/discovery/paths.rs:122-161|low|canonicalize() inside the upward loop (per-step) can stall on slow filesystems (NFS, FUSE); consider caching canonical state once at the top.
```
