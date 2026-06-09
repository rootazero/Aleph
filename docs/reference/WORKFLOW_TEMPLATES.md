# Workflow Templates

> Declarative, named, re-runnable workflow templates that compile into the
> existing coordination-task DAG. Added 2026-05-29.

## Motivation

Anthropic's [*Building Effective Agents*](https://www.anthropic.com/research/building-effective-agents)
distinguishes two shapes:

- **Agents** — the LLM dynamically directs its own process. Aleph already has
  this: the Think→Act harness loop, and the **LLM-authored** team task DAG
  (`coord_tasks` created at runtime by a leader agent via `task_create`).
- **Workflows** — LLMs orchestrated through **predefined code paths**. This is
  the piece Aleph lacked: a workflow you can *author once, name, save, and
  re-run* with new inputs.

Gap analysis against the reference project (OpenHands) confirmed OpenHands has
**no** workflow/DAG engine at all (single-agent event loop + keyword-triggered
"microagents" for knowledge injection). Aleph's execution infrastructure
(DAG + dispatcher + harness) already exceeds it; the only real gap was the
declarative **template** layer.

## Design — connect, don't rebuild

A template never gets its own scheduler or reasoning. It is pure data that
**compiles down** to the infrastructure that already exists:

```
WorkflowDef (template)  ──compile──▶  coord_tasks (blocked_by edges)
                                              │
                                     TeamDispatcher (R10 dumb loop)
                                              │
                                     Orchestrator → AgentHarness (one run per step)
```

| Concern              | Reused module                                            |
|----------------------|----------------------------------------------------------|
| Step execution       | `orchestrator` → `harness` (one agent run per step)      |
| Scheduling / concurrency | `teams::dispatcher::TeamDispatcher` (Tokio semaphore) |
| Dependency DAG + cycle check | `agents::swarm::tasks` (`blocked_by`, `dag.rs`)  |
| Prompt chaining (upstream outputs) | `teams::dispatcher::handoff::build_handoff_context` |
| Human-in-the-loop control points | `workflow_step_review` tool (approve/reject/retry/skip) |
| Visualisation        | `team_workflow_canvas` (Obsidian JSON Canvas)            |

This keeps the new code to a schema + a file store + a deterministic compiler
(R10 thin-harness / R7 LLM-sovereignty safe).

## Module layout (`src/workflow/`)

- `def.rs` — `WorkflowDef` / `WorkflowStepDef` schema, `validate()` (unique
  ids, resolvable deps, acyclic via Kahn topo-sort), `render_prompt`.
- `store.rs` — atomic file persistence under `$ALEPH_HOME/workflows/*.json`
  (temp-file + rename), reusing `canvas_io::sanitise_name` for traversal-safe
  filenames.
- `compile.rs` — `materialize(def, input, team_id, &CoordTaskStore)`: creates
  one `coord_task` per step in topological order, mapping `depends_on` →
  `blocked_by`, tagging `{"managed_by": "dispatcher"}` so the autonomous loop
  picks them up.

## LLM surface (R8 — everything is a tool)

The `workflow` builtin tool (`builtin_tools/workflow_tool.rs`) exposes
`save` / `list` / `describe` / `delete` / `run`. `run` loads a saved template,
materialises it onto a team, and signals the dispatcher. Typical flow:

```
team_create(...)                              # host the run
workflow(action='save', definition={...})     # author once
workflow(action='run', name='...', team_id='...', input='...')
```

Each step's `agent` resolves to a team member at dispatch time; the team is
just the execution namespace.

## Performance vs reference

Steps with no mutual dependency run **concurrently** on the dispatcher's
Tokio semaphore (`max_concurrent`, default 4) — true parallel fan-out that the
single-agent OpenHands design cannot express. Rust's type-safe schema rejects
malformed templates before they ever reach disk.

## Deliberately deferred (not dead-coded now)

These are pure I/O wiring with no logic; left for follow-up PRs so this change
stays a tight, fully-wired vertical slice:

- **Gateway RPC family** `workflow.{save,list,describe,delete,run}` (R4/R6
  panel + CLI surface). The tool already gives the LLM full control; the RPC
  family is for direct UI access. Mirror `gateway/handlers/teams.rs`.
- **Panel UI** for browsing/editing templates and watching a run's canvas.
- ~~**`lead_review_required` per-step gate**~~ — **wired** (2026-06). A step
  declared with `review: true` stamps `lead_review_required` into its task
  metadata at materialisation; the dispatcher parks a successful run in
  `WaitingReview` instead of `Completed`, the `TeamNotifier` alerts the
  leader, and `workflow_step_review` (approve/reject/retry/skip) resolves the
  gate. Downstream steps stay blocked until the verdict. Clarify steps reject
  the flag at `validate()` time (no agent run to review).
- **Immediate dispatcher signal on a team that does not exist yet**: `run`
  requires the caller to `team_create` first; auto-provisioning a throwaway
  team from the template's distinct agents is a possible convenience but adds
  coupling — deferred until there's a real consumer.
