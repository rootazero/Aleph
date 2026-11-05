# SessionKey Identity Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Status:** Design proposal for Phase 5 Task 3. Do not implement until this design is approved; Phase 5 Task 4 is the implementation handoff.

**Goal:** Make `crate::routing::session_key::SessionKey` the canonical application-session identity used by capability ownership and visibility, while naming the ACP manager's distinct process-pool key according to its actual domain.

**Architecture:** The routing key remains the authority for gateway sessions and `SessionService::SessionId`; `OwnerRef::Session` and `VisibilityScope.session` will use that same type. The ACP manager key remains a separate `(harness_id, normalized cwd, name)` lookup key, renamed to `AcpSessionPoolKey`; there is no conversion between these domains. Capability registration must receive the routing key explicitly rather than infer it from ACP pool coordinates.

**Tech Stack:** Rust workspace (`alephcore`), Tokio, serde, existing `routing::SessionKey` key-string API, capability ownership/fingerprint code.

**Spec:** This document is the Phase 5 Task 3 design and implementation plan; Task 4 implements it after approval.

## Global Constraints

- Do not change the identity, parsing, or persistence semantics of `routing::session_key::SessionKey`.
- Keep ACP process-pool identity as `(harness_id, canonicalized cwd, name)`; do not treat it as a gateway conversation/session identity.
- Do not add `From`/`TryFrom` conversions between the two domains.
- Capability session ownership and visibility use the routing key; registration callers must pass it explicitly.
- Keep descriptor fingerprinting deterministic and preserve every identity-bearing routing-key dimension.
- Before each `cargo` command, run `python qa/browser_live/check_memory.py`, per `docs/reference/DEVELOPMENT.md`.

## Review Focus

1. **Wrong application session:** a capability scoped to one route key must not become visible to a different agent, channel/peer, main key, or epoch. Pin with `visibility_session_scope_distinguishes_route_keys`.
2. **ACP pool key accidentally used as owner:** the manager lookup key must not type-check as `OwnerRef::Session` or `VisibilityScope.session`. Pin with the ownership field type and a compile/check gate; do not add a conversion shim.
3. **Fingerprint identity collapse:** different routing keys must not produce the same session-owner contribution to a descriptor fingerprint. Pin with `descriptor_fingerprint_changes_when_session_owner_route_key_changes` across representative key variants.
4. **Lost routing identity at registration boundary:** a registration path must not fabricate a routing key from only harness/cwd/name. Pin with an API/type review of all construction sites; capability registration takes the routing key as an explicit input.
5. **Accidental ACP pool behavior change:** equivalent normalized working directories should still share an ACP pool entry, while distinct names or harnesses remain isolated. Pin with `acp_session_pool_key_preserves_normalization_and_name_isolation`.

---

## 当前状态 / Current State

### 两种类型 / The two types

| Type | Shape and identity | Current consumers |
|---|---|---|
| `crate::routing::session_key::SessionKey` | A serializable enum for application/gateway sessions: `Main`, `DirectMessage`, `Group`, `Task`, `Subagent`, and `Ephemeral`. It carries routing identity such as `agent_id`, channel/peer, scope, epoch, parent key, or ephemeral id. | Routing and gateway code; `SessionService::SessionId` is a type alias for this exact type (`src/session/service.rs:12`). It is also the authority for session event storage and other route-key consumers. |
| `crate::acp::manager::SessionKey` | A struct `{ harness_id: String, cwd: PathBuf, name: String }`. `cwd` is made absolute and normalized; empty `name` is the default pool name (`src/acp/manager/session_key.rs:14-34,66-89`). | ACP manager's process/session pool map and lifecycle operations (`src/acp/manager/lifecycle.rs:53-59,141-148,253-264`). It is also currently imported by capability ownership (`src/capability/ownership.rs:11,65,87`) and channel-registration fingerprinting (`src/gateway/channel_registry/binding.rs:4,240-242`). |

### 使用与转换 / Usage and conversions

