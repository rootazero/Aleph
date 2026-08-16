# Severed-Wire Audit — `src/wizard`

- **Batch:** agents-batch-6
- **Module:** `src/wizard` (6 files, 1,338 LOC)
- **Date:** 2026-08-16
- **Reviewer:** static (severed-wire-audit skill)

## Summary

The wizard module is a session-based wizard framework: `types.rs` (step/status/result
types), `session.rs` (`WizardSession` + `WizardFlow` trait), `prompter.rs`
(`WizardPrompter` trait + `RpcPrompter`), and `flows/onboarding.rs` (the one live flow).
It is wired server-side: `src/gateway/handlers/wizard.rs` consumes the session API and
registers `wizard.start/next/answer/cancel/status`, and the boot path installs an
`OnboardingFlow` factory.

The module is **mostly clean** — registration, call/handler, and channel parity all hold
(`next`/`answer`/`cancel`/`status`/`id` are all consumed). The stub sweep is clean (no
`unimplemented!`/`todo!` in scope). But one wire is genuinely severed: the onboarding
flow collects a full configuration and then **discards it**. The remaining findings are
dead scaffolding left behind by an earlier refactor.

| Severity | Count |
|----------|-------|
| critical | 0 |
| high | 1 |
| medium | 0 |
| low | 4 |

**Decisions:** 2 DECIDE, 3 CUT, 0 CONNECT.

---

## Findings

### [HIGH] src/wizard/flows/onboarding.rs:346 — Onboarding collects `OnboardingData` but the data is discarded
- **Category:** logic
- **Decision:** DECIDE
- **Description:** `OnboardingFlow::run()` builds `let mut data = OnboardingData::default()`,
  populates it across `configure_primary/secondary/thinking/messaging` (API keys, providers,
  models, thinking level, messaging apps), then `review_and_finalize` only *displays* the
  summary and drops `data` when `run()` returns `Ok(())`. The `WizardFlow::run` signature
  returns `Result<(), WizardSessionError>` — there is no channel for the collected data to
  escape the flow. Grep confirms `OnboardingData` and its fields are referenced nowhere
  outside `onboarding.rs`. The inline comment at line 323 claims "the caller routes them onto
  the live Config," but no such route exists: the boot-path factory (`start/mod.rs:713`)
  returns `Box<dyn WizardFlow>` whose `run()` returns `()`, and `WizardNextResult`
  (`types.rs:236`) has no `data` payload (the original pairing-wizard plan proposed one, but
  it never landed). Net effect: the 10-stage first-run onboarding is reachable via
  `wizard.start` but is a **silent no-op** — the user believes Aleph is configured and
  nothing is persisted.
- **Suggested fix:** DECIDE, then connect. Likely: (a) add a `data: Option<Value>` /
  `finish_data` slot to `WizardNextResult` (mirroring the original plan) or change
  `WizardFlow::run` to return the collected data; and (b) decide how onboarding fields map
  onto the live `Config`, including vault placement for the API keys. Do not blindly persist
  without deciding the schema mapping.

### [LOW] src/wizard/types.rs:154 — `WizardStep::progress()` + `StepType::Progress` + `StepExecutor::Gateway` are dead scaffolding
- **Category:** quality
- **Decision:** CUT
- **Description:** No caller of `WizardStep::progress()` exists repo-wide (definition only).
  Its only effects — the `StepType::Progress` variant (types.rs:40) and the
  `StepExecutor::Gateway` variant (types.rs:49) — are never constructed or matched by live
  code (`StepExecutor` is always its `Client` default). The onboarding flow's comment
  (onboarding.rs:322-327) confirms the progress stub was deliberately removed "because the
  trait had no real client-visible progress channel," but the builder and both enum variants
  were left behind.
- **Suggested fix:** CUT `progress()`, the `Progress` variant, and the `Gateway` variant.
  These enums are `#[non_exhaustive]` + `Serialize`; verify no client renderer matches the
  `progress`/`gateway` string before removing the serialized variants (none exists in this
  workspace).

### [LOW] src/wizard/types.rs:187 — `WizardStep::with_validation()` and the `validation`/`validation_error` fields are inert
- **Category:** quality
- **Decision:** CUT
- **Description:** No caller of `with_validation()` exists. The `validation` and
  `validation_error` fields are set by no other path, so they are always `None` at runtime
  and absent from serialized output. No Rust-side consumer reads them.
- **Suggested fix:** CUT the builder and both fields (and their `#[serde]` attributes) unless
  a client renderer is planned to consume them.

### [LOW] src/wizard/session.rs:26 — `WizardSessionError::AlreadyDone` is never constructed or matched
- **Category:** quality
- **Decision:** CUT
- **Description:** `AlreadyDone` is defined but never constructed and never matched. Terminal
  state is handled by the sticky `WizardStatus` transitions in `settle()` (a late
  `answer`/`next` returns the terminal `WizardNextResult`, and the manager evicts the
  session), so the error variant is redundant. Its far end is already covered by
  `wizard_error_code`'s wildcard `_ => INTERNAL_ERROR`.
- **Suggested fix:** CUT the variant. (Enum is `#[non_exhaustive]`, so removal is
  additive-safe for external consumers.)

### [LOW] src/wizard/prompter.rs:19 — `WizardPrompter` trait is a single-implementor abstraction never used as a bound
- **Category:** architecture
- **Decision:** DECIDE
- **Description:** `WizardPrompter` is implemented only by `RpcPrompter` and is never used as
  `dyn WizardPrompter` or a generic `<P: WizardPrompter>` bound. Its only "use" is the import
  in onboarding.rs bringing the trait methods into scope so they resolve on the concrete
  `RpcPrompter` type. The polymorphic seam has no second implementor and no caller-side
  abstraction.
- **Suggested fix:** Either collapse the trait into `RpcPrompter`'s inherent impl (removing
  the dead seam + `async_trait` indirection), or keep it as a documented extension point for a
  future in-memory/test prompter. Flagged for judgment rather than auto-cut.

---

## What was NOT done / not covered

- No source code was edited (read-only audit; only this report + `summary.json` written).
- The consumer-side seam audit covered the whole workspace via grep, but cross-crate client
  code (e.g. any WASM/webchat renderer of step types) was checked only for symbol references
  in `*.rs`; no such references exist, so no client-side wizard UI is present in this
  workspace.
- `initial_data` is accepted by `wizard.start` / the flow factory but ignored by
  `OnboardingFlow` — noted as part of finding 1's "no return channel" problem, not a separate
  finding (the parameter plumbing lives in `src/gateway/handlers/wizard.rs`, outside
  `src/wizard` scope).
- No guard script (phase 5) was installed; per the task this batch is read-only
  enumeration + triage.
