# SP-2 — Linux Landlock + seccomp-bpf Defense-in-Depth

**Date**: 2026-05-20
**Status**: Design
**Branch**: `feat/sandbox-hardening-cycle1` (continued in same worktree)
**Predecessor**: `2026-05-20-sandbox-hardening-cycle1-design.md` § 9 (SP-2 entry)

## 1. Goal & scope

Add two independent Linux kernel mechanisms on top of cycle 1's `bwrap`
namespace isolation:

- **Landlock**: in-process filesystem ACL — restrict R / W / Execute
  within paths that bwrap has already mount-namespaced in.
- **Seccomp-bpf**: syscall filter — return `EPERM` for a denylist of
  kernel-attack-surface calls (`mount`, `umount`, `kexec_*`,
  `init_module`, `bpf`, `perf_event_open`, `ptrace`, `keyctl`,
  `userfaultfd`, `pivot_root`, `chroot`, `mknod`, `swapon`, `reboot`,
  `clone` with `CLONE_NEWUSER`).

The two compose: landlock restricts files inside the mount namespace;
seccomp restricts the kernel attack surface independent of files.
codex uses the same pair for the same reason.

**In scope (this cycle)**:
- Landlock ruleset construction from `SandboxCapabilities` (ABI v1, R/W/Exec).
- Seccomp denylist with `EPERM` fallthrough.
- Embed an `aleph-server sandbox-init` subcommand that bwrap launches; the
  subcommand applies landlock + seccomp before `execvp`-ing the target.
- Old-kernel soft-degrade: kernels without landlock ABI v1 log a warning
  and continue without it. Seccomp is universally supported (Linux 3.5+),
  no degrade path needed.

**Out of scope** (deferred to other specs):
- Landlock network filtering (ABI v4, Linux 6.7+) — would partially
  duplicate SP-4 / SP-6 hostname work; tackle when SP-4 graduates to Linux.
- cgroups v2 memory / CPU limits → SP-5.
- macOS / Windows equivalents (different mechanisms; tracked separately).
- Allowlist-mode seccomp — too fragile for arbitrary user code.

**Success criteria**:
1. On Linux ≥ 5.13 with bwrap installed, a sandboxed command runs under
   bwrap namespace + landlock ruleset + seccomp filter; attempts at the
   denylisted syscalls return `EPERM`.
2. On Linux < 5.13, landlock is skipped with a warning; bwrap + seccomp +
   rlimit still apply.
3. macOS / Windows code path is untouched (`#[cfg(target_os = "linux")]`
   discipline).
4. Existing cycle 1 / SP-4 tests continue to pass.

## 2. Architecture

### 2.1 Invocation chain

Before SP-2:

```
bwrap [namespace+mount args] -- /usr/bin/python script.py
```

After SP-2:

```
bwrap [namespace+mount args]
      --ro-bind <current_exe> /aleph-sandbox-init
      --
      /aleph-sandbox-init --policy='<json>' -- /usr/bin/python script.py
```

The new `sandbox-init` subcommand:

1. Parses `--policy=<json>` (the serialized `LinuxInitPolicy` with
   `landlock_paths` + `seccomp_denylist`).
2. Applies the landlock ruleset (best-effort; skip on unsupported ABI).
3. Applies the seccomp filter.
4. Calls `prctl(PR_SET_NO_NEW_PRIVS, 1)` (required by seccomp before
   `EPERM` arms safely; also useful generally).
5. `execvp(target, target_args)`.

bwrap continues to do user-namespace / mount-namespace / `--unshare-net`.
After it sets up the namespace, it `execve`s its argv[0] which is now
`sandbox-init`, which adds the two new layers before launching the user
program. This puts the LSM hooks in exactly the right place: after bwrap
has dropped what it can but before the untrusted code runs.

### 2.2 Why an embedded subcommand (vs. a separate binary)

R3 (core minimalism) → no separate `aleph-sandbox-helper` binary. The
init logic lives in `src/sandbox/platforms/linux/sandbox_init.rs` and is
exposed via a hidden CLI subcommand on the existing `aleph-server`
binary. `bwrap`'s `--ro-bind` makes the running aleph-server binary
visible inside the sandbox at a fixed mount path (e.g.
`/aleph-sandbox-init`). Zero distribution overhead; zero new artifacts.

### 2.3 Policy passing

