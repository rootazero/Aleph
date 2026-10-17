# Task 4 — public telecom runs metered in the formal spend ledger

Commit: `feat(telecom): meter public runs in spend ledger` (spec §8: separate public
principal under the owner ledger, own cap, fixed over-cap notice, never inferred from metadata).

## What changed
- `src/spend/mod.rs`: typed `Principal::TelecomPublic` (key `@telecom-public`, `as_key`/`from_key`
  round-trip). `Principal::user("@telecom-public")` and ambient people still collapse to
  `Unattributed`; the variant can only be named explicitly. `check_with` applies
  `telecom_public_usd` (falling back to `per_user_usd`) as the per-principal line for it only;
  `total_usd` still applies to everything.
- `src/config/types/policies/spend.rs`: `[policies.spend] telecom_public_usd` (optional, skipped when
  unset); `enabled()` counts it. No hardcoded owner budget reuse.
- `src/providers/metering.rs`: `MeteringProvider.principal: Option<Principal>`, `with_principal`,
  `pub(crate) metered_public_provider(inner)` (agent_id `telecom-public`). Floor check and
  `record_spend` use the explicit principal if set, else the ambient one (ordinary path unchanged).
- `src/orchestrator/harness_bridge/public_run.rs` (`run_public_text_impl`, one line): wraps
  `default_provider.current()` (same chain/failover) with `metered_public_provider`.
- `src/gateway/telecom/public_admission.rs`: admission consults
  `spend::check_with(&Principal::TelecomPublic, ..)` when `[policies.spend]` is enabled (process
  globals for `global()`), no owner principal / `deny_if_over_spend_and_report`. In-memory
  `tokens_spent` is only the bounded fallback when the policy is disabled; `record_spend` does not
  count while the ledger is authoritative (no double accounting). `new()` is fallback-only (deterministic tests).
- `src/gateway/handlers/spend.rs`, metering test globals: struct literal gained `telecom_public_usd: None`.

## Tests (all pass, `--test-threads=1`, serial group `spend_global_policy`)
- spend: `telecom_public_key_is_stable_and_round_trips`, `telecom_public_is_never_minted_from_strings_or_ambient_people`,
  `check_with_applies_telecom_public_line_only_to_the_public_principal`, `principals_in_reconstructs_the_telecom_public_principal`
- config: `telecom_public_usd_parses_and_is_skipped_when_unset`, `enabled_when_either_ceiling_is_set` (+ public-only enabled)
- metering: `explicit_public_principal_records_under_public_row_not_ambient_owner`,
  `ordinary_provider_still_charges_ambient_person_not_public`, `public_over_cap_denies_before_inner_call_without_touching_owner`
- admission: `formal_public_cap_refuses_without_any_owner_principal`, `formal_public_cap_is_independent_of_owner_rows`,
  `unset_public_cap_falls_back_to_per_user_ceiling`, `formal_mode_does_not_double_count_in_memory`,
  `disabled_policy_uses_bounded_fallback_and_never_touches_ledger` (+ existing fallback tests)
- real run canary: `harness_bridge::public_spend_tests::public_run_meters_under_telecom_public_not_the_ambient_owner`,
  `public_run_over_public_cap_is_refused_before_any_provider_call`
- regression bundle: `spend:: providers::metering public_admission public_spend_tests harness_bridge::public_run config::live_apply handlers::spend` → 137 passed, 0 failed; `telecom::public_run` 9 passed.
- `cargo check -p alephcore --lib --tests` clean (existing warnings only). rustfmt applied per touched file only.

## Residuals / not done
- Policy disabled ⇒ NOT ledger-compliant: only the bounded in-memory token cap (`DEFAULT_PUBLIC_TOKEN_CAP`) applies.
- If only `total_usd` is set (no `telecom_public_usd`/`per_user_usd`), the public principal is bounded by the total only.
- Cap is enforced pre-call per request (existing floor semantics): one in-flight call can overshoot, as for owners.
- Unpriced models record `unpriced_calls` not USD, so a USD cap does not bind them (same as owner ledger).
- Commit-state note: `public_spend_tests.rs` is committed in the HEAD-compatible form (`PublicRunRequest`
  without `history`). The public-history agent's working tree adds `history: Vec::new()` there; whoever
  commits that change must keep the working-tree version of this test file. `public_run.rs` was staged as
  HEAD + my single-line hunk only; the public-history hunks stay uncommitted.
- Owner A/B approvals, docs/reference updates, and other boundaries are separate and untouched.
