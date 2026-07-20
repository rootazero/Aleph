# SP-4 — Hostname-Based Network Filtering (DNS Pre-Resolution)

**Date**: 2026-05-20
**Status**: Design
**Branch**: `feat/sandbox-hardening-cycle1` (continued in same worktree)
**Predecessor**: `2026-05-20-sandbox-hardening-cycle1-design.md` § 9 (SP-4 entry)

## 1. Goal & scope

Make `NetworkPolicy::AllowHosts { hosts: ["github.com", "api.openai.com", "1.2.3.4"] }` work on macOS instead of hard-erroring as cycle 1 left it.

**In scope (this cycle)**:
- macOS: pre-resolve hostnames to IPs at command launch, feed into the existing Seatbelt `(remote ip ...)` pipeline.
- Workspace-level `dns` module that runs between the approval gate and `profile_for()`.
- Fail-closed semantics: DNS failure → `SandboxError::DnsResolutionFailed`.

**Out of scope** (deferred to other specs):
- Linux network filtering (SP-2 / iptables; bwrap has no per-IP filter).
- Windows network filtering (SP-3b WFP / SP-6 AppContainer).
- Outbound HTTP proxy (codex-style port-allowlist).
- DNS-over-HTTPS / custom resolvers.
- TTL refresh / wildcard hostname matching.

**Success criteria**:
1. `AllowHosts(["github.com"])` on macOS allows traffic to GitHub's IPs and denies everything else.
2. DNS resolution failure returns `SandboxError::DnsResolutionFailed { hostname, source }`.
3. Linux/Windows behavior unchanged — `AllowHosts` of any form still returns `SandboxError::UnsupportedPolicy`.
4. Zero new third-party crates.

## 2. Architecture & data flow

The 6-step `WorkspaceSandbox::execute` pipeline (cycle 1 §8) gains one new step between **step 4 (approval gate)** and **step 5 (profile_for)**:

```
SandboxCommand { network = AllowHosts(["github.com", "1.2.3.4"]) }
  │
  ▼
[step 3] workspace check  (unchanged)
[step 4] approval gate    (unchanged)
[step 4.5] dns::resolve_hosts_in_capabilities   ← NEW
  │   - host_is_ip_literal(h)?  → keep as-is
  │   - else lookup_host(h).await
  │       success → push every unique IP string
  │       failure → SandboxError::DnsResolutionFailed { hostname, source }
  │   - result: cmd.capabilities.network now contains IPs only
  ▼
[step 5] driver.profile_for(&cmd.capabilities, &cwd)
  │   - macOS seatbelt: existing (remote ip ...) path accepts IPs unchanged ✓
  │   - linux bwrap / windows driver: still returns UnsupportedPolicy (unchanged)
  ▼
[step 6] driver.run(...)
```

DNS resolution lives in `workspace.rs` (the orchestrator), **not** in any driver, so:

- The `OsSandboxDriverTrait::profile_for` signature stays sync.
- All three platforms benefit from the same DNS layer once they grow IP-filtering capabilities (SP-2 / SP-3b / SP-6).
- The `dns` module is reusable by any future caller that needs hostname→IP normalization in a `SandboxCapabilities` value.

## 3. File-level changes

| File | Change | Approx LOC |
|---|---|---|
| `src/sandbox/command.rs` | Add `SandboxError::DnsResolutionFailed { hostname: String, source: std::io::Error }` | +8 |
| `src/sandbox/dns.rs` *(new)* | `pub(crate) async fn resolve_hosts_in_capabilities(caps: &mut SandboxCapabilities) -> Result<(), SandboxError>`. Uses `tokio::net::lookup_host` wrapped in `tokio::time::timeout(Duration::from_secs(5), ...)`. Deduplicates results. IP literals skip DNS. | ~80 |
| `src/sandbox/mod.rs` | `mod dns;` (private) | +1 |
| `src/sandbox/workspace.rs` | Call `dns::resolve_hosts_in_capabilities(&mut cmd.capabilities).await?` between approval gate and `profile_for()` (around line 215). | +3 |

**Note**: `src/sandbox/platforms/macos/seatbelt.rs` is **not modified**. The cycle 1 `host_is_ip_literal` check stays as defense-in-depth — `OsSandboxDriverTrait::profile_for` is `pub`, so a future caller that bypasses the workspace pipeline still gets a typed `UnsupportedPolicy` instead of generating an invalid SBPL `(remote ip github.com)` clause. SP-4 is purely additive at the workspace layer.

No new third-party crates. `tokio::net::lookup_host` is already pulled in via the existing `tokio` dependency.

## 4. Error handling

| Scenario | Behavior |
|---|---|
| `lookup_host` returns NXDOMAIN / NotFound | `SandboxError::DnsResolutionFailed { hostname, source: io::Error }` |
| `lookup_host` times out (>5s) | `SandboxError::DnsResolutionFailed { hostname, source: io::Error::new(TimedOut, ...) }` |
| `lookup_host` returns 0 records (theoretical) | Same `DnsResolutionFailed` with `source = io::Error::new(NotFound, "empty result")` |
| Host already an IP literal (`"1.2.3.4"`, `"[::1]"`, `"::1"`, `"1.2.3.4:443"`) | Skip DNS, keep as-is |
| `NetworkPolicy::None` / `AllowAll` | No-op |
| `AllowHosts` with empty `hosts` Vec | No-op (resolved capabilities equal input) |

