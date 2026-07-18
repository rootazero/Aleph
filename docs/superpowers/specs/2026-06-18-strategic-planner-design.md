# Strategic Planner — Design Spec

> Status: **design approved, ready for implementation plan**
> Date: 2026-06-18
> Provenance: StraTA (Strategic Trajectory Abstraction, agentic-RL paper) **application-layer** pattern, hardened by a 9-agent verify+critique workflow against the real Aleph codebase.

---

## 0. 一句话 (TL;DR)

在 `/goal`·`/loop`·`/workflow` 这三条长任务入口的**最顶端、任何工具调用之前**，加一个**独立、无工具的 LLM 战略规划节点**：它产出一段简短结构化的 **Strategy**，被**焊进**后续所有执行节点的 system prompt（稳定可缓存前缀），让长任务"开始前先画地图、过程中不忘初心"。

This imports StraTA's *structural* move (plan-first, then weld the plan into every downstream turn) **without** StraTA's RL training (which would violate R7). The "Hierarchical Context Caching" synthesis decides *where* to weld: the Strategy sits in the **stable, prefix-cacheable** portion of the prompt so its KV-cache is reused across every turn of the long task.

**Two papers, same nickname, different topics — for the record:** the design intent comes from `StraTA = Strategic Trajectory Abstraction` (the explainer markdown). The `Strata: Hierarchical Context Caching for Long Context LLM Serving` PDF (arXiv 2508.18572, cs.DC) is an unrelated KV-cache *serving-systems* paper; it only informs the *placement/caching* decision here.

---

## 1. Problem & Goal

