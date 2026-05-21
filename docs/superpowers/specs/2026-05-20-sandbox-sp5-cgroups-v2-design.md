# SP-5 — Linux cgroups v2 Resource Limits

**Date**: 2026-05-20
**Status**: Design
**Branch**: `feat/sandbox-hardening-cycle1` (continued in same worktree)
**Predecessor**: `2026-05-20-sandbox-hardening-cycle1-design.md` § 9 (SP-5 entry)

## 1. Goal & scope

Replace the cycle 1 `setrlimit(RLIMIT_AS)` "virtual address space ceiling"
with a real RSS-based limit using cgroups v2, and add CPU bandwidth + PID
count controllers in the same setup. `RLIMIT_AS` stays as a fallback when
cgroups are unavailable.

**Why cgroups over `RLIMIT_AS`?**
- `RLIMIT_AS` counts virtual memory. `mmap(PROT_NONE, ...)` can inflate
  the VAS count without consuming real memory — easy bypass.
- cgroup `memory.max` enforces against RSS (resident set size, real
  physical memory). Combined with `memory.swap.max=0` it is the
  industry-standard memory containment primitive.
- cgroups also expose `cpu.max` (CPU bandwidth) and `pids.max` (process
  count), which are essential for sandbox use cases.

**In scope (this cycle)**:
- Discover cgroup v2 unified hierarchy (`/sys/fs/cgroup`).
- Create a per-execution sub-cgroup under the current process's cgroup.
- Enable `+memory +cpu +pids` in the parent's `subtree_control`.
- Set `memory.max` from `SandboxCapabilities.max_memory_mb`.
- Set `cpu.max` and `pids.max` from `LinuxSandboxConfig` defaults.
- Write the spawned bwrap child's PID into the cgroup via `pre_exec`.
- Cleanup the cgroup directory after the child exits.
- Soft-degrade on every failure path; `RLIMIT_AS` remains the memory
  backstop. Config flag `require_cgroups: bool` (default `false`)
  promotes degradation to hard error.

**Out of scope** (deferred):
- `io.max` per-device I/O bandwidth (per-device path enumeration is messy
  and overlaps with mount-namespace, future cycle).
- Custom `memory.swap.max` (we set to 0 always — no swap inside sandbox).
- Per-call `cpu_quota` / `max_pids` in `SandboxCapabilities` (config
  defaults are sufficient for now — promote to capability if user
  pressure appears).
- cgroup v1 fallback (v2 has been default on every major distro since
  2019; v1-only systems are out of band).
- systemd-run integration to obtain delegated cgroups (adds complexity;
  spec relies on existing delegation or root).

**Success criteria**:
1. On a kernel ≥ 5.5 with cgroup v2 unified hierarchy delegated to the
   user (or running as root), the sandboxed process is contained in a
   per-execution cgroup with `memory.max`, `cpu.max`, `pids.max` set.
2. Without delegation: the sandbox runs anyway; `RLIMIT_AS` enforces
   memory; a single `tracing::warn!` per process explains the degradation.
3. cgroup directories are cleaned up after the child exits (no orphan
   accumulation under repeated execution).
4. macOS / Windows code path is untouched.

## 2. Architecture

### 2.1 Where the cgroup work happens

Host-side, inside `BubblewrapDriver::run`, *not* inside `sandbox-init`.
Reasoning:
- `/sys/fs/cgroup` is mounted on the host. bwrap's namespace setup
  remounts cgroup hierarchies inside the new namespace, which would
  complicate writes from `sandbox-init`.
- Host has the original cgroup path for the aleph-server process
  (`/proc/self/cgroup`), which is the natural parent.
- Cleanup (rmdir of the per-execution sub-cgroup) needs to happen
  after `wait()` — that's host territory.
- `sandbox-init` already does plenty (landlock + seccomp +
  no_new_privs + execvp); pulling cgroup wiring into it would tangle
  responsibilities.

### 2.2 The host-side flow

