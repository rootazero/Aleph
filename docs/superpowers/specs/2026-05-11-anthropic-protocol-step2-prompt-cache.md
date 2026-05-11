# Anthropic Protocol Step 2 — Prompt Cache (Dual TTL) Design

**Status**: Design approved, awaiting plan + implementation
**Date**: 2026-05-11
**Predecessor**: [Step 1 — Stability Hardening](./2026-05-11-anthropic-protocol-step1-stability.md) (shipped: `c001f1d7c`, `e62032df9`)
**Reference**: openclaw `src/agents/anthropic-payload-policy.ts` (`cacheRetention: "short" | "long" | "none"` policy)

---

## Goal

Wire Aleph's existing-but-unused `CacheControl` infrastructure to Anthropic's prompt
cache feature, with **two TTL tiers** (5min default, 1h opt-in) and **hostname-gated
defaults** that keep custom Anthropic-compatible backends safe.

Aleph today has `CacheControl::Ephemeral` as a type and 38 `cache_control: None`
construction sites — the type system is plumbed end-to-end but nothing ever sets a
non-None value. This spec wires the missing line: pick `Short`/`Long`/`Off` per
provider config, inject `cache_control` at two cache breakpoints inside
`AnthropicProtocol::build_request`, and (for `Long`) add the
`extended-cache-ttl-2025-04-11` beta header alongside any existing OAuth beta.

---

## Architecture

Three layers, surgical patch into each:

```
┌─────────────────────────────────────────────────────────────────┐
│ Config:    ProviderConfig.cache_retention: Option<CacheRetention>│
│            { Off | Short | Long }                                │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ (effective_cache_retention)
┌─────────────────────────────────────────────────────────────────┐
│ Adapter:   AnthropicProtocol::build_request                      │
│   ─ hostname gate config.cache_retention vs base_url             │
│   ─ inject cache_control at 2 breakpoints (system + last user)   │
│   ─ if Long: append `extended-cache-ttl-2025-04-11` to           │
│     `anthropic-beta` header (alongside existing OAuth beta)      │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ (serde)
┌─────────────────────────────────────────────────────────────────┐
│ Type:      CacheControl::Ephemeral { ttl: Option<EphemeralTtl> }│
│            EphemeralTtl::OneHour                                 │
│            serde:                                                │
│              None     → {"type":"ephemeral"}                     │
│              OneHour  → {"type":"ephemeral","ttl":"1h"}          │
└─────────────────────────────────────────────────────────────────┘
```

R7 + R10 compliance: zero reasoning logic. Hostname check is a single `host_str()
== "api.anthropic.com"` comparison; breakpoint placement is a fixed two-position
strategy (no scoring, no LLM call, no policy DSL).

---

## § 1 Type Layer Changes

**File**: `src/providers/message.rs`

Rewrite `CacheControl` from unit variant to struct variant:

```rust
/// Cache control hint for API providers that support prompt caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CacheControl {
    Ephemeral {
        /// `None` ≡ Anthropic default 5min TTL.
        /// `Some(OneHour)` ≡ 1h TTL, requires
        /// `extended-cache-ttl-2025-04-11` beta header.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl: Option<EphemeralTtl>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EphemeralTtl {
    #[serde(rename = "1h")]
    OneHour,
}
```

Wire output:
- `Ephemeral { ttl: None }` → `{"type":"ephemeral"}`
- `Ephemeral { ttl: Some(OneHour) }` → `{"type":"ephemeral","ttl":"1h"}`

`ContentBlock::Text.cache_control: Option<CacheControl>` field **unchanged**.
`ToolResult` is a top-level `UnifiedMessage` variant whose internal
`content: Vec<ContentBlock>` reuses `Text.cache_control` — no new fields needed
elsewhere.

