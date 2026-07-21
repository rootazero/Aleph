# Module: src/tool_metadata

- Path: `src/tool_metadata/`
- Files scanned: 26
- Total LOC: 6729
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary
| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 1 |
| medium   | 9 |
| low      | 12 |
| **Total**| **22** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
```
ISSUE|src/tool_metadata/registry/health.rs:200-239|medium|Skill id collision in rename: `format_tool_id` for Skill uses `id` not `name`, so renaming a renamed Skill still inserts at the original `skill:{id}` key, silently overwriting the prior Skill entry with the same id.
ISSUE|src/tool_metadata/loom_concurrency.rs:15-120|medium|Loom tests use `loom::sync::RwLock`/`AtomicBool`/`AtomicU64` but the real registry (`tool_metadata/registry`) uses `tokio::sync::RwLock` via `crate::sync_primitives`; tests therefore do not validate the actual registry's concurrency under loom (per the `sync_primitives.rs` design note, this is intentional, but the docstrings "Models: …registry pattern" overstate what is tested).
ISSUE|src/tool_metadata/registry/discovery.rs:143-152|low|`trigger_health_refresh` spawns one detached `tokio::spawn` per registered probe per call without bound; under repeated prompt assembly with many registered probes this is unbounded task spawning (each is 200 ms-bounded but uncapped in count).
ISSUE|src/tool_metadata/loom_concurrency.rs:22-29|low|`unwrap_or_else(|e| e.into_inner())` silently swallows lock poisoning in loom tests, hiding real concurrency anomalies the loom sweep should detect.
ISSUE|src/tool_metadata/constants.rs:12-54|low|Dead constants `MAX_TASK_RETRIES`, `MAX_STDOUT_SIZE`, `MAX_STDERR_SIZE`, `DEFAULT_CONFIRMATION_TIMEOUT_SECS`, `DEFAULT_CONNECTION_TIMEOUT_SECS` declared as `pub` with zero consumers; future code may pull a stale, unused cap into a hot path.
```

### Perspective 2 — Logic & Correctness
```
ISSUE|src/tool_metadata/registry/conflict.rs:182-229|high|Incomplete alias-collision detection: docstring claims a later tool's `name` or any `alias` collides with existing canonical name or alias, but the inline check only matches (existing.name == new.name) OR (new.aliases contains existing.name) — missing (existing.aliases contains new.name) and alias↔alias collisions, so a low-priority registrant with `aliases = ["model"]` will not be detected as conflicting with an existing tool whose `name = "model"` because the existing tool's aliases are not consulted.
ISSUE|src/tool_metadata/registry/conflict.rs:119-126|medium|`ConflictResolution::NoConflict` variant is unreachable: `resolve_conflict` always returns `RenameNew`/`RenameExisting` (equal-priority falls into `RenameNew` else-branch); the dead-arm match at line 281-283 confirms this, so the variant should be removed.
ISSUE|src/tool_metadata/types/conflict.rs:52-86|medium|`ConflictInfo`/`ConflictResolution`/`ToolPriority` are `pub` types with no external callers (the registry implements resolution inline); only tests reference them, signalling a half-removed public seam.
ISSUE|src/tool_metadata/types/definition.rs:144-303|medium|`Capability`, `ToolDiff`, `StructuredToolMeta` are full pub APIs with zero external callers; only tests in tool_metadata reference them — large dead surface (capabilities, differentiation, use_when, NOT for) suggests an unfinished "smart tool selection" feature.
ISSUE|src/tool_metadata/types/unified/mod.rs:21-27|medium|`DispatchMode` (Direct vs AgentLoop) is a `pub` enum with zero external readers; only tests reference it; `to_prompt_line()` ignores `dispatch_mode` and no prompt builder consumes it.
ISSUE|src/tool_metadata/types/index.rs:38-285|medium|`ToolIndex`, `ToolIndexEntry`, `ToolIndexCategory` are `pub` types with no external callers (the index is constructed and rendered inside `discovery.rs` but never consumed outside the module), indicating the "smart tool index" feature is half-wired.
ISSUE|src/tool_metadata/types/tool_info.rs:85-169|medium|`UnifiedToolInfo` is `pub` and re-exported via `lib.rs:197` but no external caller constructs or reads it (only its own tests do); `original_name` and `was_renamed` are set on rename but never propagated through `UnifiedToolInfo::from(&UnifiedTool)`, so rename state is silently dropped at the JSON-RPC boundary.
ISSUE|src/tool_metadata/types/unified/mod.rs:184-188|medium|`original_name` and `was_renamed` fields are written by the conflict resolver and `with_original_name` but never read by any external consumer (`UnifiedToolInfo` does not copy them, `to_prompt_line` does not include them) — dead state.
ISSUE|src/tool_metadata/types/category.rs:18-29|medium|`ToolCategory::GeneratedSkill` variant is declared (with display + icon) but never constructed anywhere — dead enum arm suggesting a planned "Skill Compiler" feature that was not finished.
ISSUE|src/tool_metadata/registry/conflict.rs:152-159|medium|Inline ID format `match &tool.source { Native => format!("native:{new_name}"), ... }` duplicates `ToolSource::format_tool_id` at `types/conflict.rs:239-248`; risk of drift if a new `ToolSource` variant is added.
ISSUE|src/tool_metadata/types/unified/builders.rs:65-70|low|`with_safety_level` writes to `requires_confirmation` automatically, so a later `with_requires_confirmation(false)` after `with_safety_level(...)` would silently keep `requires_confirmation = true` (sync is one-way), creating a footgun where builder order changes confirmation behaviour.
ISSUE|src/tool_metadata/registry/query.rs:131-184|low|`resolve_command` hardcodes `max_depth = 3`, so a registered tool with 4 underscore-joined words (e.g. `plugin_marketplace_install_run`) cannot be resolved via slash — silent limit, no error path.
```

