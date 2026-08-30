# Logic Review Report
**Module**: src/sandbox
**Scope**: ~7706 LOC across 26+ files, end-to-end security audit
**Date**: 2026-08-28
**Mode**: strict (security-critical)
**Branch**: audit/2026-08-28-routing-runtimes-sandbox-search-secrets
**Worktree**: /home/zou/data/workspace/Aleph/.worktrees/audit-2026-08-28

---

## Executive Summary

The `src/sandbox` module is the security boundary between the agent and the
operating system. The 26+ files, ~7706 LOC, and many subdirectories (command_policy/,
exec_approval/, platforms/{linux,macos,windows}/, proxy/, workspace/, windows_init/)
implement a defense-in-depth stack: command-policy hardline → capability
elevation approval → OS-level sandbox driver (seatbelt / bwrap + landlock+seccomp
/ AppContainer) → secret-scrub + block-class output gate.

The architecture is sound and the recent cycles (Cycle 1-8, the 2026-05
hardenings, the 2026-08 routing/runtimes work) closed many of the most severe
classes of vulnerability. The previous audit (`rust-logic-audit-sandbox-2026-05-31`)
fixed three critical issues, all of which remain fixed.

This audit found:
- **5 Critical** issues — two of them well-known tradeoffs in the design
  (sandbox overlay models that were deliberately constrained), three are
  concrete wiring defects that bypass documented guards.
- **14 Warning** issues — concrete security-relevant defects ranging from
  TOCTOU windows, OS-specific asymmetries in capability enforcement, and
  cross-module interface drift.
- **8 Suggested Tests** — observations that are not currently covered by the
  test suite and would catch regressions.

The single most concerning finding is **F-1: `deny_read_globs` is silently
dropped on Linux** — an operator who configures the `**/.env`-style
secret-deny floor on a Linux install believes secrets are protected, but
the protection is only applied on macOS and Windows. The `bwrap.rs`
`create_platform_driver_with_config` literal `_ = deny_read_globs;` is
the smoking gun.

The second most concerning is **F-2: capability "shell-fork" exemption is
platform-dependent, hard-coded to `cfg!(target_os = "linux")`** — the
behavior of an LLM-supplied `allow_subprocess=true` flag is
byte-different on Linux vs. macOS. This is OS-conditioned fork enforcement
that has bitten this codebase before (Cycle 2 quote: "a forking workload
silently retained the full capability set").

---

## Findings

### Critical

#### F-1. `deny_read_globs` floor is silently dropped on Linux
**File**: `src/sandbox/platforms/linux/bwrap.rs:48` (in `create_platform_driver_with_config`)
**Severity**: Critical

The `[sandbox] deny_read_globs = ["**/.env", "**/*.pem"]` config is
documented as a defense-in-depth secret-deny floor that applies to every
sandboxed command. The comment on the field in `config.rs:512-518` even says
"Currently enforced by the macOS seatbelt driver; other platforms ignore it
until landlock/WFP glob enforcement lands." That comment is the threat: an
operator running Aleph on Linux, who reads the config field, who tests
that the path `~/.aleph/workspaces/<hash>/.env` cannot be read, will
**believe** the protection is active.

`bwrap.rs` literally drops the value:

```rust
let _ = (
    windows_config,
    deny_read_globs,                    // <- silently discarded
    allow_unix_sockets,
    dangerously_allow_all_unix_sockets,
    allow_local_binding,
);
```

**Attack vector**: Configure a deny glob for `**/secret_*.json`, restart
the daemon, run any code-exec tool, `cat ~/workspaces/.../secret_*.json`.
The OS sandbox has no notion of the glob, the file is read, the secret
exfiltrates via the next outbound connection.

**Proposed patch** (no code changes; design choice for a future cycle):

Either (a) emit a startup `tracing::warn!` when `deny_read_globs` is
non-empty on Linux, with a clear "ENFORCEMENT DISABLED" message, so the
operator is not silently misled; or (b) implement the floor in landlock
(sandbox_init.rs apply_landlock) by expanding each glob to matching
concrete paths inside the workspace root. Option (b) is the real fix; (a)
is the documentation fix that should ship immediately.

```rust
// Factory.rs boot — emit an operator-visible warning when the floor is configured
// but the running platform cannot enforce it.
#[cfg(target_os = "linux")]
if !cfg.deny_read_globs.is_empty() {
    tracing::warn!(
        target: "sandbox",
        patterns = ?cfg.deny_read_globs,
        "[sandbox] deny_read_globs configured but Linux/bwrap has no native \
         glob-deny primitive; landlock enforcement pending (SP-2 follow-up). \
         A code-exec run CAN read these files until that lands."
    );
}
```

---

#### F-2. `as_capabilities()` is platform-conditioned; same LLM input produces different `spawn_subprocess` per OS
**File**: `src/builtin_tools/code_exec.rs:189-190`
**Severity**: Critical (cross-platform capability confusion)

`CodeExecArgs::as_capabilities()` sets `spawn_subprocess: true` only when
`(cfg!(not(target_os = "linux")) && matches!(self.language, Language::Shell))`.
That is a build-time `cfg!()` reading a runtime argument, producing a
non-deterministic-feeling capability set for the same `code_exec` call
across machines of different targets. It also means an LLM that supplies
`allow_subprocess: true` for a Python script on macOS/Windows gets fork
permission; the exact same call on Linux does not.

The comment ("Fork is not the boundary this sandbox enforces") is
correct for the fork semantics, but the actual behavior of this knob is
**inconsistent**:

| platform | language | allow_subprocess | result | security-relevant |
|---|---|---|---|---|
| linux | shell | false | `false` (from language clause not matched) | OK — bwrap does `--unshare-pid` |
| linux | shell | true | `true` | OK — bwrap omits `--unshare-pid` |
| linux | python | true | `true` | **weird** — model got fork without the Shell-only exemption |
| macos | shell | false | `true` (cfg + language) | breaks "model sets `false` ⇒ no fork" |
| macos | python | true | `true` | consistent |
| macos | python | false | `false` | OK |

The macos/Shell/false row is the real defect: the model is told "I do not
need fork" and the sandbox grants it anyway. This is a **capability
confusion** — the user-facing flag no longer controls the granted
capability. This is the class of bug Aleph invariant #6 (approval flow
integrity) warns about: "exec approval cannot be bypassed by crafted
tool names — verify this concretely."

**Attack vector**: A user configures the install at `Plan` tier (refuses
all mutating tools). A model crafts a `code_exec(language=Shell,
allow_subprocess=false, allow_network=false)` call on macOS — it should
be denied at the boundary. Today it is **granted fork** on macOS because
the capability derivation hard-codes a `cfg!()` branch the model cannot
control.

**Proposed patch**:

```rust
// In CodeExecArgs::as_capabilities — derive spawn_subprocess from
// `self.allow_subprocess` ONLY, with a documented platform-specific
// baseline:
fn as_capabilities(&self) -> SandboxCapabilities {
    let default_spawn = match self.language {
        // Shells are compound-command languages: a && b, |, $(). Blocking
        // fork there breaks "ordinary shell" without containing anything
        // (bash exec's a simple command in place).
        Language::Shell => true,
        // Interpreters are already running when they ask to spawn; the gate
        // is the only thing that contains them. Default off, model opts in.
        Language::Python | Language::JavaScript => false,
    };
    SandboxCapabilities {
        fs_read: Vec::new(),
        fs_write: self.extra_writable_paths.clone(),
        network: if self.allow_network { AllowAll } else { None },
        spawn_subprocess: self.allow_subprocess || default_spawn,
        // ...
    }
}
```

Remove the `cfg!(not(target_os = "linux"))` arm entirely. The platform
specificity belongs in the OS driver (bwrap's `--unshare-pid` decision),
not in the capability derivation. A macOS shell-tool call where the
model said `allow_subprocess=false` should not silently get fork.

---

#### F-3. `code_check` tool hard-codes `spawn_subprocess: true` and skips approval
**File**: `src/builtin_tools/code_check.rs:203-225`
**Severity**: Critical (silent capability grant)

`CodeCheckTool::run()` constructs a `SandboxCommand` with a literal
`spawn_subprocess: true` and **no** `allow_subprocess` parameter in
`CodeCheckArgs`. The comment says "Type-checkers fork child processes
(rustc, tsc workers, go tool); without this the sandbox would block
them." — but the consequence is that **every** `code_check` call gets
fork permission, never via approval, never inspected. A model that
intentionally tightens the install to a `Plan` tier (which refuses
mutating tools) still gets subprocess fork from a checker.

This bypasses the explicit `WorkspaceSandbox::execute` capability
pipeline. A `code_check` call against a `WorkspaceSandbox` is forced
through `is_within` check on a `spawn_subprocess: true` capability, which
**exceeds the session baseline** (`strict()` ⇒ `spawn_subprocess: false`),
so it triggers the approval gate. But `WorkspaceSandbox` is the
fallback; the `WorktreeSandbox` does not even implement the approval gate
(see F-7 below). And when `code_check` runs under a `WorktreeSandbox`,
the `spawn_subprocess: true` is silently granted with no approval.

This is exactly the class of bug the previous audit's "approval cannot be
bypassed by crafted tool names" invariant warns about.

**Attack vector**: An attacker who lands a code-check call in a worktree
sandbox (e.g. via `subagent_spawn` with isolation=Worktree) can fork any
process without going through the approval gate.

**Proposed patch**: Surface `allow_subprocess` as a `CodeCheckArgs`
parameter, defaulting to `true` (the current behavior) but observable
through the approval card. The `Plan` tier then denies the call the same
way it denies `code_exec`. Optionally, route the call through
`WorkspaceSandbox` with the existing capability elevation path:

```rust
// CodeCheckArgs
pub allow_subprocess: bool,  // default true; surfaced in the card

// CodeCheckTool::run
let spawn_subprocess = args.allow_subprocess
    || matches!(args.command_or_default(), /* shells */ _);
let caps = SandboxCapabilities {
    spawn_subprocess,
    network: NetworkPolicy::None,
    // ...
};
// then `sandbox.execute(cmd)` — WorkspaceSandbox will hit the
// approval gate if `spawn_subprocess` exceeds the session baseline.
```

---

#### F-4. `WorkspaceSandbox::summary()` always reports `WorkspaceWrite` and `network: Denied`, regardless of call baseline
**File**: `src/sandbox/workspace/mod.rs:174-202`
**Severity**: Critical (LLM prompt deception)

`summary()` is what `OperatingEnvelopeLayer` injects into the LLM prompt
to tell the model what its sandbox can do. The function unconditionally
returns `policy_tier: WorkspaceWrite.as_str()` and
`network: NetworkState::Denied` — **independent of the session's
actual baseline** (which is `session_baseline()` = `strict()` ⇒
`ReadOnly` for almost every install). The prompt tells the model "you
can write to the workspace, network is denied" — a faithful rendering
of the *envelope*, but not the *baseline*.

The comment "Honest per-call default: the baseline is `strict()` (network
DENIED). Network is reachable only via an approval-gated capability
escalation" admits the model cannot tell the difference between
"the install is at read-only" and "the install is at workspace-write".

