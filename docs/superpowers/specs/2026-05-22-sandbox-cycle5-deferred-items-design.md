# Sandbox Cycle 5 — Deferred-Item Planning & Protected-Path Fixes (Linux + Windows)

> Date: 2026-05-22. Plans the two items deferred by
> [Cycle 4](./2026-05-21-sandbox-cycle4-bugfix-hardening-design.md) and
> recorded in `docs/reference/SANDBOX.md § Cycle 4`. Item 1 is **implemented
> this cycle**; Item 2 is **planned only** (genuinely multi-cycle). The
> Windows analogue of Item 1 — originally recorded below as out of scope —
> was **implemented as a same-cycle follow-up**; see § *Windows parallel
> gap*.

## Context

Cycle 4 surfaced two gaps it could not close in a single macOS-dev-box
session:

1. **Linux protected-path creation gap** — `--ro-bind-try` silently no-ops
   for a non-existent protected metadata dir, so a sandboxed process can
   `mkdir .git` and write inside it. macOS denies this; the platforms are
   inconsistent.
2. **Per-host network filtering** — `NetworkPolicy::AllowHosts` /
   `ProxyOnly` still hard-fail on Linux and Windows.

This cycle treats both as a planning deliverable, and implements Item 1
because it is small, well-understood, and the only stated blocker
(compile-verification) is addressable.

---

## Item 1 — Linux protected-path creation gap  *(implemented this cycle)*

### Problem

`push_metadata_protection_args` (`src/sandbox/platforms/linux/bwrap.rs:386`)
emits, for every protected subpath of every writable root:

```
--ro-bind-try <p> <p>
```

bubblewrap's `--ro-bind-try` is defined to **silently succeed when the
source does not exist**. For a brand-new workspace none of
`.git` / `.aleph` / `.codex` / `.agents` exist yet, so the protection
arguments are no-ops. A sandboxed process with workspace-write access can
then `mkdir .git` (or `.aleph`, …) inside the writable workspace and write
arbitrary content there — exactly the history/audit-trail rewrite the
protected-paths mechanism exists to prevent.

macOS Seatbelt does not have this gap: its
`(deny file-write* (subpath "<ws>/.git"))` rule matches by path and applies
whether or not the path exists. The two platforms are therefore
**inconsistent**, and Linux is the weaker one.

### codex's solution

codex handles the absent-path case explicitly
(`/Volumes/TBU4/Github/codex/codex-rs/linux-sandbox/src/bwrap.rs:1085-1104`):

```rust
fn append_empty_directory_args(bwrap_args: &mut BwrapArgs, path: &Path) {
    bwrap_args.args.push("--perms".to_string());
    bwrap_args.args.push("555".to_string());
    bwrap_args.args.push("--tmpfs".to_string());
    bwrap_args.args.push(path_to_string(path));
    bwrap_args.args.push("--remount-ro".to_string());
    bwrap_args.args.push(path_to_string(path));
}
```

A synthetic empty `tmpfs` is mounted at the absent protected path with
mode `555` (`r-xr-xr-x` — traversable, not writable), then `--remount-ro`
makes the whole mount read-only. The sandboxed process sees an empty
directory it can `stat` and `cd` into but cannot write to or replace.
`--tmpfs` over a non-existent mount point is fine: bwrap creates the
mount point inside its own tmpfs root.

### Aleph's fix

Split `push_metadata_protection_args` on `Path::exists()`:

```rust
/// Append codex-inspired metadata-protection mounts to a bubblewrap
/// argument vector. For every writable root, each protected subpath
/// (`.git`, `.aleph`, `.codex`, `.agents`) is shielded so the sandboxed
/// process can neither write into an existing one nor create a missing
/// one:
///
/// - **Existing** path → `--ro-bind-try` remounts it read-only, so tools
///   that read metadata (`git log`, `git status`) keep working.
/// - **Absent** path → a synthetic empty read-only `tmpfs` is mounted
///   (`--perms 555 --tmpfs <p> --remount-ro <p>`). Without this,
///   `--ro-bind-try` silently no-ops for a non-existent source and the
///   sandboxed process can `mkdir .git` inside the writable workspace.
///   Matches macOS Seatbelt, whose `(deny file-write* (subpath …))` rule
///   applies whether or not the path exists.
///
/// Must be emitted *after* the writable `--bind` because bwrap mounts
/// override in declaration order.
fn push_metadata_protection_args<'a, I>(args: &mut Vec<String>, writable_roots: I)
where
    I: IntoIterator<Item = &'a std::path::Path>,
{
    let roots: Vec<&std::path::Path> = writable_roots.into_iter().collect();
    let protected =
        crate::sandbox::protected_paths::protected_paths_for(roots.iter().copied());
    for path in protected {
        let Some(path_str) = path.to_str() else { continue };
        if path.exists() {
            args.push("--ro-bind-try".into());
            args.push(path_str.into());
            args.push(path_str.into());
        } else {
            // Synthetic empty read-only directory — blocks creation.
            args.push("--perms".into());
            args.push("555".into());
            args.push("--tmpfs".into());
            args.push(path_str.into());
            args.push("--remount-ro".into());
            args.push(path_str.into());
        }
    }
}
```

