# Occam's Razor Classification & Routing: Batch ROUTING

**Target Subsystem**: `src/routing`  
**Review Run**: `2026-11-11-occams-providers-resilience-routing`  
**Inspector Tier**: Jev (Fast Static Analysis, Classification & Routing)  
**Files Audited**:
- `src/routing/config.rs`
- `src/routing/identity_links.rs`
- `src/routing/mod.rs`
- `src/routing/overlay.rs`
- `src/routing/resolve.rs`
- `src/routing/session_key.rs`

---

## 1. Executive Summary

| Category | FIX-NOW | FIX-CAREFULLY | DEFER | REJECT | Total |
| :--- | :---: | :---: | :---: | :---: | :---: |
| **REDUNDANT-ALLOCATION** | 4 | 4 | 1 | 0 | **9** |
| **ACCIDENTAL-COMPLEXITY** / **DEAD-CODE** | 3 | 0 | 0 | 0 | **3** |
| **CONTROL-FLOW** / **INDIRECTION** | 1 | 0 | 0 | 1 | **2** |
| **PANIC-RISK** / **DEFENSIVE-CHECK** | 0 | 0 | 0 | 1 | **1** |
| **CORRECTNESS** / **INVARIANT-TIGHTENING** | 0 | 0 | 0 | 0 | **0** |
| **Total** | **8** | **4** | **1** | **2** | **15** |

---

## 2. Detailed Candidate Inventory

### File 1: `src/routing/config.rs`

#### [ROUTING-01] Heap Allocation in ASCII Case Check (`report_peer_problems`)
- **Location**: `src/routing/config.rs:146`
- **Code**:
  ```rust
  if !["dm", "group"].contains(&kind.to_ascii_lowercase().as_str()) {
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Routing**: `FIX-NOW`
- **Rationale**: `kind.to_ascii_lowercase()` allocates a temporary `String` on the heap for every peer check merely to compare against two static string literals. Replacing with `!kind.eq_ignore_ascii_case("dm") && !kind.eq_ignore_ascii_case("group")` is completely zero-allocation and non-breaking.

#### [ROUTING-02] Pass-by-Reference on `Option` Parameters
- **Location**: `src/routing/config.rs:141, 160`
- **Code**:
  ```rust
  fn report_peer_problems(who: &str, peer: &Option<PeerMatchConfig>, out: &mut Vec<String>)
  fn report_account_problem(who: &str, account_id: &Option<String>, out: &mut Vec<String>)
  ```
- **Taxonomy**: `CONTROL-FLOW` / `INDIRECTION`
- **Routing**: `FIX-NOW`
- **Rationale**: Passing `&Option<T>` is an unnecessary indirection anti-pattern in Rust. Passing `Option<&PeerMatchConfig>` and `Option<&str>` simplifies caller call sites and avoids taking references to temporary Option wrappers. Private helpers only.

#### [ROUTING-03] Intermediate `Vec<String>` Allocation per Binding
- **Location**: `src/routing/config.rs:104-106, 110-130`
- **Code**:
  ```rust
  for (i, binding) in bindings.iter().enumerate() {
      out.extend(binding_problems_for(i, binding));
  }
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Routing**: `FIX-NOW`
- **Rationale**: `binding_problems_for` allocates a new `Vec<String>` for every binding, which is then extended into the caller's `out` vector. Passing `&mut Vec<String>` directly into `binding_problems_for` appends problems in-place without per-binding intermediate heap allocations.

---

### File 2: `src/routing/identity_links.rs`

#### [ROUTING-04] Full Table Collect and Sort in `validate_identity_links`
- **Location**: `src/routing/identity_links.rs:55-58`
- **Code**:
  ```rust
  let mut pairs: Vec<(String, Vec<String>)> = id_to_canonicals.into_iter().collect();
  pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
  for (id, mut canonicals) in pairs {
      if canonicals.len() > 1 { ... }
  }
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION` / `ACCIDENTAL-COMPLEXITY`
- **Routing**: `FIX-NOW`
- **Rationale**: Allocates a `Vec` for the entire link table and sorts all non-conflicting links just to find the first conflict (which almost never exists on valid configs). Filtering first via `.filter(|(_, c)| c.len() > 1).min_by(|(a, _), (b, _)| a.cmp(b))` preserves exact deterministic error reporting (returning the lexicographically smallest conflicting ID) while eliminating vector allocation and O(N log N) sorting for all valid configurations.

