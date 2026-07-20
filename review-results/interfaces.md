# Module: interfaces

- Path: `interfaces/` (cli, tui, webchat)
- Files scanned: 391 (incl. generated/embedded)
- Total LOC: ~95k

## Summary
| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 2 |
| medium   | 8 |
| low      | 10 |
| **Total**| **20** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
No findings.

### Perspective 2 — Logic & Correctness
```
ISSUE|interfaces/tui/src/tui/cost.rs:19|medium|R4 violation: hardcoded provider pricing table with substring match lives in shell layer; core owns pricing
```

### Perspective 3 — Architecture Compliance
```
ISSUE|interfaces/cli/src/main.rs:583|medium|R4 violation: marketplace-vs-direct routing decision lives in shell layer (heuristic from source string)
ISSUE|interfaces/cli/src/commands/doctor.rs:235|high|R4 violation + DRY: shell embeds bespoke repair-prompt engineering that lists specific tools
ISSUE|interfaces/cli/src/commands/plugin_cmd.rs:104|high|TOML injection: plugin name interpolated unescaped into manifest.toml — quote/backslash/newline in name breaks TOML or smuggles fields
ISSUE|interfaces/tui/src/tui/app/trace.rs:112|medium|R4-adjacent: AgentTraceEvent variant routing for business logic hardcoded in shell layer
```

### Perspective 4 — Code Quality (selected low/medium)
```
ISSUE|interfaces/cli/src/commands/daemon.rs:47|low|PID u32 → i32 cast wraps silently on systems with PIDs > i32::MAX
ISSUE|interfaces/cli/src/commands/plugins_cmd.rs:147|medium|TOCTOU + predictable temp dir: filename from URL path segment written into /tmp/aleph-plugin-download
ISSUE|interfaces/cli/src/output/spinner.rs:107|low|Spinner Drop aborts without awaiting cleanup; line-clear escape may not run on panic
ISSUE|interfaces/tui/src/tui/mod.rs:73|low|Panic-hook restoration order fragile on all paths
ISSUE|interfaces/tui/src/tui/commands.rs:84|low|Unused `textarea` parameter
ISSUE|interfaces/tui/src/tui/slash.rs:148|low|/tools subcommand normalizes cmd but not args — /tools VERBOSE silently no-ops
ISSUE|interfaces/webchat/src/context.rs:692|medium|RPC call uses hardcoded 30s timeout with no backpressure — slow server causes global UI stall
ISSUE|interfaces/webchat/src/platform/wide/views/settings/security/mod.rs:100|medium|js_sys::eval used for regex validation — CSP-unfriendly
ISSUE|interfaces/webchat/src/context.rs:393|low|Hardcoded fallback gateway URL when window/location unavailable
ISSUE|interfaces/webchat/src/state/hotkey.rs:75|low|install() not idempotent — global keydown listener leaked without guard
ISSUE|interfaces/webchat/src/app.rs:139|low|30fps set_interval runs forever; wakes scheduler every 33ms
ISSUE|interfaces/webchat/src/components/chat_sidebar.rs:498|medium|Poll-and-wait subscription race: hardcoded 5s deadline with magic numbers
ISSUE|interfaces/webchat/src/context.rs:1239|low|Unresolved architectural debt: TODO comment marks alert-system integration as requiring refactor
ISSUE|interfaces/cli/src/commands/connect.rs:14|low|Magic number: Spinner frame started twice in parallel without drop-guard ordering
ISSUE|interfaces/cli/src/commands/doctor.rs:262|medium|Massive DRY violation: launch_llm_repair re-implements entire stream-event loop
```

### Perspective 1 alt-found
(cli/commands/doctor.rs is documented above; CLI plugin_cmd.rs:104 is the high-severity item.)