- `SessionService` already uses the routing key through `SessionId`; there is no mismatch at that boundary (`src/session/service.rs:12`, `docs/reference/SESSION_SERVICE.md`, “Public surface”).
- The ACP key is built from manager inputs `(harness_id, cwd, session_name)` and addresses an ACP pool entry. Lifecycle code uses it to acquire, look up, replace, or evict a pooled entry. It is not the ACP protocol's returned session id, nor a gateway conversation address.
- Capability ownership currently imports the ACP struct for both `OwnerRef::Session` and `VisibilityScope.session` (`src/capability/ownership.rs:11,61-88`). The channel binding code's `session_value` helper turns that struct into `{harness_id, cwd bytes, name}` JSON for the descriptor fingerprint's owner and visibility identity (`src/gateway/channel_registry/binding.rs:195-200,224-230,240-242`). That fingerprint helper is the only production-code use of the ACP key in this file identified by the search; it is not an ACP session-pool lookup.
- **Why it was chosen is not documented by the source.** The code establishes that the ACP key was used as the session-shaped type available to capability ownership and that channel binding serializes its tuple to preserve fingerprint distinctions. It does not establish that capability ownership intentionally follows the ACP process-pool lifetime. Do not invent that historical/design rationale.
- `ChannelLifecycleContract` carries `owner`, `lifetime`, and `visibility` as separate fields (`src/gateway/channel_registry/binding.rs:43-60`). In `ownership.rs`, `LifetimeScope` is explicitly the RAII/lifetime axis while `VisibilityScope` is the ACL axis (comments at `src/capability/ownership.rs:167-172`); the `OwnerRef::Session` payload names the owner identity, not how long the binding lives. Thus an ACP-shaped value in `OwnerRef::Session` does not by itself prove a process-pool lifetime contract.
- The channel lifecycle declaration is explicitly cold/source-private and is not mounted for production admission (comments at `src/gateway/channel_registry/binding.rs:1,54`). The inspected channel capability test helper constructs `OwnerRef::Runtime` and `LifetimeScope::Runtime` (`src/gateway/channel_registry/capability.rs:226-232`); tests do exercise session visibility/fingerprinting elsewhere, but no production session-bound channel-registration producer was found. Therefore the current code does not demonstrate an actual lifecycle association between a channel capability and an ACP manager pool entry.
- A repository search found no existing `From`/`TryFrom` conversion between the types. Current tests also use ACP pool keys in capability scope tests (`src/capability/ownership.rs:1273-1299`) and fingerprint tests (`src/gateway/channel_registry/binding.rs:1358+`).

### 明确假设与决策门 / Explicit assumption and decision gate

This proposal interprets “session” in capability `OwnerRef::Session` and `VisibilityScope.session` as the application/gateway session represented by `SessionService::SessionId`. That is supported by the project's canonical session authority, but it is a design decision for the capability boundary—not a proven explanation for why the ACP key was originally selected. If the intended meaning is instead an ACP pool-entry owner/lifetime, do **not** replace that identity with a routing key: model the ACP pool domain explicitly (for example, a separately named owner variant and matching visibility dimension) and decide which lifecycle event revokes it. `LifetimeScope::Session` alone does not answer this question; it describes binding duration independently from the identity stored in `OwnerRef`.

## 问题分析 / Why the Types Differ

These types answer different questions:

- **Routing key:** “Which application session/conversation is this?” It may identify a direct message, group, main conversation, task, nested subagent, or ephemeral session. The type is serializable and has a canonical key-string representation used by routing/storage.
- **ACP pool key:** “Which reusable ACP manager entry should this call acquire?” It identifies a harness/backend in a normalized working directory and optional parallel-session name. The manager uses it as a hash-map key to pool ACP entries.

Neither type contains enough information to losslessly derive the other:

- Routing → ACP is underdetermined: a routing key does not carry ACP `harness_id`, working directory, or ACP pool `name`.
- ACP → routing is underdetermined: an ACP key does not carry the gateway `agent_id`, channel/peer, DM policy, main key/epoch, task identity, parent route, or ephemeral id. One ACP pool coordinate tuple can serve calls that originate from distinct application sessions.
- `TryFrom` does not solve this: it could reject missing context or cover a hand-picked subset, but it cannot provide a total, bidirectional identity mapping. A caller-supplied association would be a relationship between identities, not a conversion, and its owner/lifetime would need a separate contract.

## 推荐方案 / Recommendation — Option A, With Domain Separation

**Select Option A for the canonical application-session authority:** `routing::session_key::SessionKey` is the single canonical type for application session identity, including capability owner/visibility session identity.

