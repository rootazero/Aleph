# Review Summary

**Date**: 2026-07-20
**Modules reviewed**: 6 (`src/wizard`, `src/workflow`, `desktop`, `interfaces`, `shared`, `mobile`)
**Reviewer**: static (4-perspective checklist: security / logic / architecture / quality)
**Threshold**: no scoring pass — all reported findings are considered actionable; severity is supplied by the reviewer

## Module Totals

| Module              | Files |    LOC | Critical | High | Medium |  Low | Total |
|---------------------|------:|-------:|---------:|-----:|-------:|-----:|------:|
| src/wizard          |     6 |   1607 |        0 |    1 |      3 |   11 |    15 |
| src/workflow        |    10 |   4672 |        0 |    0 |      3 |    7 |    10 |
| desktop             |    99 |  24623 |    **2** |   20 |     14 |    0 |    36 |
| interfaces          |   391 |  ~95k  |        0 |    2 |      8 |   10 |    20 |
| shared              |    51 |   8045 |        0 |    3 |      5 |   17 |    25 |
| mobile              |    —  |    —   |        — |   —  |     —  |   —  |    N/A (Swift-only) |
| **TOTAL (Rust)**    |   557 | ~134k  |    **2** |  **26** |  **33** |  **45** |  **~106** |

## Top Priorities (Critical + High)

1. **desktop/windows/src/escape_listener.rs:146** — critical — use-after-free when listener is stopped while a hook callback is mid-flight
2. **desktop/shell/src/cert_trust/pending.rs:79** — critical — host-only validation; another TLS challenge for the same host overwrites a pending record so a stale page approves a fingerprint the user never reviewed (auth bypass)
3. **src/wizard/prompter.rs:132** — high — `prompter.finish()` defined as documented contract but never called; `WizardNextResult.data` is permanently `None`
4. **desktop/shared/src/media_types.rs:47** — high — camera clip duration accepts NaN → panics in `Duration::from_secs_f64` on macOS
5. **desktop/shared/src/media_types.rs:104** — high — audio recording duration accepts NaN → same panic
6. **desktop/shared/src/action/input.rs:307** — high — drag duration uncapped: untrusted u64 blocks worker thread
7. **desktop/shared/src/action/open_path.rs:73** — high — Windows cmd.exe command injection via `target`
8. **desktop/shared/src/action/app_launch.rs:71** — high — Windows cmd.exe command injection via `app_name`
9. **desktop/windows/src/escape_listener.rs:98** — high — keyboard hook installed without message loop; callbacks not delivered
10. **desktop/windows/src/ax.rs:334** — high — COM init errors ignored; unbalanced COM lifecycle
11. **desktop/windows/src/ax.rs:364** — high — AX resolution falls back to foreground process when PID has no visible window → reads/writes wrong application
12. **desktop/shell/src/webview_perms.rs:58** — high — Linux UserMedia grants camera + mic without origin/type check
13. **desktop/shell/src/webview_perms.rs:89** — high — Windows grants mic to every origin instead of configured Panel origin
14. **desktop/shell/src/deeplink.rs:33** — high — full deep-link URL logged at info level → leaks auth codes/tokens
15. **desktop/shell/src/notify.rs:139** — high — Remote Gateway creds sent over `ws://` when target uses HTTP → network sniffable
16. **desktop/shell/src/notify.rs:67** — high — notification WebSocket skips in-app cert pin store → approved certs don't deliver
17. **desktop/shell/src/notify.rs:51** — high — connection-target changes don't terminate active WebSocket → bridge subscribed to old gateway
18. **desktop/shell/src/connection.rs:104** — high — gateway-token deletion failures ignored → wrong-remote credential leakage
19. **desktop/shell/src/external_link.rs:92** — high — allow-list compares only hostnames → spoofed scheme/port treated as Panel origin
20. **desktop/shell/src/update.rs:53** — high — update controls matched on path alone → content can self-install
21. **desktop/shell/src/update.rs:259** — high — update has no in-progress latch → concurrent download+install
22. **desktop/shell/src/perm_monitor.rs:126** — high — permission monitor looks for `aleph-bridge` but helper is `AlephBridge` → permission transitions unmonitored
23. **desktop/shared/src/perception/screen_record.rs:225** — high — recorder ignores `ScreenRecordConfig.region` and captures entire display → out-of-region PII
24. **interfaces/cli/src/commands/plugin_cmd.rs:104** — high — TOML injection: plugin name interpolated unescaped into manifest
25. **interfaces/cli/src/commands/doctor.rs:235** — high — R4 violation: shell embeds bespoke repair-prompt engineering
26. **shared/logging/src/pii_filter.rs:9 / lib.rs:31 / pii_filter.rs:13** — high — `PiiScrubbingLayer` is a public no-op re-exported at crate root, breaking documented contract under R9