**Breaking change scope** — unit → struct variant means any
`match c { CacheControl::Ephemeral => ... }` pattern breaks. Aleph is a single-crate
workspace; grep across `src/` shows no destructuring pattern (only construction
sites with `cache_control: None` or `Some(CacheControl::Ephemeral)`). One existing
serde test (`cache_control_serializes_correctly` in `message.rs`) needs the new
constructor form: `CacheControl::Ephemeral { ttl: None }`.

---

## § 2 Configuration Layer

**File**: `src/config/types/provider.rs`

Add enum + ProviderConfig field:

```rust
/// Prompt cache retention policy for streaming protocols that support it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    /// Never inject cache_control breakpoints.
    Off,
    /// 5-minute ephemeral cache (Anthropic default TTL).
    #[default]
    Short,
    /// 1-hour ephemeral cache. Anthropic-only.
    /// Triggers `anthropic-beta: extended-cache-ttl-2025-04-11` header.
    Long,
}

// ProviderConfig new field:
/// Prompt cache retention policy. Currently honored only by the Anthropic
/// protocol adapter; other protocols ignore this field.
///
/// `None` (unset) means "use hostname-gated default":
///   - host == api.anthropic.com → Short
///   - host == anything else     → Off
///
/// Setting an explicit value (Short/Long/Off) is always respected.
#[serde(default)]
pub cache_retention: Option<CacheRetention>,
```

### `effective_cache_retention` decision table

Resolved at `build_request` time inside the Anthropic adapter:

| `config.cache_retention` | host = `api.anthropic.com` | host = anything else |
|---|---|---|
| `None` (unset) | **Short** | **Off** |
| `Some(Off)` | Off | Off |
| `Some(Short)` | Short | Short |
| `Some(Long)` | Long | **Long + warn log** |

Implementation:

```rust
fn effective_cache_retention(config: &ProviderConfig, base_url: &str) -> CacheRetention {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    let is_official = host.as_deref() == Some("api.anthropic.com");

    match config.cache_retention {
        Some(explicit) => {
            if matches!(explicit, CacheRetention::Long) && !is_official {
                tracing::warn!(
                    base_url = %base_url,
                    "cache_retention = long on non-official Anthropic host; trusting explicit opt-in",
                );
            }
            explicit
        }
        None if is_official => CacheRetention::Short,
        None => CacheRetention::Off,
    }
}
```

Rationale (per Q2 decision C + Q3 decision A):

- Official host **auto-opts-in** to Short (95% of users get default cache benefit).
- Third-party hosts **stay Off** unless explicit (safety: kimi-for-coding, T8Star,
  and other custom backends may or may not support `cache_control`; conservative
  default prevents breakage).
- Explicit `Long` on third-party host is **trusted with warn** (some compatible
  backends do support 1h; force-downgrading would be paternalistic).

---

## § 3 Placement Strategy (Two Breakpoints)

**File**: `src/providers/protocols/anthropic/adapter.rs`

Inside `build_request`, after the existing payload serialization:

```
1. retention = effective_cache_retention(config, base_url)
2. if retention == Off: return as-is
3. cc = CacheControl::Ephemeral {
       ttl: if retention == Long { Some(OneHour) } else { None }
   }
4. inject_cache_control_into_system_array(&mut payload.system, cc)
5. inject_cache_control_into_last_user_message(&mut payload.messages, cc)
6. if retention == Long: append `extended-cache-ttl-2025-04-11` to anthropic-beta
```

### Breakpoint ① — system array, last text block

```
- If system is absent or empty: skip (no breakpoint)
- If system is a string: normalize to [{"type":"text","text":<s>,"cache_control":cc}]
- If system is an array:
    - Find the last element with type=="text"
    - Set its cache_control to cc (overwrite any pre-existing value — adapter wins)
    - If no text element exists: skip (don't inject into non-text blocks)
```

### Breakpoint ② — last user message, last non-thinking block