#### [ROUTING-05] Whole-Table HashMap Inversion and String Cloning in `resolve_linked_peer_id`
- **Location**: `src/routing/identity_links.rs:95-125`
- **Code**:
  ```rust
  let mut id_to_canonical: HashMap<String, Option<String>> = HashMap::new();
  for (canonical, ids) in identity_links { ... }
  match id_to_canonical.get(&id_lower).cloned() { ... }
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Routing**: `FIX-CAREFULLY`
- **Rationale**: Inverts the entire identity link table by constructing a new `HashMap<String, Option<String>>` from scratch on *every single incoming message resolution*, cloning strings on line 106, and re-lowering strings in `canonical_for`. A single direct pass searching for matches against `scoped` and `peer_lower` with ambiguity detection avoids allocating the HashMap, eliminates all intermediate String clones, and preserves fail-closed semantics for unvalidated duplicates and scoped-over-bare precedence.

---

### File 3: `src/routing/mod.rs`

*Clean module root; no dead imports or unnecessary re-exports.*

---

### File 4: `src/routing/overlay.rs`

#### [ROUTING-06] Repetitive String Allocation Across Match Arms in `overlay_route`
- **Location**: `src/routing/overlay.rs:98, 114, 118, 126`
- **Code**:
  ```rust
  agent_id: Some(resolved_agent.to_string()),
  ```
- **Taxonomy**: `CONTROL-FLOW` / `INDIRECTION`
- **Routing**: `REJECT`
- **Rationale**: The explicit branches directly mirror and document the precedence rules (specific binding vs channel override vs ghost binding). Unifying allocations would obscure the 1-to-1 mapping with the documented decision table. Intentional design.

---

### File 5: `src/routing/resolve.rs`

#### [ROUTING-07] Redundant Defensive Checking in `scope_satisfied`
- **Location**: `src/routing/resolve.rs:292-311`
- **Code**:
  ```rust
  if rule.guild_id.is_some()
      && !input.guild_id.as_deref().is_some_and(|g| matches_guild(rule, g)) { return false; }
  ```
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY` / `DEFENSIVE-CHECK`
- **Routing**: `FIX-NOW`
- **Rationale**: `rule.guild_id.is_some()` is checked, and then `matches_guild` re-checks and re-unwraps `rule.guild_id.as_deref()`. Matching directly on `rule.guild_id.as_deref().map(str::trim)` and comparing against `input.guild_id` eliminates redundant helper indirection and duplicate option unwraps.

#### [ROUTING-08] Intermediate `candidates.collect()` in `resolve_route`
- **Location**: `src/routing/resolve.rs:82-89`
- **Code**:
  ```rust
  let candidates: Vec<&RouteBinding> = bindings.iter().filter(...).collect();
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Routing**: `DEFER`
- **Rationale**: `candidates` is collected into a `Vec` and then traversed in 5 sequential passes for tier precedence (peer -> guild -> team -> account -> channel). While collecting on every message is an allocation, replacing it with an iterator pipeline or SmallVec touches core routing traversal and is best addressed in a dedicated architectural pass.

---

### File 6: `src/routing/session_key.rs`

#### [ROUTING-09] Gratuitous `parse_rest` Call & Enum Construction in `prefer_direct`
- **Location**: `src/routing/session_key.rs:628-634`
- **Code**:
  ```rust
  let prefer_direct = rest.len() >= 3
      && Self::parse_rest(&agent_id, rest, 0)
          .as_ref()
          .is_some_and(|d| !matches!(d, Self::Main { .. }));
  ```
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY` / `REDUNDANT-ALLOCATION`
- **Routing**: `FIX-NOW`
- **Rationale**: `parse_rest` allocates multiple `String`s and constructs a full `SessionKey` enum only to check if it's not `Main`. For `rest.len() >= 3`, `parse_rest` *never* returns `Main` (all length >= 3 patterns are `DirectMessage` or `Group`). Checking `rest.len() == 3 && matches!(rest.get(1), Some(&("dm" | "group" | "thread")))` is completely equivalent, eliminates multiple heap allocations, and avoids synthesizing throwaway enum instances.

#### [ROUTING-10] Second Heap Allocation in `append_epoch`
- **Location**: `src/routing/session_key.rs:322-328`
- **Code**:
  ```rust
  fn append_epoch(base: String, epoch: u32) -> String {
      if epoch > 0 { format!("{base}:s{epoch}") } else { base }
  }
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Routing**: `FIX-NOW`
- **Rationale**: `format!("{base}:s{epoch}")` reallocates a brand new `String` on the heap and copies `base`. Using in-place formatting via `use std::fmt::Write; let _ = write!(base, ":s{epoch}"); base` mutates `base` in-place, eliminating the second allocation.

#### [ROUTING-11] Pass-by-Reference for 1-Byte Copy Enum in `format_dm_base`
- **Location**: `src/routing/session_key.rs:308`
- **Code**:
  ```rust
  fn format_dm_base(agent_id: &str, channel: &str, peer_id: &str, dm_scope: &DmScope) -> String
  ```
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY`
- **Routing**: `FIX-NOW`
- **Rationale**: `DmScope` is `#[derive(Copy, Clone)]` and fits in 1 byte. Pass `dm_scope: DmScope` by value instead of taking a reference.