**Problem (StraTA's "健忘症"):** reactive agents make local, next-token decisions and drift off the original objective on long horizons ("走一步忘一步"): short-sighted exploration, meaningless back-and-forth, internally inconsistent behavior.

**Goal:** for long/complex tasks, generate a high-level Strategy up front and pin it as system-level context for the whole task, so the executor stays anchored to the objective and to a small set of concrete anti-distraction guardrails.

**Success criteria for this feature:** on the three long-task flows, the welded Strategy demonstrably (a) survives the entire task (does not scroll out / dilute), (b) carries concrete guardrails that can actually fire on a distractor, and (c) costs **one** extra LLM call per *task* (not per turn) plus a cache-discounted prefix render.

---

## 2. Scope & Non-Goals

### In scope (v1)
- Planner fires **only** for `/goal`, `/loop`, `/workflow` (never ordinary chat).
- Welding reaches **all** execution nodes of those flows, including subagents and workflow-DAG steps (explicit threading seams — see §7).
- A welded **Strategy** artifact (§3), a one-shot **Planner** (§4), a **StrategyStore** (§6), two prompt **layers** (§5), a `strategy` **tool** for the revise escape-hatch (§8), and `[strategy]` **config** (§9).

### Non-goals (explicit)
- **StraTA's RL training** (hierarchical GRPO / farthest-point diversity sampling / self-audit reward). That is model-training, not application-layer; adopting it would build a deterministic reward pipeline (anti-R7). We take only the markdown's own "应用层" recommendation.
- **Auto-detecting "complex requests"** outside the three flows. Scope is explicit; no deterministic complexity classifier anywhere (anti-R10). The planner *itself* may decide a task is too trivial and emit no Strategy (an LLM judgment, R7-aligned).
- **Multi-strategy diversity sampling / N-pick-1.** Future work.
- **`revise`-legitimacy judgement in code.** The `strategy revise` tool is a dumb schema-validate-and-overwrite; "high-friction" lives in prompt discourse only, never as a Rust gate/counter/classifier (anti-R10).
- **Planner pre-committing to specific tools/args.** Phases are outcome-phrased; tool selection stays 100% the executor's runtime job (R7/R10).

### Future-Proof justification (required by R10)
R10's binding gate: *"swap in a stronger model → performance rises → no harness code changes."* The scaffolding (store/layers/tool/config) passes trivially. The **planner call** is the one component a stronger model could make redundant, so we justify it explicitly:

> The welded Strategy is framed as a **model-independent context-engineering win** (KV-cache prefix reuse + attention anchoring of a small concrete guardrail set), **not** a reasoning-deficit patch. Goal-drift on very long horizons is a property of long-context attention, not only of model weakness, so anchoring survives model upgrades. **If** a future model empirically shows zero drift on these flows, the feature retires via a single config flip (`[strategy].enabled=false`) — no code change. This keeps the harness-thinning escape valve open and is exactly why §9 adds the `enabled` flag.

### R9 argument (the redline this most tensions)
One extra LLM call **per task, not per turn** (the planner is *above* the loop, not in it). Per-turn cost is only the cache-discounted stable-prefix render of a length-bounded `<strategy>` envelope + a short dynamic guardrail echo. The planner call and provider build are **fully fail-soft**: any failure ⇒ no Strategy stored ⇒ byte-identical prompt ⇒ command proceeds.

---

## 3. The Strategy artifact (content contract)

A short, lightly-structured object. **The guardrails field is the StraTA secret sauce** and carries the fine resolution; phases stay coarse.

| Field | Meaning | Rules |
|---|---|---|
| `objective` | one-line north star | restate the user's end goal |
| `approach` | the chosen overall play | advisory ("initial plan, adapt as you learn") |
| `phases` | coarse, ordered arc (NOT a tactical TODO) | outcome-phrased ("understand the failure" / "implement" / "verify"); **must not** name tools or arg shapes; tactical sequencing belongs to ExecutionPlan/scratchpad |
| `guardrails` | 1–3 **concrete, named, observable** distractors to avoid | **CONTRACT:** each must name a specific distractor tied to this task's real capability surface and be violable by a concrete next action. Reject tautologies ("stay focused", "avoid scope creep"). Seed from project redlines (Surgical Changes, YAGNI, R3). Phrase as scope-positive/observable where possible. Framed **advisory**, not hard prohibitions — the model stays sovereign over moment-to-moment relevance. |
| `success_criteria` | semantic/human success statement | **references** the existing objective gate (`gate_command`/`gate_outcome`), never re-implements verification |

**Planner self-gating:** if the planner cannot produce at least one *concrete* (non-tautological) guardrail, it stores **no** Strategy. A `None` Strategy leaves the prompt byte-identical (strictly better than welding noise). This self-limits the feature to genuinely long/complex tasks and makes trivial `/loop` polling naturally yield no Strategy — an **LLM** judgment, not a code classifier.

**Length budget:** the rendered `<strategy>` envelope is hard-clamped (mirror the existing `prompt_budget` char-clamp discipline) so a runaway planner output can't bloat the cacheable prefix / crowd the high-salience prompt head.

---

## 4. The Planner node (军师)

### Fire-points (once per task, before any tool call)
| Flow | Site (verified) | Notes |
|---|---|---|
| `/goal` | `GoalTool::call` → `GoalAction::Set`, **after** `self.store.put(&goal)?` (`src/builtin_tools/goal.rs:250`), before `Ok(GoalOutput)` | only goal-creation site; pursuit spins later in `execute.rs`, so "before pursuit" is automatic. **BLOCKER:** `GoalTool` holds only `store + session_key` (`goal.rs:79-84`) — must inject a planner provider handle. |
| `/loop` | `LoopTool::start` at `LoopState::new` + `registry.put` (`src/builtin_tools/loop_manage.rs:157-192`) | `LoopTool` already holds a session handle (`loop_manage.rs:105`). |
| `/workflow` | `WorkflowArgs::Run` at/just-before `workflow::materialize` (`src/builtin_tools/workflow_tool.rs:569`) | planner sees `input` + loaded `WorkflowDef`; Strategy is run-wide, minted once beside `run_id` (`src/workflow/compile.rs:115`). |

**Fire-exactly-once + guard:** before any planner call, check `strategy_store.get(key)` — if present, skip. Continuations only **read** via `active_strategy`, never re-plan. (Prevents the `execute.rs` continuation-hook re-fire trap and prefix-cache churn.)

### What the planner sees (tool-FREE)
User request + objective + **curated tool *descriptions*** (not just names — for the shortlist actually available to this run/agent/device-tier, so the plan is grounded in real capability and can't pre-commit to unavailable tools) + light env (OS/cwd) + for `/goal`, existing `goal.lessons`. It is told: *"these are the only capabilities; do not assume others; do not name specific tool calls."* It **cannot** call tools.

### Provider wiring (`[strategy] planner_model`)
Mirror the compaction `cheap_provider`/`summary_model` pattern **exactly except**:
- **Default = same model as executor** (planning is reasoning-heavy). **Do NOT** mirror summary_model's tier-2 `default_aux_model` fallback (`deps_builder.rs:819-824`) — that would silently downgrade the strategist to a flash model.
- Keep the same-as-primary no-op guard (`deps_builder.rs:828-834`).
- **Fail-soft to `None`** on every path (missing section, unset/empty model, same-as-primary, `create_provider` error → `tracing::warn` → `None`). `None` ⇒ planner reuses the executor's main provider; total failure ⇒ no Strategy, command proceeds.

---

## 5. The Weld (prompt pipeline)

### Cache mechanics (CORRECTED — verified)
The cacheable prefix is **not** a priority scan. `build_system_prompt_cached_with_mode` runs **two passes** (`src/thinker/prompt_builder/cache.rs:78-106`): `execute_stable_with_mode(Cached)` → `SystemPromptPart{cache:true}`, then `execute_dynamic_with_mode(Cached)` → `SystemPromptPart{cache:false}`. **A layer lands in the cacheable prefix iff `stability()==Stable`.** Priority only orders *within* each partition.

Hard invariant (`prompt_pipeline.rs:870-898` `stable_layers_come_before_dynamic` **panics** if a Stable layer outranks any Dynamic layer): Stable band = `50..1600`, Dynamic band = `1700..1760`. So a Stable layer needs a priority `<1700`.

`fit_dynamic_suffix` (`cache.rs:84-95`) trims only the dynamic suffix; the Anthropic breakpoint sits at the stable/dynamic boundary, so a per-turn-changing dynamic tail **does not** break the prefix hash. Welded `<strategy>` rides the KV-cache across turns.

### Two new layers
| Layer | File | `stability()` | `priority()` | `paths()` | Renders |
|---|---|---|---|---|---|
| `StrategyLayer` | `src/thinker/layers/strategy.rs` | **Stable** | **~70** (after `curated_memory@60`, before `profile@75`) | **must incl `AssemblyPath::Cached`** (full 5-set `[Basic,Hydration,Soul,Context,Cached]`) | full `<strategy>` envelope, **verbatim, rendered once** |
| `StrategyPointerLayer` | `src/thinker/layers/strategy_pointer.rs` | **Dynamic** | **~1756** (after `standing_goal@1754`/`execution_plan@1755`, before `session_resume@1760`) | **must incl `Cached`** | the 1–3 **guardrail lines verbatim** (<40 tokens), near the read head |

- **`paths()` MUST include `Cached`** or the layer compiles, passes Basic/Soul/Context tests, and **silently vanishes in production** (the documented RoleLayer/CitationStandards vanish bug, guarded by `cache.rs:187-206`).
- **Tail content = guardrails verbatim** (not a content-free pointer). Pointing back up a long prompt is the operation drift already fails at; restating the concrete constraint near the read head is what fights dilution.
- **De-dup vs StandingGoal for `/goal`:** `StandingGoalLayer@1754` already re-injects the objective every turn. The tail emits **only** guardrails (no objective) to avoid three near-identical end-of-prompt reminders → reminder-blindness.
- **Both layers use the 3-guard empty-path inject** (copy `standing_goal.rs:43-56` verbatim): `input.context` None → return; `ctx.strategy` `as_deref` None → return; `.is_empty()` → return. A `None` Strategy leaves the prompt **byte-identical at head AND tail** (critical: these layers run on the default pipeline for *every* chat turn).
- **Determinism:** `render_strategy_summary` is **pure and deterministic** — no timestamps, no `HashMap` iteration order (sorted/`Vec` fields), no `now_ms` in the Stable body (unlike `render_goal_summary` which intentionally stamps a deadline — that belongs only in dynamic surfaces). Render once, inject verbatim, mirroring `curated_memory_envelope` (`curated_memory.rs:38-44`).

### Full file-touch checklist (verified)
1. `src/thinker/layers/strategy.rs` (new) + `src/thinker/layers/strategy_pointer.rs` (new) — mirror `standing_goal.rs` (+`#[cfg(test)]` block).
2. `src/thinker/layers/mod.rs` — `mod` decls + `pub use` (near the standing_goal block, lines 37-43).
3. `src/thinker/prompt_pipeline.rs` — (a) add to `use super::layers::{…}` import block; (b) `Box::new(StrategyLayer)` + `Box::new(StrategyPointerLayer)` into `default_layers()` (order cosmetic — sorted by `priority()` at `:67`); (c) update the priority doc (`:274-317`); (d) **bump count asserts**: `:557` `layer_count` 40→**42**; `:930-934` `dynamic_names.len()` 14→**15** + `assert!(dynamic_names.contains(&"strategy_pointer"))`. Verify Compact/Minimal mode tests (`:599-669`) still pass (`supports_mode = mode != Minimal`, same as standing_goal).
4. `src/thinker/context.rs` — add `#[serde(skip, default)] pub strategy: Option<String>` to `ResolvedContext` (after `standing_goal`, `:179-183`) **and** add `strategy: None` to the single exhaustive literal in `ContextAggregator::resolve` (`:258-268`) — **missing this is a hard E0063**.
5. `src/orchestrator/harness_bridge/context_blocks.rs` — `pub async fn active_strategy(session_key: &str) -> Option<String>` (mirror `active_standing_goal`, `:34-44`, fail-soft) + pure `pub(crate) fn render_strategy_summary(&Strategy) -> String` (mirror `render_goal_summary`, `:54-79`; **no** `now_ms`).
6. `src/orchestrator/harness_bridge/prompt_build.rs` — extend the existing `tokio::join!` (`:388-391`) from 2-way to **3-way** (`active_strategy(&session_key_str)` shares the borrow) + `resolved_context.strategy = strategy;`.
7. `src/orchestrator/harness_bridge/mod.rs` — add `active_strategy` to the `pub use context_blocks::{…}` re-export (`:44-46`).

---

## 6. StrategyStore

Mirror `src/goal/` store **shape** (new `src/strategy/`: mod + types + store): SQLite, `Mutex<Connection>`, `ON CONFLICT … DO UPDATE` upsert, fail-safe corrupt-row handling (`get` → `Ok(None)` on bad JSON), process-global `OnceCell` + `init_global`/`global`. **Persistent** (survives `/resume` and daemon restart, matching goal/workflow).

### Key (CORRECTED — composite, not bare session_id)
**Key by composite `{session_id, flow_kind}`** where `flow_kind ∈ {goal, loop, workflow:<run_id>}`. Rationale (CRITICAL bug avoided): `GoalStore` PK and `LoopRegistry` both key by the bare `session_id` (`goal/store.rs:26`); a session running `/goal` **and** `/loop` concurrently would have the second planner's upsert **silently overwrite** the first's Strategy. Composite keying also prevents a stale goal-Strategy bleeding into a later plain-chat turn in a reused session (plain chat has no active `flow_kind` to look up). Store `goal.id` (FNV of `session_id:objective`, `types.rs:99-102`) as a cross-ref to auto-invalidate on objective change.

### Lifecycle (clear in lockstep with authoritative end-points only)
- **Clear on:** goal explicit `Clear` (`goal.rs:325`, authoritative deletion) · loop stop (`loop_manage.rs:194`) · optionally gate-confirmed Complete (`execute.rs:699`).
- **Do NOT clear on** transient `Blocked` (`execute.rs:787-790`, `:1139-1148`) — a blocked goal may resume via `goal(update, status='active')`; the welded Strategy must survive.
- **Auto-invalidate** when the objective string changes (compare stored `goal.id`).

### Concurrency
Atomic upsert + single-`get()` reads (a turn sees old-or-new, never torn). For workflow use the **metadata-threading** seam (immutable per-task stamp), not a shared store, so concurrent DAG nodes can't race. No cross-store transaction coupling Goal↔Strategy (over-engineering); fail-soft to `None` on a missing/corrupt row exactly like `active_standing_goal`.

---

## 7. Propagation seams (v1: goal + loop + workflow + subagent)

**Verified reality:** a `StrategyLayer` in the default pipeline reaches **normal runs, loop ticks, and goal continuations** (same session, `spawn_continuation_run` reuses `session_key`) — but **NOT** subagents (inline prompt) and **NOT** workflow members (fresh task session). So:

| Path | Seam | Detail |
|---|---|---|
| `/goal`, `/loop`, goal continuations | **session (automatic)** | `active_strategy(session_key)` + `StrategyLayer` in main pipeline. No threading. |
| `/workflow` DAG steps | **metadata (explicit)** | Stamp a `workflow_strategy` key onto each `CoordTask` metadata in `src/workflow/compile.rs::materialize` (beside `WORKFLOW_RUN_ID_KEY`/`WORKFLOW_MODEL_KEY`, `:171-196`); render a `## Global Strategy` section from `task.metadata` in `src/teams/dispatcher/handoff.rs::build_handoff_context` (after the `## Task` block). Byte-for-byte the proven `model_override` pattern (`schedule.rs:515`). |
| subagents | **SpawnRequest (explicit)** | Thread `strategy: Option<String>` through `AgentRuntimeConfig` (`runtime.rs:49-60`) → `SpawnRequest` (`subagent_spawner/mod.rs:99-119`) → inject into the inline `PromptBuilder` at `mod.rs:269`. **Do NOT reuse `context_summary`** (lands in the first UserMessage, gated on `ContextMode::Summary`, mutable transcript — fails the weld/immutability requirement). |

### Workflow weld = global-frame only (heterogeneous DAG)
Workflow steps have different local objectives (research vs implement). Weld **only** the run-global `objective` + cross-cutting `guardrails`, **labeled** `## Global Strategy (context — your specific task is below)`. The per-node task description (already assembled from `step.prompt`) is the **authoritative local instruction, placed after and given priority**. **Drop the coarse phase list** from the per-node weld (the DAG *is* the phase structure). Optionally let a step opt out of conflicting global guardrails via a `compile.rs` metadata flag, so a global "don't write code yet" guardrail can't forbid the implement node's actual job.

---

## 8. The revise escape-hatch (泰森那一拳)

`strategy` tool (R8: everything-is-a-tool), **two actions only**: `revise { reason, new_strategy }` and `show`.

- **Dumb write only:** the tool validates schema (reason non-empty, `new_strategy` parses) and **overwrites** the store. **All** judgement about whether to revise lives in the LLM via the tool's DESCRIPTION text (R9: intelligence in the prompt).
- **"High-friction" = prompt discourse**, never code: the tool description + StrategyLayer text say *"default: hold the Strategy; revise only on genuine environment shock that invalidates the high-level approach; tactical changes go through your scratchpad."* **Never** a deterministic gate, turn-counter, similarity score, or accept/reject classifier.
- **Non-goal (explicit in spec):** *the revise tool MUST NOT contain logic that evaluates the legitimacy of a revision.* Mirrors goal `Update` (a dumb store write; "completion is the model's explicit call, there is no judge LLM").
- A revise mutates the Stable prefix → **one** prefix-cache miss next turn (correct-by-design, kept rare by friction). Day-to-day tactical adaptation goes through ExecutionPlan/scratchpad, not here.

---

## 9. Config (`[strategy]`)

New `StrategyToml` in `src/config/types/phase6_wiring.rs` (alongside `ContextBudgetToml`, same derives/serde shape):
```toml
[strategy]
enabled = true            # opt-in/off-switch — A/B + Future-Proof escape valve
planner_model = "..."     # optional; default = executor model
```
- `enabled` (mirror `[context_budget].enabled`): one-flip off-switch.
- `planner_model: Option<String>` (mirror `summary_model`, `phase6_wiring.rs:143-144`).
- Top-level `Config.strategy: Option<StrategyToml>` in `src/config/structs.rs` (mirror `context_budget`, `:232-233`); re-export `StrategyToml` from `src/lib.rs:153-155`.
- `build_strategy_planner_provider(config, primary_provider_key) -> Option<Arc<dyn AiProvider>>` in `src/orchestrator/deps_builder.rs` (next to `build_cheap_summary_provider@801`): clone primary `ProviderConfig`, swap `models` vec, `create_provider`, **tier-1 only**, **no** `default_aux_model`, same-as-primary no-op guard, fail-soft to `None`. Re-export via `src/orchestrator/mod.rs:21`.
- Built **once** in the goal/loop/workflow start path (NOT on `AgentHarnessRunner` — planner lives above the loop, R10). `primary_provider_key` is already a param of `initialize_orchestrator` (`orchestrator_init.rs:43`).
- **E0063 audit:** a brand-new `StrategyToml` introduces no exhaustive-literal break. Always write test literals with `..StrategyToml::default()`.

---

## 10. Redline analysis

| Redline | Verdict | Note |
|---|---|---|
| **R7 LLM sovereignty** | ✅ | planner = genuine LLM reasoning; revise = LLM-authored; no deterministic middleware replacing reasoning. |
| **R8 everything-is-a-tool** | ✅ | `strategy` tool for revise/show. |
| **R9 zero extra call / no middleware tax** | ✅ (scoped) | one call per *task*, above the loop; per-turn cost = cache-discounted prefix + short tail; fully fail-soft. Argument documented in §2. |
| **R10 thin harness / dumb loop** | ✅ (conditional, closed) | `src/harness/` untouched; planner above the loop; the "5 nots" not tripped; layers are verbatim renders. Future-Proof Test answered (§2) + `enabled` flip escape valve. revise pinned to dumb-write (§8). |
| **P6 KISS/YAGNI** | ✅ | tool has only revise/show; no speculative config beyond `enabled`+`planner_model`; self-gating limits scope. |
| **P7 defensive** | ✅ | fail-soft planner/provider; 3-guard byte-identical empty path; atomic upsert; UTF-8/char-safe length clamp. |

**Redline-adjacent risks explicitly mitigated:** generic guardrails (→ content contract + self-gating + store-nothing); frozen "ignore X" hardening into self-imposed refusal (→ guardrails advisory-framed, model stays sovereign, tactical deviation via scratchpad); revise as latent cognition seam (→ dumb-write non-goal).

---

## 11. Testing

**Empty-path first (the single most important regression guard):** for BOTH layers — `no_strategy_emits_nothing`, `empty_strategy_emits_nothing`, `missing_context_emits_nothing` — assert byte-identical-to-today output. Write these before any fire logic.

**Unit (host, no LLM):**
- `render_strategy_summary` pure + **deterministic** (same input → identical bytes across two renders; no timestamp/HashMap-order).
- `StrategyStore` round-trip put/get/revise/clear, **composite-keyed**.
- `active_strategy` fail-soft (missing/corrupt row → `None`).
- `build_strategy_planner_provider`: none-when-section-missing / none-when-unset / some-when-set-and-different / none-when-same-as-primary / none-when-create_provider-fails.

**Integration:**
- **Production-path** (mirror `cached_full_prompt_carries_role_and_citation_standards`, `cache.rs:187-206`): with a Strategy present, `parts[0]` (stable) contains the `<strategy>` body **and** `parts[1]` (dynamic) contains the guardrail echo — catches both missing-`Cached` vanish and wrong stability in one test.
- **Composite-key no-clobber:** set goal-Strategy, start a loop in the same session, assert the goal row is untouched.
- **Fire-once:** two goal continuations → planner invoked **at most once** → Strategy bytes identical across both continuation prompts.
- **Provider-None fail-soft:** planner provider `None` → goal `Set` still succeeds with no Strategy.
- **Stale-leak:** complete a goal (gate-confirmed) → `active_strategy` returns `None` for the next ordinary turn in that session.
- **Workflow:** `Run` → `task.metadata` carries `workflow_strategy` → `build_handoff_context` renders the labeled global-frame block; per-node task description follows and dominates.
- **Subagent:** spawn carries `strategy` → child inline prompt welded.
- bump-and-pass: `layer_count==42`, `dynamic_names.len()==15`, Compact/Minimal mode tests.

**Behavioral / E2E (user-run):** a canned long task with a distractor (the "买酱油遇打折薯片" pattern); verify on-goal behavior with Strategy+guardrails vs. a baseline without. Optionally log a cheap **drift proxy** (tool calls / files touched outside the objective's blast radius, or iterations-to-completion for `/goal`) with strategy on vs off, so benefit is observable and the `enabled` off-switch has a metric.

---

## 12. Build order (for the implementation plan)

1. `src/strategy/` (types + composite-keyed store + global) — the prerequisite module.
2. `[strategy]` config + `build_strategy_planner_provider` (fail-soft).
3. The Planner call (tool-free, self-gating, fire-once guard) + provider injection into `GoalTool`/`LoopTool`/`WorkflowTool` start paths.
4. `ResolvedContext.strategy` + `active_strategy` + pure `render_strategy_summary` + 3-way `join!`.
5. `StrategyLayer` (Stable) + `StrategyPointerLayer` (Dynamic, guardrails verbatim) + pipeline registration + **count-assert bumps** + empty-path tests **first**.
6. Propagation seams: workflow metadata→handoff (global-frame) + subagent SpawnRequest→inline PromptBuilder.
7. `strategy` tool (revise/show, dumb-write).
8. Lifecycle clears (goal Clear / loop stop / optional gate-complete; not on Blocked) + objective-change auto-invalidation.
9. Tests green; user-run E2E.

---

## 13. Open items deliberately deferred to v2
- Multi-strategy diversity sampling (StraTA's farthest-point selection of N candidate strategies).
- A richer drift metric / dashboard.
- Per-step guardrail opt-out flag for workflow nodes (ship only if §7's conflict shows up in practice).