The existing branch keeps `--ro-bind-try` rather than `--ro-bind`: it
defends the tiny check→mount race where the directory is removed between
arg generation and bwrap launch (`--ro-bind` would hard-fail; the absent
case is already covered by the new branch on the next run).

Scope: one Linux-`cfg`-gated function. No change to the `OsSandboxDriver`
trait, the policy enum, callers, or the macOS/Windows drivers.

### Tests

`bwrap.rs`'s metadata-protection tests currently pass non-existent literal
paths (`/tmp/ws`, `/tmp/extra`); with the fix those would all take the new
`--tmpfs` branch, so the assertions must be rewritten against real
directories. `tempfile` is already an alephcore dependency.

- `workspace_only_synthesizes_tmpfs_for_absent_metadata` — `tempdir()`
  workspace, `.git` not created → assert `--perms 555` + `--tmpfs <ws>/.git`
  + `--remount-ro <ws>/.git` present, ordered after the writable `--bind`,
  and no `--ro-bind-try` for `.git`.
- `workspace_only_ro_binds_existing_metadata` — `tempdir()` workspace with
  `.git` created → assert `--ro-bind-try <ws>/.git` present, no `--tmpfs`
  for it.
- `write_paths_protects_metadata_in_each_writable_root` — rewritten with
  two real `tempdir()` roots, asserting protection (either branch) for
  every `{.git,.aleph,.codex,.agents}` under each.
- `full_write_does_not_auto_protect_metadata` — unchanged; `FullWrite`
  never calls `push_metadata_protection_args`, so it emits neither branch.

### Verification

`bwrap.rs` is `#[cfg(target_os = "linux")]`-gated, so it does not compile
in a plain macOS `cargo check`. Cycle 4 enabled Windows cross-compilation
by installing `mingw-w64`; this cycle adds the Linux toolchain via
**`cargo-zigbuild`** (`zig cc` bundles a cross-toolchain). zigbuild does
cross-compile Rust + C cleanly, but a full in-tree
`cargo-zigbuild check -p alephcore --target x86_64-unknown-linux-gnu` is
blocked by a system-library dependency — `wayland-sys` needs a Linux
sysroot for `pkg-config` (a sysroot problem, not a toolchain one).

The change is pure `std` (FFI-free, no `#[cfg]`, no platform API), so it
was instead verified with an isolated scratch crate carrying a *verbatim*
copy of `push_metadata_protection_args` plus a real copy of
`protected_paths.rs`:

- `cargo test` (native macOS) — 3/3 green, confirming the branch logic
  (absent → tmpfs, existing → ro-bind-try, mixed roots both covered).
- `cargo-zigbuild check --tests --target x86_64-unknown-linux-gnu` — the
  exact function code and the `tempfile` / `Vec::windows` / `format!`
  test patterns compile clean for the Linux target.

Running the in-tree Linux-gated unit tests (which exercise the full
`BubblewrapDriver`) still requires a Linux host; they are deterministic
argument-vector assertions and `generate_args` itself is unchanged.

### Windows parallel gap *(implemented as a same-cycle follow-up)*

Windows has the same shape of gap: Cycle 3's protected-metadata DACL deny
(`windows_init.rs`) stamped `DENY_ACCESS` ACEs only on **existing**
`<ws>/{.git,…}` subdirectories, and the workspace-root grant inherits
`GENERIC_ALL` to children — so a freshly created `.git` would be writable.
NTFS ACLs cannot deny "create a child named `.git`" by name, so the deny
ACE needs a real object to bind to: pre-create the absent metadata
directories as empty stubs before spawn.

