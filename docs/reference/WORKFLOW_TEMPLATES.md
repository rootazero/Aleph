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
  ids, resolvable deps, acyclic via Kahn topo-sort), `render_prompt` /
  `scan_prompt` / `RunInputs` / `referenced_vars` (see *Run args* below).
- `store.rs` — atomic file persistence under `$ALEPH_HOME/workflows/*.json`
  (temp-file + rename), reusing `canvas_io::sanitise_name` for traversal-safe
  filenames.
- `compile.rs` — `materialize(def, &RunInputs, team_id, &CoordTaskStore, …)`:
  creates one `coord_task` per step in topological order, mapping
  `depends_on` → `blocked_by`, tagging `{"managed_by": "dispatcher"}` so the
  autonomous loop picks them up, and stamping the per-step pins
  (`StepPins` — model / effort / phase / output contract) plus the
  `tolerate_failed_deps` flag when a step sets it.

## LLM surface (R8 — everything is a tool)

The `workflow` builtin tool (`builtin_tools/workflow_tool.rs`) exposes
`save` / `list` / `describe` / `delete` / `run` / `status` / `runs` /
`rerun_failed` / `cancel` / `pause` / `resume` / `export` / `import` and the
proposal family. `run` loads a saved template, materialises it onto a team,
and signals the dispatcher. Typical flow:

```
team_create(...)                              # host the run
workflow(action='save', definition={...})     # author once
workflow(action='run', name='...', team_id='...', input='...', args={...})
```

Each step's `agent` resolves to a team member at dispatch time; the team is
just the execution namespace.

### Run args — `{{name}}` placeholders (2026-09-03)

A step prompt may reference named values as `{{name}}` (`[A-Za-z0-9_]+`)
alongside the anonymous `{input}` every template has always had. Values are
supplied per run:

```
workflow(action='run', name='audit', team_id='t1',
         args={'region': 'the Arctic', 'topic': 'sea ice'})
```

- **The var list is derived, never declared.** `WorkflowDef::referenced_vars`
  scans the prompts (clarify questions included — an unsubstituted
  placeholder there is shown to a *human*). There is deliberately no
  `vars: [...]` manifest field: a declared list is a second spelling of a
  fact the prompts already state, and it goes stale the first time a prompt
  is edited without it.
- **`describe` / `list` / `proposals` report `vars`** — exactly the keys
  `run` will demand. `list` pays a second `store::load` per row for this
  (the `WorkflowMeta` index carries no prompts); that cost is deliberate.
- **`run` fails closed.** A missing arg is refused by name, not rendered:
  leaving the literal `{{region}}` in the prompt ships the *question* to the
  agent as if it were the instruction (P7).
- Both forms are resolved in **one pass** (`def.rs::scan_prompt`). A value is
  never re-scanned, so an arg whose text happens to contain `{input}` cannot
  be expanded a second time — no template injection through a run's own data.
  A `{{` that opens no valid name is copied through untouched, so prompts
  containing JSON or shell brace expansion survive verbatim.

### Tolerant fan-in — `tolerate_failed_deps` (2026-09-03)

By default a dependency edge means "I need this step's output", so a
`Failed`/`Cancelled` upstream leaves the dependent permanently
`Unsatisfiable`. A **synthesis / report / cleanup** step can opt out:

```json
{ "id": "synthesise", "agent": "writer", "prompt": "...",
  "depends_on": ["scan_a", "scan_b"], "tolerate_failed_deps": true }
```

- **Scope is narrow and per-step**: it tolerates only that step's **direct**
  dependencies, and only for that step. It is not inherited down the DAG; a
  step whose direct parent is itself unsatisfiable stays blocked.
- **What the downstream prompt sees**: `handoff::render_dependency` renders
  the dead upstream as `### <subject> — failed` (`— was cancelled` for a
  cancelled one) and hands over the recorded
  error text explicitly labelled as a **missing input, not as output** — the
  member is told which input it did not get and why, instead of being
  launched silently short one input. (Test:
  `a_failed_dependency_is_named_as_a_missing_input_not_as_output`.)
- **Readiness lives in the task store**, gated on the
  `tolerate_failed_deps` metadata stamp `materialize` writes only when the
  flag is set — an unstamped task's row and behaviour are byte-identical.
  `CoordTaskStatus::satisfies_dependency` is deliberately unchanged: a failed
  producer still satisfies nobody; the *consumer* merely stops waiting. See
  [MULTI_AGENT_SYSTEM.md](MULTI_AGENT_SYSTEM.md) *Tolerant fan-in* for the
  three derivation sites.
- `validate()` rejects the flag on a clarify step (there is no agent run to
  tolerate a failure of).
- A tolerant dependent flips to `Pending` on the **next dispatcher tick**
  after its last live dep dies; `fail_task` emits no extra signal for it.

### Reading and re-running: `status` / `runs` / `rerun_failed`

- `status{include_output: true}` returns each step's bounded output
  (`MAX_STEP_OUTPUT_CHARS = 1200`). **`output` is gated on the step's
  status** — only `Completed` and `WaitingReview` produce one, the same way
  `error` is gated on `Failed`. `task.result` is the task's single free-text
  slot and the dispatcher also writes it on a retry-scheduled `Pending`
  (`"retry 1/3 in 8s after: …"`) and on `Cancelled` (`"cancelled: …"`);
  ungated, those diagnostics reached the model under a field documented as
  "the step's recorded output".
- `runs{name, team_id}` lists every run of a template on a team, newest
  first: `{run_id, started_at, steps, settled, summary}`. `started_at` is
  `min(created_at)` (the run's *birth*, not its latest activity) and
  `settled` reads `CoordTaskStatus::is_settled()` rather than hand-listing
  terminal statuses. **Zero runs is an empty list, not an `Err`** — "never
  run on this team" is an answer this face can give.
- `rerun_failed{name, team_id, run_id?}` re-queues a run's `Failed` steps
  plus every step left `Unsatisfiable` by one: retry budget reset
  (`retry::with_retry_budget_reset_at`), status back to `Pending`, `result`
  cleared, any leftover lock released with its *actual* holder, then one
  dispatcher signal. Completed steps keep their results and are not re-run
  (including a tolerant step that already ran). It deliberately does **not**
  stamp `workflow_notified`: the settle sweep re-arms the terminal
  notification the moment the run stops being fully settled, and stamping
  would suppress the summary for the re-run.
  ⚠️ Implementation note: the target set is selected from **one snapshot
  before any write**, because `Unsatisfiable` is *derived* — the first write
  makes the downstream rows read `Blocked` instead.

### `save` refuses to overwrite a file it cannot read

`store::load` returns `Err` both for "no such file" and "the file is there
and did not parse", and a step carries `deny_unknown_fields`, so one typo'd
key makes a template unreadable while it still holds every
model / effort / schema / phase / whenToUse the user authored. `save`
therefore probes `resolve_path_at(..).exists()` first: path absent →
`from_def` as before; path present but unreadable → **refuse**, carrying the
parse error plus the two ways out (`delete`, or `import save=true`). Reading
the fail-closed answer as "there is nothing to preserve" silently deleted
every extra, with a success message byte-identical to a first-ever save.

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