```
- Find the last message with role=="user". If none: skip.
- If its content is a string: normalize to [{"type":"text","text":<s>,"cache_control":cc}]
- If its content is an array:
    - Find the last block whose type is NOT in {"thinking","redacted_thinking"}
    - Set its cache_control to cc (overwrite any pre-existing value)
    - If all blocks are thinking-type: skip
```

Two-breakpoint placement matches Anthropic's recommended pattern: the system
breakpoint caches the stable prefix (system prompt + tool definitions, since tools
appear before messages in the wire format), the last-user breakpoint anchors the
cache-write boundary so no in-flight content gets cached prematurely. Anthropic
allows up to 4 breakpoints; we use 2 and leave headroom.

### Breakpoint placement order (within wire payload)

Anthropic processes breakpoints in wire order: `system` → `tools` → `messages[]`.
Our two breakpoints land in `system[last]` and `messages[last user, last block]`,
which sandwich the entire `tools` array and any prior history — exactly the
prefix-cache-then-write-boundary semantics we want.

---

## § 4 Beta Header (Long TTL)

**File**: `src/providers/protocols/anthropic/adapter.rs` (and possibly
`proto_impl.rs` if header construction lives there)

Anthropic's 1h cache TTL requires:

```
anthropic-beta: extended-cache-ttl-2025-04-11
```

Aleph already adds beta headers for OAuth (`oauth-2025-04-20`). The pattern needs
to support **multiple beta tokens** joined by comma:

```
anthropic-beta: oauth-2025-04-20,extended-cache-ttl-2025-04-11
```

Implementation: replace any "single beta header" code path with one that
accumulates a `Vec<&'static str>` of beta tokens and joins with `,` at request
build time. If OAuth + Long both apply, both tokens appear.