#### [ROUTING-12] Quadruple String Allocation in `normalize_agent_id`
- **Location**: `src/routing/session_key.rs:791-826`
- **Code**:
  ```rust
  let normalized: String = trimmed.to_lowercase().chars().map(...).collect();
  let result = normalized.trim_start_matches('-').trim_end_matches('-').to_string();
  ...
  result.chars().take(64).collect()
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Routing**: `FIX-CAREFULLY`
- **Rationale**: Makes 4 separate `String` allocations: (1) `trimmed.to_lowercase()`, (2) mapping to `normalized`, (3) trimming to `result`, (4) taking 64 chars. `trimmed.to_lowercase()` is redundant because all non-ASCII characters map to `'-'` anyway, and ASCII can be lowercased with `c.to_ascii_lowercase()`. Trimming and capping can be done in a single pre-allocated `String::with_capacity(trimmed.len().min(64))`, eliminating 3 out of 4 allocations on every call.

#### [ROUTING-13] Triple String Allocation in `sanitize_component`
- **Location**: `src/routing/session_key.rs:838-861`
- **Code**:
  ```rust
  let normalized: String = trimmed.to_lowercase().chars().map(...).collect();
  ...
  normalized.chars().take(64).collect()
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Routing**: `FIX-CAREFULLY`
- **Rationale**: Allocates 3 strings: (1) `trimmed.to_lowercase()`, (2) `collect()`, (3) `chars().take(64).collect()`. Can be flattened into a single pre-allocated `String` with `c.to_ascii_lowercase()`.

#### [ROUTING-14] Intermediate `String` in Constructors via `&val.into()`
- **Location**: `src/routing/session_key.rs:164-250`
- **Code**:
  ```rust
  agent_id: normalize_agent_id(&agent_id.into()),
  channel: sanitize_component(&channel.into()),
  peer_id: sanitize_component(&peer_id.into()),
  ```
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Routing**: `FIX-CAREFULLY`
- **Rationale**: `&val.into()` creates an owned `String` from `&str`, borrows it immediately as `&str`, passes it to `normalize_agent_id` / `sanitize_component` (which allocates another String), and drops the intermediate String. If the parameter is `impl AsRef<str>`, zero intermediate String allocations occur. Must ensure zero caller breakages across the repository.

#### [ROUTING-15] Panic on Reserved Task Marker in `SessionKey::task`
- **Location**: `src/routing/session_key.rs:256-284`
- **Code**:
  ```rust
  if matches!(task_type.as_str(), "peer" | "dm" | "subagent" | "ephemeral") {
      panic!("reserved task_type `{task_type}` would produce an ambiguous session key");
  }
  ```
- **Taxonomy**: `PANIC-RISK` / `DEFENSIVE-CHECK`
- **Routing**: `REJECT`
- **Rationale**: Intentional design invariant pinned by unit test `parse_never_yields_a_task_whose_type_is_a_reserved_marker` and documented with `#[expect(clippy::panic)]`. Changing to `Result` would break ~15 infallible call sites across the codebase for a structurally unreachable state.

---

## 3. Prioritized Action Plan

### Tier 1: FIX-NOW (Safe, Zero-Risk, High Clarity Gain)
1. **[ROUTING-01]** `src/routing/config.rs:146`: Replace `kind.to_ascii_lowercase().as_str()` with zero-allocation `eq_ignore_ascii_case`.
2. **[ROUTING-02]** `src/routing/config.rs:141, 160`: Change `&Option<T>` parameters in `report_peer_problems` and `report_account_problem` to `Option<&T>`.
3. **[ROUTING-03]** `src/routing/config.rs:104, 110`: Pass `&mut Vec<String>` into `binding_problems_for` to avoid per-binding vector allocations.
4. **[ROUTING-04]** `src/routing/identity_links.rs:55`: Replace full table collect and sort in `validate_identity_links` with `min_by` on duplicate entries only.
5. **[ROUTING-07]** `src/routing/resolve.rs:292`: Flatten double unwrapping in `scope_satisfied`.
6. **[ROUTING-09]** `src/routing/session_key.rs:628`: Replace dummy `parse_rest` call in `prefer_direct` with direct slice pattern match.
7. **[ROUTING-10]** `src/routing/session_key.rs:322`: Mutate `base` in-place using `write!` in `append_epoch`.
8. **[ROUTING-11]** `src/routing/session_key.rs:308`: Pass `DmScope` by value instead of `&DmScope` in `format_dm_base`.

### Tier 2: FIX-CAREFULLY (Nuanced, Performance-Critical, Invariant-Sensitive)
9. **[ROUTING-05]** `src/routing/identity_links.rs:95`: Replace whole-map inversion with direct 1-pass search for `scoped` and `peer_lower` in `resolve_linked_peer_id`.
10. **[ROUTING-12]** `src/routing/session_key.rs:791`: Optimize `normalize_agent_id` from 4 allocations to 1 pre-allocated string.
11. **[ROUTING-13]** `src/routing/session_key.rs:838`: Optimize `sanitize_component` from 3 allocations to 1 pre-allocated string.
12. **[ROUTING-14]** `src/routing/session_key.rs:164`: Replace `impl Into<String>` + `&val.into()` with `impl AsRef<str>` across `SessionKey` constructors.

### Deferred & Rejected
- **[ROUTING-08]** (DEFER): Zero-allocation `candidates` iterator walk in `resolve_route`.
- **[ROUTING-06]** (REJECT): Repetitive allocations in `overlay_route` (preserves explicit precedence mapping).
- **[ROUTING-15]** (REJECT): Panic in `SessionKey::task` (intentional invariant).