**Keep the ACP key, but stop calling it a competing generic `SessionKey`:** rename it to `AcpSessionPoolKey` (or an equally explicit name, applied consistently) and preserve its existing pool-key behavior. Do not convert it to or from the routing key. This is a semantic boundary, not an attempt to collapse two unrelated identities into one struct.

At the capability boundary:

1. `OwnerRef::Session` and `VisibilityScope.session` carry the routing key.
2. A capability registration path receives the relevant routing key from its application-session caller; it must not synthesize one from `(harness_id, cwd, name)`.
3. Descriptor fingerprinting encodes the routing key's complete canonical identity. Prefer the existing `to_key_string()` representation, and pin its uniqueness/round-trip assumptions in tests before relying on it. Replace the old ACP-specific `session_value` helper with a routing-key encoder; do not retain an ACP-shaped JSON payload under a routing-session field.
4. ACP manager APIs continue to use `AcpSessionPoolKey` for pool lookup and lifecycle. If a future feature truly needs both, carry both as distinct fields and define the relation at that call site; no registry or global mapping is introduced in this task.
5. **Channel binding migration:** `src/gateway/channel_registry/binding.rs` must stop importing the ACP pool key for descriptor identity. Its owner/visibility fingerprint arms must consume the routing `SessionKey`; the current `session_value` helper is renamed/replaced accordingly. Since the channel contract is source-private/cold today, Task 4 should update the type/fingerprint seam and tests but must not claim a live channel registration path is now wired.

**Trade-off:** This gives capabilities one stable application-session identity and removes the current capability-to-ACP-manager type coupling. It makes registration harder for callers that currently know only the ACP pool coordinates: those callers must propagate the routing key from the owner of the application session. That is intentional; silently guessing would risk ownership or visibility being attached to the wrong session. Future ACP-to-application attachment semantics remain explicit work rather than being hidden inside a lossy conversion.

## 备选方案 / Alternative Considered — Option B

Keep both generic `SessionKey` types and add tested conversions in each direction. **Rejected:** neither direction can preserve identity without extra data, and the missing dimensions are not defaults that can safely be invented. A partial conversion would create a misleading API that appears to establish identity while either rejecting valid cases or collapsing distinct sessions. Documentation alone cannot make such a conversion safe.

## Implementation Plan (Phase 5 Task 4)

### Task 1: Align capability session identity and name the ACP pool key

**Files:**

- Modify `src/acp/manager/session_key.rs`: rename the struct and its impl to `AcpSessionPoolKey`; preserve the exact fields, canonicalization, equality, and hash behavior.
- Modify `src/acp/manager/mod.rs`, `src/acp/manager/lifecycle.rs`, `src/acp/manager/harness_admin.rs`, and `src/acp/manager/tests.rs`: update the export and manager references to the explicit pool-key name. Keep ACP persistence event fields/schema unchanged.
- Modify `src/capability/ownership.rs`: use `crate::routing::session_key::SessionKey` for `OwnerRef::Session` and `VisibilityScope.session`; update ownership tests to construct routing keys.
- Modify `src/gateway/channel_registry/binding.rs`: update session-owner fingerprint encoding to use the routing key's canonical representation; update its tests. This file currently imports the ACP key directly.
- Search all Rust sources for `acp::manager::SessionKey` and ambiguous `SessionKey` imports after the rename. No compatibility alias is proposed unless an external API compatibility requirement is established before implementation.

**Interfaces:**

- Consumes: existing `crate::routing::session_key::SessionKey`, including its `to_key_string()` and parser; existing ACP path-normalization behavior.
- Produces: `OwnerRef::Session(crate::routing::session_key::SessionKey)`, `VisibilityScope.session: Option<crate::routing::session_key::SessionKey>`, and `AcpSessionPoolKey` for ACP manager pooling. No cross-domain conversion API.

- [ ] **Step 1: Add failing ownership and fingerprint tests.**
  - Add `visibility_session_scope_distinguishes_route_keys` to pin exact-match behavior and reject at least a changed agent plus a changed epoch or route variant.
  - Add `descriptor_fingerprint_changes_when_session_owner_route_key_changes` to prove the fingerprint changes when the owner routing identity changes, and remains stable for the same key.
  - Add `acp_session_pool_key_preserves_normalization_and_name_isolation` (or adapt an existing test with equivalent assertions) to pin unchanged pool identity semantics.