Edge case: if `retention != Long` and OAuth is off → no `anthropic-beta` header
at all (don't emit empty string).

---

## § 5 Boundary Conditions

| Scenario | Behavior |
|---|---|
| `system` absent (None) | Skip breakpoint ①, still try ② |
| `system` is empty string `""` | Skip breakpoint ① |
| `system` is array of length 0 | Skip breakpoint ① |
| `system` is array with no text elements | Skip breakpoint ① |
| `messages` empty array | Skip breakpoint ② (extremely rare; tool-only round) |
| No `role == "user"` in messages | Skip breakpoint ② |
| Last user `content` is empty array | Skip breakpoint ② |
| Last user last block is `thinking` | Walk back to last non-thinking block; if none, skip |
| Pre-existing `cache_control` on target block | Adapter overwrites (last-write-wins) |
| `cache_retention = Off` | No breakpoints, no beta header |
| `cache_retention = Long`, OAuth also on | Beta header = `oauth-2025-04-20,extended-cache-ttl-2025-04-11` |
| `cache_retention = Long`, OAuth off | Beta header = `extended-cache-ttl-2025-04-11` |
| Invalid `base_url` (parse fails) | Treated as non-official → unset defaults to Off |

---

## § 6 Test Matrix

**12 new unit tests + 1 updated existing test**, split between type-layer and adapter-layer:

**Type-layer (in `src/providers/message.rs` mod tests)** — 2 new + 1 updated:
1. `cache_control_serializes_short_ephemeral` — `Ephemeral { ttl: None }` → `{"type":"ephemeral"}` (new)
2. `cache_control_serializes_long_ephemeral` — `Ephemeral { ttl: Some(OneHour) }` → `{"type":"ephemeral","ttl":"1h"}` (new)
3. Existing `cache_control_serializes_correctly` updated to use new constructor `Ephemeral { ttl: None }` (semantics unchanged, wire output unchanged)

**Config-layer decisions (in `src/providers/protocols/anthropic/adapter.rs` mod tests)** — 4 new:
4. `effective_retention_official_unset_defaults_short`
5. `effective_retention_third_party_unset_defaults_off`
6. `effective_retention_explicit_long_on_third_party_respected_with_warn`
7. `effective_retention_explicit_off_always_off`

**Adapter behavior (in `src/providers/protocols/anthropic/adapter.rs` mod tests)** — 6 new:
8. `build_request_injects_cache_control_in_system_array_last_text_block`
9. `build_request_injects_cache_control_in_last_user_message_last_block`
10. `build_request_skips_thinking_block_for_last_user_cache_control`
11. `build_request_retention_off_no_cache_control_anywhere`
12. `build_request_long_ttl_adds_extended_cache_beta_header`
13. `build_request_long_with_oauth_emits_comma_joined_beta`

Baseline 484 pre-existing test compile errors unchanged. Verification: `cargo
check -p alephcore` 0 errors (same strategy as Step 1).

---

## § 7 File Manifest

**Modified** (10):
1. `src/providers/message.rs` — type rewrite + serde tests
2. `src/config/types/provider.rs` — `CacheRetention` enum + `ProviderConfig.cache_retention` field
3. `src/providers/protocols/anthropic/adapter.rs` — `effective_cache_retention`, `inject_cache_control_into_system_array`, `inject_cache_control_into_last_user_message`, `build_request` call sites, beta header join
4. `src/providers/protocols/anthropic/proto_impl.rs` — touched only if the existing OAuth beta-header construction code lives here rather than in `adapter.rs`; impl task #1 begins by `grep`-ing for `anthropic-beta` to locate the call site, then routes the multi-token join through that same module
5–9. 5 existing `ProviderConfig` literal construction sites (same set as Step 1):
   - `src/gateway/provider_factory.rs` (2 sites)
   - `src/gateway/handlers/oauth.rs`
   - `src/gateway/handlers/providers/handlers.rs`
   - `src/gateway/handlers/providers/helpers.rs`
   - `src/providers/auth_profile_registry.rs`
   Each adds `cache_retention: None,`
10. `CHANGELOG.md` — Added + Changed entries (English)

**Created** (2):
- `docs/superpowers/specs/2026-05-11-anthropic-protocol-step2-prompt-cache.md` (this file)
- `docs/superpowers/plans/2026-05-11-anthropic-protocol-step2-prompt-cache.md` (next step)

---

## § 8 Commit Split (2 atomic commits)

**Commit 1 — type + config (no wiring)**:
- `CacheControl::Ephemeral { ttl }` + `EphemeralTtl` enum + serde tests
- `CacheRetention` enum + `ProviderConfig.cache_retention` field
- 5 production literal sites add `cache_retention: None`
- Updates the existing `cache_control_serializes_correctly` test
- 2 new type-layer serde tests
- **Verification**: `cargo check -p alephcore` 0 errors. No behavior change in
  production (every literal site emits `cache_retention: None`, every adapter
  ignores the field).

**Commit 2 — adapter wiring**:
- `effective_cache_retention` (hostname gate + warn on Long-non-official)
- `inject_cache_control_into_system_array`
- `inject_cache_control_into_last_user_message`
- `build_request` calls both injectors at the end of payload assembly
- beta header accumulator (replaces single-beta path) + Long → append
  `extended-cache-ttl-2025-04-11`
- 10 adapter-layer mod tests (4 config decisions + 6 injection/header behaviors)
- CHANGELOG.md two entries
- **Verification**: `cargo check -p alephcore` 0 errors; `cargo clippy -p alephcore
  --lib --no-deps` no new lints on touched paths.

---

## § 9 Architectural Red-line Compliance

- **R3 (Core Minimalism)** ✅ — no new heavy deps. Single existing `url` crate
  call for hostname parse (already in tree).
- **R7 (LLM Sovereignty)** ✅ — zero reasoning. Two fixed breakpoint positions,
  one hostname equality check. No scoring, no policy DSL, no LLM call.
- **R8 (Everything is a Tool)** ✅ — `cache_retention` is configurable via
  `aleph.toml`, hence reachable by LLM via the existing config-edit tools (no
  new tool plumbing needed).
- **R9 (Intelligence in Prompt)** ✅ — no smart logic in adapter; cache TTL
  selection is a user-level config decision, not a runtime inference.
- **R10 (Thin Harness, Dumb Loop)** ✅ — adapter is mechanical: read config,
  parse hostname, set two fields. No turn-level state, no recovery branching.

---

## § 10 Risk Register

| Risk | Likelihood | Mitigation |
|---|---|---|
| 38 `cache_control: None` sites break on enum reshape | Low | Construction syntax `Some(CacheControl::Ephemeral)` becomes `Some(CacheControl::Ephemeral { ttl: None })` — straightforward find/replace if any explicit construction exists; current `None` sites unaffected |
| Existing `cache_control_serializes_correctly` test breaks | Certain | Spec includes test update; serde wire output unchanged for `ttl: None` path |
| Third-party host returns 400 on unexpected `cache_control` field | Low (mitigated by Q2 decision C) | Default Off for non-official hosts; explicit opt-in required |
| 1h beta header on non-supporting host | Low (Long + non-official requires explicit opt-in + warn) | User signals their backend supports it; warn log audit-trails the trust |
| Multiple beta tokens not parsed by some custom backends | Medium | All standard-compliant Anthropic SDKs accept comma-joined; non-compliant backend → user shouldn't be on Long anyway |
| Baseline 484 test compile errors block test-run validation | Certain | Same as Step 1: validate via `cargo check -p alephcore` only; manual integration test (Task 17 in plan) covers e2e |

---

## § 11 Out of Scope (Not Step 2)

- **Tools array cache breakpoint** (would be breakpoint ③) — Q3 decision A
  excluded. Tools are typically <1k tokens; ROI vs complexity poor.
- **System prompt cache boundary tokens** (openclaw splits stable prefix /
  dynamic suffix via in-text markers) — out of scope; future Step 3 candidate
  if data shows system prompt churn.
- **`service_tier` (auto/standard_only)** — orthogonal feature, separate spec.
- **OpenAI protocol prompt caching** — different mechanism (automatic
  server-side cache); no Aleph-side action needed.
- **`prompt_cache_hit_tokens` metric reporting** — out of scope; future
  observability spec.

---

## § 12 Verification Plan

Same strategy as Step 1 (baseline 484 errors block `cargo test`):
- `cargo check -p alephcore` after each commit: 0 errors
- `cargo clippy -p alephcore --lib --no-deps`: no new lints on touched files
- Manual integration test post-Commit 2:
  - Start `aleph-server` with `cache_retention = "short"` in `aleph.toml`
  - Send a multi-turn conversation through the Anthropic-backed provider
  - Confirm response includes `cache_creation_input_tokens` / `cache_read_input_tokens`
    in the usage block (response metadata visible via gateway logs)
  - Toggle `cache_retention = "long"` and confirm `anthropic-beta` request header
    contains `extended-cache-ttl-2025-04-11`

---

## § 13 Acceptance Criteria

- ✅ `CacheControl::Ephemeral { ttl: Option<EphemeralTtl> }` compiles and serializes correctly
- ✅ `CacheRetention { Off, Short, Long }` enum exists with `Short` as `Default`
- ✅ `ProviderConfig.cache_retention: Option<CacheRetention>` field added
- ✅ 5 production literal sites updated with `cache_retention: None`
- ✅ `effective_cache_retention` returns correct value for all 4 (config × host) combinations
- ✅ `build_request` injects `cache_control` at system[last text] and last-user[last non-thinking]
- ✅ Long TTL adds `extended-cache-ttl-2025-04-11` to `anthropic-beta` header
- ✅ Multiple beta tokens joined by `,`
- ✅ 12 new unit tests compile and assert correct behavior (+ 1 existing serde test updated)
- ✅ `cargo check -p alephcore`: 0 errors after each commit
- ✅ No new clippy lints on touched files
- ✅ CHANGELOG.md updated (English)
- ✅ Manual integration test confirms cache hit/write metrics in production response