`ensure_protected_metadata_deny` (renamed from
`stamp_protected_metadata_deny`) now, for each of the four subpaths: if it
exists → stamp the deny ACE (Cycle 3 behavior); if it is absent →
`create_dir` an empty stub, then stamp the ACE. The cross-platform
classifier `classify_protected_metadata` (replacing
`protected_metadata_targets_under`) returns all four paths tagged with
on-disk existence, so the partition logic stays unit-testable off-Windows.

After the target exits, the post-wait cleanup revokes every deny ACE and
`remove_dir`s every stub it created. `remove_dir` (not `remove_dir_all`)
only succeeds on an empty directory, so a stub the target somehow
populated is left in place rather than destroying data — and a real
`.git` is never touched, because only *absent* paths get a stub.

Verified in-tree: native `cargo test windows_init` 14/14 green (including
the three new `classify_*` tests) + `cargo check --target
x86_64-pc-windows-gnu` clean (the Win32 `imp` module compiles). This
closes the Windows half of the Cycle 4 protected-path gap; only Linux was
in the original Item 1 scope.

---

## Item 2 — Per-host network filtering  *(planned only — phased)*

### Problem & current state

`NetworkPolicy` has three meaningful states: `None`, `AllowAll`,
`AllowHosts(ips)` (plus `ProxyOnly { ports }`). Today:

| Policy | macOS | Linux | Windows |
|---|---|---|---|
| `None` | `(deny network*)` | `--unshare-net` + seccomp socket deny | token / no caps |
| `AllowAll` | `(allow network*)` | shared netns | network capability |
| `AllowHosts` | `(allow … (remote ip …))` | **`UnsupportedPolicy`** | **`UnsupportedPolicy`** |
| `ProxyOnly` | localhost rule | **`UnsupportedPolicy`** | **`UnsupportedPolicy`** |

macOS enforces per-host via Seatbelt's IP matcher. Linux and Windows
hard-fail because every enforcement mechanism needs either a privilege the
host process does not hold (`CAP_NET_ADMIN`, admin) or a managed proxy
intercepting client traffic. The `AllowHosts` / `ProxyOnly` enum variants
are the wiring points — the work is to make them enforce instead of reject.

### codex's architecture

codex routes all sandbox egress through a loopback proxy
(`/Volumes/TBU4/Github/codex/codex-rs/linux-sandbox/src/proxy_routing.rs`):

- **Host bridge** (`spawn_host_bridge`, lines 419-473) — a forked process
  that listens on a Unix domain socket and forwards each connection to a
  real remote TCP endpoint.
- **Local bridge** (`spawn_local_bridge`, lines 475-524) — runs *inside*
  the `--unshare-net` namespace, listens on loopback TCP, forwards each
  connection to the host bridge's UDS. It also brings the `lo` interface
  up via `SIOCSIFFLAGS` ioctl (lines 544-615).
- **Env rewrite** (`activate_proxy_routes_in_netns`, lines 121-167) —
  rewrites `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` (+ npm/pip/yarn/…
  variants) to point at the local bridge's port.
- **seccomp ProxyRouted mode** (`landlock.rs:218-246`) — allows
  `socket(AF_INET/AF_INET6)` (needed to reach the local bridge) but denies
  `AF_UNIX` socket / socketpair, so the target cannot bypass the bridge.

Data flow: `app → 127.0.0.1:local → local bridge → UDS → host bridge →
remote`. On Windows codex uses WFP (`windows-sandbox-rs/src/wfp.rs`):
12 persistent ALE_AUTH_CONNECT filters scoped to the sandbox account SID,
installed by an **admin-elevated** setup helper.

### Phased plan

**Phase A — shared `ManagedProxy` allowlist proxy.** New module
`src/sandbox/proxy/`: an in-process async HTTP-CONNECT + SOCKS5 proxy
bound to `127.0.0.1:0`. On each connection it parses the target host:port,
matches it against the resolved `AllowHosts` allowlist (re-resolving
hostnames and re-checking IPs to blunt DNS rebinding), and rejects
non-allowlisted targets. The sandbox receives `HTTP_PROXY` / `HTTPS_PROXY`
/ `ALL_PROXY` env vars pointing at it.
- *macOS*: the sandbox shares the host netns, so the env vars plus a
  host-loopback proxy work directly — this also gives macOS a hostname
  allowlist without DNS pre-resolution, layered on top of the Seatbelt IP
  rules (defense in depth).