Policy is a JSON `LinuxInitPolicy` struct passed via argv (size bounded
by capability count; typical < 4 KiB; argv has ample headroom on Linux
even after `MAX_ARG_STRLEN`). Stdin would be cleaner but argv keeps the
init binary fully synchronous and free of stdin/stdout interleaving with
the target program.

```rust
struct LinuxInitPolicy {
    /// Paths to allow READ_FILE / READ_DIR / EXECUTE (system + caps.fs_read).
    read_paths: Vec<PathBuf>,
    /// Paths to allow READ_FILE / READ_DIR / WRITE_FILE / REMOVE_*/MAKE_* /
    /// EXECUTE (caps.fs_write).
    write_paths: Vec<PathBuf>,
    /// Optional ceiling on max landlock ABI we'll request; bounded by what
    /// the kernel exposes via `landlock_create_ruleset(NULL, 0,
    /// LANDLOCK_CREATE_RULESET_VERSION)`.
    landlock_abi_cap: u8,
}
```

(Seccomp denylist is a fixed constant compiled into the init binary; no
need to thread it through argv.)

### 2.4 Old-kernel soft degrade

`landlock::Ruleset::try_new()` returns `Err(...)` on kernels < 5.13 (no
ABI). The init subcommand catches this, logs `landlock unavailable on
this kernel — skipping (bwrap+seccomp still active)`, and continues.

Add a `LinuxSandboxOptions.require_landlock: bool` (default `false`).
When `true`, the init exits non-zero on landlock failure, which surfaces
as `SandboxError::ExecutionFailed` to the caller — useful for hardened
production deployments.

## 3. File-level changes

| File | Change | Approx LOC |
|---|---|---|
| `Cargo.toml` | Add `landlock = "0.4"`, `seccompiler = "0.5"` under `[target.'cfg(target_os = "linux")'.dependencies]` | +2 |
| `src/sandbox/platforms/linux/sandbox_init.rs` *(new)* | `LinuxInitPolicy` struct + `run_init(argv: Vec<String>) -> !` entry point + landlock builder + seccomp filter constants. Compiles only on Linux. | ~250 |
| `src/sandbox/platforms/linux/mod.rs` | `#[cfg(target_os = "linux")] pub mod sandbox_init;` | +2 |
| `src/sandbox/platforms/linux/bwrap.rs` | Extend `generate_args` to: (a) emit `--ro-bind <current_exe> /aleph-sandbox-init`, (b) emit init invocation as the program slot. Refactor `run()` to wrap the user program with the init prelude. | +40 / −10 |
| `src/sandbox/config.rs` | Add `LinuxSandboxOptions.require_landlock: bool` (default false) | +3 |
| `src/main.rs` (or wherever the CLI dispatcher lives) | Add hidden subcommand `sandbox-init` that dispatches to `sandbox_init::run_init` on Linux. On non-Linux, the subcommand returns "unsupported on this platform". | +15 |
| `tests/sandbox_linux_landlock_seccomp.rs` *(new)* | Linux-gated integration tests; `#[ignore]` by default (require landlock-capable kernel + bwrap). | ~120 |
| `docs/reference/SANDBOX.md` | New "Linux defense-in-depth (SP-2)" subsection; deferred table updated. | +30 |

Net: ~450 LOC additions, single new crate dep × 2 (Linux-only).

## 4. Landlock ruleset construction

Built from `SandboxCapabilities` at `generate_args` time, serialized into
the JSON policy. The init subcommand reconstructs the `landlock::Ruleset`
from this list.