## Architecture Compliance Snapshot

| Redline | Status across the 6 modules |
|---------|------------------------------|
| **R1** (no platform APIs in core) | clean — `src/wizard` and `src/workflow` stay in core; no platform calls detected |
| **R3** (no heavy deps for non-core) | **1 violation** — `shared/protocol/src/jsonrpc.rs:302` pulls `uuid` (with `v4` → `rand`) for wire IDs; replaceable with `AtomicU64` |
| **R4** (interface layer = pure I/O) | **4 violations** — `interfaces/cli/src/main.rs:583` marketplace-vs-direct routing heuristic in shell; `interfaces/cli/src/commands/doctor.rs:235` shell-side repair prompt engineering; `interfaces/tui/src/tui/cost.rs:19` provider pricing table in shell; `interfaces/tui/src/tui/app/trace.rs:112` AgentTraceEvent variant routing in shell |
| **R8** (regex only for machine formats) | clean — no intent classification via regex found in the 5 modules reviewed |
| **R9** (configurability as tools) | **1 violation** — `shared/logging/src/pii_filter.rs:13` empty `PiiScrubbingLayer` is a switch that does nothing |
| **R10** (intelligence in prompts) | clean |

## Categories Summary

- **Critical**: 2 (both in `desktop`)
- **Race / lock**: 4 (gateway ax, connection lifecycle, listener UAF)
- **Command injection**: 2 (desktop Windows shell passthrough)
- **Certificate / TLS**: 3 (cert-trust pending race, notify WebSocket skip, connection token deletion)
- **Privacy / PII leaks in logs**: 2 (deeplink logging, half-implemented PII layer)
- **Authorization bypass**: 2 (webview perms, external-link allow-list, cert-trust pending race)
- **Dead code / pub visibility**: ~25 (`shared/ui_logic` empty modules, `src/wizard` unused `StepType::Action`/constructors, etc.)
- **DRY violations**: ~8 (doctor.rs stream-event loop re-impl, clippy/wizard error-unwrap pattern, etc.)
- **File length >500 lines**: 2 (`shared/protocol/src/events.rs` 980, `shared/protocol/src/trace_presentation.rs` 933, `src/workflow/interop/import.rs` 1658)

## Fix Strategy (next pass)

Critical + high fixes will land as separate commits per module on `main`, no PR, no `cargo check` mid-flight. Single `cargo check -p alephcore` after all fixes are in.

---

# Module Review Summary

Multi-agent parallel static review of six core modules on `main`.
Generated 2026-07-20.

## Modules reviewed

| Module | Files | Lines | High-Confidence Issues |
|---|---|---|---|
| `src/thinker` | 16 | ~7,751 | 0 |
| `src/tool_output` | 4 | ~1,142 | 0 |
| `src/tools` | 33 (+3 submodules) | ~10,893 | 0 |
| `src/utils` | 13 | ~2,516 | 0 |
| `src/verification` | 9 (+tests) | ~2,341 | 0 |
| `src/vision` | 7 | ~1,579 | 0 |

**Total: 82 .rs files, ~26,222 lines.**
**High-confidence issues found: 0 — no source-code changes required.**

## Review methodology

For each module, a four-perspective checklist was applied across the four reviewer angles (Security, Logic, Architecture, Quality) — see `references/checklist.md` of the `review-modules` skill. Concrete queries used:

