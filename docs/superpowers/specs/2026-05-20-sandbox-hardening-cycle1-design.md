# Sandbox Hardening Cycle 1 — Design

**Date**: 2026-05-20
**Scope**: A + C (bug fixes + resource limits)
**Reference**: Compared against codex sandbox (`/Volumes/TBU4/Github/codex`) — `codex-rs/sandboxing/`, `linux-sandbox/`, `windows-sandbox-rs/`

---

## 1. Background

Aleph already has a 5,200-line sandbox subsystem with the right abstractions: `Sandbox` trait, `WorkspaceSandbox` 6-step pipeline, per-OS `OsSandboxDriverTrait` impls (`SeatbeltDriver` / `BubblewrapDriver` / `WindowsSandboxDriver`), and a working HITL `ApprovalGate` wired into the channel-approval bridge. The skeleton is solid.

However, a direct comparison against codex's three-OS sandbox surfaced four classes of defect in Aleph's current implementation:

### 1.1 Silent policy degradation (P7 violation)

The Linux driver currently logs a warning and falls back to `--unshare-net` when callers ask for `NetworkPolicy::AllowHosts` or `NetworkPolicy::ProxyOnly` (`src/sandbox/platforms/linux/bwrap.rs:117-130`). The macOS driver accepts hostnames in `NetworkPolicy::AllowHosts` and embeds them in `(allow network-outbound (remote ip "example.com"))` — but Seatbelt's `remote ip` matcher takes IP literals, not hostnames, so the rule never matches and the policy is silently void (`src/sandbox/platforms/macos/seatbelt.rs:244-253`). The Windows driver doesn't even attempt enforcement.

All three cases violate P7 ("fail fast with clear error messages"): the caller is told "your network policy is in effect" while it isn't.

### 1.2 Windows defense-in-depth gap

`src/sandbox/platforms/windows/driver.rs:139` imports `create_restricted_token` but never calls it. The actual process spawn at line 158 is plain `tokio::process::Command::spawn()` with only a JobObject for protection. The 267-line `token.rs` is dead code in production — driving a false sense of security.

### 1.3 Stub façades pretending to be implementations

Four Windows files claim to implement security primitives but are pure no-ops with zero production callers (verified via `grep`):

| File | Lines | Status |
|---|---|---|
| `wfp.rs` | 150 | `WfpFilter::new()` hardcoded `Err`; all other methods `Ok(())` no-ops |
| `appcontainer.rs` | 192 | Every method returns `Err("requires windows-sys 0.61+")` |
| `acl.rs` | 136 | `dacl_allows_access` defined; **zero callers** anywhere in tree |
| `filter.rs` | 192 | `FilterSet`/`FilterRule` defined; only own-file tests reference them |

`appcontainer.rs` is `pub use`'d from `windows/mod.rs:21`, leaking misleading API stubs into the crate's public surface. The remaining three are mod-private and entirely unreferenced.

This is the exact anti-pattern R10 calls out under "YAGNI 撤回模式": speculative scaffolding that pretends to be a feature, with zero current consumers.

### 1.4 Resource limits never enforced

`SandboxPolicy::process::max_memory_mb` exists (`src/sandbox/policy.rs:53`) but:

- `SandboxCapabilities` has no equivalent field — callers cannot express memory limits at all.
- `From<&SandboxCapabilities> for SandboxPolicy` hardcodes `max_memory_mb: None` (line 105) and `timeout_secs: 60` (line 104).
- None of the three drivers consume `policy.process.max_memory_mb` — even the Windows driver that parses it into `ParsedProfile.max_memory_mb` (driver.rs:273) never threads it into `SandboxJob::new`.

The field is end-to-end dead. Same pattern as recent Aleph "structurally-dead chain" cases (cron/heartbeat, MCP, exec-approval).

### 1.5 Misleading documentation

