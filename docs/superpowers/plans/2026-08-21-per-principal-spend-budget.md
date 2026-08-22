# Per-Principal Spend Budget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a multi-user Aleph a per-principal (and machine-total) USD spend ceiling that is enforced at both run admission and every LLM call, is durable across restarts, and can be read and raised without a restart.

**Architecture:** A new `src/spend/` module owns one predicate (`spend::check`) and one ledger (a per-period UPSERT row in the existing `SecurityStore` SQLite database, fronted by a write-through in-process cache). Two arms call that one predicate: the run-admission gate (which produces a user-facing receipt) and `MeteringProvider` (the single funnel every LLM call already flows through, which produces a provider error the model self-heals from). Configuration is a new `[policies.spend]` section wired into the live-apply path so raising a limit takes effect immediately; the read face is an admin-gated `spend.query` RPC plus an `aleph spend` CLI.

**Tech Stack:** Rust (tokio, serde, schemars, rusqlite, arc-swap, chrono), `alephcore` + `aleph-protocol` + `aleph-cli` crates, bash/python for the real-machine QA fixture.

**Spec:** `docs/superpowers/specs/2026-08-21-per-principal-spend-budget-design.md`

## Global Constraints

- **Branch/worktree:** `multiuser-round7` in `/Volumes/TBU4/Workspace/Aleph-mu7`, based on `multiuser-round6` (still unmerged to `main`). Do not merge to `main` in this round.
- **Naming:** the module, the config section and every type use the word **`spend`**, never `budget` — `src/context/budget/` and `TerminateReason::BudgetExhaustedPartialResult` already own that word for two different things.
- **Config section:** `[policies.spend]` with exactly three keys — `per_user_usd`, `total_usd`, `period` (`"month"` | `"day"`, default `"month"`).
- **Both limits absent ⇒ the ledger is never consulted and never written.** A single-user box must be byte-identical. This is asserted, not assumed (guard G8).
- **Unpriced calls never become zero dollars and never deny.** `CostStatus::Unknown` increments `unpriced_calls` only; `usd` does not move. Only a *measured* price can accumulate, so a *missing* price can never produce a denial (guards G3, G4).
- **`Limit::Total` is a fieldless variant.** Machine-level numbers appear only on the admin-gated `spend.query` / `aleph spend` (guard G12).
- **The spend principal is a `users.user_id` or the reserved `@unattributed`.** Never an agent id, never `u-owner` as a fallback (guard G15).
- **Period boundaries are local-timezone calendar boundaries**, computed in exactly one place (`src/spend/period.rs`), and `period_start_ms` + `period_end_ms` ride every verdict and every read response.
- **Comments and doc comments in English; user-facing copy goes through `src/gateway/i18n.rs` in both `Zh` and `En`.**
- **Commit style:** `<scope>: <description>`, English, no attribution trailers.
- **Every guard is falsified once by hand** after it is written: break the production line it pins, confirm the test goes red and names a file/line, restore.
- **Minimum verification set** (judgment-list §10) after every task that touches the relevant crate:
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib --no-run
  cargo test -p aleph-cli -p aleph-tui --no-run
  cargo clippy --all-targets
  ```
  `cargo check` alone does not compile `#[cfg(test)]` and is not sufficient.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/config/types/policies/spend.rs` | **Create.** `SpendPolicy` + `SpendPeriod`; the only place the three config keys are spelled. |
