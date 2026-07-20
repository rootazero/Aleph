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