`src/sandbox/capabilities.rs:7-8` documents that Linux `AllowHosts` "is enforced via bubblewrap network namespaces combined with seccomp or iptables rules" — but the actual implementation drops the rule on the floor. Lines 10-18 document Windows `AllowHosts` as a deliberate `AllowAll`/`None` fallback ("not enforceable via standard Windows APIs") — but the actual driver behavior is undefined because the WFP stub never executes.

---

## 2. Goals

1. **Zero silent policy degradation.** Every unsupported policy/feature on every OS returns a typed error with a remediation hint.
2. **`max_memory_mb` actually limits process memory** on all three platforms.
3. **Delete dead Windows stubs** (wfp.rs, appcontainer.rs, acl.rs, filter.rs, token.rs) — ~937 lines of `pub use`'d falsehood.
4. **Honest documentation** in `capabilities.rs` and `docs/reference/SANDBOX.md`: only document what the code actually does.

## 3. Non-goals (deferred to future cycles)

- **Full WFP** Windows network filtering — requires admin-only Win32 surgery, separate spec.
- **Landlock + seccomp-bpf** Linux defense-in-depth — Cycle 2; standalone spec needed.
- **AppContainer** Windows isolation — requires `windows-sys` major-version upgrade.
- **Hostname-based filtering** on macOS/Linux — needs proxy-based design (codex uses port-based proxy allowlists for this reason); standalone spec.
- **Windows RestrictedToken wiring via `CreateProcessAsUserW`** — significant unsafe Win32 surgery (~200 lines) not testable from a Mac dev box; deferred to a Windows-focused cycle once we have CI coverage.
- **cgroups v2 memory limits on Linux** — `setrlimit(RLIMIT_AS)` covers the bulk; cgroup adds enforcement against `mmap` games but is more invasive. Cycle 2 territory.

## 4. Architecture overview

No new modules. All changes happen inside `src/sandbox/`. Wiring shape stays identical to today:

```
SandboxCapabilities (caller)
   │   + new fields: max_memory_mb, timeout_secs
   ▼
SandboxPolicy (internal)         ◄── transparent passthrough
   │
   ▼
OsSandboxDriverTrait::profile_for
   │   returns OsSandboxProfile { contents, max_memory_mb (new) }
   ▼
OsSandboxDriverTrait::run
   │   applies setrlimit via pre_exec (mac+linux) / JobObject memory cap (win)
   │   spawns process via tokio Command
   ▼
SandboxOutput
```

The trait surface change is one new field on `OsSandboxProfile` (additive). The capability surface change is two new optional fields on `SandboxCapabilities` (additive, serde-defaulted).

## 5. Component-by-component design

### 5.1 Capability and policy plumbing

**`src/sandbox/capabilities.rs`**

Add two optional fields:

```rust
pub struct SandboxCapabilities {
    // ... existing fields ...
    #[serde(default)]
    pub max_memory_mb: Option<u64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}
```

Update `is_within` to treat memory and timeout as "tighter is stricter" — child's limit must be `<=` baseline's (or baseline must be `None` meaning unlimited). Rewrite the misleading platform-restriction docstring at the top of the file to reflect what the code actually does after this cycle.

**`src/sandbox/policy.rs`**

Rewrite `From<&SandboxCapabilities> for SandboxPolicy` to pull `max_memory_mb` and `timeout_secs` from the caps instead of hardcoding `None`/`60`. The default for `timeout_secs` stays at 60 when the cap is `None`.

**`src/sandbox/command.rs`**

Add error variant:

```rust
#[error("sandbox policy not supported on {platform}: {feature} — {reason}")]
UnsupportedPolicy {
    platform: &'static str,
    feature: String,
    reason: String,
},
```

**`src/sandbox/driver.rs`**

Extend `OsSandboxProfile`:

```rust
pub struct OsSandboxProfile {
    pub contents: String,
    pub max_memory_mb: Option<u64>,
}
```

Existing mock drivers in `factory.rs`, `workspace.rs`, and `platforms/mod.rs` need their `Ok(OsSandboxProfile { contents: ... })` literals updated to include `max_memory_mb: None`. Counted: 5 construction sites.

### 5.2 Hard-fail unsupported network policies