- *Privilege*: none — a loopback listener.
- *Limitation*: only constrains clients that honor proxy env vars; a
  target opening raw sockets bypasses it. Phases B–D add OS enforcement.
- *Effort*: ~one focused cycle. The single largest, most reusable
  deliverable; recommended as the next sandbox cycle.

**Phase B — Linux netns bridge.** A `--unshare-net` Linux sandbox cannot
reach the host's loopback proxy (separate loopback). Port codex's
TCP→UDS→TCP bridge (host bridge + in-netns local bridge + `lo` bring-up)
and add the seccomp **ProxyRouted** mode (allow `AF_INET`/`AF_INET6`, deny
`AF_UNIX`) to `sandbox_init`. `--unshare-net` stays — only the bridge UDS
crosses the boundary. Builds directly on Phase A's proxy.
- *Privilege*: none beyond the user + net namespace bwrap already creates.
- *Effort*: one cycle; depends on Phase A.

**Phase C — Linux nftables (true kernel egress).** Optional hardening that
does not depend on apps honoring proxy env vars: create a netns where the
host process holds `CAP_NET_ADMIN`, wire connectivity through
`slirp4netns` / `pasta`, and install nftables rules permitting only the
resolved IPs. Real per-IP enforcement, but adds rootless-network plumbing.
- *Privilege*: `CAP_NET_ADMIN` within an owned user+net namespace.
- *Effort*: multi-cycle; lower priority than A/B.

**Phase D — Windows WFP.** Port codex's WFP filter installation, scoped to
the per-execution AppContainer SID. Persistent kernel filters; requires
the existing elevated setup helper.
- *Privilege*: Administrator (one-time filter installation).
- *Effort*: multi-cycle; lowest priority — admin requirement, and the
  AppContainer capability model already covers the common deny case.

### Recommended sequencing

`A → B`, then reassess. Phase A alone converts `AllowHosts` / `ProxyOnly`
from hard-fail to *enforced for proxy-honoring clients* on all three OSes
and is the highest value-per-cycle. Phase B gives Linux real isolation for
the common case. C and D stay deferred until a concrete need (a workload
that bypasses proxy env vars, or a Windows hard-isolation requirement).

### Why not this cycle

Phase A is a whole new networking module (proxy protocol parsing,
allowlist matching, env injection, lifecycle) — its own brainstorm → spec
→ plan → implement cycle. Phase B is FFI-heavy (`fork`, UDS, ioctl,
seccomp) and Linux-runtime-bound. The goal explicitly records Item 2 as
"跨多周期的大工程"; this spec is its decomposition.

---

## Verification plan

| Item | Verification |
|---|---|
| 1 — code | scratch crate (verbatim copy of the function): `cargo test` native macOS 3/3 green + `cargo-zigbuild check --target x86_64-unknown-linux-gnu` compiles clean. Full in-tree zigbuild blocked by `wayland-sys` sysroot dep. In-tree Linux unit-test run deferred to a Linux session |
| 1 — no regression | macOS compile graph untouched — the change is entirely inside the `#[cfg(target_os = "linux")]` `bwrap.rs` module, which macOS never compiles |
| 1 — Windows follow-up | verified **in-tree**: `cargo test windows_init` 14/14 green native (3 new `classify_*` tests) + `cargo check --target x86_64-pc-windows-gnu` clean (the `#[cfg(target_os = "windows")]` `imp` module compiles). Win32 ACE / stub wiring runs on a Windows host |
| 2 | none — spec only |

## Risks

- **Item 1 TOCTOU**: a protected dir created/removed between the
  `path.exists()` check and the bwrap mount. Window is arg-generation
  only (the sandboxed process is not yet running), and either outcome
  still yields a protected mount on the next run — accepted for v1;
  codex's "transient empty metadata path" handling is the hardening for
  concurrent-bwrap setups, which Aleph (one sandbox per session) does not
  have.
- **Item 1 `.git`-as-file** (git-worktree layout): only the *existing*
  branch sees a file, handled correctly by `--ro-bind-try` (bind works on
  files); the absent branch always synthesizes a directory, which is the
  correct shape for a path that does not exist.
- **`cargo-zigbuild`**: the full in-tree cross-check is blocked by
  `wayland-sys` (a transitive GUI dep needing a Linux sysroot for
  `pkg-config`). Resolved by verifying via an isolated scratch crate
  instead — see § Verification. zigbuild itself works; the blocker is a
  missing target sysroot, out of proportion to fix for this change.