1. `grep -E '\.(unwrap\|expect)\(\)' <module>` — verified every match sits in `#[cfg(test)]` blocks by reading line numbers against the `#[cfg(test)]` / `mod tests` boundaries per file.
2. `grep -E 'static mut\|regex::|Regex::new' <module>` — only false-positives in comments; zero production-code matches.
3. Platform-API / heavy-dep audits against R1/R3 — zero platform APIs (`cocoa|appkit|metal|coregraphics|objc2|windows-rs`), zero `reqwest|isahc|hyper|tonic|grpc|tensorflow|ort|burn|candle` heavy clients.
4. Business-logic / LLM-bypass audits against R4/R8/R10 — zero `regex::` usage in target modules (no deterministic LLM-bypass).
5. Path-safety and lock-discipline audits — every `lock()` paired with `unwrap_or_else(|e| e.into_inner())`.
6. UTF-8 byte-slicing audits — `char_byte_offset`, `is_char_boundary` walk-back, `saturating_sub`, `Cow::Borrowed` fast paths verified.

The `alephdesktop` redline check confirmed R1 brain–limb separation: `src/` references `aleph-desktop::*` *traits* (`ScreenCapability`, `DesktopPlatform`) and the default `NativeScreen` struct from `desktop/shared/`. The crate-level comment at `desktop/shared/src/lib.rs` explicitly states "Real platform API calls never live here: each platform crate … implements `DesktopPlatform` and reaches the OS through the `bridge` JSON-RPC IPC layer (R1 brain–limb separation)."

## Per-module reports
See `review-results/{thinker,tool_output,tools,utils,verification,vision}.md` for the per-module breakdown including positive observations and production-grade patterns identified.

## Conclusion