**Linux — `src/sandbox/platforms/linux/bwrap.rs`**

In `generate_args`, replace the warn-and-degrade branches with errors:

```rust
NetworkPolicy::AllowHosts(_) => {
    return Err(SandboxError::UnsupportedPolicy {
        platform: "linux/bwrap",
        feature: "AllowHosts".into(),
        reason: "fine-grained network filtering requires landlock+seccomp or iptables (deferred to a future cycle). Use AllowAll or None.".into(),
    });
}
NetworkPolicy::ProxyOnly { .. } => {
    return Err(SandboxError::UnsupportedPolicy {
        platform: "linux/bwrap",
        feature: "ProxyOnly".into(),
        reason: "proxy-only network mode requires a managed proxy backend (deferred to a future cycle). Use AllowAll or None.".into(),
    });
}
```

Update `generate_args_allow_hosts_fallback` test (line 477) to assert `Err(UnsupportedPolicy { .. })`.

**Windows — `src/sandbox/platforms/windows/driver.rs`**

Same pattern in `generate_profile`. Reason text references "WFP-backed enforcement deferred to a future cycle". Profile generation never silently produces a `network=allow_hosts` line that the run path can't honor.

**macOS — `src/sandbox/platforms/macos/seatbelt.rs`**

For `AllowHosts`, validate each entry parses as `IpAddr` (use `std::net::IpAddr::from_str`). Any non-IP entry → `UnsupportedPolicy` with reason "macOS Seatbelt 'remote ip' matches IP literals only; resolve hostnames before passing them". Pure-IP lists continue to work (codex's same primitive is IP-only too).

For `ProxyOnly`, current implementation generates valid SBPL targeting `localhost:port` — keep as-is.

### 5.3 `max_memory_mb` enforcement

**macOS — `src/sandbox/platforms/macos/seatbelt.rs`**

`profile_for` populates `OsSandboxProfile { contents, max_memory_mb: policy.process.max_memory_mb }`. `run` reads the field and, if `Some(mb)`, registers a `pre_exec` hook:

```rust
// SAFETY: setrlimit with RLIMIT_AS is async-signal-safe and
// well-defined. The limit is inherited by the exec'd child.
cmd.pre_exec(move || {
    let bytes = mb.saturating_mul(1024 * 1024);
    let rlim = libc::rlimit { rlim_cur: bytes, rlim_max: bytes };
    if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
});
```

The `pre_exec` runs in the `sandbox-exec` child between `fork` and `exec`; the limit propagates to the eventual target binary via the standard rlimit inheritance.

**Linux — `src/sandbox/platforms/linux/bwrap.rs`**

Same `pre_exec` setrlimit on the bwrap process. Bwrap then `exec`s the target, which inherits the limit. Same SAFETY comment.

Note: bubblewrap itself doesn't have a `--rlimit` flag; pre_exec on the bwrap process is the cleanest mechanism. cgroups v2 would give better enforcement (covers `mmap` overcommit) but is deferred — `RLIMIT_AS` catches the common overrun case.

**Windows — `src/sandbox/platforms/windows/job.rs`**

Change signature:

```rust
pub unsafe fn new(
    max_active_processes: u32,
    max_memory_mb: Option<u64>,
) -> Result<Self, String>
```

When `max_memory_mb.is_some()`:

- OR `JOB_OBJECT_LIMIT_PROCESS_MEMORY` into `LimitFlags`.
- Set `ExtendedLimitInformation.ProcessMemoryLimit = mb * 1024 * 1024` (as `usize`).

`src/sandbox/platforms/windows/driver.rs` passes `parsed.max_memory_mb` into `SandboxJob::new` at the call site (currently line 153).

### 5.4 Dead-code dissolution

Delete in entirety:

- `src/sandbox/platforms/windows/wfp.rs` (150 lines) — every method is a stub, zero callers.
- `src/sandbox/platforms/windows/appcontainer.rs` (192 lines) — every method errors; `pub use` removes misleading public-API surface.
- `src/sandbox/platforms/windows/acl.rs` (136 lines) — `dacl_allows_access` has zero callers (verified by tree-wide grep).
- `src/sandbox/platforms/windows/filter.rs` (192 lines) — `FilterSet` referenced only by own-file tests.
- `src/sandbox/platforms/windows/token.rs` (267 lines) — `create_restricted_token` only "consumed" by the unused `use` in driver.rs; helpers `set_default_dacl` / `world_sid` are internal to this file.

Total: **937 lines deleted**.

Update `src/sandbox/platforms/windows/mod.rs`:

- Remove `mod acl;`, `mod appcontainer;`, `mod filter;`, `mod token;`, `mod wfp;`.
- Remove `pub use appcontainer::{AppContainer, AppContainerCapability};`.
- Keep `mod job;` and `pub mod driver;`.
- Rewrite the file's docstring to describe what the code actually provides (JobObject + UI restrictions) rather than what it aspired to.

In `windows/driver.rs`, delete the unused `use super::token::create_restricted_token;` (line 139).

This is the R10 "YAGNI 撤回模式" applied literally: zero current consumers → delete. A future spec that implements RestrictedToken or WFP can re-introduce a focused, working module without inheriting dead skeletons.

### 5.5 Documentation refresh

**`src/sandbox/capabilities.rs`** — rewrite top-of-file docstring:

- Remove the lie about Linux iptables/seccomp enforcement.
- Remove the contradictory Windows AllowHosts paragraph (it now errors, not falls back).
- Document the new `max_memory_mb` / `timeout_secs` fields with their per-OS mechanism.

**`docs/reference/SANDBOX.md`** — add a "Cycle 1 hardening (2026-05-20)" section noting:

- AllowHosts/ProxyOnly now error on Linux+Windows (and on macOS for non-IP hosts) instead of silently degrading.
- `max_memory_mb` is honored on all three OSes via the per-OS mechanisms above.
- 937 lines of speculative Windows scaffolding removed; current Windows sandbox = JobObject + UI restrictions only. RestrictedToken / WFP / AppContainer are explicitly deferred work, not "partially implemented".

## 6. Testing strategy

### 6.1 Unit tests (per driver, hosted in module's own `#[cfg(test)]`)

- **Linux**: `generate_args_allow_hosts_returns_unsupported`, `generate_args_proxy_only_returns_unsupported`. Update existing `generate_args_allow_hosts_fallback` to assert the new error.
- **Windows**: `generate_profile_allow_hosts_returns_unsupported`, `generate_profile_proxy_only_returns_unsupported`. Update existing positive tests for `AllowHosts` to use a different policy.
- **macOS**: `generate_profile_allow_hosts_ip_succeeds`, `generate_profile_allow_hosts_hostname_returns_unsupported`. Update existing `generate_profile_with_network` to use IP literals.
- **All three**: `generate_profile_threads_memory_limit` — assert profile carries `max_memory_mb` through.
- **`policy.rs`**: `from_caps_threads_memory_limit`, `from_caps_threads_timeout`.
- **`capabilities.rs`**: `is_within_respects_memory_tighter`, `is_within_respects_timeout_tighter`.

### 6.2 Integration tests

- Extend `tests/sandbox_capability_approval.rs`:
  - Add a `memory_limit_threads_through_pipeline` scenario using the existing `RecordingDriver` to assert the value reaches the driver.
  - Add an `unsupported_policy_returns_typed_error` scenario covering the Linux AllowHosts path.

### 6.3 OS-specific runtime tests

Gate with `#[cfg(target_os = "...")]`. Most existing driver tests don't actually run the sandbox (they test `generate_args` / `generate_profile`). For real rlimit verification we add one gated integration test per OS that spawns a tiny memory-bombing program (e.g., a python one-liner allocating 1GB with `max_memory_mb = 64`). Marked `#[ignore]` for default CI; runs on demand via `cargo test --ignored -p alephcore sandbox::rlimit`.

### 6.4 Baseline-failure check

Per `project_baseline_test_failures.md`, main has 19 pre-existing `cargo test --lib` failures plus one deadlocking concurrency test (`parallel_adds_do_not_lose_entries`). Verification is `(failures-on-branch) - (failures-on-fork-point) == 0`.

## 7. Implementation sequence

Branch worktree from main; commit per layer; no squash until merge.

1. **S1 foundation** — `SandboxError::UnsupportedPolicy`; extend `OsSandboxProfile` and `SandboxCapabilities`; update `From<&SandboxCapabilities>`; update all 5 `OsSandboxProfile` construction sites in mock/unsupported drivers. Compile-clean.
2. **S2 hard-fail Linux** — bwrap.rs returns `UnsupportedPolicy` for AllowHosts/ProxyOnly. Update test.
3. **S3 hard-fail Windows** — driver.rs returns `UnsupportedPolicy`. Update tests.
4. **S4 hard-fail macOS** — seatbelt.rs validates IP literals in AllowHosts. Update tests.
5. **S5 memory cap macOS** — pre_exec setrlimit. New unit test.
6. **S6 memory cap Linux** — pre_exec setrlimit. New unit test.
7. **S7 memory cap Windows** — extend `SandboxJob::new` signature; wire into driver. New unit test.
8. **S8 dissolution** — delete five files; clean up `mod.rs` and the unused `use` in driver.rs.
9. **S9 docs** — rewrite `capabilities.rs` docstring; update `SANDBOX.md`.
10. **S10 verify** — `cargo check -p alephcore`, `cargo test -p alephcore --lib`, `cargo clippy -p alephcore -- -D warnings` on changed files. Diff failures against baseline list.

## 8. Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| Windows code change breaks Win-only paths Mac can't compile | Med | Strict `#[cfg(target_os = "windows")]` discipline; rely on Github Actions Windows runner |
| Removing `pub use AppContainer` breaks an unknown external consumer | Low | Tree-wide grep confirmed zero consumers; if anything emerges, compile error is immediate |
| `OsSandboxProfile` struct change forces 5 mock-driver updates | Cert | Pre-flight grep already counted sites; all in-tree |
| `setrlimit(RLIMIT_AS)` over-counts virtual memory and kills processes that mmap large files | Med | Document the semantics; cgroups v2 (RSS-based) is the Cycle-2 follow-up |
| Hard-failing AllowHosts breaks an in-tree caller that expected silent degradation | Med | Grep callers before merge; convert any found callers to `AllowAll` (with explicit comment) |
| Test count increase pushes CI runtime | Low | New tests are pure-Rust unit tests; gated rlimit tests are `#[ignore]` |

## 9. Out-of-scope follow-up specs

After this cycle ships, the following items are tracked but unstarted:

- **SP-2 Linux landlock + seccomp-bpf** — defense-in-depth layered on bwrap.
- **SP-3 Windows RestrictedToken** — `CreateProcessAsUserW` + manual stdio pipes + `spawn_blocking` wait.
- **SP-4 Hostname-based filtering** — proxy-based design across all three OSes (codex's port-allowlist pattern).
- **SP-5 cgroups v2** Linux memory + CPU enforcement.
- **SP-6 Windows AppContainer** — depends on `windows-sys` 0.61+ upgrade.

## 10. Alignment with Aleph's architectural redlines

- **R1 (brain-limb separation)**: All changes stay in `src/sandbox/`; no AppKit/CoreGraphics/native bridge touched. ✓
- **R3 (core minimalism)**: No new dependencies. `libc` (already present), `windows-sys` (already present). ✓
- **R10 (thin harness, dumb loop)**: S8 dissolution is the canonical example — "zero current consumers → delete, not preserve". ✓
- **P5 (least knowledge)**: `OsSandboxProfile` stays a value type; no new methods on it. ✓
- **P6 (KISS / YAGNI)**: No abstractions for future WFP/landlock. Each deferred item gets its own spec when actually needed. ✓
- **P7 (defensive design)**: Silent degradation → typed errors. Pre-cycle behavior actively misled callers; post-cycle behavior is honest. ✓