| `src/config/types/policies/mod.rs` | **Modify.** Add the `spend` field to `PoliciesConfig` and re-export the two types. |
| `src/spend/period.rs` | **Create.** Local-timezone calendar period boundaries + the retention floor. The only answer to "which window is this". |
| `src/spend/mod.rs` | **Create.** `Principal` / `Spent` / `Delta` / `Limit` / `Verdict`, the `SpendLedger` trait, the process-global ledger + policy handles, the single `check` predicate, and the two principal resolvers. |
| `src/spend/sqlite.rs` | **Create.** The durable `SpendLedger` over the existing `SecurityStore` connection, with a write-through cache and the retention sweep. |
| `src/spend/tests.rs` | **Create.** The spend module's own guards — G8, G9, G10, G13, G15. Every other guard lives beside the code it pins: G3–G5 in `providers/metering.rs`, G11–G12 in the engine/i18n tests, G14 in `config/live_apply.rs`. A guard filed away from its subject is a guard nobody edits when its subject changes. |
| `src/gateway/security/store/mod.rs` | **Modify.** Schema v16 → v17: create `spend_ledger`. |
| `src/providers/metering.rs` | **Modify.** The floor arm — check before delegating, record after. |
| `src/gateway/execution_engine/run_loop/mod.rs` | **Modify.** The shared admission helper both engines call. |
| `src/gateway/execution_engine/{execute,simple}.rs` | **Modify.** Call the helper. |
| `src/gateway/execution_engine/mod.rs` | **Modify.** `ExecutionError::SpendExhausted` + its `user_receipt` / `receipt_kind` arms. |
| `src/gateway/i18n.rs` | **Modify.** `ReceiptKind::SpendExhausted` + `ReceiptKind::is_transient()` + the two parameterised `Msg` variants. |
| `src/gateway/execution_engine/{goal_continuation,execute}.rs` | **Modify.** Route the two open-coded transient predicates through `is_transient()`. |
| `shared/protocol/src/spend.rs` | **Create.** The `spend.query` response contract, shared by the server and `aleph-cli`. |
| `src/gateway/handlers/spend.rs` | **Create.** The `spend.query` handler, built **from** the contract type. |
| `src/bin/aleph-server/commands/start/mod.rs` | **Modify.** Install the ledger + policy handle, run the retention sweep, register `spend.query`. |
| `src/gateway/{method_admin,method_census}.rs` | **Modify.** Gate `spend.` and pin the new method's ruling. |
| `interfaces/cli/src/commands/spend_cmd.rs` | **Create.** `aleph spend`. |
| `interfaces/cli/src/commands/{mod,cli_args}.rs`, `interfaces/cli/src/main.rs` | **Modify.** Wire the subcommand. |
| `src/config/{reload_impact,live_apply}.rs` | **Modify.** `LIVE_SUBSECTIONS` + the `policies.spend` apply arm + subsection-aware `classify` / `classify_verified`. |
| `src/config/patcher.rs`, `src/gateway/handlers/config.rs` | **Modify.** Whole-config paths push `live_targets()`, not `LIVE_SECTIONS`. |
| `src/pricing.rs` | **Modify.** Rewrite the "never a gate" sentence to be precise instead of deleting it. |
| `qa/spend_budget/run.sh` (+ helpers) | **Create.** Eleven real-machine assertions. |
| `docs/reference/{FEATURE_LOCATOR,SECURITY}.md`, `CLAUDE.md` | **Modify.** Record the round and its judgment criteria. |

---

## Task 1 — `[policies.spend]` config type

**Deliverable:** the three keys parse, round-trip, and default to "no limit at all".

- [ ] Create `src/config/types/policies/spend.rs` holding `SpendPeriod` (`#[serde(rename_all = "lowercase")]`, `Month` default, `Day`) and `SpendPolicy` with exactly three fields:
  - `per_user_usd: Option<f64>` and `total_usd: Option<f64>`, both `#[serde(default, skip_serializing_if = "Option::is_none")]`;
  - `period: SpendPeriod` with `#[serde(default)]`;
  - plus `pub const fn enabled(&self) -> bool { self.per_user_usd.is_some() || self.total_usd.is_some() }`.
