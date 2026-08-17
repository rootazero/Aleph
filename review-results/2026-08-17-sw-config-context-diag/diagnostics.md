# src/diagnostics — Severed Wire Audit (2026-08-17 round)

**Scanned:** 19 files under `src/diagnostics/` (incl. `checks/`)

**Summary:** 0 candidates. No severed wires.

---

All 14 HealthCheck impls are registered:
- `default_registry()` at `src/diagnostics/mod.rs:71-82` — 12 checks (DataDir, LoopGraph, CacheHealth, CacheHitRate, StaleLock, SqliteIntegrity, DiskSpace, ConfigParse, Vault, HooksConsent, BrowserRuntime, DuplicateInstance)
- `with_runtime_checks(...)` at `src/diagnostics/mod.rs:100` — ProvidersConnectivity
- `with_extension_usage_check(...)` at `src/diagnostics/mod.rs:121` — IdleExtensions

Three live consumer faces all wire the full battery:
1. `doctor` builtin tool at `src/builtin_tools/doctor.rs:121-125` — builds default_registry + runtime + extension
2. `diagnostics.run` RPC handler at `src/gateway/handlers/diagnostics.rs:77-80` — same
3. `aleph doctor` CLI at `src/bin/aleph-server/commands/doctor.rs:17` — default_registry only

`HooksConsentCheck::diagnose` is also consumed directly by `aleph hooks doctor` subcommand at `src/bin/aleph-server/commands/hooks.rs:217` (entropy-reduction pattern, noted in the check's own doc comment).

`redact_secrets` has three external consumers beyond the engine chokepoint:
- `src/config/patcher.rs:502`
- `src/gateway/handlers/providers/types.rs:32`
- `src/gateway/handlers/providers/handlers.rs:657`

No TODO/unimplemented stubs, no inert config, no path/name drift, no feature-gated dead code, no client ghosts.