Rationale: fail-closed matches cycle 1's posture (P7 defensive design). Silently substituting an empty IP list would let the command run with all traffic denied — confusing and inconsistent with the user's intent.

## 5. Testing

### Unit tests (in `src/sandbox/dns.rs`)

1. `ip_literal_v4_skips_dns` — `["1.2.3.4"]` → unchanged, no DNS call.
2. `ip_literal_v6_bracketed_skips_dns` — `["[::1]"]` → unchanged.
3. `ip_literal_v6_bare_skips_dns` — `["2606:2800:220:1:248:1893:25c8:1946"]` → unchanged.
4. `ip_literal_v4_with_port_skips_dns` — `["1.2.3.4:443"]` → unchanged.
5. `hostname_localhost_resolves` — `["localhost"]` → contains "127.0.0.1" or "::1" (system resolver behavior, stable on CI).
6. `hostname_invalid_returns_dns_resolution_failed` — `["nonexistent.invalid"]` → `SandboxError::DnsResolutionFailed`.
7. `empty_hosts_is_noop` — `AllowHosts { hosts: vec![] }` → unchanged.
8. `network_none_is_noop` — `NetworkPolicy::None` → unchanged.
9. `network_allow_all_is_noop` — `NetworkPolicy::AllowAll` → unchanged.
10. `mixed_ip_and_hostname` — `["1.2.3.4", "localhost"]` → IP literal preserved + localhost expanded.

### Integration test (extend `tests/sandbox_capability_approval.rs`)

11. `dns_resolution_threads_resolved_ips_to_driver_profile`:
    - Build cmd with `AllowHosts { hosts: vec!["localhost".into()] }`.
    - Extend `RecordingDriver::profile_for` to capture `capabilities.network` clone.
    - Assert the captured network contains only IP literals (no "localhost").

### macOS profile generation

Cycle 1's `generate_profile_with_ip_allow_hosts_succeeds` already exercises the downstream `(remote ip ...)` rendering once IPs reach the driver. Cycle 1's `generate_profile_with_hostname_allow_hosts_returns_unsupported` **continues to pass and is kept** — it locks in the defense-in-depth guarantee that any caller bypassing the workspace DNS layer is still rejected, not allowed through with malformed SBPL.

## 6. Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| `tokio::net::lookup_host` uses libc `getaddrinfo` which can block the runtime thread on macOS | Low | tokio already spawns these on its blocking pool; a single 5s-bounded lookup per command is acceptable. |
| DNS resolution adds latency to every command with hostname allowlist | Low | System resolver caches; typical latency <1ms after warmup. Document in spec. |
| Hostname → multi-IP expansion explodes Seatbelt profile size | Low | Each unique IP is ~30 bytes of SBPL; even 100 IPs is <3KB. Hard cap not needed. |
| `nonexistent.invalid` test depends on the resolver returning NXDOMAIN, not a captive-portal redirect | Med | Use `nonexistent-host-for-aleph-sp4-test.invalid`; `.invalid` is reserved per RFC 2606. |
| Future contributor edits seatbelt's hostname check, forgetting it's now a defense-in-depth backstop | Low | Spec § 3 documents the invariant; cycle 1 test `generate_profile_with_hostname_allow_hosts_returns_unsupported` will regress immediately if removed. |

## 7. Alignment with redlines & principles

- **R3 (core minimalism)**: zero new dependencies. ✓
- **R7 (LLM sovereignty)**: no impact on LLM reasoning path. ✓
- **R10 (thin harness)**: `dns.rs` is a single-purpose ~80-line module; no new traits, no abstractions for hypothetical future resolvers. ✓
- **P5 (least knowledge)**: `dns` exposes one function; the caller doesn't see DNS internals. ✓
- **P7 (defensive design)**: fail-closed on DNS error, 5s timeout, empty-result guard. ✓

## 8. Implementation sequence

1. `command.rs` — add `DnsResolutionFailed` variant + `Display` arm.
2. `dns.rs` — implement `resolve_hosts_in_capabilities` + the 10 unit tests.
3. `mod.rs` — register module.
4. `workspace.rs` — wire the resolution call between approval and profile_for.
5. `tests/sandbox_capability_approval.rs` — extend `RecordingDriver` to capture network + add the integration test.
6. `cargo check -p alephcore` → `cargo test -p alephcore --lib sandbox` → `cargo test -p alephcore --test sandbox_capability_approval` → `cargo clippy -p alephcore --lib -- -D warnings` (only assess regressions on touched files vs main baseline).
7. Update `docs/reference/SANDBOX.md` Cycle 1 section with a new "Hostname support (macOS, SP-4)" subsection.
8. Commit with message `sandbox: SP-4 — DNS pre-resolution unlocks AllowHosts hostnames on macOS`.

## 9. Out-of-scope follow-up specs

Unchanged from cycle 1 §9 minus SP-4. Remaining: SP-2 (Linux landlock+seccomp), SP-3a (Windows RestrictedToken), SP-3b/SP-6 (Windows network filtering, pending decision), SP-5 (Linux cgroups v2).