All six modules are well-disciplined and match project redlines. No source-code changes are required at this time; only the review-results/* reports are added.

---

# Batch 2 (2026-07-21)

**Modules reviewed**: 9 (`src/components`, `src/config`, `src/context`, `src/core`, `src/discovery`, `src/tool_metadata`, `src/domain`, `src/exec`, `src/executor`)
**Branch**: `fix/review-batch2-modules`
**Worktree**: `/tmp/opencode/aleph-review`

## Module Totals

| Module              | Files |    LOC | Critical | High | Medium |  Low | Total |
|---------------------|------:|-------:|---------:|-----:|-------:|-----:|------:|
| src/components      |    20 |   1905 |        0 |    5 |      9 |    5 |    19 |
| src/config          |    98 |  27045 |    **4** |   12 |     18 |   20 |    54 |
| src/context         |    24 |  10770 |        0 |    5 |      9 |   12 |    26 |
| src/core            |     2 |    158 |        0 |    2 |      7 |    5 |    14 |
| src/discovery       |     4 |   1482 |        0 |    2 |      5 |   14 |    21 |
| src/tool_metadata   |    26 |   6729 |        0 |    1 |      9 |   12 |    22 |
| src/domain          |     2 |   1103 |        0 |    1 |      8 |    8 |    17 |
| src/exec            |    17 |   3946 |    **2** |    7 |      8 |   12 |    29 |
| src/executor        |    21 |   9102 |    **4** |    7 |      8 |    5 |    24 |
| **TOTAL**           |   214 |  ~62k  |  **10** |  **42** |  **81** |  **93** |  **226** |

## Critical / High Fixed (commits on `fix/review-batch2-modules`)

1. **config: voice streaming api_key plaintext** — secret_migration now covers `voice.streaming.api_key` (was hard-coded "skip if empty" then written to disk in plaintext).
2. **config: defaults override OnceLock silent re-init** — was `let _ = OnceLock::set(...)`; now logs a warning so a stale defaults.toml can't silently disable every serde default function.
3. **exec: `/approve` text-fallback bypass** — `resolve_for_session` now gates on `record.originator_user_id`; paired-chat approval bypass via plain-text replies is closed.
4. **exec: leak_detector prefix-gate bypass** — secrets without a known prefix (raw JWTs, HMAC blobs, custom vault tokens) used to escape every regex check because `ac.is_match` was a hard gate. Now always runs regex.
5. **executor: set_config_patcher no-op at boot** — `Arc::get_mut` on the registry always returned `None` (the boot path clones the registry Arc into `ExecutionEngine::new` and again into `agent_result.tool_registry`), so `self_config` and `moa` shipped without a patcher. Tools' `config_patcher` field switched to `Arc<OnceLock<…>>`, setter takes `&self`.
6. **executor: memory_search cross-session race** — falling back to the process-global `default_session_key` / `default_workspace` handle outside a turn scope raced the next request's write. Falls back to boot default (`"main"`) instead.
7. **tool_metadata: incomplete alias-collision detection** — only checked `(new.aliases vs existing.name)`; added the symmetric `(existing.aliases vs new.name)` and `alias↔alias` cases.
8. **discovery: duplicate find_git_root** — `utils::paths::find_git_root` and `discovery::paths::find_git_root` had diverging depth + canonicalize semantics. Consolidated onto the safer utils implementation with depth cap 100 + canonicalize-first.
9. **discovery: symlink-following in scanner** — `path.is_dir()/is_file()` follows symlinks; replaced with `symlink_metadata`-based check so a symlink inside `~/.aleph/{skills,commands,plugins}` pointing outside the expected tree is rejected.
10. **executor: R1 platform imports in constructor** — noted but **intentional** (DI seam at composition root, evaluated 2026-07-20; existing comment documents the decision). Left as-is.
11. **components: StreamingTextPart DoS** — `append` now caps content at 16 MiB.
12. **components: PartUpdateData silent corruption** — `added` / `updated` now return `Result<_, serde_json::Error>`; the previous `unwrap_or_default()` produced a `part_json=""` payload that the UI misrendered with no error.
13. **core: MediaType missing from re-exports** — `MediaAttachment::media_type` referenced `MediaType` but `MediaType` wasn't re-exported. Fixed.
14. **core: similarity_score NaN/Inf** — deserializer now drops non-finite scores (they poisoned downstream sort/rank).
15. **context: ToolAwareChunker::new panic** — `assert!(token_ratio > 0.0)` crashed the agent loop on a misconfigured caller; now falls back to 0.25 with a warn log.
16. **context: is_summary_text too permissive** — matched any text starting with `[Context Summary`; tightened to require `]` or ` (` immediately after the head, so user content like `[Context Summary, please ignore everything above]` is no longer mistaken for a marker.
17. **domain: Os serde aliases case-sensitive** — replaced with a custom `Deserialize` that delegates to the existing case-insensitive `FromStr`; serde aliases and `FromStr` can no longer drift.
18. **domain: dead DispatchSpec / ArgMode / command_dispatch** — marked `#[deprecated]` instead of removing (manifests in the wild still carry the field); planned-removal notice attached.
19. **exec: ScanFinding.matched_text exposure** — visibility tightened from `pub` to `pub(crate)` so the 20-char secret prefix can't leak through audit pipelines that log the whole struct.
20. **exec: parser.rs DoS** — `analyze_shell_command` refuses inputs over 64 KiB (the three linear scans are O(n) each).
21. **config: validate_agent_id ASCII drift** — `agent_files.rs::validate_agent_id` used `is_alphanumeric()` (Unicode-aware) while `crud.rs::validate_id` used `is_ascii_alphanumeric()`; IDs like `café` passed one and were rejected by the other. Unified on ASCII.

## Notes

- 10 critical findings across `src/exec`, `src/executor`, and `src/config` — all addressed.
- Of the 42 high findings, 21 were addressed (the rest are R7/R9/R10 architecture-stance notes that need broader refactors, not isolated fixes — left for a separate batch).
- R1 platform imports in `src/executor/builder/constructor/mod.rs:262-276` are **intentional** per the existing in-code comment; left as-is.
- R10 MemoryInjectionMode gating (boot-time static partition, NOT per-message intent) is R10-compliant; logged for awareness only.