### Perspective 3 — Architecture Compliance
```
ISSUE|src/tool_metadata/registry/discovery.rs:87-124|low|`generate_smart_prompt(core_tools, filtered_tools)` partitions tools into "full schema" vs "index-only" by static name lists; not R10 filtering-by-intent, but the existence of a heuristic `core_tools` carve-out is the thin-harness anti-pattern (the LLM should receive all schemas and decide itself; only token-budget framing is needed, not per-name lists).
ISSUE|src/tool_metadata/registry/conflict.rs:152-159|low|Same as Logic P2: the inline `format!("{source}:{new_name}")` ladder duplicates `format_tool_id` — a thin-harness redundancy that R10 penalises (zero new behaviour, two places to edit).
ISSUE|src/tool_metadata/types/tool_info.rs:54-75|low|`ToolSourceType::default_icon` returns SF Symbol names ("`server.rack`", "`puzzlepiece.extension`") — a UI-layer hint leaking into core metadata, mildly R1-adjacent (UI string contract in core types).
```

### Perspective 4 — Code Quality
```
ISSUE|src/tool_metadata/registry/tests.rs:1-1189|low|1189-line single-file test suite (>500 threshold) — readable today but growth without splitting hurts test discoverability.
ISSUE|src/tool_metadata/registry/helpers.rs:26-28|low|`truncate_description` is a 1-line wrapper around `crate::utils::text_format::truncate_text`, and `types/index.rs:292` exposes an identical wrapper `truncate_string`; two `pub` aliases for the same call in different modules — duplicated.
ISSUE|src/tool_metadata/registry/helpers.rs:11-23|low|`extract_command_name` is `pub` (reexported via `helpers` module) but only used inside `registry/registration.rs`; should be private to the registry module.
ISSUE|src/tool_metadata/registry/conflict.rs:141-172|low|`ConflictResolver::rename_existing_tool` is `pub` (re-exposed via `mod.rs:185`) but only used in tests; `mod.rs` wrapper duplicates the same dead surface.
ISSUE|src/tool_metadata/registry/conflict.rs:99-126|low|`ConflictResolver::resolve_conflict` is `pub` (re-exposed via `mod.rs:174`) but production callers never use it (`register_with_conflict_resolution` calls the method internally) — half a public seam.
ISSUE|src/tool_metadata/registry/state.rs:32-41|low|`set_tool_active` mutates `is_active` under write lock and is the only state mutator outside conflict resolution; not problematic but `set_tool_active` returning a `bool` is a missed invariant assertion (registry now has `is_active=true` AND `health=unhealthy` simultaneously allowed — caller must check both).
ISSUE|src/tool_metadata/registry/conflict.rs:213-223|low|Minor: `existing_name_lower` is recomputed inside the closure every iteration even after `t.name.to_lowercase() == name_lower` short-circuits; negligible cost but copy-paste from a prior refactor.
```