# Wizard Onboarding Update — Scope Question

**Status:** Blocked on user input. Phase 3 triage flagged "wizard onboarding update" as a deferred item without specifying what to update.

## Current State

| File | Status | Last touched (model list fingerprint) |
|------|--------|----------------------------------------|
| `src/wizard/flows/onboarding.rs` | **Live, main tree** | `claude-opus-4-8`, `gpt-5.5`, `gemini-2.5-pro`, `claude-haiku-4-5` |
| `archive/wizard/flows/onboarding.rs` | **Stale duplicate** | `claude-3-5-haiku-20241022`, `gpt-4o`, `gemini-2.0-flash` |

The wizard module (`src/wizard/`) is wired at:
- `src/gateway/handlers/mod.rs:140` — `pub mod wizard;`
- `src/gateway/handlers/mod.rs:1165-1171` — RPC surface stubs (`wizard.start`, `wizard.next`, `wizard.answer`, `wizard.cancel`, `wizard.status`)
- `src/bin/aleph-server/commands/start/mod.rs:896-908` — `WizardSessionManager` install via `install_wizard_handlers`

The OnboardingFlow factory in `bin/.../start/mod.rs` points at `alephcore::wizard::OnboardingFlow::new()` — the live main-tree copy. The archive directory exists in source but is never compiled (CLAUDE.md "Don't propose unrelated refactoring" — but this also reads as a 判据 #5 "列举法只覆盖立法当天的世界" candidate: the archive dir accumulates stale code because nothing owns its lifecycle).

## What "Update" Could Mean

(a) **Refresh model picker list** against the live `MODEL_CATALOG`. The current list already targets the latest Aleph-side names; a refresh would be a routine `git grep` sweep.

(b) **Add a new wizard stage** — e.g., a channel-pairing step that drives `channel_manage` (per the new Task 5 iMessage R8 coverage). This is a new stage, not a model list change.

(c) **Replace the onboarding flow** with a new design. Bigger scope — affects the wizard contract itself.

(d) **Clean up the `archive/wizard/` directory** so it stops confusing `git grep`. Trivial: `git rm -r archive/wizard/`. Not a feature change.

## What's Needed to Act

A concrete answer to "which of (a)–(d)". Without that, the work is unbounded and any single change risks silently drifting the others.

## Why This Is Blocked, Not Just Deferred

Per CLAUDE.md R10 + AGENTS.md "Ask > Assume": I would be guessing the user's intent if I picked one. The risk of guessing wrong here is non-trivial: a model-list refresh (a) is a low-risk housekeeping task, but a flow replacement (c) is an architectural change that affects every install. Both could plausibly match "update"; the cost of being wrong about which one is the user's intent is asymmetric — a wrong (c) ships a new wizard, a wrong (a) leaves the real ask unaddressed.

## Resolution Path

Pick (a) / (b) / (c) / (d) and I'll write a separate plan for it. Each is small enough to ship in a single PR. If multiple apply, name them in priority order.