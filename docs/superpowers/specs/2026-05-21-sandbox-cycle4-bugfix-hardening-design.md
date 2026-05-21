# Sandbox Cycle 4 — Bug-fix & Hardening (codex-parity)

**Date:** 2026-05-21
**Branch:** `sandbox-cycle4-bugfix-hardening`
**Predecessors:** [Cycle 2](../../reference/SANDBOX.md) (`1f5545eea`), Cycle 3 (`232bf235d`)

## Context

Cycles 2 & 3 ported codex's SBPL platform defaults, Windows protected-metadata
DACL, and Linux socket-family seccomp. A fresh codex-vs-Aleph deep-dive of the
whole `src/sandbox/` subsystem surfaced a set of **concrete bugs** in Aleph's
own code — independent of codex feature parity. This cycle fixes them.

The remaining large codex-parity feature — per-host network filtering via a
managed proxy — is genuinely multi-cycle (needs CAP_NET_ADMIN / admin or a
proxy backend) and stays deferred, consistent with Cycle 3's honest-deferral
precedent.

## Bugs found

Rows 1–3, 9, 10 are fixed by this cycle. Rows 11–12 were surfaced by the
same deep-dive but were fixed **independently on `main`** (`c5f5e384b`,
`c0b808ed9`) while this branch was in flight — the merge takes `main`'s
version, so this cycle does not re-fix them.

| # | Severity | File | Defect |
|---|----------|------|--------|
| 1 | **CRITICAL** | `platforms/linux/bwrap.rs` | Test `generate_args_workspace_only_without_platform_defaults` constructs `LinuxSandboxOptions` with 3 of 8 fields → `cargo test` does not compile on Linux. The struct already has a `Default` impl. |
| 2 | **HIGH** | `config.rs` / `platforms/mod.rs` / `platforms/windows/driver.rs` | `WindowsSandboxConfig.use_job_object` (default `true`) and `max_active_processes` (default `8`) are never threaded into `WindowsSandboxOptions`. The driver hard-codes the job-object active-process limit as `if allow_fork {32} else {1}` and always creates a job. Two config fields are dead. |
| 3 | MEDIUM (security) | `workspace.rs` | `normalize_path` collapses `.`/`..` **lexically** only. A symlink inside the workspace whose name passes the lexical `starts_with(ws.cwd)` prefix check but whose target is outside the workspace escapes the cwd jail. codex canonicalizes. |
| 9 | MEDIUM | `platforms/{macos/seatbelt,linux/bwrap}.rs` | `SandboxOutput.signal` is hard-coded `None`. A child killed by a signal (SIGSEGV, SIGKILL from an rlimit/cgroup breach) reports `exit_code: None, signal: None` — the caller cannot tell it was signalled. Windows has no Unix signals, so its `None` is correct and unchanged. |
| 10 | LOW | all three drivers | Output truncation slices the raw `Vec<u8>` at `[..max_output_bytes]`, which can cut a UTF-8 codepoint in half (one `U+FFFD` downstream). Violates project rule P7 (UTF-8 safety). The truncation block is **triplicated** verbatim across the three drivers (rule-of-three violation). |
| 11 | **CRITICAL** (fixed on `main`) | `windows_init.rs` | `alephcore` did **not compile for Windows at all** — 6 `windows-sys` 0.61 API-drift errors: `GENERIC_ALL`/`GENERIC_WRITE` moved `System::SystemServices` → `Foundation`; `SE_GROUP_INTEGRITY` moved `Security` → `System::SystemServices`; `DeriveCapabilitySidsFromName` moved `Security::Isolation` → `Security`; `SE_GROUP_*` are now `i32` constants assigned to `u32` fields. Aleph ships a Windows `.msi`, so this is severe. Fixed on `main` by `c5f5e384b`. |
| 12 | HIGH (fixed on `main`) | `bin/aleph-server/daemon.rs` | `expand_path` called `libc::getuid()` unconditionally, so the `aleph-server` binary did not compile for Windows. Fixed on `main` by `c0b808ed9`. |

## Approach

### BUG-1 — replace the 3-field literal with `..LinuxSandboxOptions::default()`

The struct has had a `Default` impl since SP-5. One-line test fix.

### BUG-2 — thread the two config fields through

- `WindowsSandboxOptions` gains `use_job_object: bool` and
  `max_active_processes: u32`. Its `#[derive(Default)]` is replaced with a
  manual `Default` carrying the same production defaults as
  `WindowsSandboxDriver::new()` (which then delegates to `Default`).