Always-allowed read+exec paths (system minimum required for ld-linux,
libc, bwrap-mounted binaries):
- `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, `/etc`

From `SandboxCapabilities`:
- Each `fs_read` path → `READ_FILE | READ_DIR | EXECUTE`
- Each `fs_write` path → `READ_FILE | READ_DIR | WRITE_FILE | REMOVE_FILE | REMOVE_DIR | MAKE_REG | MAKE_DIR | MAKE_SYM | EXECUTE`
- `cwd` (workspace root) → automatically in `fs_write` because workspace is R/W by design

Landlock is unioning — overlapping paths sum their permissions, which is
what we want.

ABI v1 is the floor; we negotiate higher (v2 REFER, v3 TRUNCATE) when
the kernel exposes them but never require them.

## 5. Seccomp denylist

`seccompiler::SeccompFilter` with default action = `SECCOMP_RET_ALLOW`
and an explicit `SECCOMP_RET_ERRNO(EPERM)` for each denylisted syscall.

Denylist (final, frozen for SP-2):

| Syscall | Reason |
|---|---|
| `mount`, `umount`, `umount2` | Filesystem manipulation |
| `pivot_root`, `chroot` | Filesystem root change |
| `kexec_load`, `kexec_file_load` | Kernel reload |
| `init_module`, `finit_module`, `delete_module` | LKM load/unload |
| `bpf` | eBPF program load |
| `perf_event_open` | Perf subsystem (kernel info leak) |
| `ptrace` | Debugger / breakout vector |
| `keyctl`, `add_key`, `request_key` | Kernel keyring |
| `userfaultfd` | Page-fault handler injection |
| `io_uring_setup`, `io_uring_register`, `io_uring_enter` | io_uring escape vectors |
| `pivot_root` | Mount-namespace escape |
| `mknod`, `mknodat` | Device file creation |
| `swapon`, `swapoff` | Swap manipulation |
| `nfsservctl` | NFS server control |
| `syslog` | Kernel log access |
| `reboot` | System reboot |
| `clone` with `flags & CLONE_NEWUSER != 0` | Nested user-namespace escape (argument filter) |
| `unshare` with `flags & CLONE_NEWUSER != 0` | Same as above (argument filter) |
| `setns` | Namespace switch |

`EPERM` (not `SIGKILL`) keeps the program survivable so it can log /
report errors. Argument-aware rules use `seccompiler::SeccompRule` with
`SeccompCmpArgLen::Dword` comparisons.

## 6. Error handling

| Failure | Behavior |
|---|---|
| `current_exe()` returns error (procfs unmounted, exotic ENV) | bwrap.run returns `SandboxError::ExecutionFailed("cannot determine aleph-server path: ...")`; no sandbox is constructed |
| Landlock ABI < 1 (kernel < 5.13) | If `require_landlock=true` → init exits 64; else init logs warning and continues |
| Seccomp filter rejected by kernel | init exits 65; this is unrecoverable (SP-2 considers seccomp non-optional, unlike landlock) |
| Init subcommand can't parse argv | exit 66 + stderr error message |
| `execvp(target, ...)` fails | init exits 67 + stderr |
| Existing bwrap failure modes | Unchanged |

Init exit codes 64–67 propagate to caller as the child's exit code (>0)
which surfaces as `SandboxOutput { exit_code: Some(64..=67), .. }` for the
caller to handle. We do NOT use `SandboxError` for these because they
happen after `cmd.spawn()` succeeded.

## 7. Testing strategy

### Unit tests (cross-platform — pure logic)

- `sandbox_init::landlock_paths_from_capabilities` builds the right
  `read_paths` / `write_paths` for various `SandboxCapabilities` inputs.
- `sandbox_init::seccomp_denylist_is_frozen` snapshot test pinning the
  exact set of denylisted syscall names so a future contributor can't
  silently shrink the list.
- JSON round-trip of `LinuxInitPolicy`.

### Linux integration tests (`#[cfg(target_os = "linux")] + #[ignore]`)

- `cycle1 + sp2 + sp4 chain still passes existing tests` — runs the
  cycle 1 RecordingDriver + SP-4 DNS test on Linux to confirm no
  regression.
- `landlock_blocks_writes_outside_workspace` — sandbox config restricts
  fs_write to workspace; child attempts `write` to `/tmp/foo` → `EPERM`.
- `seccomp_blocks_mount_syscall` — child attempts `mount("/tmp",
  "/mnt", ...)` → `EPERM`.
- `seccomp_blocks_userfaultfd` — child attempts `userfaultfd(0)` →
  `EPERM`.
- `old_kernel_soft_degrades` — mock `landlock::Ruleset::try_new()`
  failure → init continues, sandbox still runs.

These are `#[ignore]`d so `cargo test` on a Mac dev box passes; CI Linux
runner picks them up with `cargo test -- --ignored`.

### What we don't test

- Whether `EPERM` actually thwarts a real exploit. That's threat-model
  validation, not unit-testable. Trust the kernel + denylist.

