# Static Review — `src/approval/`

**Reviewer:** pi sub-agent (`general-purpose`)
**Worktree:** `/home/zou/data/workspace/Aleph/.worktrees/review-approval`
**Branch:** `review/approval` (forked from `main`)
**Scope:** 12 files / ~4.8 k LOC under `src/approval/`
**Date:** 2026-09-05 (project clock)

This module gates every agent-initiated browser / desktop / automation /
PIM / media action. It is the **last line of defence** between a model and
the host, so the review weights security and default-deny correctness above
everything else. Fail-open is a P0; cosmetic issues are deferred.

`cargo check` is **intentionally not run** at this stage per the task contract;
a unified cargo check runs after all modules finish. Fixes below were
applied statically and verified against the surrounding code paths.

---

## Headline counts

| File | P0 | P1 | P2 | Notes |
|------|----|----|----|-------|
| `config.rs` | **1 (FIXED)** | 1 | 0 | fail-open blocklist rule + missing `#[non_exhaustive]` (cross-module) |
| `types.rs` | 0 | 1 | 0 | public enums lack `#[non_exhaustive]` (cross-module) |
| `tool_call.rs` | 0 | 1 | 0 | public struct with public fields |
| `node_requester.rs` | 0 | 1 | 0 | dead branch + incomplete ANSI strip in `sanitize_for_display` |
| `guardian_requester.rs` | 0 | 0 | 1 | `ends_with('…')` heuristic is fragile |
| `mod.rs` | 0 | 0 | 1 | `.expect()` on const slice |
| `policy.rs` | 0 | 0 | 0 | clean |
| `audit.rs` | 0 | 0 | 0 | clean |
| `callback_sink.rs` | 0 | 0 | 0 | clean |
| `session_route.rs` | 0 | 0 | 0 | clean |
| `operator_requester.rs` | 0 | 0 | 0 | clean |
| `adapters.rs` | 0 | 0 | 0 | clean |
| **total** | **1** | **4** | **2** | |

---

## Findings

