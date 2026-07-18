---
title: Host Sandbox Network Isolation — Decision and Honesty Spec
date: 2026-05-20
status: draft
spec_owner: Aleph
brainstorming_source: superpowers:brainstorming session 2026-05-20
related:
  - .claude/memory/project_skill_system_wiring_shipped.md (the deferred-item note)
  - src/tools/markdown_skill/executor.rs (current host-mode warn implementation)
  - src/tools/markdown_skill/spec.rs (SandboxMode enum)
  - src/sandbox/platforms/linux/bwrap.rs (the existing isolated-Linux path)
  - src/sandbox/platforms/macos/seatbelt.rs (the existing isolated-macOS path)
follow_up:
  - Optional Phase C-Plus — add `SandboxMode::Bwrap` (Linux-only, deferred until user demand)
  - Spec A — Skill data model unification (separate cycle, same session)
  - Spec B — Evolution AutoLoader dissolution (separate cycle, same session)
---

# Host Sandbox Network Isolation — Decision and Honesty Spec

## 1. Background

The 2026-05-20 skill-system wiring memory note listed as a deferred item:

> host 沙箱真正的网络命名空间隔离(`unshare(CLONE_NEWNET)`)未实现, host 模式只 warn, 真正隔离需用 Docker 模式

This spec resolves the deferral — **by explicitly deciding not to implement `unshare(CLONE_NEWNET)` in host mode**, and documenting the reasoning so the question does not re-surface every six months.

### 1.1 What "host mode" currently does

A Markdown CLI skill declares its sandbox mode in SKILL.md frontmatter:

```yaml
aleph:
  security:
    sandbox: host        # or "docker", "virtualfs"
    network: none        # or "local", "internet"
```

`SandboxMode` is defined at `src/tools/markdown_skill/spec.rs:133`:

```rust
pub enum SandboxMode {
    Host,        // Run on host with SafetyGate
    Docker,      // Run in Docker container
    VirtualFs,   // Run with virtual filesystem (future)
}
```

When the executor encounters `sandbox: host` with `network: none`, the current implementation at `src/tools/markdown_skill/executor.rs:55-70` does the honest thing:

```rust
// Apply network restrictions if specified.
// Host sandbox cannot truly isolate the network (that requires a
// network namespace). Be honest: set NO_PROXY as a partial mitigation
// and warn that real isolation needs Docker mode.
if let Some(aleph_meta) = &self.spec.metadata.aleph {
    if matches!(aleph_meta.security.network, NetworkMode::None) {
        cmd.env("NO_PROXY", "*");
        cmd.env("no_proxy", "*");
        warn!(
            skill = %self.spec.name,
            "skill declares network=none but runs in host sandbox; \
             network is NOT truly isolated — use sandbox: docker for \
             enforced isolation"
        );
    }
}
```

This is correct and honest. It is **not** a defect to be fixed.

### 1.2 Why the deferral was framed as a gap

The original deferral note phrased the situation as "未实现" ("not implemented"), implying the absence is a gap to close. The recon for this spec found that:

1. Real Linux netns isolation already exists in the project at `src/sandbox/platforms/linux/bwrap.rs` (506-line bubblewrap driver).
2. Real macOS isolation already exists at `src/sandbox/platforms/macos/seatbelt.rs` (547-line Seatbelt driver).
3. Real Windows isolation exists across `src/sandbox/platforms/windows/{acl,appcontainer,driver,filter,job,token,wfp}.rs`.
4. The skill-level `sandbox: host` mode is, by design, the explicit opt-out of those mechanisms — "I want this skill to run with full host privileges, please warn me when I asked for impossible network restrictions."

So the "gap" was a misframed expectation: host mode is *supposed* to be the no-isolation path. The real opportunity is to give users who want isolation without Docker a properly-named alternative — see §6 (deferred follow-up).

## 2. Decision

This spec records four binding decisions:

### 2.1 ✅ Decision 1: Do not add `unshare(CLONE_NEWNET)` to host mode