- `create_platform_driver_with_config` copies both fields from
  `windows_config`.
- `WindowsSandboxDriver::run`:
  - Job object becomes `Option<SandboxJob>` — `None` when
    `use_job_object == false` (config honored; process runs without the
    job's kill-on-close + UI restrictions, as the operator asked).
  - Active-process limit: `if allow_fork { max_active_processes.max(1) }
    else { 1 }` — a forking command is capped by the configured ceiling
    (8 by default vs the old hard-coded 32); a non-forking command stays
    pinned to 1. `.max(1)` guards against a `0` misconfiguration that
    would otherwise make the job kill everything.

### BUG-3 — canonicalize before the containment check

In `WorkspaceSandbox::execute`, after lexical `normalize_path`, resolve the
candidate cwd and the workspace root with `tokio::fs::canonicalize` and
compare the **canonical** paths. A cwd that fails to canonicalize (does not
exist, or is a dangling symlink) is treated as outside the jail and denied
through the existing hook-aware denial path. `normalize_path` is kept — it
still turns a relative path absolute against `ws.cwd` before canonicalize.

### BUG-9 + BUG-10 — one shared helper, used by all drivers

A single `platforms/common.rs` addition removes the triplication and fixes
both bugs:

```rust
/// Truncate captured output to ≤ max_bytes, never splitting a UTF-8
/// codepoint (project rule P7). Returns the buffer + whether it was cut.
pub fn truncate_output(buf: Vec<u8>, max_bytes: usize) -> (Vec<u8>, bool);

/// The Unix signal that terminated a child, if any. `#[cfg(unix)]`.
pub fn termination_signal(status: &std::process::ExitStatus) -> Option<i32>;
```

`truncate_output` backs the cut index off any UTF-8 continuation byte
(`0b10xx_xxxx`); worst case for binary output drops ≤3 extra bytes.
All three drivers call `truncate_output`; the two Unix drivers also call
`termination_signal`. Windows keeps `signal: None`.

## Out of scope (documented follow-ups)

- **Linux protected-path creation gap** — `push_metadata_protection_args`
  uses `--ro-bind-try`, which silently no-ops for a protected metadata
  path (`.git`/`.aleph`/`.codex`/`.agents`) that does not yet exist,
  letting a sandboxed process `mkdir .git` and write inside it. macOS
  denies this (`deny file-write*` applies whether or not the path
  exists); Linux is inconsistent. The fix (codex-style synthetic empty
  read-only bind mount) is **new Linux-only logic that cannot be
  compile-verified on this macOS dev box** (no `x86_64-unknown-linux-gnu`
  target). Deferred to a Linux-capable session. Recorded in SANDBOX.md.
- **Per-host network filtering** (managed proxy / WFP / nftables) — still
  multi-cycle, unchanged from Cycle 3.
- **`WorktreeSandbox` is a non-enforcing `Sandbox`**, **dead
  `FullRead`/`FullWrite`/`ProxyOnly` policy variants**, **decorative
  `EnvPolicy`** — design-level concerns, not surgical fixes; a refactor
  cycle, not this one.

### BUG-11 + BUG-12 — the Windows build (fixed on `main`)

This deep-dive independently re-discovered the broken Windows build
and had a fix staged, but `main` shipped equivalent fixes
(`c5f5e384b`, `c0b808ed9`) first. The Cycle 4 merge therefore takes
`main`'s `windows_init.rs` and `daemon.rs` verbatim. Cycle 4's lasting
contribution to the Windows story is operational: it installed
`mingw-w64` so `cargo check --target x86_64-pc-windows-gnu` can
cross-compile (the `ring` build script needs a Windows C toolchain) —
which is what lets the Windows build be verified at all on a macOS
dev box.

## Verification

- macOS: `cargo check -p alephcore --tests` → exit 0;
  `cargo test -p alephcore --lib sandbox::` → 199/199 pass (188
  baseline + 11 new).
- Windows: `cargo check -p alephcore --target x86_64-pc-windows-gnu`
  → exit 0. `mingw-w64` installed via Homebrew so `ring`'s build
  script can cross-compile.
- Linux: BUG-1 verified by inspection (`Default` exists, unambiguous);
  BUG-9/10 in `bwrap.rs` are mechanical helper-call swaps against
  helpers already compiled on macOS. No `x86_64-unknown-linux-gnu`
  target installed (a glibc cross-toolchain is impractical on this
  macOS box) — same constraint as Cycles 2–3.