```
BubblewrapDriver::run(cmd):
  // existing
  bwrap_args  = profile.contents.split('\n')
  bind init   = (if profile.linux_init_policy.is_some())

  // new — SP-5
  cg = CgroupV2Scope::try_create(            // None on any failure (soft)
         parent  = read_proc_self_cgroup(),
         scope   = format!("aleph-sandbox-{}", pid_seed()),
         mem_mb  = profile.max_memory_mb,
         cpu_pct = linux_cfg.cpu_quota_percent,
         pids    = linux_cfg.max_pids,
       );
  if cg.is_none() && linux_cfg.require_cgroups {
      return Err(SandboxError::ExecutionFailed("cgroups required ..."))
  }

  cmd = Command::new(bwrap)
    .args(...)
    .pre_exec(move || {
      if let Some(ref cg) = cg {
          cg.attach_current_pid()?;  // writes /proc/self → cgroup.procs
      }
      // RLIMIT_AS still applies — defense in depth + fallback.
      setrlimit_memory_as(...);
      Ok(())
    })

  let child = cmd.spawn()?;
  let output = wait_with_timeout(child, ...);

  // new — cleanup happens regardless of outcome
  if let Some(cg) = cg { cg.cleanup(); }

  output
```

### 2.3 cgroup discovery + sub-scope creation

```
1. unified = "/sys/fs/cgroup"
2. if !exists(unified + "/cgroup.controllers"): not v2 → None
3. self_cg = parse /proc/self/cgroup line "0::<path>" → unified + <path>
4. enable controllers in PARENT (self_cg + "/cgroup.subtree_control"):
     try write "+memory +cpu +pids"
     ignore EBUSY (already enabled) and EPERM (not delegated → None)
5. create child: self_cg + "/aleph-sandbox-<pid>-<rand>"
     ignore EEXIST (unlikely with random suffix; race-safe rmdir later)
6. write memory.max, cpu.max, pids.max
7. return CgroupV2Scope { path }  // ready for attach_current_pid()
```

### 2.4 cgroup lifecycle

```
Drop for CgroupV2Scope:
  rmdir(self.path)   // best-effort; ignore ENOENT / ENOTEMPTY
                     // ENOTEMPTY means a child process is still alive —
                     // shouldn't happen because we always rmdir after
                     // wait(), but log warn if it does.
```

A best-effort `rmdir` in `Drop` means cleanup happens even on early
returns from `run()` (timeout, IO error, etc.). The kernel reuses
cgroup IDs aggressively, so orphan directories aren't a security risk
— just hygiene.

## 3. File-level changes