- [ ] Module doc states why the word is **spend** and not `budget`: `crate::context::budget` is the context-window budget and `TerminateReason::BudgetExhaustedPartialResult` is the per-run token budget — three different things, and one shared word would make every future grep ambiguous.
- [ ] Add `mod spend;` + `pub use spend::{SpendPeriod, SpendPolicy};` to `src/config/types/policies/mod.rs`, and the field on `PoliciesConfig` shaped like its six siblings — `#[serde(default)]`, **no** `skip_serializing_if`:
```rust
    /// Per-principal and machine-total USD spend ceilings. Absent ⇒ disabled.
    #[serde(default)]
    pub spend: SpendPolicy,
```
- [ ] Write the failing tests FIRST, in `spend.rs`'s `#[cfg(test)] mod tests`:
  - an empty `[policies.spend]` table parses and `enabled()` is `false`;
  - `period = "day"` parses to `SpendPeriod::Day`, and an **unknown** period string is an error, not a silent default — a typo that quietly means "month" is exactly the class of failure this round exists to prevent;
  - a `SpendPolicy` carrying only `total_usd` round-trips through TOML with no `per_user_usd` key emitted (that is what `skip_serializing_if` buys, and `save_incremental`'s clearing path depends on it).
- [ ] Run them, watch them fail, implement, watch them pass.
- [ ] `cargo test -p alephcore --lib --no-run && cargo test -p alephcore --lib policies::spend`
- [ ] Commit: `config: add the [policies.spend] section`

## Task 2 — `src/spend/period.rs`, the only answer to "which window is this"

**Deliverable:** two pure functions giving local-calendar period boundaries.

- [ ] Create `src/spend/period.rs` with `period_start_ms(now_ms: i64, period: SpendPeriod) -> i64` and `period_end_ms(now_ms: i64, period: SpendPeriod) -> i64`, built on `chrono::Local` (already a dependency — `src/security/audit.rs` uses chrono today).
- [ ] Failing tests first:
  - a day window starts at **local** midnight, not UTC midnight — the test sets its own `TZ` rather than trusting the machine's;
  - a month window rolls Jan 31 → Feb 1, not "31 days later";
  - across every hour of a DST-transition day in a DST timezone, `period_end_ms > period_start_ms` — the 23-hour and 25-hour days are precisely where an hours-arithmetic implementation yields a boundary in the past;
  - `period_start_ms(period_end_ms(t))` is the **next** window's start: the windows tile with no gap and no overlap.
- [ ] Module doc records the tradeoff as a decision, not an accident: local boundaries mean a machine that changes timezone sees one short or long period; UTC boundaries would put the reset mid-workday for most of the world. Local was chosen.
- [ ] `cargo test -p alephcore --lib spend::period`
- [ ] Commit: `spend: local-calendar period boundaries`

## Task 3 — `src/spend/mod.rs`: types, handles, and the two principal resolvers

**Deliverable:** every type the rest of the round names, plus the two process-global handles, compiling with an in-memory ledger. No call site yet.

- [ ] Create `src/spend/mod.rs` with:
```rust
/// Who a dollar is charged to. Always a `users.user_id` or the reserved
/// sentinel — never an agent id (an agent is not a person and cannot hold a
/// budget), and never `u-owner` as a fallback (charging an unattributed run
/// to the machine owner is a silent misattribution, not a default).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal { User(String), Unattributed }

impl Principal {
    /// The ledger's primary-key text. `"@unattributed"` cannot collide with a
    /// real id: `users.user_id` values are `u-`-prefixed.
    pub fn as_key(&self) -> &str { match self { Self::User(id) => id, Self::Unattributed => "@unattributed" } }
}

/// What has been spent in the window that is open right now.
pub struct Spent {
    pub usd: f64,
    /// `CostStatus::Unknown` calls — real spend that carries no price.
    pub unpriced_calls: u64,
    /// `CostStatus::PartialMissingPrice` calls — `usd` is a lower bound.
    pub partial_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
}

/// Which ceiling was hit. Shape, not role predicate — see spec §4.8.
pub enum Limit {
    /// The caller's own ceiling: both numbers are his own spend, so both are
    /// safe to tell him.
    PerUser { spent: f64, limit: f64 },
    /// The machine-wide ceiling. Deliberately **fieldless**: `user_receipt`
    /// takes no actor and `caller_identity` is dead inside a spawned run, so
    /// there is no point at which "may this person see the machine total?"
    /// could be answered. Machine numbers live on the admin-gated read face.
    Total,
}

pub enum Verdict { Allowed(Spent), Denied { limit: Limit, spent: Spent } }
```
- [ ] Add the trait and the handles:
```rust
pub trait SpendLedger: Send + Sync {
    fn record(&self, principal: &Principal, period_start_ms: i64, delta: Delta) -> anyhow::Result<()>;
    fn spent_for(&self, principal: &Principal, period_start_ms: i64) -> anyhow::Result<Spent>;
    fn total_for(&self, period_start_ms: i64) -> anyhow::Result<Spent>;
    fn sweep_before(&self, period_start_ms: i64) -> anyhow::Result<usize>;
}

/// One increment. Exactly one of the three fields moves per call, which is
/// what makes "an unpriced call never becomes zero dollars" a property of the
/// type rather than a rule someone has to remember.
pub struct Delta { pub usd: f64, pub unpriced: u64, pub partial: u64 }
```
- [ ] Process-global install/read for **both** the ledger and the policy, following the `global_cache_monitor()` precedent — **not** constructor parameters. `MeteringProvider` has 8 production construction sites; threading a handle through them would wire some and miss others, and the missed ones would meter without a floor while every unit test stayed green (judgment list: "数出来是七个就别再想构造点了，配置该接在类型上").
- [ ] The **two principal resolvers**, each with the shape it is used in:
```rust
/// Floor arm — called from inside the run's task-local nest.
pub fn ambient_principal() -> Principal { … current_room_author().or_else(ambient_owner) … }

/// Admission arm — called before the nest exists, off the request metadata.
pub fn principal_from_metadata(meta: &HashMap<String, String>) -> Principal {
    … meta.get(AUTHOR_USER_KEY).or_else(|| meta.get(scope::OWNER_META_KEY)) …
}
```
  Both read the same two facts in the same order. They are provably equivalent because `run_loop::with_request_scope` seeds `CURRENT_ROOM_AUTHOR` from `request.metadata[AUTHOR_USER_KEY]` verbatim, with **no** `ScopeId::Project` filter — unlike `scope::room_author()`, which returns `None` for every non-room scope and would therefore charge nearly every install's spend to `@unattributed` with no test going red. Neither resolver may call `visibility::ambient_actor()` or `turn_context::current_agent_id()`: `ambient_actor`'s third arm is an **agent id**.
- [ ] Guards, each falsified by hand after writing:
  - **G13** — the two resolvers agree: build a `RunRequest` whose metadata carries an author, run `with_request_scope` over it, and assert `ambient_principal()` inside equals `principal_from_metadata` outside. Break one arm's ordering and watch it name the file.
  - **G15** — source-level: `src/spend/` contains neither `ambient_actor` nor `current_agent_id`. Strip comments before scanning (a comment explaining the ban would otherwise satisfy a naive `contains`), and assert the scan actually read a non-empty production prefix so a CRLF checkout cannot make it vacuously green.
- [ ] Both guards live in `src/spend/tests.rs` (`mod tests;` from `mod.rs`), together with G8–G10 from Task 5.
- [ ] `cargo test -p alephcore --lib spend::`
- [ ] Commit: `spend: core types, global handles, and the two principal resolvers`

## Task 4 — `src/spend/sqlite.rs`: the durable ledger

**Deliverable:** spend survives a restart, and the machine total is derived rather than stored twice.

- [ ] Bump `SCHEMA_VERSION` in `src/gateway/security/store/mod.rs` from `16` to `17` and add the table to the migration:
```sql
CREATE TABLE IF NOT EXISTS spend_ledger (
  principal_id    TEXT    NOT NULL,
  period_start    INTEGER NOT NULL,           -- epoch ms, from spend::period
  usd             REAL    NOT NULL DEFAULT 0,
  unpriced_calls  INTEGER NOT NULL DEFAULT 0,
  partial_calls   INTEGER NOT NULL DEFAULT 0,
  updated_at      INTEGER NOT NULL,
  PRIMARY KEY (principal_id, period_start)
);
CREATE INDEX IF NOT EXISTS idx_spend_period ON spend_ledger(period_start);
```
- [ ] `record` is a single `INSERT … ON CONFLICT(principal_id, period_start) DO UPDATE SET usd = usd + excluded.usd, …` — an UPSERT, never a read-modify-write. The read-modify-write shape is what makes two writers lose an update, and there are two arms writing here.
- [ ] `total_for` is `SELECT SUM(usd), SUM(unpriced_calls), SUM(partial_calls) FROM spend_ledger WHERE period_start = ?`. **Do not** keep a second `@org` row: a stored aggregate is a second source of truth for a number the rows already answer, and the two drift the first time a write lands on one and not the other.
- [ ] Write-through in-process cache in front of it (the floor arm runs on every LLM call and must not hit SQLite each time): the cache is authoritative for reads within the current period, the UPSERT is what makes it durable, and a cache miss reads through.
- [ ] The non-journaling constructor is `#[cfg(test)]`. A second writer to a process-global table is not a hypothetical: `ProcessRegistry` shipped that exact bug, and it only surfaced under the parallel test binary while every isolated test stayed green.
- [ ] `sweep_before` deletes rows older than the retention floor, returning the count.
- [ ] Failing tests first: durability across a reopen; UPSERT accumulates rather than replaces; `total_for` equals the sum of three principals' rows; concurrent `record` from N threads sums exactly (no lost update); `sweep_before` leaves the current period alone.
- [ ] `cargo test -p alephcore --lib spend::sqlite`
- [ ] Commit: `spend: durable per-period ledger on the security store`

## Task 5 — `spend::check`, the single predicate

**Deliverable:** one function both arms call. Total-first ordering.

- [ ] `pub fn check(principal: &Principal, now_ms: i64) -> Verdict`:
  - if `policy.enabled()` is false, return `Allowed` **without reading the ledger** — no query, no cache fill, no row;
  - compute the window once via `spend::period`;
  - evaluate the machine total **first**: when both ceilings are blown, report `Limit::Total`, because that is the one the caller cannot move by asking someone to raise his own line;
  - `period_start_ms` / `period_end_ms` ride the verdict either way, so every consumer can say *when it resets* without recomputing the window and risking a second answer.
- [ ] Guards:
  - **G8** — with both limits `None`, a `SpendLedger` test-double whose every method panics is installed, and a full `check` still returns `Allowed`. This is what makes "byte-identical on a single-user box" an assertion instead of a claim.
  - **G9** — both ceilings blown ⇒ `Limit::Total`, not `PerUser`.
  - **G10** — a principal exactly *at* the ceiling is denied and one cent under is allowed (the boundary is `>=`, stated once).
- [ ] `cargo test -p alephcore --lib spend::check`
- [ ] Commit: `spend: the single admission predicate`

## Task 6 — the floor arm in `MeteringProvider`

**Deliverable:** every LLM call in the process is gated and metered, including the ones no admission gate ever sees.

- [ ] Why this arm exists at all: sub-agent, MoA-advisor and compactor spend is **not** in the parent run's `token_breakdown` — each wraps its own `MeteringProvider` (the source comment says wrapping again "would double-count"). Gating only at admission would therefore systematically under-count exactly the runs that spend the most.
- [ ] `process` and `execute_streaming_dyn` are already structurally identical and already funnel into one `record_usage`. Add the check **inside** the `Box::pin(async move { … })`, immediately before `fut.await`. `self.inner.process(req)` only *builds* the future; awaiting is what makes the network call, so denying there denies before any request leaves the box.
- [ ] On `Verdict::Denied`, return an `AlephError` whose text names the ceiling and the reset time. This is a provider error the model sees and self-heals from (A2: let the model see and self-heal; do not let the harness pick a recovery strategy).
- [ ] After the call, extend `record_usage` — the one funnel both arms already share — to also write the ledger:
  - price with `crate::pricing::estimate(provider, model, &breakdown)`, taking provider/model from `serving_provider_hint()` / `serving_model_hint()` (`ProviderResponse` has no model field);
  - `CostStatus::Complete` ⇒ `Delta { usd, .. }`;
  - `PartialMissingPrice` ⇒ `Delta { usd, partial: 1, .. }` — a lower bound is still measured money;
  - `Unknown` ⇒ `Delta { usd: 0.0, unpriced: 1, .. }`. **A missing price never moves `usd`**, so it can never produce a denial. That is the load-bearing half of `pricing.rs`'s "never a gate" sentence, and it is preserved rather than deleted.
- [ ] Document the bounded overshoot honestly: the check is before the call and the record is after, so a principal can exceed the ceiling by at most the in-flight calls' cost. The alternative — reserving an estimate up front — would require refunding on every error path, and a missed refund silently shrinks someone's budget forever.
- [ ] Guards: **G3** (an `Unknown` estimate leaves `usd` at exactly `0.0` and increments `unpriced_calls`), **G4** (a ledger whose `unpriced_calls` is enormous and `usd` zero still returns `Allowed`), **G5** (the streaming arm records identically to the non-streaming one — same double asserted through both entry points, since the streaming gap is a bug this file already shipped once).
- [ ] `cargo test -p alephcore --lib providers::metering`
- [ ] Commit: `spend: gate and meter every LLM call at the metering funnel`

## Task 7 — the admission arm and its receipt

**Deliverable:** a denied run gets a localized, terminal receipt instead of a raw error string.

- [ ] Add the shared helper in `src/gateway/execution_engine/run_loop/mod.rs`, next to `with_request_scope` — it resolves the principal off `request.metadata` (Task 3's admission resolver) and returns the verdict. Both engines call the helper; neither open-codes the check. `execute.rs` gates at `admit_run` (:151); `simple.rs` does **not** use `admit_run` and needs its own call site at its `execute` entry (:71) — a floor that only one engine honours is not a floor.
- [ ] In `src/gateway/execution_engine/mod.rs`, add `ExecutionError::SpendExhausted { limit: Limit }` and its arms:
  - `receipt_kind()` ⇒ a new `ReceiptKind::SpendExhausted` (code `"SPEND_EXHAUSTED"`);
  - `user_receipt()` needs no special case — it already routes through `t(Msg::ErrReceipt(kind), locale)`.
- [ ] The wording is chosen by **shape**, not by a role predicate: `Limit::PerUser { spent, limit }` renders both numbers (they are the caller's own), `Limit::Total` renders none. `user_receipt(&self, locale)` takes no actor and `caller_identity::current_caller_user()` is dead inside a spawned run, so there is no place a "may this person see the machine total" question could even be asked — which is why the variant is fieldless rather than gated.
- [ ] Both `Msg` variants exist in `Zh` and `En`, parameterized like the existing `Msg::NewSessionStarted` / `QueuedMessagesDropped` precedents. Every receipt names the reset time, so the answer to "what do I do now" is on the card.
- [ ] **Add `ReceiptKind::is_transient()`** and route the two open-coded copies through it — `goal_continuation.rs:537` and `execute.rs:1546` both spell `matches!(kind, RateLimited | Unreachable)` verbatim. Converge them *in this task*, because the new kind must be terminal in both, and two hand-written predicates is exactly how one of them ends up saying otherwise.
- [ ] Guards: **G11** — `SpendExhausted.is_transient()` is `false`, asserted at both the goal-park and loop-park sites, so a budget denial can never be parked-and-retried into an infinite billing loop. **G12** — source-level: no `Limit::Total` arm formats a number.
- [ ] `cargo test -p alephcore --lib execution_engine:: i18n::`
- [ ] Commit: `gateway: refuse a run whose principal is out of budget`

## Task 8 — `spend.query`: the contract and the handler

**Deliverable:** an admin can read what has been spent, by whom, in the window that is open now.

- [ ] Create `shared/protocol/src/spend.rs` with `SpendQueryParams` and `SpendQueryResult` + a `SpendRow` per principal, and register `pub mod spend;` in `lib.rs`. The contract type lives here because the server and `aleph-cli` are two crates and `aleph-cli` may not depend on `alephcore`: a hand-copied shape on either side is the `aleph workspace create` / `providers list` bug, which shipped three times.
- [ ] `SpendQueryResult` carries `configured: bool`. When no ceiling is set, the answer is **not** `0` — `0` and "not measured" are different facts, and a reader who cannot tell them apart will read an unconfigured box as a thrifty one.
- [ ] Also carry `period_start_ms` / `period_end_ms`, `unpriced_calls` and `partial_calls` per row: a total whose confidence is invisible invites a decision the number cannot support.
- [ ] Create `src/gateway/handlers/spend.rs` modelled on `handlers/security_audit.rs` (242 lines, same shape). **Build the response FROM the contract type** — never assemble a `json!` literal that happens to match. Constructing from the type makes over-sending a compile error instead of a review item (the `workspace.get` over-send shipped four unread fields precisely because the test only parsed, and parsing proves superset, never equality).
- [ ] Register in all four places, none of which the compiler will remind you about: `handlers/mod.rs` (`pub mod spend;`), `start/mod.rs` (the registration call), `method_admin.rs` (`"spend."` in `ADMIN_PREFIXES`), `method_census.rs` (`("spend.query", Class::Admin)`).
- [ ] Deliberately **no** `spend.reset`. Zeroing a ledger is indistinguishable from a write that never happened; raising the ceiling is the reversible way to say the same thing and leaves a trail.
- [ ] Tests: key-set equality in both directions against a real handler response, with the expected keys **derived from the contract type** (serialize a value, take its keys) rather than written as a literal list — a literal list is the same enumeration bug one level up. An unconfigured box answers `configured: false`, not zeros.
- [ ] `cargo test -p alephcore --lib handlers::spend && cargo test -p aleph-protocol`
- [ ] Commit: `gateway: spend.query read surface`

## Task 9 — `aleph spend`

**Deliverable:** a headless deployment can read the same numbers without a Panel.

- [ ] Create `interfaces/cli/src/commands/spend_cmd.rs` mirroring `audit_cmd.rs` (205 lines, same shape), consuming `aleph_protocol::spend` — no locally-declared row struct.
- [ ] Wire all four points: `commands/mod.rs` (`pub mod spend_cmd;`), `commands/cli_args.rs` (the `Spend` variant on `Commands`), `main.rs` (the dispatch arm), and `--json` passthrough as its siblings do.
- [ ] Every column the table prints must be a field the response actually sends. `providers list` printed `type` / `default` while the server sent `provider_type` / `is_default`, so every row rendered a dash from the day it was written — and a dash reads as "no value yet", not as a bug, which is why nobody reported it.
- [ ] `configured: false` prints a sentence, not an empty table: an empty table means "nobody spent anything".
- [ ] Test: assert every rendered column name is present in a `SpendQueryResult` serialized from the contract type. Prove it goes RED by renaming one wire key.
- [ ] `cargo test -p aleph-cli --no-run && cargo test -p aleph-cli spend`
- [ ] Commit: `cli: aleph spend`

## Task 10 — raising a ceiling without a restart

**Deliverable:** `[policies.spend]` applies live, and says so honestly when it does not.

- [ ] The escape hatch must exist: a person locked out by his own ceiling cannot raise it if raising it needs a restart, and "restart the server to unblock yourself" is how a budget becomes an outage.
- [ ] **Do not add `"policies"` to `LIVE_SECTIONS`.** That list is top-level, and `PoliciesConfig` has six other fields with no live handle — declaring the parent live would advertise "no restart needed" for all seven, which is verbatim the bug `live_apply.rs` exists to prevent.
- [ ] Add `LIVE_SUBSECTIONS: &[&str] = &["policies.spend"]`, checked **before** the top-level match in `ReloadImpact::classify` (most specific wins), and widen `classify_verified` to match a subsection the same way.
- [ ] Add the apply arm shaped like `"route"`: store into the process-global policy handle from Task 3; a **missing** handle returns `false` so the verdict downgrades to `Restart` honestly. Reporting `Live` because a knob that cannot fail succeeded is the mirror of the bug this module fixes — `execution`'s arm documents exactly that reasoning already.
- [ ] Extend `every_live_section_has_an_apply_arm` to cover subsections too. Its `known_arms` is a hand-written literal list; a subsection with no arm must fail it by name, in both directions.
- [ ] The two whole-config callers pass `LIVE_SECTIONS` directly (`patcher.rs:645`, `handlers/config.rs:100`). Introduce `live_targets()` returning sections **and** subsections and use it at both, or a whole-file rollback silently skips `policies.spend` — the same "one caller acts on the table, the others only assert it" shape this module was created to remove.
- [ ] Guard **G14** — patch `policies.spend.per_user_usd` with a handle installed, assert `Live` and that a subsequent `check` uses the new ceiling *without a restart*; then drop the handle and assert the verdict downgrades to `Restart` rather than lying.
- [ ] `cargo test -p alephcore --lib config::live_apply config::reload_impact`
- [ ] Commit: `config: apply [policies.spend] live`

## Task 11 — retention, and the sentence in `pricing.rs`

**Deliverable:** the ledger does not grow forever, and the doc that this round appears to contradict is made precise instead of deleted.

- [ ] Call `sweep_before` at boot in `start/mod.rs`, keeping a documented number of past periods (mirroring `DEFAULT_RETENTION_SECS` in `src/security/audit.rs`, which is the existing answer to "how long do we keep this kind of row"). Log the count swept.
- [ ] Rewrite `src/pricing.rs`'s module doc. It currently says pricing is "best-effort, never a gate". After this round that sentence is false as written and true in the part that matters, so it becomes: *a missing price is never a gate — only a measured price accumulates toward a ceiling, and `CostStatus::Unknown` moves no dollars.* Do **not** delete it: it is the reason `Unknown` increments a counter instead of estimating.
- [ ] Add a source-level guard pinning that property so the doc and the code cannot drift: `spend` must have no path from `CostStatus::Unknown` to a non-zero `usd`.
- [ ] Docs: `docs/reference/FEATURE_LOCATOR.md` §5.22 (the round entry, with anchors), `docs/reference/SECURITY.md` (the spend ceiling as a control: what it buys and what it does not — it is a **ceiling**, not a hard cap, because of the bounded overshoot in Task 6), and `CLAUDE.md`'s judgment list with the criteria this round earned. Each criterion states the failure it prevents, not the feature it describes.
- [ ] Commit: `spend: retention sweep, pricing doc, and the round's criteria`

## Task 12 — `qa/spend_budget/run.sh`: the real-machine fixture

**Deliverable:** eleven assertions against a live server, each with an effect, not a call count.

- [ ] Build on `qa/multiuser_audit/` (round-6): `run.sh` + a python helper, `qa/lib/scratch_home.sh::qa_redirect_home` for the isolated `HOME` (this is not optional — six fixtures each open-coded that redirect and rustup installed a full toolchain into a scratch dir about to be deleted).
- [ ] Two principals via the round-6 loopback-mint / LAN-redeem pattern: loopback is unconditionally operator, so a second **member** principal only exists if his ticket is redeemed over the LAN URL.
- [ ] The eleven assertions:
  1. no `[policies.spend]` ⇒ `spend.query` answers `configured: false`, and the `spend_ledger` table has **zero rows** after a real run (G8 observed from outside the process);
  2. with a ceiling, a run under it succeeds and the ledger shows a **non-zero** `usd` for that principal;
  3. the member's spend lands on the **member's** row, not the operator's — the failure this whole round is about;
  4. a run past the per-user ceiling is refused with code `SPEND_EXHAUSTED`, and the message names the reset time;
  5. the same refusal in `language = "en"` is English (the receipt goes through `i18n`, not a hardcoded string);
  6. the machine-total refusal names **no** numbers (G12 on the wire, not in a unit test);
  7. raising `per_user_usd` via `config.patch` reports `Live` **and** the next run succeeds — no restart (G14 end-to-end);
  8. `spend.query` from a **member** is refused (admin-gated), and refused as *not permitted*, not as *not found*;
  9. `aleph spend` prints a row per principal with no dashes — every column carries a real value;
  10. spend survives a server restart: stop, start, query, same numbers (the whole point of a durable ledger);
  11. a call whose model is absent from the pricing table increments `unpriced_calls`, leaves `usd` untouched, and **does not** deny (G3/G4 on a live box, using a deliberately unknown model id).
- [ ] Every assertion checks an effect. "The RPC returned 200" is not an assertion — round-6's fixture caught its first three real bugs precisely because its assertions read state rather than counting calls.
- [ ] Run it. Fix what it finds. **A fixture's first run is the measurement of the round**, not a formality: if it goes green on the first try, first suspect the fixture.
- [ ] Commit: `qa: real-machine fixture for the per-principal spend budget`

---

## Final Verification

- [ ] Full minimum verification set, all five commands, from a clean tree.
- [ ] Every guard G3–G15 falsified once by hand: break the production line, confirm the test goes red **and names a file/line**, restore. A guard that has never been falsified is not a guard.
- [ ] Count the reds when you falsify. **Fewer than expected means your model of the code is wrong, not that the guard is blind** — read the deepest layer of that path before concluding anything.
- [ ] `git log --oneline multiuser-round6..multiuser-round7` reads as one coherent round.
- [ ] Do **not** merge to `main`: `multiuser-round6` is still unmerged and the two land together.
- [ ] Write the round up in memory (`project-multiuser-round7.md` + one line in `MEMORY.md`), recording the criteria this round earned and, explicitly, **what was not done**.

