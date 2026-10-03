# Final fix B1 — harness spawn run-context propagation

## Root cause

`run_agent_loop` publishes `EXEC_WORKSPACE`, `CURRENT_FS_SCOPE`, and `RUN_PENDING_MEDIA`, but `Orchestrator::dispatch` crosses a bare `tokio::spawn`. Tokio task-locals do not propagate through that boundary. The existing hand-written harness scope stack restored project, attribution, originator, and transcript context, but omitted these three values.

Consequences inside gateway-backed harness runs:

- command sandbox cwd/jail and `writable_roots` lost the gateway-authorized workspace;
- file tools lost the per-run filesystem base/rebase;
- model-initiated media harvesting lost the channel delivery buffer.

## Interrupted diff review

The inherited `dispatch.rs` diff had the right production direction, but was not valid/minimal: it duplicated the same 100+ line test (duplicate function name), carried excessive explanatory prose, and could not compile. I restored the file to `HEAD` and rebuilt B1 test-first.

## Change

- Capture `current_exec_workspace()`, `fs_scope::current()`, and `current_pending_media()` immediately before the harness spawn.
- Re-establish all three around `harness.run`, preserving every pre-existing scope.
- Add one table-driven source guard that pins the complete wire for each value: gateway publisher → pre-spawn capture → spawned harness wrapper.

No harness files changed.

## Regression evidence

RED before production change:

- `orchestrator::dispatch::outcome_tests::the_harness_spawn_carries_gateway_run_context`
- failed on missing pre-spawn `current_exec_workspace()` capture.

GREEN after production change:

- `orchestrator::dispatch::outcome_tests`: 16 passed
- `sandbox::context::tests`: 5 passed
- `tools::fs_scope::tests`: 8 passed
- `gateway::media::tests`: 7 passed

Static checks:

- `rustfmt --check --edition 2021 src/orchestrator/dispatch.rs`: passed
- `git diff --check`: passed
- `git diff 8c467e05d -- src/harness/`: empty

## Not done / residual risk

- Did not launch `aleph-server` or perform real-machine gateway QA.
- Did not run the full crate/workspace suite; validation is intentionally bounded to the changed boundary and the three context APIs.
- The spawn still uses an explicit nested list of task-locals; future run-wide task-locals must be added deliberately and guarded similarly.