| File | Change | Approx LOC |
|---|---|---|
| `src/sandbox/cgroup_v2.rs` *(new)* | `CgroupV2Scope` struct + `try_create` / `attach_current_pid` / `Drop` impl + `parse_proc_self_cgroup_path` parsing helper + 8 unit tests on the pure parser. Lives at top of `sandbox/` (same pattern as SP-2's `sandbox_init.rs`) so the cross-platform parsing + formatting tests compile + run on macOS dev boxes; the kernel-touching `try_create` / `attach_current_pid` / `Drop` body is `#[cfg(target_os = "linux")]`-gated. | ~250 |
| `src/sandbox/mod.rs` | `pub(crate) mod cgroup_v2;` | +1 |
| `src/sandbox/platforms/linux/bwrap.rs` | In `run()`: create `CgroupV2Scope` before spawn; write child PID in `pre_exec`; let `Drop` clean up. ~40 lines added; no removal of existing `setrlimit_memory_as` (it stays as fallback). | +40 |
| `src/sandbox/config.rs` | Add `LinuxSandboxConfig.cgroup_enabled: bool` (default `true`), `require_cgroups: bool` (default `false`), `cpu_quota_percent: Option<u32>` (default `None` → no CPU cap), `max_pids: Option<u32>` (default `Some(200)` → fork-bomb defense). | +20 |
| `src/sandbox/platforms/mod.rs` | Thread the 4 new fields into `LinuxSandboxOptions`. | +4 |
| `docs/reference/SANDBOX.md` | New "Linux resource limits (SP-5)" subsection; deferred table updated. | +25 |

Net: ~340 LOC additions, zero new third-party crates.

## 4. Capability ↔ cgroup mapping

| `LinuxSandboxConfig` / `SandboxCapabilities` field | cgroup file | Format |
|---|---|---|
| `caps.max_memory_mb = Some(n)` | `memory.max` | `<n * 1024 * 1024>\n` (bytes) |
| `caps.max_memory_mb = None` | `memory.max` not written | (kernel default = "max" — unlimited) |
| Always | `memory.swap.max` | `0\n` (no swap, prevents memory-pressure escape) |
| `linux_cfg.cpu_quota_percent = Some(p)` | `cpu.max` | `<p * 1000> 100000\n` (quota_us period_us) |
| `linux_cfg.cpu_quota_percent = None` | `cpu.max` | not written (= "max 100000") |
| `linux_cfg.max_pids = Some(n)` | `pids.max` | `<n>\n` |
| `linux_cfg.max_pids = None` | `pids.max` | not written (= "max") |

CPU quota example: `cpu_quota_percent = Some(50)` → file content
`"50000 100000\n"` → the process group can use 50ms of CPU per 100ms
window, effectively 50% of one core.

## 5. Error handling

Every failure point downgrades to "no cgroup, log warn, continue with
RLIMIT_AS". The escalation knob is `LinuxSandboxConfig.require_cgroups`.

| Failure | Default behavior | `require_cgroups=true` behavior |
|---|---|---|
| Not cgroup v2 (no `cgroup.controllers`) | warn + skip | `SandboxError::ExecutionFailed("cgroup v2 required")` |
| `/proc/self/cgroup` unparseable | warn + skip | `ExecutionFailed("unparseable self cgroup")` |
| Subtree controllers write `EPERM` (not delegated) | warn + skip | `ExecutionFailed("cgroup delegation required")` |
| `mkdir` child cgroup fails (`EPERM`/`EACCES`) | warn + skip | `ExecutionFailed("cannot create sub-cgroup")` |
| `memory.max` write fails | warn + skip | `ExecutionFailed("cannot set memory.max")` |
| `cgroup.procs` write fails in `pre_exec` | child is killed by `pre_exec` returning Err → bwrap fails to spawn → surfaced as `ExecutionFailed` | same |

The single `tracing::warn!` per process (gated by `Once`) avoids
log spam in environments where cgroups perpetually fail (CI without
delegation, etc).

## 6. Testing strategy

### Unit tests (cross-platform — pure parsing)

- `parse_proc_self_cgroup_v2_canonical` — `"0::/user.slice/.../foo.scope\n"` → `Some(PathBuf::from("/user.slice/.../foo.scope"))`.
- `parse_proc_self_cgroup_v1_only` — `"3:freezer:/foo"` → `None` (v1 hierarchy, SP-5 doesn't handle).
- `parse_proc_self_cgroup_empty` → `None`.
- `parse_proc_self_cgroup_root` — `"0::/\n"` → `Some(PathBuf::from("/"))`.
- `cpu_quota_format_renders_50pct` — `cpu_quota_percent(50) → "50000 100000"`.
- `cpu_quota_format_renders_100pct` — `cpu_quota_percent(100) → "100000 100000"`.
- `cpu_quota_format_caps_at_kernel_max` — `cpu_quota_percent(999) → "999000 100000"` (kernel accepts; we don't pre-clamp).
- `memory_max_bytes_renders_correctly` — `memory_max_bytes(64) → "67108864"`.

### Linux integration tests (`#[cfg(target_os = "linux")] + #[ignore]`)

- `cgroup_v2_scope_real_kernel_when_supported` — try `CgroupV2Scope::try_create`; assert `Some` on systemd-delegated CI runners.
- `cgroup_attach_current_pid_persists_in_cgroup_procs` — create scope, attach current PID, read back `cgroup.procs`, assert PID is there.
- `cgroup_drop_cleans_up_directory` — create, drop, assert directory gone.

### What we don't test

- `memory.max` actually OOM-kills a profligate process. That requires
  spawning a memory hog, waiting for kill, and is the kernel's job to
  verify. We test that we wrote the file correctly.

## 7. Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| `/sys/fs/cgroup` is read-only inside Docker / k8s pods without `--privileged` | High in containers | Soft-degrade is the entire design point. Document. |
| User runs aleph-server outside systemd (e.g. via `nohup`); no cgroup delegation | Med | Same — falls back to `RLIMIT_AS`. |
| `subtree_control` write fails after `mkdir` succeeds (partial setup) | Low | `Drop` always runs `rmdir`; partial state cleaned up. |
| Race between `rmdir` and a slow child still exiting (`ENOTEMPTY`) | Low | We `wait()` before `Drop`. If `Drop` still races (e.g. zombie), `rmdir` returns `ENOTEMPTY`; we log + leak. Kernel reaps when the last process exits. |
| `cpu.max` set so low that bwrap setup itself starves | Low | We set `cpu.max` *before* writing child PID, but cgroup limits don't apply until task is in the cgroup. By then, bwrap is past setup. |
| Multiple concurrent `aleph-server` instances racing on same parent's `subtree_control` (`EBUSY`) | Low | EBUSY = "controllers already enabled" → benign, treat as success. |
| `pre_exec` callback panics → child UB | Low | Use `?` propagation, no `panic!`/`unwrap`. |

## 8. Alignment with redlines & principles

- **R3 (core minimalism)**: zero new crates; pure `std::fs` writes to a few cgroup files. ✓
- **R7 (LLM sovereignty)**: no LLM path involved. ✓
- **R10 (thin harness)**: `cgroup_v2.rs` is single-purpose ~250 lines; the `CgroupV2Scope` API is `try_create` / `attach_current_pid` / `Drop`. No traits, no engine. ✓
- **P5 (least knowledge)**: `bwrap.rs` calls 3 methods total on `CgroupV2Scope`; everything else is internal. ✓
- **P7 (defensive design)**: every failure path is documented; soft-degrade default; opt-in hard-error; `Drop`-based cleanup. ✓

## 9. Implementation sequence

1. `src/sandbox/cgroup_v2.rs` (new — at top of `sandbox/`, not under `platforms/linux/`):
   - Cross-platform: `parse_proc_self_cgroup_path(s: &str) -> Option<PathBuf>`, `cpu_quota_max_line(pct: u32) -> String`, `memory_max_bytes(mb: u64) -> u64`.
   - `pub struct CgroupV2Scope { path: PathBuf, ... }` (struct declaration cross-platform; field set + methods that need kernel APIs are gated).
   - `#[cfg(target_os = "linux")] CgroupV2Scope::try_create(...) -> Option<Self>`.
   - `#[cfg(target_os = "linux")] CgroupV2Scope::attach_current_pid(&self) -> std::io::Result<()>`.
   - `#[cfg(target_os = "linux")] Drop for CgroupV2Scope { ... }`.
   - `#[cfg(test)] mod tests` for all the pure parsing + formatting.
2. `src/sandbox/mod.rs` — `pub(crate) mod cgroup_v2;`.
3. `src/sandbox/config.rs` — add the 4 new `LinuxSandboxConfig` fields.
4. `src/sandbox/platforms/mod.rs` — thread the 4 new fields into `LinuxSandboxOptions`.
5. `src/sandbox/platforms/linux/bwrap.rs`:
   - Extend `LinuxSandboxOptions` with the 4 new fields.
   - In `run()`: build `CgroupV2Scope` before `cmd.spawn`; pass an `Arc<Option<CgroupV2Scope>>` into `pre_exec` for PID attach. Honor `require_cgroups`. Let `Drop` clean up.
6. `cargo check -p alephcore` on macOS (Linux deps shouldn't compile; cgroup_v2.rs is Linux-only so its parser tests run on Linux CI only).
7. Unit tests for pure parsing.
8. `cargo test -p alephcore --lib sandbox` (regression check; +N tests).
9. `cargo clippy -p alephcore --lib --tests -- -D warnings` (only assess touched-file regressions vs main baseline).
10. Update `docs/reference/SANDBOX.md` Cycle 1 section with "Linux resource limits (SP-5)" subsection; mark SP-5 strike-through in deferred table.
11. Commit: `sandbox: SP-5 — cgroups v2 memory/cpu/pids on Linux`.

## 10. Out-of-scope follow-up specs

Unchanged from SP-2 close: SP-3a (Windows RestrictedToken) + SP-3b/SP-6 (Windows network filtering, pending decision).