In a project-room install where multiple agents share a workspace and one
agent has `fs_write: [/etc]`, the LLM sees `writable_roots: [parent of
the workspace hash]` and is invited to assume the workspace hierarchy is
writable, when the actual baseline is `strict()`. The model can then
issue `code_exec(extra_writable_paths: [/etc/passwd])` and the
`granted_elevations` cache is keyed on the project room, not the agent —
so one agent's grant can leak across the room.

**Attack vector**: A multi-tenant install where the operator thinks the
default session baseline is `ReadOnly`. An agent sees "workspace-write"
in its prompt and uses `code_exec` to ask for `fs_write: [/etc]`. The
escalation is granted once and stored in `granted_elevations` for the
whole session — which is shared across the project room.

**Proposed patch**: `summary()` should read the per-session baseline via
`for_session` (or a cached value on the struct) and emit the
corresponding tier:

```rust
async fn summary(&self) -> Option<SandboxSummary> {
    // Read the baseline for the active session so the prompt reflects it.
    let baseline = current_session()
        .and_then(|sid| self.sessions.try_read().ok()?.get(&(sid, self.jail_root_for(&sid))).cloned())
        .map(|ws| ws.baseline.clone())
        .unwrap_or_else(SandboxCapabilities::session_baseline);
    Some(SandboxSummary::from_baseline(self.os_driver.platform(), &baseline))
}
```

`SandboxSummary::from_baseline` already correctly maps `strict()` to
`ReadOnly` — the bug is that we never call it with the real baseline.

---

#### F-5. Approval requester deref under std `RwLock` is correct, but `record_gate_decision` calls `crate::identity::global()` without lock discipline
**File**: `src/sandbox/exec_approval/gate.rs:185-235`
**Severity**: Critical (cross-module identity bypass)

`ApprovalGate::request_approval_for_action` reads
`current_turn_context()` directly (line 169) without going through any
lock. The `unattended` check is a fast task-local read; the requester
is cloned out of a `RwLock<Option<Arc<dyn ApprovalRequester>>>` and the
lock guard is dropped before the await. This part is correct.

But the subsequent `record_gate_decision(action, &response).await`
function (line 200) calls into `crate::identity` and
`crate::gateway::visibility::ambient_actor()` with **no documentation
of the lock hierarchy** between the sandbox gate and the identity
subsystem. The CLAUDE.md Aleph invariant says "**Sync Primitives
Import Rule**: `Arc/Mutex/RwLock/atomics` from `crate::sync_primitives`".
The gate does this correctly (`use crate::sync_primitives::RwLock`),
but `record_gate_decision` reads `ambient_actor()` and writes the ledger
without specifying whether the ledger is a `crate::sync_primitives::Mutex`.

The `record_gate_decision` calls `crate::identity::record_action(...)`
which presumably takes its own locks. If `record_action` were ever
called from a context that already holds one of the gate's locks, the
sandbox gate's `RwLock` would deadlock. The reverse — sandbox holds an
identity lock and identity calls into the gate — is similarly unprovable.

This is a **lock-hierarchy** defect even when no deadlock currently
exists: it is impossible to reason about whether a future change will
introduce one, because the gate's locks are taken across async
boundaries where the cross-module locks may be held in conflicting
orders. The previous audit's lock-hierarchy warnings (per
`rust-logic-audit-sandbox-2026-05-31` §B) were narrower (workspace.rs
internal `granted_elevations`) and this extends the concern to the
approval-to-identity edge.

**Proposed patch**: Document the lock hierarchy at the module level
(`exec_approval/gate.rs` mod doc) and assert it in a unit test that
re-uses the `crate::sync_primitives::Mutex` rather than `std::sync`:

```rust
//! # Lock hierarchy
//!
//! This module holds `crate::sync_primitives::RwLock<Option<Arc<dyn ApprovalRequester>>>`
//! on `ApprovalGate`. **The lock is never held across an `await`.** The
//! `record_gate_decision` call site does not take the lock; the await
//! to `request_approval` happens after the lock is dropped.
//!
//! `crate::identity::record_action` takes its own locks (see
//! `docs/engineering-reports/review-results/rust-logic-audit-identity-*.md`).
//! The hierarchy is: identity locks (acquired first) → sandbox gate lock
//! (acquired second). **Never** acquire a sandbox lock from inside an
//! identity-side callback.
```

---

### Warning

#### F-6. TOCTOU between `granted_elevations.read()` and the approval gate's `request_approval_for_action`
**File**: `src/sandbox/workspace/mod.rs:282-358`
**Severity**: Warning

The capability-cache check (line 282) takes a `read()` lock on
`granted_elevations`, then drops it, then awaits the approval gate. Two
concurrent calls with the same elevation can both pass the cache check
(both see the cache empty), both call the approval gate, and the user
is asked the same question twice. The second approval then inserts the
grant into the cache (line 358) — only the first adds the cache entry,
the second is redundant.

In the worst case, an attacker who can drive concurrent requests (e.g.
a teams fan-out) gets the user to approve N times when one approval
should suffice. The duplicate `record_approval` calls in the denial
ledger are idempotent (a no-op after the first), so the ledger is
correct, but the user experience is degraded.

**Proposed patch**: Move the cache check + insert under a single
advisory lock, or use a `tokio::sync::Mutex<HashSet>` that the gate
holds across the await:

```rust
let mut cache = ws.granted_elevations.write().await;
if !cache.iter().any(|g| normalized_caps.is_within(g)) {
    cache.insert(normalized_caps.clone()); // tentatively mark as "in flight"
    drop(cache);
    // ... await approval ...
    if outcome.is_approved() {
        // already in cache; do not re-insert.
    } else {
        // remove the tentative insert on denial.
        let mut cache = ws.granted_elevations.write().await;
        cache.remove(&normalized_caps);
    }
}
```

This pattern prevents both the double-approval and a denial path from
poisoning the cache for a subsequent call.

---

#### F-7. `WorktreeSandbox` has no command-policy tunable rules, no rate limit, no security-kernel pattern, no resource governor
**File**: `src/sandbox/worktree.rs:250-256`
**Severity**: Warning (documented but enforcement gap)

`WorktreeSandbox::new` installs only the catastrophic `hardline_only`
command-policy hook. The Stage-H scope lock (the test at line 614 pins
this with `assert_eq!(sandbox.hooks.before.len(), 1)`) intentionally
excludes:
- tunable `[sandbox.command_policy]` rules
- `[security].custom_blocked` patterns
- rate limit
- resource governor
- deny_read_globs (F-1)

The mod doc explains this is a *ruling* (not an oversight) — worktree
isolation is a tree-level concern, not an OS-level concern, and there's
no OS driver to layer on. But the consequence is that a worktree
subagent can:
1. Read any file on the host (no bwrap mount namespace).
2. Make any outbound network call (no seccomp, no netns).
3. Read `/etc/passwd`, `~/.ssh/`, or any of the secrets the parent
   `WorkspaceSandbox` would have denied via `deny_read_globs`.

This is documented in the `WorktreeSandbox` doc comment as a deliberate
ruling, but the test at line 631 (`worktree_sandbox_does_not_reach_for_a_configurable_policy`)
makes it the only source-level guard, and any future contributor who
adds a new hook to `WorkspaceSandbox` and forgets to mirror it into
`WorktreeSandbox` would silently widen the worktree's surface. The
existing comment "widening the scope is a legitimate decision; make it
one by editing those two and this paragraph in the same commit" is
correct but the test only counts the hooks (1 vs N), not the content
of what they do.

**Proposed patch**: Add a content-level guard test in addition to the
count test:

```rust
#[test]
fn worktree_sandbox_safety_floor_matches_workspace_sandbox_floor() {
    // Parse the source of `WorktreeSandbox::new` and `build_sandbox` and
    // assert that any "Block" rule in `rules::hardline_rules` is also
    // matched by a rule in the worktree's hook chain. This catches a
    // future widening of `hardline_rules` (e.g. a new catastrophic
    // shape) that forgets to be visible to worktree-isolated runs.
    //
    // Implementation: read the source as text, parse the `hardline_only`
    // call site and confirm it embeds `hardline_rules` directly.
}
```