**Rationale**:
- **Cross-platform breakage**: `unshare` is Linux-only. On macOS / Windows the code would have to silently degrade to "no isolation," recreating the exact dishonesty this spec is trying to eliminate.
- **Privilege requirement**: `unshare(CLONE_NEWNET)` without root requires user namespaces (`CLONE_NEWUSER`), which are disabled by default on many distros (RHEL, hardened kernels) and inside containers. The fallback path would be "silently no isolation" — again dishonest.
- **Naming integrity**: "host" should mean "host". Adding hidden isolation inverts the user's clear declaration. Skill authors who write `sandbox: host` are choosing to trust the host environment.
- **Duplication**: bwrap already provides netns. Reinventing the unshare wrapper in markdown_skill duplicates code that has been hardened for ~500 lines in `src/sandbox/platforms/linux/bwrap.rs`.

### 2.2 ✅ Decision 2: Keep the current `NO_PROXY` + warn behavior as the canonical host-mode honesty contract

The 16-line block at `executor.rs:55-70` is exactly right. This spec **mandates that future changes preserve** the principle: *partial mitigation + truthful warning*, never *silent ineffective isolation*.

### 2.3 ✅ Decision 3: Improve the warn message to suggest the cross-platform fix

Replace:
> `use sandbox: docker for enforced isolation`

With:
> `use sandbox: docker for enforced cross-platform isolation (or wait for sandbox: bwrap on Linux — tracked in C-Plus deferred follow-up)`

The new message is honest about platform availability and points to both the current solution (Docker) and the future cheaper option (Bwrap mode, see §6).

### 2.4 ✅ Decision 4: Document the host-mode contract in user-facing docs

Add a paragraph to the skill-authoring documentation (location TBD by §3 step 3.2) stating clearly:
> *"`sandbox: host` runs the skill with your full user privileges. `network: none` under host mode sets `NO_PROXY=*` but cannot stop the binary from opening sockets directly. For enforced network isolation, use `sandbox: docker` (cross-platform) or `sandbox: bwrap` (Linux-only, planned)."*

## 3. Implementation plan (this spec — small)

| Step | Change | Files | LOC |
|------|--------|-------|-----|
| 3.1 | Update the `warn!` message at `src/tools/markdown_skill/executor.rs:63` per §2.3 | 1 file | ~5 LOC |
| 3.2 | Add the contract paragraph from §2.4 to skill-authoring docs. If no `docs/reference/MARKDOWN_SKILL_AUTHORING.md` exists yet, create it minimally; otherwise append a "Sandbox modes" section. | 1 doc file | ~30 LOC |
| 3.3 | Add a unit test in `src/tools/markdown_skill/executor.rs` that the warn fires on `(SandboxMode::Host, NetworkMode::None)` (use `tracing-test` or `tracing-subscriber` capture). If too much harness setup is needed, settle for an assertion that `NO_PROXY` env var is set on the resulting Command. | 1 file | ~30 LOC |
| 3.4 | Add a contract comment block above the host-mode `execute_on_host` function (lines 30-90) explicitly referencing this spec by date+path, so future maintainers see the decision rationale inline. | 1 file | ~10 LOC |

**Total diff**: ~75 LOC across 3 files. Single commit. Implementation in a dedicated worktree per `feedback_worktree_for_implementation`.

**Verification**:
- `cargo check -p alephcore` clean
- `cargo test -p alephcore --lib markdown_skill::executor` — new test passes
- `grep -n "sandbox: docker" src/tools/markdown_skill/executor.rs` returns the updated message
- The new docs file (if created) is linked from `docs/reference/ARCHITECTURE.md` or a skill-authoring index

## 4. Out of scope (explicit)

- **Adding `unshare(CLONE_NEWNET)`** — explicitly rejected by Decision 1.
- **Changing `bwrap.rs`, `seatbelt.rs`, or any `src/sandbox/platforms/` driver** — they are correct as-is.
- **Adding `SandboxMode::Bwrap`** — deferred to optional follow-up §6.
- **Adding macOS-side equivalent (`pfctl` or app-sandbox)** — `SandboxMode::Docker` already handles this need cross-platform; a third option per-platform is not warranted by user demand.
- **Changing the `VirtualFs` mode behavior** — out of scope; that's a separate filesystem-isolation concern.

## 5. Future restart criteria

This decision (Decisions 1 + 2) is meant to be permanent. Re-evaluate **only** if all three of these become true:

