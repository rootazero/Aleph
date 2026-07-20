# Phase 5 Task 11 — Skipped (No Migration Needed)

**Date:** 2026-04-20
**Conclusion:** The plan assumed `src/agents/swarm/`, `src/teams/`, `src/agents/sub_agents/` contained ~8 `AgentLoop::new(...)` call sites needing migration to `orchestrator.dispatch(...)`. Verification shows **none** of these modules use `AgentLoop::new` — they compose via their own coordinators/buses/drivers without touching the agent_loop crate directly.

**Grep verification:**
```
$ grep -rn "AgentLoop::new" src/ --include='*.rs' | grep -v "src/agent_loop/"
src/gateway/execution_engine/run_loop.rs:628:            let mut agent_loop = AgentLoop::new(
```
Only one hit, already marked `// PHASE-6-LEGACY` by Task 10.

**Exit Criterion 9 status:** satisfied with 1/5 `PHASE-6-LEGACY` budget used.

**Phase 6 follow-up (out of Phase 5 scope):**
- teams/swarm/sub_agents coordinators may dispatch to sub-agents via their own SessionDriver / AgentRuntime layer. Unifying them to route through `Orchestrator::dispatch` is Phase 6 work, not a correctness fix.

**Action:** skip Task 11 — no code change needed.