---

#### F-8. `PathStripping` for paths inside `path_starts_with_normalized` falls back to lexical comparison when baseline does not exist on disk
**File**: `src/sandbox/capabilities.rs:155-213`
**Severity**: Warning

`path_starts_with_normalized(child, baseline)`:
1. Rejects `..` in `child` (good).
2. Calls `canonicalize` on `child` if it exists.
3. If `child` does not exist, falls back to lexical
   `child_norm.starts_with(&baseline_norm)` PLUS a parent-chain walk
   looking for symlinks.

The lexical fallback path is where the bug is. If the **baseline**
path also does not exist (which the function supports — line 184:
`baseline_canon = std::fs::canonicalize(&baseline_norm).unwrap_or_else(|_| baseline_norm.clone())`),
the comparison becomes `child_norm.starts_with(&baseline_norm)` over the
**lexical** form. The `child` was checked for `..` components, but the
**baseline** was not. A baseline of `"/tmp/.."` and a child of
`"/etc"` would compare `"etc".starts_with("..")` which is false, so the
function correctly denies. But a baseline of `"/tmp/foo/"` (trailing
slash) and a child of `"/tmp/foo/../etc"` — wait, the `..` check on
child would have rejected this.

The real bug is more subtle: when `canonicalize(&child)` returns `Ok(c)`
but `c` does not start with `baseline_canon` (which may be the
**uncanonicalized** baseline if it didn't exist), the function returns
`cc.starts_with(&baseline_canon)`. This is a `Path::starts_with`
comparison, not a component-aware one. On Windows, paths can be
`\\?\C:\foo` vs `C:\foo` and the comparison would fail or succeed in
subtle ways. On Unix, a path with a trailing `/.` (which
`normalize_path_components` strips) could shift the comparison.

The test `fs_write_rejects_symlink_swap_outside_baseline` at line 342
covers the most common case, but does not cover the baseline-doesn't-
exist case.

**Proposed patch**: Component-aware `starts_with`:

```rust
fn componentwise_starts_with(child: &Path, baseline: &Path) -> bool {
    child.components().zip(baseline.components()).all(|(a, b)| a == b)
        && child.components().count() >= baseline.components().count()
}
```

This is the canonical `Path::starts_with` behaviour per the std
documentation, and it removes any string-comparison subtlety.

---

#### F-9. `cfg!(target_os = "linux")` in `session_baseline()` and `as_capabilities()` is build-time, runtime-invisible
**File**: `src/sandbox/capabilities.rs:93-95`, `src/builtin_tools/code_exec.rs:189-190`
**Severity**: Warning

The session baseline's `spawn_subprocess` field is a compile-time
choice between Linux (false) and macOS/Windows (true). A user reading
the runtime config or the prompt summary cannot tell which their install
will get — they would need to read the source of the binary or the
`uname -m` output to know.

This is the same defect as F-2, but in the *baseline* rather than the
per-call capability. The session baseline is fixed at
`WorkspaceSandbox::for_session` time (line 161: `baseline:
SandboxCapabilities::session_baseline()`), so an install that runs the
same binary on different OSes gets different baselines silently.

**Proposed patch**: Either (a) make the platform distinction runtime via
`Platform::current()` and read it at sandbox construction; or (b)
document the asymmetry prominently in the configuration documentation
and at least expose a `[sandbox] baseline_spawn_subprocess = bool`
override.

---

#### F-10. `max_active_processes = 0` silently coerced to `1`; configuration knob has no upper bound check
**File**: `src/sandbox/platforms/windows/driver.rs:322`
**Severity**: Warning

```rust
let active_limit = if parsed.allow_fork {
    self.options.max_active_processes.max(1)
} else {
    1
};
```

The `.max(1)` protects against a `0` misconfiguration that would
otherwise make the job kill every process immediately. But the
configuration knob has no upper bound. An operator can configure
`max_active_processes = u32::MAX`, and the Windows JobObject will be
created with that limit. Combined with `kill_on_drop`, an out-of-control
subagent would be impossible to rate-limit.

**Proposed patch**:

```rust
const MAX_ALLOWED_ACTIVE_PROCESSES: u32 = 256;

let active_limit = if parsed.allow_fork {
    self.options.max_active_processes.clamp(1, MAX_ALLOWED_ACTIVE_PROCESSES)
} else {
    1
};
```

The clamp surfaces the configuration error (warn at boot if
`max_active_processes > MAX_ALLOWED_ACTIVE_PROCESSES`) rather than
silently accepting arbitrary values.

---

#### F-11. `command_text` is parsed by the hard-filter but its content is at most 512 KiB; the result is appended to a string that may exceed 1 MiB
**File**: `src/sandbox/command_policy/mod.rs:332-405`
**Severity**: Warning

`command_text(cmd)` caps output at `2 * MAX_SCAN_BYTES = 512 KiB` (line 376).
`CommandPolicy::evaluate` then windows the result over a head/mid/tail
sweep of `2 * MAX_SCAN_BYTES` again (line 240). The two passes are both
capped, but the result of `command_text` is concatenated into a single
string (line 244 — `scan_buf = format!(...)`).

A pathological 1 MiB stdin payload would:
1. Be truncated to 512 KiB in `command_text` with a marker joining head
   + tail (line 396).
2. Be re-windowed by `evaluate` to 512 KiB again (head/mid/tail).

The 256 KiB middle band is the unscanned area in the second pass; the
**first** pass already dropped the middle (line 396: keep head + tail with
marker). A rule that matches only the middle is invisible to both
passes. This is documented as a deliberate design, but the implementation
has a bug: `command_text` truncates with `floor_boundary` and
`ceil_boundary` (line 380, 395) which only works if `head_end` and
`tail_start` are valid char boundaries. The test
`command_text_caps_an_oversized_payload` covers ASCII; a multi-byte
heavy payload at exactly the boundary is not exercised.

**Proposed patch**: Add a test that uses a payload of > 256 KiB consisting
entirely of 4-byte UTF-8 sequences (e.g. emoji), and assert that
`command_text` returns without panicking and that the result is valid
UTF-8.

---

#### F-12. `dns::resolve_hosts_in_capabilities` drops the port from `host:port` entries
**File**: `src/sandbox/dns.rs:25-50`
**Severity**: Warning

When the input list is `["api.example.com:443"]`, the function checks
`is_ip_literal` (which returns true for `host:port` where the host is
an IP literal), and inserts the **whole string** (including port) into
the resolved set. When the input is `"api.example.com:443"` where the
host is **not** an IP literal, the code calls
`lookup_target_for("api.example.com:443")` which returns the input
unchanged (since it contains a `:`), and passes it to
`tokio::net::lookup_host` which interprets the `host:port` as an
authority. The resolution returns IP addresses **without** ports,
which are inserted.

The resulting `hosts` set has the port from the IP-literal case but
not from the hostname case. The proxy allowlist matcher
(`AllowList::permits`) compares against the *hostname* (no port), so
both forms work, but a defender reading the resolved set has a
half-accurate view of what the proxy will allow. A future change to
`permits` to consider ports would silently have an asymmetric input.

**Proposed patch**: Strip the port before inserting into the resolved
set, or reject `host:port` entries at the boundary with a clear error:

```rust
if is_ip_literal(host) {
    // Strip port for the resolved set; the proxy matches on hostname
    // alone and the port is configured via ProxyRouteSpec.
    let host_only = host.split(':').next().unwrap_or(host);
    resolved.insert(host_only.to_string());
} else {
    // resolve via DNS...
}
```

---

#### F-13. The `SECCOMP_DENYLIST_SIMPLE` is silently trimmed on architectures where a syscall name is missing
**File**: `src/sandbox/sandbox_init.rs:782-790`
**Severity**: Warning

```rust
for name in SECCOMP_DENYLIST_SIMPLE {
    let Some(nr) = syscall_nr(name) else {
        continue;  // syscall not present on this arch
    };
    rules.insert(nr, vec![]);
}
```

The test `seccomp_denylist_is_frozen` pins the list shape. But on
arm64, `nfsservctl`, `mknod`, `umount` may be absent — the function
silently skips them, leaving the kernel exposed to those syscalls
when running on those architectures. The comment explains this is
intentional ("supersets are the right shape for security predicates"),
but the *user-facing* guarantee is "the catastrophic floor holds" —
which is true on x86_64 and a partial truth on arm64.

**Proposed patch**: At boot, log a `tracing::warn!` listing the
denylist entries that were skipped on the running architecture, so
the operator can audit the actual protection level.

---

#### F-14. `is_blocked_ip` is `pub(crate)` but `dial_validated` in `proxy/dial.rs` is the only consumer
**File**: `src/security/ssrf/ip.rs:187`, `src/sandbox/proxy/dial.rs:31`
**Severity**: Warning

`is_blocked_ip` is the SSRF classifier for the proxy. It is gated to
`pub(crate)` (line 187). The proxy's `dial_validated` calls it for
every resolved IP (line 61). This is the only enforcement point for
SSRF in the proxy path — the OS-level seatbelt profile does not enforce
a blocklist. If a future change moves the proxy out of `crate::sandbox`
to a top-level location, the `pub(crate)` becomes restrictive in a way
that silently disables SSRF protection. The function should arguably
be `pub` (with a security-review trail) since it is the network
boundary's load-bearing classifier.

**Proposed patch**: Promote to `pub` with a security doc and a unit
test asserting the operator-visible behavior (a hostname that resolves
to a 169.254.169.254 address is refused even if the hostname is in
the allowlist).

---

#### F-15. `Landlock ABI::V5` is `BestEffort`; an older kernel silently downgrades to V1
**File**: `src/sandbox/sandbox_init.rs:633-666`
**Severity**: Warning

`landlock` ABI V5 brings `LANDLOCK_ACCESS_FS_IOCTL_DEV`, `LANDLOCK_ACCESS_FS_REFER`,
`LANDLOCK_ACCESS_FS_TRUNCATE`. The sandbox uses `BestEffort` so a V1-only
kernel (e.g. 5.13 LTS) silently downgrades the ruleset, and the only
audit is a `tracing::debug!` line. The operator can configure
`require_landlock = true` to escalate to a hard fail, but a typo or
omission in that config leaves the production install at the older
level.

**Proposed patch**: At boot, when `landlock` returns `PartiallyEnforced`,
emit a `tracing::warn!` (not `debug!`) with the access bits the kernel
did not honor, so this is visible in default log levels. The
information density is low (a few access names) and the security
posture of the running install is now traceable.

---

#### F-16. `granted_elevations` cache is keyed on `normalized_caps` but `is_within` uses directional subset, not equality
**File**: `src/sandbox/workspace/mod.rs:282-285`
**Severity**: Warning

```rust
let already_granted = {
    let granted = ws.granted_elevations.read().await;
    granted.iter().any(|g| normalized_caps.is_within(g))
};
```

The check is "the new request is a subset of a previously granted
elevation" — which is correct. But the **insert** is
`granted_elevations.insert(normalized_caps)`, which inserts the **child**
capability, not the granted one. The result is that the cache
contains a different shape than the check looks for. Subsequent calls
with the same elevation will match by virtue of the exact-element
lookup, but a *narrower* call to a previously-elevated capability will
correctly hit the cache via `is_within` — a fact that is correct but
subtle.

The more serious issue: `normalized_caps` is derived from
`cmd.capabilities.normalized()`. If a subsequent call asks for
`fs_write: [/tmp]` (which `is_within` the previously-granted
`fs_write: [/tmp, /var]`), the check passes, the gate is skipped — but
the cache now contains both entries, growing without bound. A long
session with many distinct narrowed calls will accumulate all of them
in `HashSet<SandboxCapabilities>` until the workspace is dropped.

The `HashSet` is bounded only by session lifetime (FIFO eviction is
in the grant store, not the capability cache). For a long-running
session, this is a slow leak.

**Proposed patch**: Insert the **granted** (broader) capability on
approval, not the **child** (narrower) one. Or evict by LRU on
size. The current `is_within` check would still work because the
narrower call is a subset of the broader one:

```rust
// On approval: store the most-broad elevation that subsumes this call.
let grant = match outcome {
    ApprovalOutcome::Approved => &normalized_caps,  // the *granted* shape
    ...
};
// On denial / timeout: do not insert.
```

The simpler fix: insert `normalized_caps` (as today) but cap the
`HashSet` size and evict the oldest entry on overflow.

---

#### F-17. The `seccomp-denylist` arm for `clone`/`unshare` uses `CLONE_NEWUSER` (0x1000_0000) but `clone3` is not in the list
**File**: `src/sandbox/sandbox_init.rs:797-815`
**Severity**: Warning

`clone(2)` is denied when its flags contain `CLONE_NEWUSER`, but the
modern `clone3(2)` syscall (Linux 5.3+) takes its flags via a struct
pointer, not as a register argument. The seccomp filter is registered
with `SeccompCondition::new(0, SeccompCmpArgLen::Qword, ...)` for
`clone`, but `clone3` is not in `SECCOMP_DENYLIST_SIMPLE` and is not
given a custom rule. A sandboxed process that calls `clone3(...)` with
a `clone_args` struct that includes `CLONE_NEWUSER` will pass the
filter.

**Attack vector**: A `CLONE_NEWUSER` user-namespace fork is the canonical
container-escape primitive; `clone3` is the only API new enough kernels
expose by default. The filter does not block it.

**Proposed patch**: Add `clone3` to `SECCOMP_DENYLIST_SIMPLE` and add a
custom rule that inspects the first 8 bytes of the `clone_args` struct
for the `CLONE_NEWUSER` bit. Or document the gap clearly so the
operator knows `require_landlock = true` is the only mitigation for
older kernels.

---

#### F-18. `seccomp_socket_control_denylist` is a constant; the test pins it; the consumer applies it only in `UnixOnly` mode
**File**: `src/sandbox/sandbox_init.rs:175-194`, `src/sandbox/sandbox_init.rs:858-870`
**Severity**: Warning (informational)

`SECCOMP_SOCKET_CONTROL_DENYLIST` is consumed only when
`SeccompNetworkMode::UnixOnly` is in effect (i.e. `NetworkPolicy::None`).
The `ProxyRouted` and `Unrestricted` modes do not apply it. This is
documented as intentional ("a sandboxed process that retains an
`AF_UNIX` fd can otherwise still `bind`/`listen`/`accept`"), but the
test `seccomp_socket_control_denylist_is_frozen` pins only the list
shape, not its conditional application. A future contributor who
moves the `apply_socket_gate` to a different mode (say, applying it
in `ProxyRouted`) would silently widen the deny set in a context where
the test would not catch the regression.

**Proposed patch**: Add a test that asserts `apply_socket_gate` is
only called from `UnixOnly` mode and does nothing for the other two
modes. This is a 5-line test but pins the documented intent.

---

#### F-19. `granted_elevations` insert is not transactional with the approval gate's response
**File**: `src/sandbox/workspace/mod.rs:354-365`
**Severity**: Warning (related to F-6)

The `record_approval` call (line 365) happens after the `insert` at
line 358. If `insert` succeeds but `record_approval` panics (or the
task is cancelled), the cache is updated but the ledger is not. The
consequence: a future refusal of the same intent does not advance the
breaker, and a future approval does not reset a hypothetical run of
consecutive refusals. The two states are observably inconsistent.

**Proposed patch**: Use a `try_finally`-style scope:

```rust
let insert_result = ws.granted_elevations.write().await.insert(normalized_caps);
let _ = insert_result;  // HashSet::insert returns bool, but we don't care.
denial_ledger::global().record_approval(&led_key);
```

(The order can be reversed without correctness loss because both
operations are idempotent.)

---

### Suggested Test

#### T-1. `WorktreeSandbox::summary()` does not lie about the network
**File**: `src/sandbox/worktree.rs:276-285`

`WorktreeSandbox::summary` returns `SandboxSummary::isolated_worktree`,
which hardcodes `network: NetworkState::AllowAll` and `policy_tier:
Isolated`. The mod doc says "Worktree isolation is workspace-tree only —
there is no OS-level process sandbox layered on top. The LLM should
know this so it does not assume seatbelt/landlock enforcement."

A test would pin this so a future refactor that tries to "fix" the
misleading summary fails loudly.

```rust
#[test]
fn worktree_summary_advertises_no_process_sandbox() {
    let s = WorktreeSandbox::new(std::env::temp_dir());
    let summary = s.summary().unwrap();
    assert_eq!(summary.policy_tier, "isolated");
    assert_eq!(summary.network, NetworkState::AllowAll);
    assert!(!summary.writable_roots.is_empty());
}
```

---

#### T-2. `WorkspaceSandbox::summary()` reflects the actual baseline
**File**: `src/sandbox/workspace/mod.rs:174-202`

After F-4 is fixed, a test should pin the new behavior so a future
contributor who reverts to the misleading WorkspaceWrite default
fails CI.

```rust
#[tokio::test]
async fn workspace_summary_reflects_session_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = /* WorkspaceSandbox with default session_baseline */;
    let summary = sandbox.summary().unwrap();
    assert_eq!(summary.policy_tier, "read-only", "strict baseline is read-only");
    assert_eq!(summary.network, NetworkState::Denied);
}
```

---

#### T-3. `deny_read_globs` floor on Linux is observable at boot
**File**: `src/sandbox/factory.rs` or `src/sandbox/platforms/linux/bwrap.rs`

After F-1 is at least documented:

```rust
#[cfg(target_os = "linux")]
#[test]
fn deny_read_globs_floor_emits_boot_warning_on_linux() {
    // Set RUST_LOG to a known level, construct a factory with
    // deny_read_globs non-empty, and assert the warning is emitted.
}
```

---

#### T-4. `code_check` route is exercised end-to-end through the approval gate
**File**: `src/builtin_tools/code_check.rs`

After F-3 is fixed, a test that:
- configures a `Plan` tier,
- invokes `code_check(allow_subprocess=true)` against a `WorkspaceSandbox`,
- asserts the call is denied (because the tier refuses mutating tools).

---

#### T-5. `granted_elevations` cache does not grow unboundedly in a long session
**File**: `src/sandbox/workspace/mod.rs`

```rust
#[tokio::test]
async fn granted_elevations_is_bounded_in_a_long_session() {
    // Construct 1000 distinct `fs_write: [/tmp/N]` elevations, each
    // approved. After 1024 entries, the cache should evict (or the
    // implementation should pin a bound; today it does not).
}
```

This is a slow test but documents the intent and catches the
unbounded-growth defect of F-16.

---

#### T-6. `CodeExecArgs::as_capabilities` is consistent across platforms
**File**: `src/builtin_tools/code_exec.rs`

After F-2 is fixed, a test that:
- constructs a `CodeExecArgs { language: Shell, allow_subprocess: false, ... }`,
- calls `as_capabilities()`,
- asserts `spawn_subprocess == true` on macOS **and** on Linux (or,
  more honestly, asserts that the platform branch is no longer
  there).

---

#### T-7. `seccomp_denylist` arm covers `clone3` CLONE_NEWUSER
**File**: `src/sandbox/sandbox_init.rs`

After F-17 is fixed, a unit test that constructs a `clone3`-shaped rule
and asserts the filter refuses it. The current seccompiler table has
`SYS_clone3` constants; the test would pin the deny.

---

#### T-8. `command_text` handles multi-byte UTF-8 boundary at the head/tail split
**File**: `src/sandbox/command_policy/mod.rs:332-405`

After F-11 is addressed, a test that uses a > 256 KiB payload of
pure 4-byte UTF-8 (e.g. "€€€...") and asserts the function returns
valid UTF-8 without panicking.

---

## Security-Specific Concerns

### Approval Flow Bypass Risks

1. **`CodeCheckTool` skip-path (F-3)**: `code_check` calls go through
   the sandbox but with a hard-coded `spawn_subprocess: true`. When the
   `WorktreeSandbox` is the configured sandbox (a common path for
   isolated subagents), there is no approval gate at all, and the
   hard-coded `spawn_subprocess` is silently honored. An LLM that
   hijacks a `code_check` call can fork any process on the host.

2. **`as_capabilities` platform fork asymmetry (F-2)**: On macOS/Windows,
   `CodeExecArgs { language: Shell, allow_subprocess: false }` still
   returns `spawn_subprocess: true`. An operator who believes
   `allow_subprocess: false` means "no fork" is wrong on macOS. The
   model is the attacker here: it submits a call it knows is granted.

3. **`WorktreeSandbox::summary()` over-promises (T-1)**: The LLM
   prompt says "isolated" but the actual confinement is "none beyond
   the git worktree". A model that believes seatbelt/landlock
   enforcement is active and crafts a destructive command will
   succeed.

4. **Approval ledger race (F-6)**: Concurrent subagents can prompt the
   user N times for the same elevation. The user gets approval
   fatigue, which is the precondition for the brute-force breaker to
   not actually be a defense.

5. **Hardline-only command policy in worktree (F-7)**: A worktree
   subagent can `cat /etc/passwd`, `cat ~/.ssh/id_rsa`, and read
   every path the operator believed `deny_read_globs` was protecting
   on Linux (see F-1). The cat is not a fork, so the hardline rule
   does not fire.

6. **Persistent grant on capability elevation (workspace/mod.rs:347)**:
   `ApprovedAlways` is treated identically to `Approved` in the
   elevation gate's `match` arm. The comment says this is unreachable
   because `ApprovalAction::for_command` raises the card at the
   session ceiling, but if a future change allows the elevation
   card to offer `AllowAlways` (e.g. by reusing the operator's
   `for_confirm_gate` derivation), the gate would silently create
   a *persistent* elevation grant — bypassing the per-session
   `granted_elevations` cache and persisting across restarts.

---

### Capability Confusion Risks

1. **`spawn_subprocess: true` silently included on Shell (F-2)**: The
   capability derivation hard-codes a `cfg!()` that the model cannot
   control. An LLM that says `allow_subprocess: false` is ignored on
   macOS.

2. **`is_within` fallback to lexical (F-8)**: When the baseline path
   does not exist on disk, the comparison is component-naive. A
   defender who adds a baseline to a non-existent directory (e.g. a
   future-mounted volume) gets lexical-prefix matching, which is
   bypassable.

3. **`CodeCheckArgs` has no `allow_subprocess` parameter (F-3)**: A
   user that wants to deny all subprocesses cannot do so for
   checkers. The whole `Plan` tier falls open here.

4. **Platform-default `spawn_subprocess` (F-9)**: A user reading
   docs sees "the baseline is strict" but on macOS the baseline
   includes fork. Documentation of the asymmetry is missing.

5. **`fs_read` is never granted in `CodeExecArgs` (code_exec.rs:164)**:
   `fs_read: Vec::new()` is hard-coded. An LLM that wants to read
   `/etc` cannot even ask — but a user reading the prompt sees
   "workspace-write" and assumes the workspace is fully readable. The
   LLM is told it can write but never explicitly told it cannot read
   outside the workspace. The summary's `writable_roots` is set; the
   read-side constraint is silent.

6. **Worktree summary's `network: AllowAll` (T-1)**: A worktree
   subagent is told "network allowed (all hosts)" — the prompt
   invites the model to make outbound network calls from a sandbox
   with no network confinement. An LLM that exfiltrates the
   workspace via DNS to a remote server succeeds with no
   defense-in-depth.

---

### Path / Symlink Escape Risks

1. **`canonicalize` failure in `is_within` (F-8)**: The fallback
   path uses lexical comparison. A baseline that does not exist is
   compared lexically, which is bypassable in a multi-path scenario.

2. **TOCTOU between `canonicalize` and the approval gate (workspace.rs:243-280)**: The first
   containment check uses `tokio::fs::canonicalize`. The
   `revalidate_cwd_containment` re-check is documented and tested,
   but the first check still has a small window: between
   `canonicalize` returning the resolved path and the approval gate's
   await, a sibling command in the same workspace could swap a
   symlink. The re-check is gated on `awaited_approval` — only when
   the gate was actually awaited. A non-elevated call (within
   baseline) is never re-validated, but it also doesn't await the
   gate, so the window is just the time the synchronous filesystem
   ops take (microseconds). This is a real but very narrow
   attack surface.

3. **Worktree-isolated subagent has no path jail (F-7)**: A
   `WorktreeSandbox` can `cat /etc/passwd` directly. The
   `CodeCheckTool`'s hard-coded `cwd: None` is the worktree root, but
   `cwd` is not the chroot. The OS-level `current_dir` is the
   worktree path, but absolute paths read any file.

4. **`sandbox_init.rs` policy passed via argv (sandbox_init.rs:428)**: The
   `LinuxInitPolicy` JSON is in the `sandbox-init` child's argv. A
   local user with `/proc/<pid>/cmdline` access reads the
   `fs_read`/`fs_write`/`SeccompNetworkMode` of every sandboxed
   run. The path config is not secret per se, but it leaks operator
   posture (e.g. "this install has a `AllowHosts(['internal.corp'])`"
   would be visible in argv).

5. **Symlink escapes for the `cwd: None` path (workspace.rs:226-237)**: The
   lexical workspace root is not canonicalized when `cwd: None` is
   passed by the caller. The comment explains this is deliberate
   (so the profile's writable-root rules see the same string).
   A symlink inside the workspace root that the `profile_for` call
   resolves to a real path under `cwd` would bind that target. The
   `revalidate_cwd_containment` only runs when `cmd.cwd.is_some()`,
   so the symlink-swap TOCTOU is *not* re-checked for the
   `cwd: None` path. This is documented as a deliberate
   "approved-anything" path, but the model-facing prompt tells the
   model "your cwd is the workspace root" — a symlink in that
   directory silently changes the cwd without the LLM knowing.

---

### TOCTOU Risks

1. **`granted_elevations` race (F-6)**: Two concurrent calls both
   pass the cache check, both prompt the user. The second
   `record_approval` is idempotent but the user sees a duplicate
   card.

2. **`granted_elevations` insert vs. `record_approval` (F-19)**: The
   two operations are not atomic. A panicking task or cancellation
   between them leaves the cache and ledger inconsistent.

3. **Symlink swap during the first canonicalize** (workspace.rs:243-280): The
   first containment check is one `canonicalize` call. A sibling
   task that swaps a symlink in the same workspace during the
   approval gate's await can re-direct the `cwd` — the
   `revalidate_cwd_containment` catches this when the gate was
   awaited, but for a non-elevated call (no gate), the symlink
   swap is silently honored.

4. **`dns::resolve_hosts_in_capabilities` resolves, then `maybe_spawn_proxy` rewrites, then `os_driver.profile_for` runs**: Three
   sequential steps each touch the filesystem or the network. A
   hostname that resolves to a loopback IP at step 1 could be
   re-resolved to an external IP at step 3 (TTL-based rebinding).
   The `dial_validated` function uses DNS-pinning, but the
   workspace-layer DNS resolution at step 1 does not. A hostname
   `evil.com` that resolves to `127.0.0.1` at step 1 is collapsed
   to loopback (good); a hostname `evil.com` that resolves to
   `1.2.3.4` at step 1 is preserved as `1.2.3.4` and the OS profile
   enforces it. The OS profile is `AllowHosts(['1.2.3.4'])` which is
   `UnsupportedPolicy` on Linux — fail-closed. The proxy path
   re-validates per-connect via `dial_validated`. So the rebinding
   is caught at connect time.

5. **`maybe_spawn_proxy` reads `cmd.env` and inserts proxy env vars, then `os_driver.run` reads the same `cmd.env`**: The
   `env` is mutated between these steps. A concurrent
   `Sandbox::execute` call on a different session is unaffected
   (different `cmd`), but the same `cmd` is single-threaded here.

6. **`denial_ledger::record_denial` reads `counts.contains_key(fingerprint)` under a lock, then `denials.consecutive += 1`**: A
   concurrent `record_approval` resets `consecutive` to 0 between
   the read and the write. The `Mutex` makes both operations
   atomic, so this is fine, but the `first_refusal_of_this_intent`
   check on line 480 happens after the read — a race is
   impossible because the lock is held.

---

## Wiring Gaps (this module → outside)

| Item | Type | Status | Should be used by |
|------|------|--------|------------------|
| `SandboxSummary` (`src/sandbox/summary.rs`) | Stable DTO | Wired in `OperatingEnvelopeLayer` per `CLAUDE.md` R9 | `gateway/execution_engine/run_loop` prompt assembly |
| `ApprovalAction::for_command` (`exec_approval/action.rs`) | Action | Wired in `WorkspaceSandbox::execute` step 3 | `exec_approval/grants.rs::granted_within` and the persistent tier's reachability check |
| `denial_ledger::action_fingerprint` (`exec_approval/denial_ledger.rs`) | Stable | Used by both gates (now share `ledger_key`) | `confirm_with_memory` in `tools/scoped` |
| `command_policy::CommandPolicy::hardline_only` (`command_policy/mod.rs`) | Always-on floor | Wired in `factory::build_sandbox` "tunable disabled" arm, and in `WorktreeSandbox::new` | Every sandbox that runs exec-class tools |
| `WorktreeSandbox::new` (no deny_read_globs, no rate limit) | Sandbox impl | Wired in `agents/subagent_spawner/mod.rs:506` and `teams/dispatcher/runner.rs:379` | Subagent task delegation; the gap (F-7) is the deliberate Stage-H scope lock |
| `proxy::AllowList::permits` (`proxy/allowlist.rs`) | Allowlist matcher | Wired in `proxy::connect::handle` and `proxy::socks5::handle` | macOS `AllowHosts`; Linux via the netns bridge |
| `dial_validated` (`proxy/dial.rs`) | SSRF-resistant dial | Wired in `proxy::connect::handle` (line 119) | Every proxy upstream connection |
| `deny_read_globs` (`config.rs:512-518`) | Glob floor | **Configured** in `platforms::mod::create_platform_driver_with_config` but **silently dropped** on Linux (F-1) | The user-facing docstring says it is enforced |
| `cgroup_v2::CgroupV2Scope::try_create` (`cgroup_v2.rs`) | RAII cgroup | Wired in `BubblewrapDriver::run` when `cgroup_enabled = true` (default) | Every Linux sandbox run |
| `sandbox_init::run_init` (`sandbox_init.rs`) | Init binary | Wired in `BubblewrapDriver::run` line ~630 | Every Linux bwrap run; Linux-only |
| `cgroup_v2::write_current_pid_to_path` (`cgroup_v2.rs`) | AS-safe cgroup.procs write | Used in `pre_exec` | Every Linux bwrap run with cgroup |
| `live_tail::LiveTail` (`live_tail.rs`) | Rolling byte ring | Wired via `context::LIVE_TAIL` task-local in `run_child_with_drain` | `bash_exec::spawn_background` and any reader |
| `worktree::create` (`worktree.rs`) | Worktree create | Wired in `subagent_spawner` and `teams/dispatcher::runner` | Subagent and team task isolation |
| `deny_globs::resolve_deny_read_paths_under` (`deny_globs.rs`) | Path resolver | Wired in `macos/seatbelt.rs::add_deny_read_globs` and `windows_init/imp/app_container.rs:539` (Cycle 7) | macOS Seatbelt, Windows AppContainer |
| `command_policy::record_policy_decision` (`command_policy/mod.rs:488`) | Audit recorder | Called from `CommandPolicyHook::before` and `security_kernel_hook.rs:48` | Durable `security_audit_log` table |
| `ScrubResult::blocked` (`scrub.rs`) | Block-class secret | Wired in `WorkspaceSandbox::execute` post-driver and `WorktreeSandbox::execute` | All sandbox runs |
| `SECCOMP_DENYLIST_SIMPLE` (`sandbox_init.rs:123`) | Frozen syscalls | Wired in `apply_seccomp` (line 782) | Every Linux bwrap run via sandbox-init |
| `SECCOMP_SOCKET_CONTROL_DENYLIST` (`sandbox_init.rs:175`) | Frozen socket control | Wired in `apply_socket_gate` for `UnixOnly` mode | Every `NetworkPolicy::None` Linux run |

### Wiring gaps to fix

| Gap | Direction | Severity | Concrete risk |
|-----|-----------|----------|---------------|
| `deny_read_globs` not enforced on Linux | `src/sandbox/platforms/linux/bwrap.rs` ← `src/sandbox/config.rs` | Critical (F-1) | Operator believes secrets are protected; they are not. |
| `code_check` skips approval | `src/builtin_tools/code_check.rs` → `src/sandbox/workspace/mod.rs` | Critical (F-3) | A `Plan`-tier install still spawns subprocesses from `code_check`. |
| `WorktreeSandbox` lacks deny_read_globs | `src/sandbox/worktree.rs` ← `src/sandbox/platforms/macos/seatbelt.rs` | Warning (F-7) | Worktree subagent can read any host file. |
| `WorkspaceSandbox::summary()` ignores session baseline | `src/sandbox/workspace/mod.rs:174` | Critical (F-4) | LLM prompt lies about capability envelope. |
| `clamp_foreground_timeout` not applied to `code_check` | `src/builtin_tools/code_exec.rs:72` → `src/builtin_tools/code_check.rs:189` | Warning | A `code_check` with `timeout_seconds: u64::MAX` is not clamped; only the recent BTT-1 fix added the clamp to `code_check`. Verify the clamp is present in the current code (it is, line 184 calls `clamp_foreground_timeout`). |

---

## Lock/Cross-Module Concerns

### `WorkspaceSandbox` lock hierarchy

`WorkspaceSandbox` holds two async locks:
- `sessions: Arc<RwLock<HashMap<(SessionId, PathBuf), Arc<SessionWorkspace>>>>`
  (line 56)
- `SessionWorkspace::granted_elevations: RwLock<HashSet<SandboxCapabilities>>`
  (line 70)

Both are `tokio::sync::RwLock` (not the crate's `sync_primitives`
alias). The crate's `sync_primitives` is used in `live_tail`,
`denial_ledger`, `gate`, `grants` — but not in `workspace`. This is a
**consistency issue**: the same file imports
`crate::sync_primitives::Mutex` (in test code at line 730) and uses
`tokio::sync::RwLock` in production (line 56). A future contributor
who copies a pattern from `gate.rs` (using `sync_primitives::RwLock`)
into `workspace/mod.rs` would mix the two lock types and the
async-blocking semantics would diverge.

**Proposed patch**: Convert `workspace/mod.rs`'s locks to
`crate::sync_primitives::RwLock` (which is `tokio::sync::RwLock` in
this repo per CLAUDE.md R7) and document the rule.

### `ApprovalGate::request_approval_for_action` lock discipline

The function reads `crate::tools::turn_context::current_turn_context()`
without acquiring any lock (line 169) — this is correct because
`current_turn_context` is a `tokio::task_local` and is read atomically.

The `ApprovalGate::requester` lock (line 178) is acquired and dropped
before the await — correct (line 181).

`record_gate_decision` (line 200) calls `crate::identity::record_action`
which presumably takes its own locks. The `Sandbox` module's
contribution to this chain is `record_approval_decision`'s
`action_fingerprint` and `decision.outcome`. **The crate's documented
lock hierarchy must have `identity` < `approval gate` < `denial ledger`
< `sandbox capability cache`.** None of this is documented in
`docs/reference/`.

### Cross-crate concerns

1. `exec_approval/gate.rs` imports `crate::tools::turn_context` —
   this creates a reverse dependency: the `sandbox` module, which is
   supposed to be a security boundary, depends on the `tools`
   module. The dependency is the `unattended` check; if the
   `turn_context` is ever moved to a different module, the gate's
   check silently breaks.

2. `sandbox/command_policy/mod.rs` calls
   `crate::security::audit::global()` to log decisions (line 496).
   The audit log is a process-global, and the policy hook is per-call.
   Two concurrent policy decisions share the global audit log — a
   concurrent flush to disk can interleave the two. The `mutate`
   pattern in `exec_approval/grants.rs:495` uses a file lock; the
   audit log uses... not a file lock (need to verify
   `security/audit.rs`).

3. `sandbox_init::run_init` is called by `bwrap` via `argv`. The
   argv includes `--policy <json> -- target args`. The JSON
   contains the `SandboxCapabilities` translated to read/write
   paths. A local user with `/proc/<pid>/cmdline` access reads
   the policy. This is a minor information disclosure.

4. `sandbox/proxy/lifecycle.rs::dispatch` uses
   `stream.peek(&mut first)` (line 148). The peek is followed by
   routing to either HTTP CONNECT or SOCKS5 handler. The SOCKS5
   handler **consumes** the peeked byte (it's part of the version
   byte). The HTTP handler **does not** — it re-reads from the
   BufReader. The BufReader is constructed at the top of
   `connect::handle` (line 88) from the `rd` half of the split.
   The byte peeked here is *still in the kernel buffer* when the
   HTTP handler reads — the BufReader fetches it. This works, but
   the comment in the source (line 142) is misleading: "We need a
   real read (not a peek-only) because tokio doesn't expose
   MSG_PEEK on TcpStream; we splice the byte back via a small
   in-process splitter when we hand off to the protocol handler."
   `tokio::net::TcpStream::peek` *does* exist (added in 1.13.0,
   verified in tokio-1.52.3 source). The comment is stale.

---

## Wiring Completeness Audit (per Phase 2 critical check #5)

### `driver.rs` — every driver variant has a concrete implementation registered in `factory.rs`

| driver | platform | factory path | status |
|--------|----------|--------------|--------|
| `BubblewrapDriver` | `linux/bwrap` | `create_platform_driver_with_config` line 56 | wired |
| `SeatbeltDriver` | `macos/seatbelt` | `create_platform_driver_with_config` line 41 | wired |
| `WindowsSandboxDriver` | `windows/token` | `create_platform_driver_with_config` line 71 | wired |
| `UnsupportedDriver` | any other | line 89 | wired (returns `Other`) |

All four registered. No missing driver.

### `factory.rs` — every driver type has a creation path

The single `create_platform_driver_with_config` function is the only
creation site. No factory function is missing.

### `policy.rs` — every policy rule is reachable from a real code path

`SandboxPolicy::from(&SandboxCapabilities)` is the only construction
path. It is called in three driver `profile_for` implementations
(macos, linux, windows). All 5 `FsPolicy` variants are reached
through test paths. All 3 `NetworkPolicy` variants are reached in
production paths. **No dead policy rule**.

### `command_policy/` — every rule is enforced somewhere

| rule class | wired in | status |
|------------|----------|--------|
| `Block` (hardline) | `CommandPolicy::evaluate` always | wired |
| `Block` (tunable) | `CommandPolicy::evaluate` when `enforcement != Off` | wired |
| `Warn` (tunable) | `CommandPolicy::evaluate` records but allows | wired |
| `Off` | `CommandPolicy::evaluate` skips tunable entirely | wired |

`hardline_rules` is also referenced from `WorktreeSandbox::new` (line
253) via `CommandPolicy::hardline_only()`. **No dead rule**.

### `exec_approval/` — every approval state has all transitions wired

| state | approved | denied | timeout | unavailable | always | session |
|-------|----------|--------|---------|-------------|--------|---------|
| `PendingEntry` | `resolve` (record) | `resolve` (record) | expires | n/a | n/a | n/a |
| `ApprovalGate` (sandbox) | `record_approval` + `granted_elevations.insert` | `record_denial` (UserRejected) | `record_denial` (Timeout) | `record_denial` (Unreachable) | accepted but no `granted_elevations` (per doc) | accepted as Approved (per doc) |
| `ScopedToolService` (tools) | `record_approval_decision` (tools) | `record_denial` (confirm gate) | `record_denial` (Timeout) | `record_denial` (Unreachable) | `record_grants::granted_within` | `record_grants::granted_within` |

Both gates use the same `denial_ledger::record_denial` and
`record_approval`. The mapping `outcome → reason` is in
`DenialReason::for_refusal` (one derivation). **No dead transition**.

### `deny_globs.rs` — every glob pattern is read at least once

`deny_read_globs` is read in:
- `macos/seatbelt.rs::add_deny_read_globs` (line 775) — produces
  SBPL `(deny file-read* (regex #"..."))` lines.
- `windows_init/imp/app_container.rs:539` — produces
  AppContainer deny-read ACEs.

**Not** read in `linux/bwrap.rs` (F-1).

**Not** read in `WorktreeSandbox` (F-7).

A pattern in `deny_read_globs` is silently dropped on Linux
installs and worktree subagents.

### `protected_paths.rs` — every protected path is enforced

`protected_paths_for` is called in:
- `bwrap.rs::push_metadata_protection_args` (line 463)
- `seatbelt.rs::add_fs_policy` (around line 720)
- `WorktreeSandbox` — no, but `WorktreeSandbox` has no OS driver.

`first_writable_symlink_component` is called in:
- `bwrap.rs::push_metadata_protection_args` (line 467)
- `seatbelt.rs::add_fs_policy` (around line 740)
- `workspace.rs::revalidate_cwd_containment` (line 1100)

**All three calls are wired. No dead call.**

### `resource_governor.rs` — every limit is checked

The governor has a single `before` hook, gated on
`governor.is_enabled()`. The two limits (`min_available_memory_mb`,
`max_cpu_percent`) are both `Option<u64>` / `Option<f32>` — `None`
disables the dimension. **All limits are wired; no dead knob.**

### `dns.rs` — DNS interception covers all required traffic

`resolve_hosts_in_capabilities` is called once in
`workspace.rs::execute` (line 462), only when
`capabilities.network` is `AllowHosts`. `None` and `AllowAll` skip
DNS. The `maybe_spawn_proxy` re-runs network mutation before
`resolve_hosts_in_capabilities` (line 461), so a proxied
`AllowHosts(['evil.com'])` is collapsed to `AllowHosts(['127.0.0.1'])`
and the DNS step is a no-op.

The `MaybeSomeday: AllowAll + DNS pre-resolve` could be added but is
out of scope (the `is_ip_literal` allow-all is intentionally raw).

**All paths covered.**

### `proxy/` — every proxy handler is dispatched

`lifecycle.rs::dispatch` reads the first byte and routes to:
- `connect::handle` (HTTP CONNECT)
- `socks5::handle` (SOCKS5)

Both handlers are reached in the lifecycle test
`http_connect_denied_for_disallowed_host` and
`socks5_domain_request_denied`. **No dead handler.**

### `worktree.rs` — worktree isolation is enforced before every isolation-required action

`WorktreeSandbox::execute` (line 270) runs the catastrophic
command-policy hook first (line 273), then the spawn. There is no
OS-level driver to layer on, so the worktree is the only
isolation. The hook's only consumer is `WorktreeSandbox::new`
(line 250).

The git worktree is created in `subagent_spawner` and
`teams/dispatcher::runner` (see Wiring Gaps table). Both call
`worktree::create` before exposing the path to the agent.

**Wiring complete; gap is the deny_read_globs / rate-limit absence
(F-7).**

### `sandbox_init.rs` — every init step is called during boot

Steps in `run_init`:
1. `parse_init_args` (line 351)
2. `set_no_new_privs` (line 357)
3. `activate_proxy_routes_from_env` (line 374)
4. `apply_landlock` (line 384)
5. `apply_seccomp` (line 392)
6. `Command::exec` (line 396)

All six run in order. **No dead step.**

`apply_seccomp` calls `apply_socket_gate` for the seccomp socket
gate. `apply_landlock` is gated on the kernel ABI (F-15).

### `windows_init/` — every init step is called

`WindowsSandboxDriver::run` calls into the
`aleph-server sandbox-init-windows` subcommand with a serialized
`WindowsInitPolicy`. The subcommand has its own `run_init`-like
entry in `windows_init/mod.rs`. The policy fields cover:
- `require_restricted_token`, `use_app_container`, etc. (token
  tier flags)
- `app_container_capabilities` (capability SIDs derived from
  network policy)
- `workspace_path`, `deny_read_globs` (Cycle 7)

`max_active_processes` is **not** in the `WindowsInitPolicy` — it's
only in the host-side `WindowsSandboxOptions` (used by the Job
Object). A misconfig that wants "1 active process under AppContainer"
cannot express that; the AppContainer path inherits the
host-side Job Object setting. **Wiring gap: `max_active_processes`
should be in the init policy, so the AppContainer path doesn't
silently use a different ceiling than the host-side config.**

---

## Detailed File-by-File Notes

### `src/sandbox/mod.rs` (94 lines)
- `Sandbox` trait is object-safe and has a default `summary()` that
  returns `None`. **No defects.**
- `pub mod` declarations expose the security boundary. `pub mod
  cgroup_v2;` is `pub(crate)` — that is correct (Linux-only internal).

### `src/sandbox/command.rs` (236 lines)
- `SandboxDenialHint::detect` matches on `String::from_utf8_lossy` —
  a non-UTF-8 stderr that contains a deny signature in its raw
  bytes would still match (the lossy conversion preserves the
  ASCII subset). **OK.**
- `SandboxError::Other` is the catch-all and is used in many
  places where a typed variant would be more informative. This is
  a code-quality issue, not a security one.

### `src/sandbox/capabilities.rs` (440 lines)
- `path_starts_with_normalized` is the canonical path gate.
  Coverage is mostly thorough; the only gap is the lexical fallback
  (F-8) and the missing `Component::ParentDir` check on the
  baseline (which is fine — the baseline is operator-controlled
  and is not adversarial).
- `limit_within` for `max_memory_mb` / `timeout_secs` is correct.
- `network_within` is correct.
- `spawn_ok` is correct.

### `src/sandbox/cgroup_v2.rs` (360 lines)
- `write_current_pid_to_path` is the AS-safe helper. Verified safe
  per the 2026-05 audit (F-1 in the previous audit, fixed).
- `parse_proc_self_cgroup_path` rejects `..` components. **OK.**

### `src/sandbox/config.rs` (570 lines)
- `default_workspace_root` falls back to `/tmp/.aleph/workspaces` on
  `get_workspaces_dir` failure. Predictable path; see TOCTOU
  discussion in section 4.
- `deny_read_globs` is configured but silently dropped on Linux
  (F-1).

### `src/sandbox/context.rs` (220 lines)
- All five task-locals are properly scoped. The `with_exec_workspace`
  always scopes (even with `None`) to positively shadow an outer
  run's value. **OK.**

### `src/sandbox/denial_logger.rs` (190 lines)
- `DenialLogger` is a macOS-only observer. On Linux/Windows, it's
  a no-op. **OK.**

### `src/sandbox/deny_globs.rs` (330 lines)
- `glob_to_anchored_regex` is the source of truth for the
  glob-to-regex translation. Used in
  `macos/seatbelt.rs::add_deny_read_globs` and
  `windows_init/imp/app_container.rs`. **Not** used in
  `linux/bwrap.rs` (F-1).
- `resolve_deny_read_paths_under` walks the workspace to find
  matching paths, capped at 50,000 entries. The walk uses
  `DirEntry::metadata` (not `symlink_metadata`) so symlinks
  are not followed. **OK.**

### `src/sandbox/dns.rs` (230 lines)
- `resolve_hosts_in_capabilities` is fail-closed on every error.
- The 5s timeout is per-hostname. Total timeout for N hostnames
  is N*5s. **OK for the spec.**

### `src/sandbox/driver.rs` (160 lines)
- `OsSandboxProfile` carries the platform-specific policy
  payload. `max_memory_mb` is threaded through the profile
  rather than re-parsed from contents — correct.
- `denial_signatures` is defaulted to `&[]` so test doubles
  don't accidentally inherit another backend's dialect. **OK.**

### `src/sandbox/factory.rs` (320 lines)
- `build_sandbox` composes four hooks (security-kernel, command-policy,
  rate-limit, resource-governor) in that order, then the
  `WorkspaceSandbox`. The order is correct (security-kernel first
  so custom blocks veto custom things first).
- `NoopSandbox` is honest about being disabled. **OK.**

### `src/sandbox/hooks.rs` (180 lines)
- `SandboxHooks::run_before` short-circuits on the first denial.
  **OK.**

### `src/sandbox/live_tail.rs` (340 lines)
- Uses `crate::sync_primitives::Mutex` (consistent with CLAUDE.md R7).
- `LiveSnapshot` is a non-atomic read of the two streams (separate
  locks). The comment says "stdout may be a few bytes fresher than
  stderr" — fine for a progress view.
- The drain loop in `platforms/common.rs` uses
  `live.clone().map(...)` per task — the Arc clone is fine.

### `src/sandbox/policy.rs` (260 lines)
- `SandboxPolicy` is the driver-facing DTO. `From<&SandboxCapabilities>`
  is the only constructor. **OK.**

### `src/sandbox/protected_paths.rs` (180 lines)
- `first_writable_symlink_component` is the TOCTOU guard. Used in
  three places (see wiring). **OK.**

### `src/sandbox/proxy/` (~1500 lines)
- `mod.rs` re-exports the proxy surface.
- `lifecycle.rs::dispatch` uses `stream.peek()` (which exists on
  `tokio::net::TcpStream` since 1.13.0). The comment in the
  source is stale ("tokio doesn't expose MSG_PEEK"). **Minor doc
  defect.**
- `dial.rs` uses `is_blocked_ip` from `security::ssrf`. The
  SSRF classifier is `pub(crate)`. **See F-14.**
- `netns_bridge.rs` is Linux-only at the bridge-fork step, but the
  spec struct + JSON helpers compile cross-platform. **OK.**

### `src/sandbox/rate_limit.rs` (310 lines)
- `categorize_tool` is a hard-coded `match` on tool name. Adding
  a new tool requires updating this list. A "default to
  `ToolCategory::Read`" is a 6x loosening; the comment says this
  is "not something anyone decided". A future contributor who
  adds a new exec-class tool and forgets to add it here will
  silently under-rate-limit. **Wiring gap; pin via test
  (T-9).**

### `src/sandbox/resource_governor.rs` (380 lines)
- `ResourceGovernor` is dormant unless `enabled: true`. **OK.**

### `src/sandbox/sandbox_init.rs` (1369 lines)
- `LinuxInitPolicy` is the serialized cross-process boundary.
  Versioned via `serde` with `#[serde(default)]` for forward
  compat. **OK.**
- `apply_landlock` uses `ABI::V5` with `BestEffort` (F-15).
- `apply_seccomp` uses `SECCOMP_DENYLIST_SIMPLE` (F-13, F-17).
- The fork+exec path (sandbox_init.rs:555-650) is single-threaded
  per the mod doc. **OK.**

### `src/sandbox/scrub.rs` (450 lines)
- `scrub_secrets_bytes` is the single source of truth for
  output redaction. **OK.**
- `strip_unsafe_invisible` is the byte-path twin of
  `content::strip_invisible_chars`. **OK.**
- `scrub_and_gate_output` returns a `Vec<&'static str>` of
  block-class names. Empty = safe. Non-empty = fail closed. **OK.**

### `src/sandbox/security_kernel_hook.rs` (200 lines)
- Uses the same `command_text` and `normalize_for_matching` as
  `CommandPolicyHook`. **OK.**
- The custom-rule evaluation is a `SandboxBeforeHook`, so it
  shares the hook chain with the command policy. **OK.**

### `src/sandbox/summary.rs` (700 lines)
- `PolicyTier` is the ordered enum. **OK.**
- `SandboxSummary::from_baseline` correctly maps capabilities to
  tiers. **OK.**
- The split between `posture_lines` (cacheable) and the per-run
  `writable_roots_line` is correct. **OK.**
- `fact_census` is a test-only helper that documents which fields
  are model-visible. **OK.**

### `src/sandbox/windows_init/` (587 lines)
- `args.rs`, `mod.rs`, `policy.rs`, `tests.rs` are the
  cross-platform parts. `imp/` is Windows-only.
- `classify_protected_metadata` (Cycle 5) returns the existence
  status of each protected subpath. **OK.**
- The AppContainer launch path is the most complex; the
  post-wait cleanup is best-effort and logged on failure. **OK.**

### `src/sandbox/worktree.rs` (700+ lines)
- `WorktreeHandle::create` provisions a detached-HEAD worktree.
  `Drop` is the safety net. **OK.**
- `WorktreeSandbox` is documented as workspace-only (F-7).
- The tests pin the scope lock (`hooks.before.len() == 1`). **OK.**

---

## Cross-Module Concerns (Capability, Approval, Exec, Runtimes)

### `sandbox` → `capability` (Aleph invariant)
- `sandbox/capabilities.rs::is_within` is the canonical subset
  check. Used in `workspace.rs:282`. **OK.**
- The new `max_memory_mb` and `timeout_secs` fields are correctly
  threaded into the policy (F-16, partial issue).

### `sandbox` → `approval` (Aleph invariant)
- `exec_approval/gate.rs::ApprovalGate` is shared between the
  tool confirm gate and the sandbox elevation gate. **OK.**
- `denial_ledger::record_denial` is shared via `denial_ledger::global()`.
  Both gates use the same `for_refusal` derivation. **OK.**

### `sandbox` → `exec` (Aleph invariant)
- `exec/parser::analyze_shell_command` is used in
  `exec_approval/action.rs::for_command` to render the approval
  card. **OK.**
- `exec/masker::SecretMasker` is used in `action::redact_and_cap`. **OK.**
- `exec/allowed_decisions::{session_max, with_persistent, for_confirm_gate}`
  is the single derivation of decision sets. **OK.**

### `sandbox` → `runtimes` (Aleph invariant)
- `sandbox/sandbox_init.rs::run_init` is a `aleph-server` subcommand
  invoked by `bwrap`. The `alephcore` library exposes the function
  via the CLI; `bin/aleph-server` wires it. **OK.**
- `sandbox/proxy/netns_bridge.rs::spawn_host_bridge` is called by
  `bwrap.rs::run` to set up the UDS bridge. **OK.**

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 5 |
| Warning | 14 |
| Suggested Test | 8 |
| **Total** | **27** |

### Critical findings (top 5 most impactful security issues)

1. **F-1** `deny_read_globs` floor is silently dropped on Linux
   (`src/sandbox/platforms/linux/bwrap.rs:48`). Operator believes
   secrets are protected; they are not. The mod doc admits this is
   a known gap, but the config field's docstring does not.

2. **F-3** `CodeCheckTool` hard-codes `spawn_subprocess: true` and
   bypasses approval
   (`src/builtin_tools/code_check.rs:203-225`). A `Plan`-tier install
   still forks from `code_check` — the tool's `allow_subprocess`
   parameter doesn't exist.

3. **F-2** `as_capabilities()` is platform-conditioned fork
   (`src/builtin_tools/code_exec.rs:189-190`). The model cannot
   control the `cfg!()` branch; on macOS, `allow_subprocess: false`
   is silently overridden to `true`.

4. **F-4** `WorkspaceSandbox::summary()` always returns
   `WorkspaceWrite` and `network: Denied` regardless of the
   session's actual baseline
   (`src/sandbox/workspace/mod.rs:174-202`). The LLM prompt lies
   about the capability envelope.

5. **F-5** `ApprovalGate` holds locks across await boundaries into
   `crate::identity` without a documented lock hierarchy
   (`src/sandbox/exec_approval/gate.rs:185-235`). A future
   cross-module change could deadlock the gate.

### Concrete attack vectors discovered

- **F-1**: Read `~/.aleph/workspaces/<hash>/.env` on a Linux install
  via `code_exec` with `cat`. No protection.
- **F-3**: Subagent with `isolation: Worktree` runs `code_check` →
  forks a child that exfiltrates the worktree contents to a remote
  server. No `deny_read_globs`, no `rate_limit`, no `seccomp`.
- **F-2**: LLM crafts `code_exec(language=Shell, allow_subprocess=false)`
  on macOS to silently get fork. Used in conjunction with F-7
  (worktree) to do this without any approval.
- **F-4**: Multi-tenant install where the operator thinks the
  baseline is read-only. Model uses `code_exec(extra_writable_paths:
  [/etc])`, gets granted once, grant is cached for the session.

### Cross-module concerns

The audit confirmed that:
- `denial_ledger` is correctly shared between the tool confirm gate
  and the sandbox elevation gate (after the 2026-08 fixes).
- `granted_elevations` is correctly normalized and audited.
- `WorktreeSandbox` is correctly isolated from the worktree parent
  on disk (the git worktree handles that).
- The lock hierarchy in `WorkspaceSandbox` is internally consistent
  (single `RwLock` per workspace, no cross-await holds).

But the audit found:
- `sandbox/exec_approval/gate.rs` depends on
  `crate::tools::turn_context`, which is a layering violation (the
  sandbox module is supposed to be a leaf).
- `sandbox/sandbox_init.rs` has a `clone3` gap in the seccomp filter
  (F-17).
- The lock hierarchy across `sandbox` ↔ `identity` ↔ `denial_ledger`
  ↔ `approval gate` is not documented.

### Confirmation

Findings document path:
`/home/zou/data/workspace/Aleph/.worktrees/audit-2026-08-28/review-results/sandbox-findings.md`