- [ ] **Step 2: Run the focused tests and record expected RED.** Run each focused test under `cargo test -p alephcore --lib <filter>` after `python qa/browser_live/check_memory.py`; expected failure is a type/field mismatch or the missing test, not an unrelated test failure.
- [ ] **Step 3: Rename the ACP pool key and switch capability ownership/visibility to the routing key.** Update channel registry fingerprint encoding to the canonical route-key string; preserve the ACP manager key's existing `(harness_id, normalized cwd, name)` behavior and ACP persistence event schema.
- [ ] **Step 4: Run focused tests and compile all test targets.** Run `cargo test -p alephcore --lib capability::`, `cargo test -p alephcore --lib acp::manager::`, `cargo test -p alephcore --lib gateway::channel_registry::binding::tests::`, and `cargo check -p alephcore --tests`, invoking `python qa/browser_live/check_memory.py` before each cargo command. Expected result: all focused tests pass and no code can pass an ACP pool key where a routing session key is required.
- [ ] **Step 5: Run final repository checks for this change.** Run `cargo clippy -p alephcore --tests -- -D warnings`, `cargo fmt --all --check`, and `git diff --check`, each preceded by the required memory check before cargo. Inspect the diff for accidental routing-key wire changes and ACP event/schema changes.

## Testing Strategy / 测试策略

- **Unit:** routing-key-based ownership and visibility comparisons; identity-distinguishing fingerprint tests; unchanged ACP pool-key path normalization and named-session isolation.
- **Integration/compile boundary:** `cargo check -p alephcore --tests` catches any remaining consumers that depend on the old ACP key type and confirms the capability boundary accepts only the routing identity.
- **Serialization contract:** assert representative routing variants produce stable fingerprint inputs and that `SessionKey::parse(key.to_key_string())` round-trips before using the string for fingerprint identity. Include `Main` with a nonzero epoch, `DirectMessage`, `Group`, `Task`, nested `Subagent`, and `Ephemeral`, or use an exhaustive test helper tied to the enum so a new variant cannot be silently omitted.
- **No live-registration E2E is claimed:** the channel lifecycle declaration in `src/gateway/channel_registry/binding.rs` is source-private/cold and not mounted for production admission. This task verifies type and fingerprint boundaries only; a production registration flow requires its own real producer and end-to-end proof.

## 风险评估 / Risk Assessment

| Risk | Mitigation / decision gate |
|---|---|
| `OwnerRef::Session` was intentionally designed to mean ACP pool-entry lifetime, not application session identity. | The source does not prove that intent. Confirm the explicit assumption above before Task 4. If false, keep ACP pool ownership as a separate named domain; do not convert it to a routing key. `LifetimeScope::Session` does not establish which identity domain is intended. |
| Channel binding's ACP key was selected for a hidden lifecycle association. | Current code only uses it in fingerprint serialization; the declaration is cold/source-private, and no production session-bound channel producer was found. Task 4 must migrate the fingerprint seam but must not claim lifecycle wiring. |
| Callers have ACP coordinates but not the routing key at registration time. | Make the missing routing key a visible API input requirement. Do not default, synthesize, or look it up through an unowned global map. |
| Changing fingerprint owner representation causes revision/fingerprint changes. | Update the canonical encoder and focused tests in the same task; check any persisted/externally observed fingerprint contract before implementation. Current search establishes the fingerprint callsite, not whether downstream consumers persist these hashes. |
| Renaming the ACP key can break downstream Rust callers of the re-export. | Repository-wide references were searched; no out-of-repository consumers or semver policy were established. Check the crate's compatibility policy before deleting the old exported name; if compatibility requires an alias, make it deprecated and never use it internally. |
| Canonical route-key string omits a field or collides for a future variant. | Pin route-key round trips and fingerprint uniqueness for representative variants; make the test exhaustive so a newly added variant requires an explicit case. |
| The source-private channel-binding path is not a production registration test. | State the limit in the release note and defer live registration QA until the producer is mounted; do not claim production registration safety from unit tests alone. |

## Task 3 Scope / Task 3 范围

Task 3 produces this design only. No Rust source, tests, ACP wire schema, or runtime behavior are changed here. Task 4 implements the approved recommendation; no test or build validation is claimed for this document-only task.
