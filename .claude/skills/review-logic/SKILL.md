---
name: review-logic
description: Logic-level code review checklist (Aleph local skill). STUB — fill in concrete trigger phrases, scope, and step-by-step procedure before relying on this skill. See docs/rust-logic-audit-prompts.md for seed material.
---

# Review Logic (Stub)

This skill is a placeholder so the project's local skill registry can load
the directory without YAML-frontmatter errors. It does not yet describe a
runnable workflow.

## TODO

- Decide the trigger surface (slash command name, voice aliases, proactive
  invocation rules) — currently nothing dispatches here.
- Define the review checklist that distinguishes "logic-level" review from
  the generic `code-review` skill (correctness, invariants, state machines,
  data-flow assumptions, error propagation, R7 LLM-sovereignty checks).
- Wire it to `docs/rust-logic-audit-prompts.md` as the canonical prompt
  source, or inline a condensed version here.
- Add example invocations and expected output format.

Until those are filled in, prefer the existing `rust-logic-audit` skill or
`code-review` plugin agents for actual logic review work.