#### [P0] blocklist-rule-compile-fail-silent-allow
- **File:** `src/approval/config.rs:149-181` (pre-fix)
- **Rule:** fail-open blocklist (Category: Security / Reliability)
- **Context:** Inside `compile_rules_grouped(rules)`, called from
  `ConfigApprovalPolicy::new()` at lines 196 / 197 for **both** blocklist
  and allowlist. The blocklist is the **most security-sensitive half** of
  the policy (decision step 1: "If the target matches any **blocklist**
  entry for the action type → Deny").
- **Before:**
  ```rust
  // Rules whose patterns fail to compile are skipped with a warning.
  fn compile_rules_grouped(rules: &[PolicyRule]) -> HashMap<ActionType, Vec<CompiledRule>> {
      ...
      Err(e) => {
          warn!(
              pattern = %rule.pattern,
              error = %e,
              "Failed to compile glob pattern; skipping rule"
          );
      }
      ...
  }

  pub fn new(config: PolicyConfig) -> Self {
      let blocklist_by_type = compile_rules_grouped(&config.blocklist);
      let allowlist_by_type = compile_rules_grouped(&config.allowlist);
      ...
  }
  ```
- **After:**
  ```rust
  fn compile_rules_grouped(
      rules: &[PolicyRule],
      fail_closed: bool,
      label: &str,
  ) -> HashMap<ActionType, Vec<CompiledRule>> {
      static MATCH_ANYTHING: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
      ...
      Err(e) => {
          if fail_closed {
              let match_anything = MATCH_ANYTHING.get_or_init(|| {
                  regex::Regex::new("(?s).*")
                      .expect("static regex pattern `(?s).*`")
              });
              error!(
                  pattern = %rule.pattern,
                  error = %e,
                  "Failed to compile {label} glob pattern; denying all matching actions (fail-closed)"
              );
              grouped.entry(rule.action_type.clone()).or_default().push(CompiledRule {
                  pattern: rule.pattern.clone(),
                  regex: match_anything.clone(),
              });
          } else {
              warn!(/* unchanged: skip rule */);
          }
      }
      ...
  }

  pub fn new(config: PolicyPolicyConfig) -> Self {
      let blocklist_by_type = compile_rules_grouped(&config.blocklist, true, "blocklist");
      let allowlist_by_type = compile_rules_grouped(&config.allowlist, false, "allowlist");
      ...
  }
  ```
- **Why:** A single typo'd / size-bomb / ReDoS-shaped blocklist pattern for
  a default-**Allow** action type (`BrowserNavigate`, `BrowserClick`,
  `BrowserType`, `BrowserFill`, `BrowserPressKey`, `BrowserScroll`,
  `BrowserHover` — seven of the curated 24) used to fall through the entire
  decision tree to `Allow`. The shape is exactly the fail-open the
  curated-vs-broken file split (`safe_default()`) exists to prevent — but
  it was only applied at the file boundary, not at the per-rule boundary.
  Asymmetric fix: blocklist fail-closed (deny all matching actions of the
  affected type), allowlist stays fail-open (a broken allowlist is a
  convenience loss, defaults remain in force).

**Status:** **FIXED in this commit.** Two new unit tests pin the asymmetry
(`a_blocklist_rule_that_fails_to_compile_denies_all_matching_actions`,
`an_allowlist_rule_that_fails_to_compile_is_silently_skipped`).

---

#### [P1] public-enums-non-exhaustive
- **File:** `src/approval/types.rs:39-90`, `types.rs:130-140`, `types.rs:144-152`
- **Rule:** `pub enum` without `#[non_exhaustive]` (Category: API design)
- **Context:** `ActionType`, `ApprovalDecision`, `DefaultDecision` are all
  `pub` and consumed across `alephcore`, `aleph-cli`, `aleph-tui`,
  `aleph-panel`, `shared-ui-logic`. Adding a new variant is a semver
  breaking change — every exhaustive `match` downstream breaks at compile.
- **Why:** Defended by the "inherited_from" mechanism (already in place for
  `BrowserIdentityOverride` / `BrowserSessionState` inheriting from
  `BrowserCookiesWrite`) — the project DOES handle renames safely. But the
  enums themselves lack the attribute that would let `alephcore` opt in to
  forward compatibility. Same shape as the `shared::protocol` 58-enum gap
  already on file in `review-results/aggregate.md` ("deferred to dedicated
  change").
- **Status:** **DEFERRED (cross-module).** Adding `#[non_exhaustive]` is a
  1-line attribute per enum but breaks every exhaustive `match` site across
  crate dependencies; per the existing aggregate ruling this needs a
  crate-spanning migration plan, not a per-module drop.

---

#### [P1] call-identity-public-fields
- **File:** `src/approval/tool_call.rs:24-32`
- **Rule:** public struct with public fields (Category: API design)
- **Context:** `pub struct CallIdentity { pub turn_id: TurnId, pub call_id: String }`.
  Used as the pairing key that lets an approval card correlate to the exact
  tool row it belongs under (see module doc on the
  `newest_tool_call` heuristic this replaced).
- **Why:** Public fields freeze the layout and prevent the type from growing
  invariants (e.g. "call_id is non-empty", "turn_id is canonicalised").
  Audit-only consumers would benefit from a `new(...)` constructor + accessors;
  same for `ActionRequest` (next item).
- **Status:** **DEFERRED.** Low-value cosmetic; the type is fine for its
  current single-purpose use and refactoring would ripple into
  `harness::AgentHarness::act` (caller) and `ExecApprovalRecord::from_request`
  (downstream stamper).

---

#### [P1] action-request-public-fields
- **File:** `src/approval/types.rs:171-188`
- **Rule:** public struct with public fields (Category: API design)
- **Context:** `pub struct ActionRequest { pub action_type, pub target,
  pub display_target, pub agent_id, pub context, pub timestamp }`. Built by
  every policy-gated tool via `audit_identity(...)` and submitted to
  `ApprovalPolicy::check`.
- **Why:** Same shape as `CallIdentity` — layout-frozen, no constructor to
  enforce "agent_id must be set" or "timestamp must be in the call window".
  Documented lifecycle is "every tool builds one", so a builder would have
  callers across the gated tool surface.
- **Status:** **DEFERRED.** Touches every gated tool (`desktop`, `browser`,
  `system`, `pim`, `automation`); not actionable inside this module alone.

---

#### [P1] sanitize-display-dead-branch
- **File:** `src/approval/node_requester.rs:243-262`
- **Rule:** dead branch + incomplete ANSI escape strip (Category: Reliability)
- **Context:** `sanitize_for_display(s, max_len)` runs over the node's
  RPC-supplied `node_name`, `reason`, and `shown` before formatting them
  into the operator-facing `command` field. A malicious cluster node can
  otherwise inject newlines / ANSI escapes that forge audit-log entries or
  corrupt terminal rendering.
- **Before:**
  ```rust
  for c in s.chars() {
      if c.is_control() {
          out.push(' ');
      } else if c == '\u{1b}' {
          // ESC — drop the start of any ANSI sequence by replacing with space.
          out.push(' ');
      } else {
          out.push(c);
      }
      ...
  }
  ```
- **Why:** `char::is_control()` already covers `\u{001B}` (ESC) — the
  second branch is unreachable. Worse, only the ESC itself is stripped;
  `\x1b[31mRED\x1b[0m` becomes ` [31mRED [0m`, leaving the visual code
  visible (and confusing some terminals). The "strip ANSI" claim in the
  docstring is partial. The primary defence (no newlines, no control
  chars) IS working — `c.is_control()` covers `\n`, `\r`, `\t`, etc., and
  the test suite asserts no forgery. So the live risk is **cosmetic**,
  not a log-forgery vector.
- **Status:** **DEFERRED.** Fix is straightforward (`for c in s.chars()`,
  on `\u{1b}` consume `c` plus any subsequent `[`-led sequence chars
  until a final `@-~` letter); the comment-vs-behaviour drift is real
  but the security invariant holds.

---

#### [P2] guardian-blind-judge-ellipsis-heuristic
- **File:** `src/approval/guardian_requester.rs:325-336`
- **Rule:** fragile marker dependency (Category: Reliability)
- **Context:** `can_fully_judge(action)` decides whether the judge LLM is
  allowed to auto-approve. Returns false (forces human escalation) if
  `action.summary.ends_with('…')` AND no `analysis.segments` is present.
- **Why:** The truncation marker is a single Unicode codepoint (`U+2026`).
  If `ApprovalAction::summary` is ever constructed by a tool whose
  user-supplied target legitimately ends with `…` (e.g. the model
  passed `open https://example.com/…` as a command), the guard fires and
  we lose a clean low-risk auto-approval. The reciprocal is also fragile:
  changing the truncation marker elsewhere (any future
  `format!("{summary}…")`) silently disables the blind-judge guard.
- **Status:** **DEFERRED.** The right fix is a typed `Summary::Truncated`
  flag on `ApprovalAction`, which is a cross-module change
  (`sandbox::exec_approval`).

---

#### [P2] const-slice-expect
- **File:** `src/approval/mod.rs:152` (`reminder_schedule`), `mod.rs:525, 549`
  (test code)
- **Rule:** `.expect()` on a hard-coded constant (Category: Reliability)
- **Context:** `APPROVAL_REMINDER_BACKOFF_SECS: &[u64] = &[120, 300, 900]`
  is `.last()`-then-`.expect("…non-empty literal")` in production code
  (`reminder_schedule`); `.first()`-then-`.expect("non-empty literal")`
  in tests.
- **Why:** If the literal is ever edited to `&[]`, the `.expect` panics
  instead of returning `None`. The invariant is documented but enforced
  only by code review. A `const` block returning `Option<&[u64]>` or a
  `match` on `slice::is_empty()` would remove the panic path.
- **Status:** **DEFERRED.** Trivial in code, but the failure mode is
  "test runner panics, project-wide reminder logic stops firing" — low
  blast radius for the cost of touching.

---

## Findings fixed in this PR

1. **`config.rs::compile_rules_grouped` — fail-open blocklist rule** (P0 →
   FIXED). Compile failure of a blocklist glob now denies every target of
   that action type instead of silently allowing through to the default
   posture. Asymmetric with allowlist (still fail-open — broken allowlist
   rule is a convenience loss, not a security regression). Two new unit
   tests pin both halves: `a_blocklist_rule_that_fails_to_compile_denies_all_matching_actions`
   and `an_allowlist_rule_that_fails_to_compile_is_silently_skipped`.

---

## Findings deferred / cross-module

1. **`#[non_exhaustive]` on `ActionType` / `ApprovalDecision` /
   `DefaultDecision`** (P1) — same shape as the `shared::protocol` 58-enum
   gap in `aggregate.md`; requires a crate-spanning migration plan with
   `try_match!` adoption. Out of scope for one module.
2. **Public fields on `ActionRequest` / `CallIdentity`** (P1) — touches
   every policy-gated tool (`desktop`, `browser`, `system`, `pim`,
   `automation`) and `harness::AgentHarness::act`; not actionable inside
   this module alone.
3. **`sanitize_for_display` dead branch + partial ANSI strip**
   (`node_requester.rs:243-262`) (P1) — cosmetic drift, security
   invariant holds; can be cleaned in the same pass as any future
   cross-cluster RPC hardening.
4. **`can_fully_judge` `ends_with('…')` heuristic**
   (`guardian_requester.rs:325-336`) (P2) — needs a typed
   `Summary::Truncated` flag on `ApprovalAction`, owned by
   `sandbox::exec_approval`.
5. **`APPROVAL_REMINDER_BACKOFF_SECS` `.expect()`**
   (`mod.rs:152`, tests) (P2) — low blast radius, defer to the next
   pass over the no-timeout ruling.

---

## State of negative (explicit non-actions)

- **Not** running `cargo check`, `cargo clippy`, or `cargo test` —
  per the task contract, a unified cargo check runs after all modules
  finish; this is a static-only review.
- **Not** adding `#[non_exhaustive]` to the three public enums in
  `types.rs` — deferred per the `shared::protocol` precedent in
  `aggregate.md` (needs crate-spanning migration).
- **Not** removing the public fields on `ActionRequest` / `CallIdentity`
  — would touch every gated tool and the harness act phase.
- **Not** rewriting `sanitize_for_display` to consume full ANSI
  sequences — the security invariant (no newlines / no control chars)
  holds via `char::is_control()`; the cosmetic drift is deferred.
- **Not** fixing the `ends_with('…')` heuristic — needs a typed flag on
  `ApprovalAction`, owned by `sandbox::exec_approval`, not this module.
- **Not** changing `.expect("non-empty literal")` on
  `APPROVAL_REMINDER_BACKOFF_SECS` — invariant is documented; defer
  until the next pass over the no-timeout ruling.
- **Not** auditing `ConfigApprovalPolicy::redact_target` for the
  `DefaultHasher` stability claim — the module's own comment names the
  trade-off explicitly and the hash is not persisted or compared across
  processes; acceptable.
- **Not** flagging the curated default `BrowserNavigate = Allow` as
  fail-open — that posture is intentional per the project's ruling and
  is exhaustively defended by the
  `curated_default_covers_every_action_type` drift-guard test in
  `config.rs`.
- **Not** flagging `lift_ask` / `approval_timeout_for_current_turn` —
  these are operator-ruled surfaces with extensive doc + tests;
  refactoring without a new ruling would be a regression.
- **Not** touching the 7-day / 16-layer deep cross-trust-boundary
  sanitisation surface in `run_node_approval` / `OperatorApprovalRequester`
  — both have test-pinned invariants (`publishes_node_context_and_resolves`,
  `the_fallback_leg_still_addresses_the_card_to_its_own_session`, etc.).
- **Not** adding a `compile_rules_grouped_fail_closed` public helper —
  the fail-closed behaviour is private to `config.rs` and only consumed
  by `ConfigApprovalPolicy::new`.
- **Not** flagging `cached_glob_regex`'s 512-entry cap as "the cache is
  shared across processes / operators" — patterns come from operator
  config (low cardinality by construction); the cap is a defensive
  guard against pathological callers.