## 8. Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| `current_exe()` returns a path that bwrap can't bind-mount (e.g. on /proc) | Low | `current_exe()` returns the *real* path on Linux (uses `/proc/self/exe` → readlink); bind-mounting that always works because procfs links into a real file. Document. |
| Some target binary needs a syscall on the denylist | Med | List is conservative; nothing on it is needed by normal user-space code. If a real false positive appears, surface as a spec amendment, not a config knob. |
| Landlock ABI v1 lacks REFER → renames across allowed dirs fail | Med | Document; users who need cross-dir rename get the natural error. We negotiate up to v2 when available. |
| Seccomp filter blocks some new syscall a future glibc starts using | Med | Denylist (not allowlist) sidesteps this — new syscalls default-allow. |
| `bwrap` itself starts using one of the denylisted syscalls (e.g. `mount` for namespace setup) | N/A | Seccomp applies inside `sandbox-init` (after bwrap finished its setup), so bwrap is unaffected. |
| Tests can't run on CI without elevated capabilities | Low | Tests are `#[ignore]` and only run on Linux runners with bwrap + appropriate kernel. CI matrix already has both. |

## 9. Alignment with redlines & principles

- **R1 (brain-limb separation)**: All changes in `src/sandbox/` + `Cargo.toml`. No bridge / IPC / desktop crate touched. ✓
- **R3 (core minimalism)**: Two crates (landlock + seccompiler), both Linux-only, both single-purpose. No new binary artifact. ✓
- **R7 (LLM sovereignty)**: Sandbox runs target program; no LLM path involved. ✓
- **R10 (thin harness)**: `sandbox_init.rs` is single-purpose ~250 lines; no new traits, no policy DSL, no engine abstraction. ✓
- **P5 (least knowledge)**: `LinuxInitPolicy` is the only contract between bwrap.rs and sandbox_init.rs. ✓
- **P7 (defensive design)**: Soft-degrade on old kernels with explicit opt-in to hard-fail; seccomp `EPERM` not `SIGKILL` so programs can log. ✓

## 10. Implementation sequence

1. `Cargo.toml` — add `landlock = "0.4"` and `seccompiler = "0.5"` under Linux-only target. Verify `cargo check` passes on macOS (Linux deps shouldn't compile).
2. `src/sandbox/platforms/linux/sandbox_init.rs` (new):
   - Define `LinuxInitPolicy`.
   - `pub fn run_init(args: Vec<String>) -> !`.
   - `apply_landlock(policy: &LinuxInitPolicy, require: bool) -> Result<()>`.
   - `apply_seccomp() -> Result<()>` (denylist constant inline).
   - All `#[cfg(target_os = "linux")]`.
3. `src/sandbox/platforms/linux/mod.rs` — `pub mod sandbox_init;` (Linux-only).
4. `src/sandbox/config.rs` — add `require_landlock: bool` to `LinuxSandboxOptions`.
5. `src/sandbox/platforms/linux/bwrap.rs`:
   - In `generate_args` (or per-run, since the path is host-side), call `std::fs::canonicalize(std::env::current_exe()?)?` to get aleph-server's absolute on-disk path. Emit `--ro-bind <canon-path> /aleph-sandbox-init`.
   - In `run()`, replace the program slot with `/aleph-sandbox-init` and prepend init args: `["sandbox-init", "--policy", policy_json, "--", program, args...]`.
   - Do NOT use `--ro-bind /proc/self/exe ...` — `/proc/self/exe` in bwrap resolves to bwrap's exe, not aleph-server's.
6. Wire CLI subcommand. Find the existing CLI dispatcher; add `sandbox-init` arm.
7. Unit tests for landlock-paths construction + seccomp-list snapshot + JSON round-trip.
8. `#[ignore]`-gated Linux integration tests.
9. `cargo check -p alephcore` (macOS), then verify Linux-only paths compile cleanly via `cargo check -p alephcore --target x86_64-unknown-linux-gnu` if cross-compilation is set up; otherwise rely on CI.
10. Update `docs/reference/SANDBOX.md` Cycle 1 section with "Linux defense-in-depth (SP-2)" subsection; mark SP-2 strike-through in deferred table.
11. Commit: `sandbox: SP-2 — landlock + seccomp defense-in-depth on Linux`.

## 11. Out-of-scope follow-up specs

Unchanged from SP-4 close: SP-3a (Windows RestrictedToken), SP-3b / SP-6 (Windows network filtering, decision pending), SP-5 (Linux cgroups v2 — can layer on SP-2's `sandbox-init` for cgroup attach).