1. Linux distros widely enable unprivileged user namespaces by default (e.g., default on Debian/RHEL/Fedora stable), removing the privilege-fallback dishonesty problem.
2. A cross-platform isolation primitive equivalent to `unshare(CLONE_NEWNET)` becomes available on macOS and Windows in stable APIs.
3. User feedback consistently asks for "isolation without Docker" on a platform where bwrap is not viable.

Until then, the answer is: **use `sandbox: docker`**.

## 6. Optional follow-up: `SandboxMode::Bwrap` (deferred design — not this spec)

The legitimate underlying user need is "real network isolation without spinning up a Docker daemon." This need is best served by routing host-mode-style skills through the existing bwrap driver — **not** by hand-rolling unshare in the markdown_skill executor.

A follow-up spec could design:

- A new `SandboxMode::Bwrap` enum variant in `src/tools/markdown_skill/spec.rs`.
- A new executor branch `execute_in_bwrap()` that constructs a `SandboxCommand` and routes through `crate::sandbox::Sandbox::execute()` with the existing Linux bwrap driver (already provides netns).
- Linux-only: on macOS/Windows, loading a SKILL.md with `sandbox: bwrap` fails fast at load time with "this skill requires Linux".
- Documentation that recommends Bwrap mode for trust-the-skill-but-cap-its-network workflows.

This follow-up is **NOT in scope** for the current spec. It should be triggered only by:
- A specific user request for "isolation without Docker on Linux", OR
- A clawhub-published skill that declares `requires_isolation_without_docker: true` (or similar), OR
- The first time a skill author files a bug saying "Docker is too heavy for my use case".

Until then, the C-Plus follow-up is a **dormant idea**, not a planned spec.

## 7. Alternatives considered

| Option | Description | Decision | Reason |
|--------|-------------|----------|--------|
| **C1** (chosen) | Decision + warn improvement + doc + test. No new code path. | ✅ | Resolves the deferral honestly. Stops re-litigating the question every cycle. |
| **C2** | Implement `unshare(CLONE_NEWNET)` in host mode. | ❌ | Cross-platform broken; requires user namespaces; duplicates bwrap; corrupts the meaning of "host mode". See Decision 1. |
| **C3** | Add `SandboxMode::Bwrap` now (the §6 follow-up, but done in this spec). | ❌ for now, ✅ as dormant idea | Real user demand not yet established. Premature implementation per YAGNI / R10 dissolution philosophy. |
| **C4** | Silently delete the `NO_PROXY` + warn block; treat `network: none` as advisory-only on host. | ❌ | Loses the partial mitigation. Loses the warn. Loses the only honest signal users currently get. |
| **C5** | Add `unshare` behind a `#[cfg(target_os = "linux")]` block and silently no-op on other platforms. | ❌ | Most dishonest of all options — same nominal behavior, different runtime semantics per OS. Exactly what §2.1 forbids. |

## 8. Risks

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Future contributor sees the warn-only code and assumes it is a TODO to implement netns | Medium-high (this is how the deferral note originally read the code) | Step 3.4 adds an inline contract comment referencing this spec by date+path so the rationale is co-located with the code |
| The deferred §6 follow-up never lands, leaving users without a non-Docker isolation path on Linux | Medium | Acceptable. Docker is universally available; bwrap is a convenience optimization, not a missing capability |
| Updating the warn message (§2.3) breaks downstream tests that grep on the exact warn string | Low | Run `grep -rn "use sandbox: docker for enforced isolation" tests/ src/` before changing; update any matches |
| `tracing-test` crate is not in the project's dev-dependencies, blocking the §3.3 test | Low | Fall back to asserting on the `Command::get_envs()` for `NO_PROXY` — sufficient evidence the host-mode network-none path executed |

## 9. Open questions

None. The decision is binding.

## 10. Acceptance criteria

- [ ] `src/tools/markdown_skill/executor.rs:63` warn message updated per §2.3
- [ ] Inline contract comment added above `execute_on_host` per §3.4
- [ ] Skill-authoring doc paragraph added per §3.2
- [ ] Unit test added per §3.3 (or `Command` env-var inspection fallback)
- [ ] `cargo check -p alephcore` clean
- [ ] `cargo test -p alephcore --lib markdown_skill::executor` passes
- [ ] One commit, English message per `feedback_changelog_english`
- [ ] Spec C archived under `docs/superpowers/specs/` (already done by writing this file)
