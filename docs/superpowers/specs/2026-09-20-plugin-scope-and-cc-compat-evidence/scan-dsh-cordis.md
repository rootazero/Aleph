# Scan: DeepSeek Harness (dsh) + Cordis — mechanisms Aleph could absorb

Repo: `/Volumes/TBU4/Github/deepseek-harness` (HEAD as of 2026-09-20). All paths below are relative to that root.
Read-only scan. Written incrementally — each `##` section is complete on its own.

Reader orientation: dsh is a pnpm monorepo of ~200 `@deepseek-ai/dsh-*` packages, all mounted as Cordis plugins from a YAML entry tree (`cordis.yml` + patch layers). The kernel (`vendor/cordis`, 2.7k lines TS) owns exactly four things: a **Context** proxy, a **Fiber** (plugin instance lifecycle + disposer list), a **Registry** (`ctx.plugin`/`ctx.inject`), and an **Events** bus with 5 dispatch modes. Everything else — including the agent loop — is a plugin.

---

## 1. Cordis core model as used here

### 1.1 The four kernel objects

| mechanism | where | what it guarantees | Aleph-relevance |
|---|---|---|---|
| `Context` is a `Proxy`; property reads that are not own-properties go through the service resolver | `vendor/cordis/src/context.ts:71-84` (ctor builds `new Proxy(this, ReflectService.handler)`), `reflect.ts:135-206` (the `get`/`set`/`has` traps) | `ctx.tools` resolves to whatever fiber `provide`d `"tools"` **in this isolation label**; reading a name you did not `inject` throws `cannot get property "X" without inject` (`reflect.ts:144`); reading an injected-but-inactive one throws `cannot get required service "X" in inactive context` (`reflect.ts:160`) | Aleph's `AppContext` is a struct of concrete fields. The absorbable part is the **fail-loud read rule**: a consumer that reads a service it never declared is an error, not `None`. (§8 judgment "fail-closed answers consumed as values") |
| `ctx.extend(meta)` — child context via prototype chain, parent never mutated | `context.ts:99-107` | Spatial: a child sees everything the parent sees plus its own shadowing keys; parent unaffected | Maps to Aleph's per-session / per-agent scoping; Aleph does it by cloning `Arc`s, cordis by prototype inheritance |
| `ctx.isolate(name, label?)` — child context where service `name` resolves against a **fresh symbol** | `context.ts:121-125`; store keyed by label symbol at `reflect.ts:209`, `reflect.ts:238-243` (`_getImpl` looks up `store[ctx[isolate][name]]`) | Two subtrees can each have their own `ctx.tools` (or `ctx.llm`) without either seeing the other's; same `label` passed twice **joins** scopes | This is how a per-session agent preset gets "a different tool set" (`docs/architecture.zh.md` → "服务行需要 `isolate` realm"). Aleph's `src/tools/scoped/` does this by `retain`-filtering one registry; cordis does it by having N registries addressed by label |
| `ctx.intercept(name, config)` — child context that merges extra config into `name`'s resolved config for plugins below | `context.ts:139-145`; consumed in `service.ts:86-102` (`[resolveConfig]` walks the intercept prototype chain, ancestors first) | Config overlays compose top-down without the provider knowing who overlaid | Same shape as Aleph's "request > session > global" knob precedence (SESSION_KNOBS.md) but generic over any service |

### 1.2 Fiber = one plugin instance = one disposer list (this IS temporal composability)

| mechanism | where | what it guarantees | Aleph-relevance |
|---|---|---|---|
| Every registration is an **effect** that returns a disposer, collected on the current fiber | `fiber.ts:418-561` (`Fiber.effect`), collected into `this._disposables` (`fiber.ts:203`, `DisposableList` in `utils.ts:5-40`) | Disposers run **in reverse registration order** (`fiber.ts:431`), async disposers are awaited in sequence, calling a disposer twice is a no-op (`fiber.ts:428`), registering on a disposed/unloading fiber throws `INACTIVE_EFFECT` (`fiber.ts:419-422`) | Aleph's plugin host has a lifecycle but registrations (tool registry insert, event subscription, prompt section) are not uniformly "return a disposer". This is the single most portable idea: **`register()` returns `Box<dyn FnOnce>`, owned by the plugin handle** |
| `ctx.on()`, `ctx.provide()`, `ctx.accessor()`, `ctx.mixin()`, `ctx.plugin()` are all implemented **as** `fiber.effect(...)` | `events.ts:254-260` (`register` wraps in `fiber.effect`), `reflect.ts:277-305` (`provide`), `reflect.ts:345-353`, `reflect.ts:364-390`, `fiber.ts:265-297` (child fiber's own dispose is an effect on the **parent** fiber) | Unloading a plugin unwinds: its listeners, its services, its accessors, **and its child plugins** (child dispose is on parent's disposer list) — nothing leaks by construction | This is what "temporal composability" means in code: unload = run the list. No plugin needs an `on_unload` method because it never registered anything outside `effect()` |
| Epoch: a fiber's activation key is the concatenation of the uids of the fibers that provide its `inject`ed services | `fiber.ts:611-623` (`_refresh` builds `epoch = ':uid1:uid2…'`), `fiber.ts:625-639` (`_setEpoch` flips LOADING/UNLOADING) | If **any** dependency is re-provided by a different fiber (provider swapped, reloaded), every dependent is **unloaded and re-run** automatically (`reflect.ts:314-336` `notify` → `_checkImpl` → `_refresh`). No manual "restart downstream" | Aleph has no equivalent; provider swap today means restart. This is the mechanism behind "replace a provider from config and the whole product changes" |
| `provide()` disposer waits for **dependents** to unload before dropping the impl from the providing fiber's own store | `reflect.ts:297-303` (`notify` then `Promise.allSettled(fibers.map(f => f.await()))`, then `delete fiber.store[name]`) | Teardown ordering: consumers go first, provider last, so a consumer's disposer can still call the service it depends on | Equivalent Rust discipline: drop order of `Arc<dyn Service>` fields; cordis makes it explicit and awaitable |
| Load/unload are serialized through `inertia` (one in-flight transition per fiber) | `fiber.ts:200`, `646-696` (`_reload`/`_unload` re-check epoch after `await` and chain the opposite transition if epoch flipped meanwhile) | Rapid provider churn never runs plugin code for a stale epoch (`fiber.ts:650-658`) | Aleph's hot-reload for skills/MCP could adopt the "check the epoch after every await" pattern |
| `FiberState` enum: PENDING / LOADING / ACTIVE / FAILED / UNLOADING / DISPOSED; transitions emit `internal/status` | `fiber.ts:147-154`, `581-595` | A plugin whose `inject` cannot be satisfied is **PENDING**, visibly — not silently absent | This is the "declared but never mounted" observable. See §5 |
| Config validated by standard-schema before activation; failure → FAILED with the error stored, `fiber.await()` rethrows | `fiber.ts:50-62` (`resolveConfig`), `fiber.ts:655`, `704-710` | Misconfiguration cannot produce a half-mounted plugin | Aleph already validates config at boot; the addition is "the error is attached to the fiber and awaitable" |

### 1.3 Events bus — 5 dispatch modes + context filter (this IS spatial composability for events)

| mechanism | where | what it guarantees | Aleph-relevance |
|---|---|---|---|
| Five dispatch modes: `emit` (sync fire-and-forget), `parallel` (await all, AggregateError), `serial` (await in order, stop at first bail), `bail` (sync, stop at first non-null), `waterfall` (around-middleware with `next()`) | `events.ts:183-243`; mode is part of the event's **documented** contract (`@mode` JSDoc tag, checked by a generator — `docs/cordis-primer.md` "Dispatch Modes") | A listener knows from the event name whether it is observing, vetoing, or wrapping | Aleph has one event shape (broadcast). The interesting mode for a "dumb loop" is **waterfall**: the loop calls `ctx.waterfall('agent/request', req, next)`; plugins wrap; **not calling `next()` vetoes** (`events.ts:234-243`). This is the exact hook shape Claude Code's PreToolUse `deny` needs |
| Listener admission by **context filter**: dispatch `thisArg[Context.filter](hook.ctx)` decides per-listener | `events.ts:165-175` (`dispatch`), `Context.filter` symbol `context.ts:46` | A listener registered under a scoped context receives only events whose subject admits that scope | This is how one event bus serves N agents without N buses |
| `dsh-scope`: opaque `ScopeKey` tag on a child context + parent chain + `scopeTarget(base, key)` carrier whose filter admits **untagged listeners globally** and **tagged listeners for the key or any ancestor** | `packages/core/scope/src/index.ts:137-147` (`createScope` = `ctx.plugin(noop)` + `extend({[kScope]: key})`), `:170-185` (`scopeTarget`), `:39-59` (`scopeParents`, cycle-checked) | Events flow **up** the scope chain, never down: a supervisor scope sees every child agent's events; a sibling sees nothing | Directly maps to Aleph teams/subagents (`src/agents/`, `src/teams/`): parent agent observes children by scope ancestry instead of by explicit subscription plumbing |
| `ScopedLayers<L>`: one registry keeps a `global` layer + per-scope overlay layers; `chainLayers(scope)` returns ancestors-first so nearest scope wins; `peek` is deliberately chain-blind | `packages/core/scope/src/store.ts:159-200` | "Which tools does agent X see" = union of global + ancestor layers + own layer, computed at read time; registration owns its undo (`NamedEntries.insert` returns idempotent undo, `store.ts:43-54`) | Cleaner than Aleph's three `retain` passes: the layering is data, not filter code |

### 1.4 How the app is composed (profiles, bundles, patches)

| mechanism | where | what it guarantees | Aleph-relevance |
|---|---|---|---|
| The root config is **an empty list**; the whole tree is patch layers: bundle patches (in `dsh.profile.bundles` order) → profile `cordis.patch.yml` → `$DSH_HOME/cordis.patch.yml` → `--patch` overlays → telemetry switch | `apps/cli/src/profile-boot.ts:83-88` (root `[]`), `:206-213` (`allPatches` order), `:226-244` (`composeProfile`) | A patch targets an entry by `id` and replaces its whole `config` or inserts a row; every shipped row is overridable by a later layer; `--dump-config` prints the effective tree | Aleph's `config.toml` is one document. The absorbable idea is **id-addressed rows + ordered overlays**, so a user can disable/replace `session-telemetry-otel` without forking the base list |
| Each patch layer is `structuredClone`d per generation because the include pushes insert rows **by reference** and later patches mutate them in place | `profile-boot.ts:323-335` | Removing a user override reverts to bundle default instead of baking the override into the bundle row | Judgment §1 "two representations of one fact" — they hit it and fixed it at the clone boundary |
| Base composition = 84 rows in `packages/bundle/base/cordis.patch.yml` (`agent-loop` is row 82 of 84 — one plugin among peers) | `apps/cli/composition.md:11-178` (generated graph), `:181-266` (id → package table) | The loop has no privileged position in the tree | See §3 |
| Live reload: only `web` profile watches `cordis.patch.yml`; `headless`/`sdk`/`acp` freeze at boot because "a one-shot or stdio app owns work after boot, and replacing its dependencies would break that lifecycle" | `profile-boot.ts:355-385`, `docs/architecture.zh.md` "Profile 与组合包" | Temporal composability is **opt-in per profile**, not unconditional | Important for §6: they themselves turn hot-swap off for the runtime shapes closest to Aleph's daemon |
| Loader entry options: `id`, `name`, `config`, `group`, `disabled`, `inject`, plus `intercept` and `isolate` (map service → `true` (entry-local realm `#id`) or label (shared realm `@label`)) | `vendor/loader/src/config/entry.ts:9-21`, `vendor/loader/src/config/isolate.ts:6-12`, `:45-67` | A YAML row can say "this subtree gets its own `tools` service" without code | Config-driven `isolate` is what makes "agent preset = a cordis.yml" work (`packages/preset/`) |

### 1.5 One-paragraph definitions (as the code means them)

**Temporal composability** = a plugin can be mounted/unmounted at any time because (a) the kernel forbids any registration path that does not go through `fiber.effect()` (`events.ts:254`, `reflect.ts:278`, `fiber.ts:265`), so the disposer list is complete by construction; (b) unload = `_disposables.clear().reverse()` run in order (`fiber.ts:675-686`); (c) dependents are recomputed from service epochs, so a swap cascades correctly (`fiber.ts:611-623`, `reflect.ts:314-336`). Nothing here is JS-magic except the Proxy; the disposer-list + epoch-cascade are portable.

**Spatial composability** = a plugin sees only its scoped view because (a) service lookup is `store[ctx[isolate][name]]` — the label symbol is inherited down the prototype chain and rebound by `isolate()` (`reflect.ts:238-243`, `context.ts:121-125`); (b) reads of undeclared services throw rather than resolve (`reflect.ts:144-160`); (c) event admission is a per-dispatch filter on the listener's context (`events.ts:171-174`), and `dsh-scope` turns that into an ancestry rule (`scope/src/index.ts:170-185`). The Proxy is the JS-specific part; the label-keyed store and the filter-on-dispatch are portable.

---

## 2. The extension API surface a plugin gets

### 2.1 Three shapes of contribution (every one is a reversible effect)

| shape | how a plugin does it | where | Aleph-relevance |
|---|---|---|---|
| **Provide a service** (`ctx.<key>`) | `class X extends Service { constructor(ctx){ super(ctx,'x') } }` or `ctx.provide('x', impl)` | `vendor/cordis/src/service.ts:42-59`, `reflect.ts:277-305` | Aleph: trait object in `AppContext`. Missing piece is the "consumer declares `inject`, kernel gates activation on it" |
| **Register into a service's registry** (tool / prompt section / adapter / command / provider) | `ctx.tools.register(def)` → returns disposer; **every `register()` in the repo returns the disposer** (AGENTS.md "Registrations are effects") | `packages/core/tools/src/index.ts` (`ToolRuntime.register`), `packages/interaction/commands/src/index.ts:285`, `packages/subagent/subagent/src/index.ts:509`, `packages/llm/llm/src/index.ts` (`registerAdapter`) | Uniform `register() -> Disposer` convention; Aleph's registries return `()`/`Result<()>` |
| **Listen to an event** (observe / veto / wrap) | `ctx.on('tools/pre-execute', (exec, next) => …)` | `events.ts:288-302` | The event's **mode** tells the plugin whether it may veto (`waterfall`/`bail`/`serial`) or only watch (`emit`/`parallel`) |

### 2.2 Real event names, modes, payloads — the ones a "turn interceptor" plugin uses

Source of truth: the **generated** producer/consumer matrix `docs/event-producer-consumer.md` (68 harness events; each row = declared-in file:line, dispatcher packages, listener packages). That artifact is itself absorbable (see §5).

| event | mode | payload → return | who listens in the base bundle | declared at |
|---|---|---|---|---|
| `agent/session-start` | emit | `{agent, source: 'startup'\|'resume'\|'clear'\|'compact'}` | goal, hooks-claude-code, hooks-codex, agent-team | `packages/core/agent/src/runtime-types.ts:316` |
| `agent/pre-step` | **waterfall** | `{agent, messages: UserMessage[], turn, step, signal}` + `next()` → `PreStepDecision = {kind:'reject'} \| {kind:'enter', messages, startsRequestSeries?}` | 16 listeners: compaction-basic, plan-mode, hooks-*, time-context, tmux-context, session-reference, agent-instructions, tool-skill, tool-subagent, repeat-tool-reminder, session-checkpoint-policy… | `runtime-types.ts:330`; decision type `:112-120` |
| `agent/request` | **waterfall** | `{agent, turn, step, signal}` + `next()` → `LlmCallConfig` (route/model; **cannot mutate messages** — "model-visible content must use logged channels") | agent, webhook | `runtime-types.ts:347` |
| `agent/request-error` | **waterfall** | `{agent, turn, step, provider, failure: LlmFailure, retryPolicy, signal}` + `next()` → `{kind:'retry'} \| undefined` | compaction-basic (context overflow → compact + retry), llm-retry | `runtime-types.ts:363`; action type `:122` |
| `llm/stream` | **waterfall** | `GenerateOptions` (deep-frozen when loop-built) + `next()` → `AsyncIterable<StreamChunk>` | agent-loop, llm-replay (test), session-checkpoint-policy, session-title | `packages/llm/llm/src/index.ts:72` |
| `agent/assistant-stream` | emit | `{agent, frame: start\|chunk\|end}` | headless, session-controller (the only remote consumer) | `runtime-types.ts:373` |
| `tools/pre-execute` | **waterfall** | `ToolExecution` (name, parsed args, caller agent, `rootCallId`, `token`) + `next()` → `PreToolDecision = allow \| {deny, reason} \| {ask, reason?}` ("ask" becomes deny when no approval service is mounted) | hooks-claude-code, hooks-codex, tool-jobs | `packages/core/tools/src/index.ts:144`; decision `:581-584` |
| `tools/execute` | **waterfall** | `ToolDispatchExecution` (may replace **only** `signal`) + `next()` → `ToolExecutionResult` | timeout-policy, session-checkpoint-policy | `tools/src/index.ts:155` |
| `tools/post-execute` | **waterfall** | `(exec, result)` + `next()` → `PostToolDecision = {accept, content?\|value?, additionalContexts?} \| {block, feedback, additionalContexts?}` | hooks-*, spill-policy, repeat-tool-reminder, tool-fs-search | `tools/src/index.ts:167`; decision `:590-593` |
| `tools/result` | emit | frozen `(exec, result)` | agent-instructions, subagent-in-process-driver, tool-present | `tools/src/index.ts:189` |
| `agent/turn-stopping` | **serial** (no `next`) | `{agent, turn, signal}`; a listener that objects calls `agent.steer(...)` and the loop re-reads its inbox — "data decides, so listener order cannot change the outcome" | hooks-claude-code (Stop hook), hooks-codex | `runtime-types.ts:391` |
| `system-prompt/assemble` | **waterfall** | `(assembly: PromptAssembly, {scope?, signal?})` + `next()` | agent, agent-presets, session-reference | `packages/core/system-prompt/src/index.ts:31` |
| `session/event` | emit | one persisted `SessionEvent` | 28 listeners | `packages/core/session/src/index.ts:72` |
| `tools/change`, `system-prompt/change`, `llm/adapters-updated`, `skills/change`, `commands/change` | emit | none / list | deliberately **unfiltered** ("a global change concerns every agent's next assembly") | `tools/src/index.ts:199`, … |

Notable payload rules worth copying verbatim:
- `PreToolDecision` **excludes input rewriting** "because arguments are already logged and presented" (`tools/src/index.ts:576-579`). Aleph's approval gate should keep that rule (judgment §1 — a rewritten arg is a second representation of a logged fact).
- `ToolExecutionSuccess.concludesTurn?: true` and `additionalContexts?: UserMessage[]` (`tools/src/index.ts:549-558`) — a **tool result** can end the turn or ferry next-request context, so "stop the loop" is data on the result, not a side channel.
- `agent.inject(message)` (`runtime-types.ts:232-241`) is the **only** way a plugin adds model-visible text; it lands in the inbox and is claimed at the next `pre-step`, so it is logged (AGENTS.md "Model-visible ⟺ logged").

### 2.3 Service interfaces a provider plugin implements

| seam | interface | where | notes |
|---|---|---|---|
| LLM adapter | `abstract class LlmAdapter { providerInfo(provider), providerRetryPolicy(provider), resolveModel…, stream(options) }`; registered via `ctx.llm.registerAdapter(providers, adapter)`; every HTTP request must send `attributionHeaders()` | `packages/llm/llm/src/index.ts:196-215`, `:183`, `:191` | Aleph `src/providers/` has the equivalent trait; the absorbable detail is `PreparedAdapterCall` (`:186-192`): model resolution is **captured as a generation** and the stream call is bound to that generation so a hot-swapped adapter cannot mix one generation's capability with another's endpoint |
| Tool | `ToolDefinition extends ToolSchema { output: {schema, render(args,value), presentationMeta?}, execute(args, exec): Promise<unknown>, finalizeContent?, timeoutMs?, canRunInParallel? }` | `tools/src/index.ts:214-260` | Two things Aleph lacks: **mandatory output schema + pure `render`** (model text is a projection of a canonical JSON value, so the durable log stores the value), and `timeoutMs` **never sent to the model** ("`schemas()` whitelists only name/description/parameters") |
| Command (slash) | `CommandDefinition { name, description, input?, recordInput?, handler(invocation) }`; scoped via `ScopedLayers<CommandLayer>` | `packages/interaction/commands/src/index.ts:61-79`, `:285` | Runs **without** a model turn; per-agent variant = mount under `agent.ctx` |
| Subagent provider | `SubagentProvider { name, capabilities, inheritsParentContext, agentRouteDefaults?, start(request): Promise<SubagentRun>, prepareContinuable?() }` — "method presence IS the capability" | `packages/subagent/subagent/src/types.ts:344-398` | Providers: spawn-in-process, fork-in-process, acp, claude-code, codex, dsh-sdk (`packages/subagent/*`) — the same interface delegates a turn to an external product |
| Skill | `Skill { name, description, whenToUse?, invocation: SkillInvocationPolicy, source, provider, resourceBase?, rank, locator, metadata? }` + provider registry on `ctx.skills` | `packages/skill/skill/src/index.ts:51-118` | See §4 for SKILL.md parsing |
| Session persistence | `SessionPersistence { create/open/stat/list/export }` | `packages/session/session-persistence/` | JSONL provider (`session-persistence-jsonl`) is the shipped one |
| Session projection | `ctx.sessionProjections.register(unit)`; readers `stateOf()`, carriers `snapshot()`; **must fail loudly if the service is absent** (no silent default) | `packages/session/session-projection/`; decision note 2026-08-19 | Aleph's transcript-derived views (`shared/ui_logic`) are this shape |
| Out-of-process SDK | JSON-RPC over stdio: requests `initialize`, `session/prompt`, `shutdown`; notifications `session.event`, `session.status`, `subagent.started`, `subagent.finished` | `packages/sdk/protocol/src/types.ts:107-119` | Tiny surface — the SDK is a **client of the loop**, not an extension point; plugins are always in-process |

### 2.4 What is *not* on the surface

- No "middleware chain" object, no plugin ordering config: order = registration order + `prepend: true` (`events.ts:112-117`).
- No per-plugin capability manifest: a plugin's reach is exactly the services it `inject`s (`registry.ts:100-111` `Plugin.Base.inject`).
- No sync-vs-async distinction in the plugin author's API — `effect()` accepts a disposer, a promise of one, or an (async) generator of them (`fiber.ts:83-93`).

---

## 3. How the agent loop is itself a plugin (`dsh-agent-loop`)

### 3.1 Kernel side vs loop side

| owner | owns | where | notes |
|---|---|---|---|
| **cordis kernel** | Context / Fiber / Registry / Events (nothing agent-specific) | `vendor/cordis/src/*` | zero knowledge of "turn", "tool", "model" |
| **`dsh-agent`** (interface package, mounted as row `agent`) | the `Agent` interface (`options`, `session`, `inbox`, `status`, `ctx`, `cancel`, `whenIdle`, `runMaintenance`, `send`, `followup`, `steer`, `inject`); the `AgentRegistry` service on `ctx.agents` with a **single factory slot**; the `agent/*` event declarations | `packages/core/agent/src/runtime-types.ts:164-256` (interface), `:258-403` (events); `packages/core/agent/src/index.ts:245` (`AgentRegistry`), `:355-363` (`setFactory` — throws if one is already set, disposer clears the slot), `:206` (`'no agent factory registered (load an agent-loop plugin)'`) | The registry is the **stable seam**; `create()`/`resume()` on it delegate to whichever factory plugged in |
| **`dsh-agent-loop`** (row `agent-loop`, 82nd of 84 rows in the base bundle) | `class AgentLoop extends Service implements AgentFactory`, `static inject = ['agents','sessions','llm','tools','systemPrompt','sessionProjections']`; registers itself with `ctx.effect(() => ctx.agents.setFactory(this))`; `ReactLoopAgent` = the turn driver | `packages/core/agent-loop/src/index.ts:359-360` (inject list), `:426` (setFactory as effect), `agent.ts` (driver, 619 lines), `tool-calls.ts` (parallel tool scheduler, 290 lines), `inbox.ts` (247 lines) | Unload `agent-loop` → factory slot clears → `ctx.agents.create` throws the NO_FACTORY message. A **different** loop plugin can take the slot |

So: the kernel owns nothing about agents; the **interface package** owns the contract + registry + events; the **loop package** owns Think→Act scheduling. This is the same split Aleph's R10 wants ("harness only carries Think→Act round scheduling"), but with the contract physically in a separate package so the loop is replaceable.

### 3.2 The turn as the loop actually runs it (with the extension points at their real positions)

Driver: `packages/core/agent-loop/src/agent.ts`.

| phase | code | what plugins can do here |
|---|---|---|
| open turn → `session.append('turn/start')` | `agent.ts:279-284` | nothing (persistent event only) |
| **claim** inbox for `target` (`next-turn` on the first step, `next-step` after) | `agent.ts:244` (`this.inbox.claim(target, turn)`) | plugins put things in the inbox beforehand via `agent.inject()` / `agent.steer()` / `agent.followup()` |
| **assemble** system prompt + tool schemas (`systemPrompt.assemble` runs the `system-prompt/assemble` waterfall) | `agent.ts:245-248` | add/replace sections, variables; `PromptAssembly` returned |
| **`agent/pre-step`** waterfall — default `next()` returns `{enter, messages: claimed + projected runtime context}` | `agent.ts:249-256` | reject the step (turn closes with no model call, `agent.ts:290-293`) or rewrite the message batch; **compaction-basic** runs `compactIfNeeded(agent,'pressure')` **here** and then calls `next()` (`packages/compaction/compaction-basic/src/index.ts:148-166`) |
| `session.append('step/start')` | `agent.ts:302` | — |
| **`agent/request`** waterfall — default returns the frozen seed config (route/model/effort/maxTokens from options or the logged header) | `agent.ts:530-533` | switch provider/model per step; **cannot touch messages** |
| `llm.prepareCall(config)` — captures an adapter generation | `agent.ts:541` | — (adapter-owned) |
| log `request/header` / `request/context` if changed; **freeze history** from `session.deriveMessages()`; `markAgentLoopRequest` | `agent.ts:553-618` | — ; "model-visible ⟺ logged" is enforced by construction: the request is a pure function of the log |
| **`llm/stream`** waterfall around the adapter (retry/replay/routing live here) | dispatched inside `packages/llm/llm/src/index.ts` (`:72` decl, `:1106` dispatch) | wrap or replace the chunk stream |
| `agent/assistant-stream` start/chunk/end (emit) | `agent.ts:386` | UI mirrors only |
| tool calls: `tools/pre-execute` → `tools/execute` → `tools/post-execute` → `tools/result` per call, scheduled by `tool-calls.ts` (parallel groups capped by `maxParallelToolCalls`, read through on every group start `index.ts:390-394`) | `packages/core/tools/src/index.ts` pipeline; scheduler `agent-loop/src/tool-calls.ts` | allow/deny/ask; timeout wrapper; replace/block result; attach `additionalContexts`; set `concludesTurn` |
| **`agent/request-error`** waterfall on a failed attempt | `agent.ts:448-452` | `compaction-basic` returns `{kind:'retry'}` after compacting on `CONTEXT_WINDOW_EXCEEDED` (`compaction-basic/src/index.ts:180-200`); `llm-retry` handles transient codes |
| `session.append('step/end')` | `agent.ts:307` | — |
| **`agent/turn-stopping`** (serial) when tools owe nothing and `inbox.nextStep` is empty; loop **re-reads the inbox afterwards** | `agent.ts:315-319` | a listener that wants another step calls `agent.steer(...)`; Claude Code `Stop` hook lives here (§4) |
| `session.append('turn/end', {reason})` — reasons: completed / blocked / max-tokens / aborted / error | `agent.ts:339` | — |

### 3.3 The "five don'ts" as dsh states them (compare to Aleph R10)

| dsh rule | where stated | Aleph analogue |
|---|---|---|
| "Plugins, not loop changes: new behavior goes on documented extension points; changing `agent-loop` requires updating docs/architecture.md" | `AGENTS.md` Conventions | R10 "add code only after answering 3 questions" |
| Loop never mutates request messages after freezing; `agent/request` cannot mutate messages; only logged channels reach the model | `runtime-types.ts:347-361`, `agent.ts:603-618` | A1 "own context window" + "model-visible ⟺ logged" |
| Retry does not re-run assembly or `agent/pre-step` (`docs/architecture.zh.md` 轮次流程) | doc + `agent.ts:448` | A2 "error compaction ≠ error recovery" — recovery is a plugin decision (`{kind:'retry'}`), the loop just re-dispatches |
| `agent/turn-stopping`: "data decides, so listener order cannot change the outcome" | `runtime-types.ts:378-390` | R10 "harness does not pick recovery strategy for the model" — the stop decision is inbox state, not a listener vote |
| Compaction is a **maintenance task** run from the idle phase or inside pre-step, never inside the loop body | `runtime-types.ts:196-206` (`runMaintenance`), `compaction-basic/src/index.ts:376` | Aleph's `src/context/` compaction is called from inside the harness; dsh keeps the loop ignorant of compaction entirely |

### 3.4 What is genuinely different from Aleph's harness

- **Inbox with two boundaries** (`next-turn` vs `next-step`) and three verbs (`followup` / `steer` / `inject`) (`runtime-types.ts:205-241`, `agent-loop/src/inbox.ts`). Aleph's busy-input (§4.8 rounds) converged on interrupt/queue/steer; the dsh shape additionally has **`inject` = context that waits silently until something else wakes the driver** — a clean answer to "how does a plugin add a system reminder without triggering a turn".
- **Request header + request context are session events** (`agent.ts:568-600`): a change of tools/model/effort is a logged fact, so replay reproduces the exact request. Aleph logs the transcript but not the tool-schema set per request.
- **`concludesTurn` on a tool result** (`tools/src/index.ts:558`) — a tool can end the turn; no "stop tool" special case in the loop.

---

## 4. Third-party plugin compatibility

### 4.1 What dsh loads, and what it does NOT (verified by source, not README)

| format / ecosystem | loaded? | where | notes |
|---|---|---|---|
| **Claude Code `hooks.json`** or a `settings.json` whose `hooks` key holds the map | **yes** (command hooks only; `type !== 'command'` skipped with a warning) | `packages/hooks/hooks-claude-code/src/config.ts:78-123` (`parseClaudeCodeConfig`), `:11-19` (supported events: `SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, SubagentStart, SubagentStop`) | One config path per **process**, read once at load (`index.ts:45-52`, TODO per-session discovery). `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PROJECT_DIR}` substituted at parse time (`config.ts:57-62`) |
| **Codex `hooks.json`** | yes (5 events, regex-only matchers, no env/substitution, "only blocking decisions are honored") | `packages/hooks/hooks-codex/src/config.ts:11`, `index.ts:1-9` | Same shared protocol lib, different dialect |
| **`SKILL.md`** (dir bundle or flat `.md` with YAML frontmatter) | **yes** | `packages/skill/skill-filesystem/src/index.ts:797-835` (`parseSkillFile`), `:917-929` (`parseFrontmatter`), `:1000-1010` (`parseInvocationPolicy`: `disable-model-invocation`, `user-invocable`; **legacy camelCase keys throw**) | Roots scanned in rank order: `<project>/.dsh/skills`, `<project>/.agents/skills`, custom dirs, `~/.dsh/skills`, `~/.agents/skills`, bundled (`:245-264`). **`.claude/skills` is NOT a root.** `allowed-tools` is not parsed |
| **MCP servers** | yes, but **one plugin instance per server from `cordis.yml`**, `transport: 'stdio' \| 'streamable-http'`, tools exposed as `mcp__<serverName>__<rawName>` | `packages/mcp/mcp-client/src/index.ts:51-63` (stdio: `serverName, command, args, env, cwd, toolCallTimeoutMs, failOnStartupError`), `:77-86` (http: `serverName, url`) | **`.mcp.json` is NOT parsed** — no repo source references `.mcp.json`; the user writes cordis.yml rows |
| **Claude Code `plugin.json` / `.claude-plugin/` / `marketplace.json`** | **no** — zero source hits outside the DeepSeek model-package inventory | `rg` across `packages/` `apps/` | dsh's own third-party unit is an **npm package** whose `package.json` has a `dsh.bundle` field (`apps/cli/src/plugin.ts:1-10`: `dsh plugin install` = pnpm forwarder + bundle-layer reconciliation) |
| **pi extensions** | **no** — `llm-pi-ai` uses pi-ai only as an **LLM client library** (`packages/llm/llm-pi-ai`); no pi extension host | — | |
| **Claude Code as a subagent** | yes, via the official Agent SDK; runs the real CLI under dsh's subprocess owner | `packages/subagent/subagent-claude-code/src/index.ts:1-7` | Also `subagent-codex`, `subagent-acp` (ACP protocol), `subagent-dsh-sdk` |

Judgment for Aleph: dsh's compatibility story is **hooks + SKILL.md + MCP-by-config + delegate-to-the-real-CLI**. It deliberately does not emulate a foreign plugin package format. Aleph's `src/hub/` + `plugins/` submodule already goes further on `plugin.json`/marketplace; the piece worth taking is the hook protocol library.

### 4.2 The Claude Code hook contract as honored (`packages/hooks/hook-protocol`)

| step | code | contract detail |
|---|---|---|
| spawn through `ctx.shell` (credential scrub, process-group kill, timeout), stdin = `JSON.stringify(payload) + '\n'` (CC) / no newline (Codex) | `runner.ts:67-95` | timeout: per-hook `timeout` seconds → ms, else `defaultTimeoutMs` (600 000 = CC default, `runner.ts:20`); cwd = the **agent's session workspace** (`hooks-claude-code/src/index.ts:144-150`), env adds `CLAUDE_PROJECT_DIR` |
| infrastructure failure (no shell, bad cwd) → outcome with `exitCode: undefined`, never throws | `runner.ts:96-105` | "A hook that cannot run is a non-blocking error … The turn proceeds" |
| **exit 2 → `decision: 'block'`, reason = stderr** | `codec.ts:11`, `:66-69` | signal death (`exitCode === null`) → `undefined` → non-blocking |
| **exit 0 + stdout starting with `{` → parse JSON**; malformed JSON = plain stdout (lenient like the reference) | `codec.ts:72-86` | any other exit → non-blocking error, stderr kept |
| top-level fields: `continue`, `stopReason`, `systemMessage`, legacy `decision` (**only** `approve`/`block`; `allow`/`deny`/`ask` at top level are ignored), `reason` | `codec.ts:97-110`, `:33-40` | |
| `hookSpecificOutput`: `hookEventName` must equal the firing event or the block's fields are **discarded** (discriminator still recorded for the log); `permissionDecision` (allow/deny/ask) overrides legacy; `permissionDecisionReason`; `additionalContext`; `updatedInput` | `codec.ts:112-133`, `runner.ts:40-46` (`expectedEventName`) | |
| **merge** N matched hooks: `deny > ask > allow > none`; reasons of the winning rank joined with `\n\n`; first `continue:false` sticky; `additionalContext[]` and `systemMessages[]` accumulate in hook order | `merge.ts:62-99` | |
| map to extension point: `UserPromptSubmit` deny → `pre-step {reject}`, else `next()` then append context to a downstream `enter`; `PreToolUse` deny/ask → `{deny,reason}`/`{ask}`, else `next()`; `PostToolUse` deny → `{block, feedback, additionalContexts}`, else `next()` then prepend context; `Stop` deny → `agent.steer(reason)`; `SessionStart`/`SubagentStart` → `agent.inject(context)` detached | `hooks-claude-code/src/index.ts:218-294` | |
| **not honored** (logged + warned): `updatedInput`, `systemMessage`, `continue:false` (TODO run-level halt), Stop-loop guard (`stop_hook_active` always `false`, TODO cap) | `index.ts:174-179`, `:188`, `:268`, `:344` | These are the honest gaps; the README records them |
| every mid-turn hook run is **logged as a session event pair** `hook/invoked` / `hook/result` (turn, point, dialect, handlerId, matcher; decision, exitCode, stderr summary capped at 500 chars, durationMs) | `hook-protocol/src/events.ts:12-50`, `hooks-claude-code/src/index.ts:156-161`, `:180-182` | A **runtime invariant** checks the pair is turn-enclosed and balanced (`hook-protocol/src/invariant.ts:29-40`) |
| payloads (CC dialect, snake_case): `session_id, transcript_path (always ''), cwd, hook_event_name` + per event `source` / `prompt` / `tool_name, tool_input, tool_use_id` / `tool_response` (text only) / `stop_hook_active` / `agent_id, agent_type ('general-purpose')` | `hooks-claude-code/src/index.ts:320-359` | `transcript_path` empty = documented consumer gap |

Aleph-relevance: Aleph has `src/hub/` ingest and a plugin runtime, and presumably an approval gate, but (to my knowledge from CLAUDE.md) no Claude Code **hook** executor. The `hook-protocol` trio — `runner.ts` (106 lines) + `codec.ts` (134) + `merge.ts` (100) — is a near-verbatim port target: pure functions over `(exitCode, stdout, stderr)`, dialect-neutral, with the hook-event discriminator guard and deny>ask>allow fold. The two Aleph-specific decisions are (a) which extension point each CC event maps to (Aleph's tool pipeline has an approval gate already; `ask` maps there) and (b) whether to log `hook/invoked`/`hook/result` into the transcript (dsh does; judgment §15 "stamp intent before the irreversible boundary" argues yes).

---

## 5. Runtime closure / verification tricks

These are the mechanisms that catch "declared but never mounted", "injected but never provided", "listener that can never fire", and "predicate that is always green". Grouped by **when** they fire.

### 5.1 Boot-time (runtime, every launch)

| mechanism | where | what it catches | Aleph-relevance |
|---|---|---|---|
| **`assertEntriesActivated`** — after the loader tree settles, every enabled entry must be `ACTIVE`; a `FAILED` fiber is awaited to recover its original stack; a **`PENDING` fiber lists the injected services that are still `undefined`** and boot fails with `N entries did not activate` | `packages/boot/app-boot/src/index.ts:721-751` | "injected but never provided" (§7 two ends, no wire) — the config said "load X", X waits for `ctx.foo`, nobody provides `foo`; without this gate X would silently sit PENDING forever while every test stays green | Aleph's plugin host + `src/config/` diagnostics: add a post-boot sweep "every configured plugin/MCP/skill reached its terminal Ready state; list what each non-ready one is waiting for". Cheap, high-yield |
| `installFailLoud` — unhandled rejection after boot → `fatal load failure` + bounded release + exit 1 | `app-boot/src/index.ts:639-690` | a plugin that throws asynchronously after boot cannot become a zombie | Aleph has this shape in `aleph-server` main; fine |
| Cordis itself: reading an un-injected service throws; `provide()` twice in one scope throws (`service "X" has been registered at <fiber>`); `setFactory` twice throws | `reflect.ts:144`, `:289-291`; `agent/src/index.ts:357` | duplicate providers and undeclared reads fail loud at the call site | §6 "count first" — the count is enforced by the kernel |
| **Runtime invariants** (`ctx.invariants`): each package ships an optional `./invariant` companion that registers `(ctx, fail) => …` under its **npm package name**; `fail()` throws `InvariantError{code:'INVARIANT', packageName}`; allow/blocklist by regex; **mounted in `sdk-minimal`, deliberately omitted from `dsh-base`** | `packages/runtime-diagnostics/invariants/src/index.ts:1-60`, README "Use this package" | live self-checks attributed to an owner | Aleph's `qa/` scripts are external; this is the in-process twin |
| `agent-loop-invariant`: on every loop-built `llm/stream` (prepended, global so a replay listener cannot silence it) assert: request frozen, session live, log has `step/start` and `request/header`, and **`JSON.stringify(options.messages) === JSON.stringify(session.deriveMessages())`** and the header fields match the folded header | `packages/core/agent-loop/src/invariant.ts:18-59` | "model-visible ⟺ logged" checked at the wire, not claimed in a comment | This is judgment §1 as a runtime assertion: the request and the log are two representations of one fact; the invariant makes the second one **derived**, then asserts equality. Aleph's transcript/history path (§6.9, "authoritative order = record order") would benefit from exactly this check at the provider boundary |
| `agent-invariant`: `agent/status` must never repeat the same status (no-op transition) | `packages/core/agent/src/invariant.ts:15-24` | a status emitter that fires without a change | judgment §11 "a no-op that reports success" |
| `scope-invariant`: a scope-filtered event dispatched **without** a scope carrier, or with a carrier keyed to a different subject than the payload names, fails | `packages/core/scope/src/invariant.ts:16-33`; resolver map is **generated** from the TS program by `scripts/gen-scoped-events.ts` (`@dshScopeScan unsupported` required for zero matches; ambiguity fails loud) | a listener registered under agent A that can never receive A's events because the dispatcher forgot the carrier | §7 "registered but the dispatch table has no arm"; §3 "the guard only recognizes shapes it knows" — here the list of scoped events is derived from types, not hand-maintained |
| `hook-protocol-invariant`: `hook/invoked`/`hook/result` must be inside an open turn and paired | `packages/hooks/hook-protocol/src/invariant.ts:29-40` | an audit log whose pairs can drift | §15 intent-then-result pairing |

### 5.2 Static gates (CI, `pnpm run doc-sync` / `run-gates.ts`, 63 leaf gates)

| gate | where | what it catches | Aleph-relevance |
|---|---|---|---|
| **`verify-cordis-config`** — every `cordis.yml`/patch row: only `config` and `disabled` may hold `!!js` expressions; the other metadata fields (`id, name, group, inject, intercept, isolate`) must be literal ("an expression there remains truthy data and silently changes composition"); every bare plugin name must be in the owning manifest's `dependencies`; client halves declared; preset plane separation | `scripts/verify-cordis-config.ts:1-11`, `:38`, `:104-160`, `:227-460` | "config row that looks like it disables/isolates something but is actually a truthy string" (§2 恒绿); "row names a package the manifest does not carry" (§7) | Aleph's `config.toml` → a lint that every `[[plugins]]`/`[[mcp]]` row resolves to an installed artifact **before** boot |
| **`verify-runtime-closure`** — every plugin named by a shipped agent preset (per platform, honoring platform-gated `disabled`) and every required workspace peer transitively reachable from the deploy manifest must be in that manifest as `workspace:`; failure prints the dependency **chain** | `scripts/verify-runtime-closure.ts:47-99`, `:119-152` | "the packaged runtime would fail only when cordis loads the plugin" — a packaging-time §7 | Aleph: the bundled `skills/`/`plugins/` submodules embedded via `include_dir!` are the analogue; a gate that every preset/profile Aleph ships references only embedded or resolvable artifacts |
| **`verify-package-invariants`** — an invariant companion must not be **empty**, must take the `fail` reporter as 2nd param **and use it**, must not be generated, must not default-export; a package **without** a companion must have a README sentence `No … companion is published because …` | `scripts/package-invariants.ts:12-13`, `:300`, `:330-340`, `:201` | **恒真的谓词等于没判** (§2) as a lint: a check that cannot call `fail` is rejected; an absent check must be an explicit decision, not an omission | Direct port target for Aleph's `qa/` and `census` tests: "every guard must be able to go red — prove it by referencing the failure path" |
| **Generated catalogs as gates**: `gen-doc-graphs` → `docs/event-producer-consumer.md` (dispatchers + listeners per event, resolved from the TS Program), `gen-config-catalog` (cross-checks runtime schema keys against declared config types: "pasted content cannot hide a field the loader accepts"), `gen-persistence-catalog` (every `SessionEventMap` member), `gen-tool-catalog`, `gen-cordis-catalog` (checks `@mode` JSDoc against dispatch sites); freshness verified in CI | `scripts/gen-doc-graphs.ts`, `scripts/gen-config-catalog.ts`, `docs/config-catalog.zh.md` header, `docs/cordis-primer.md` "Dispatch Modes" | an event with **zero listeners** or **zero dispatchers** is visible in a table that CI regenerates (e.g. `agent-loop/config-start-failed` has `-` listeners; `skills/change` has `-`); a dispatch site using the wrong mode fails | §6 "count first" — the count is machine-produced and committed, so a drift shows as a diff. Aleph's `FEATURE_LOCATOR` counts are hand-written |
| `verify-no-bare-dispatcher` — syntax-aware (TS AST) scan that no package constructs its own undici `Agent`/passes `dispatcher` to `fetch`, because that silently bypasses the proxy | `scripts/verify-no-bare-dispatcher.ts:1-20` | one specific "silent no-op of a global policy" | Pattern: **a policy installed globally needs a lint that no call site opts out of it** (Aleph: `[sandbox.command_policy]` floor, `spend` ledger) |
| `verify-application-entrypoints` — every bin/demo classified; any Node app path that bypasses `dsh` is rejected | `scripts/verify-application-entrypoints.ts` | a second launcher that skips the composition | Aleph's "always launch from `Aleph/`" / singleton flock rule as a gate |
| `verify-export-jsdoc`, `verify-doc-budgets`, `verify-translation-pairing`, `verify-md-links` | scripts/ | doc drift | dsh treats docs as gated artifacts; the interesting rule is **"one home per fact"** (`AGENTS.md`), same as Aleph's judgment §1 |

### 5.3 Type-level (typert) — mostly not relevant

`packages/typert/*` generates a type graph from TS declarations so Client (browser) code can call Host methods as typed RPC without hand-written wire code (`packages/typert/README.md`). It is the JS answer to Aleph's `shared/protocol/` crate — Aleph already has the better version (one Rust crate both sides depend on, judgment §10). Nothing to absorb except the principle already held.

### 5.4 `guard/` — loop hygiene, not verification

`repeat-tool-reminder` (detects identical repeated tool calls, injects a reminder via `tools/post-execute` additionalContexts) and `timeout-policy` (a `tools/execute` wrapper enforcing `ToolDefinition.timeoutMs`) — `packages/guard/README.md`. Both are ordinary plugins on documented points; Aleph has equivalents (`extension_stop_gate`, tool budget). Not a verification mechanism.

---

## 6. What would be a MISTAKE to port to a Rust core

| thing | why it works in dsh | why not in Aleph | what to do instead |
|---|---|---|---|
| **The `Context` Proxy + caller-context tracing (`getTraceable`)** — when plugin A calls `ctx.tools.register(x)`, the `tools` service object is re-wrapped so that inside `register`, `this.ctx` is **A's** context and the effect lands on **A's** fiber | `vendor/cordis/src/utils.ts:117-125` (`getTraceable`), `:128-140` (`withProps` Proxy), `reflect.ts:141` (every ctx read is traced) | Needs dynamic `this` rebinding and Proxy traps on every property read; no static equivalent; and it hides the ownership transfer that Rust would make explicit | Make ownership **explicit in the signature**: `registry.register(def) -> Disposer` and the **caller** pushes the disposer into its own `PluginHandle`; or `register(owner: &PluginScope, def)`. Same guarantee, zero magic |
| **Declaration merging for the `Events`/`Context` maps** (`declare module '@deepseek-ai/cordis' { interface Events { 'tools/pre-execute'(…) } }`) | TS structural typing; every package extends the global event map at compile time | Rust has no open interface merging; a global `enum Event` is the wrong shape (every new plugin edits core, violating P3) | Typed event **keys** as zero-sized types: `trait Event { type Payload; type Ret; const MODE: Mode; }` + `bus.on::<PreToolUse>(…)` — plugins define their own key types in their own crate; the bus is generic. Aleph's `src/harness/` must stay out of it (R10) |
| **Prototype-chain child contexts** (`extend()` = `Object.create(parent)`; isolation/intercept maps are prototype-inherited dicts) | O(1) child creation, shadowing for free | Would become `HashMap` cloning or `Arc<Parent>` chains with lookup loops; fine for a few levels, but the *design* leans on "shadow anything cheaply" | Keep Aleph's explicit scoping (session → agent → tool scope) with **one** label-keyed store per service kind (`ScopedLayers`-style, §1.3), not a general-purpose prototype context |
| **Epoch-driven auto-reload of dependents** (`_refresh` / `notify` cascade) as the default for *every* service | JS can drop and re-run a plugin closure at any `await`; cordis serializes via `inertia`; and dsh **turns live reload off** for `headless`/`sdk`/`acp` anyway (`profile-boot.ts:355`, architecture doc: "replacing dependencies would break that lifecycle") | Aleph is the daemon shape they freeze; a Rust `Arc<dyn Provider>` swap mid-turn is a use-after-config bug generator; tokio tasks in flight hold old `Arc`s | Adopt the **observable** (PENDING/FAILED/ACTIVE per plugin, missing-deps list) and the **disposer list**; do NOT adopt cascade-restart. Provider swap = drain turns → dispose → mount, as today |
| **Vendored `hmr` (module hot reload via chokidar + re-import)** | ESM re-import + fiber restart | No Rust analogue; dylib reload is a different, worse beast | CUT. Aleph's reloadable units are skills/MCP/config, all data |
| **`@xterm/headless` in `terminal-bash`** (a JS VT emulator for persistent terminal sessions) | `packages/terminal/terminal-bash/package.json:42` | Aleph's disallowed list: "no second VT" — the only VT is `src/gateway/pty/screen/` | Nothing to port; if anything, note that dsh needed a headless VT for the same reason Aleph built one (agent-visible screen state) |
| **`session-query-sqlite` / JSONL session log** | JS-native persistence choices | Aleph's memory layer is locked to sqlite+sqlite-vec; its transcript store is its own (§6.9 "authoritative order = record order") | Port the **invariant** (request == derived-from-log), not the store |
| **`web-*`, `subagent-claude-code` via the Agent SDK, `code-runtime` worker threads, `webworker-runtime`** | Node ecosystem glue | Aleph already has `src/search/`, `crates/aleph-cdp`, `src/agents/`; Node worker threads have no meaning in tokio | Nothing |
| **Standard-schema config validation per plugin + `Config.merge`** (schemastery) | runtime schema objects | Aleph uses serde + schemars; equivalent exists | Nothing — but keep dsh's rule "misconfiguration fails loud **at load**", already Aleph P7 |
| **Waterfall with `next()` closure as the universal hook shape** | closures + async everywhere | Fine in Rust as `Box<dyn Fn(Payload, Next) -> BoxFuture>` but easy to over-apply; dsh itself limits waterfalls to 9 events and uses `emit` for 50+ | Use waterfall **only** where a plugin may veto/wrap (pre-step, pre/post tool, request, request-error, stream, prompt assemble). Everything else stays broadcast. Do not put a waterfall inside `src/harness/` (R10) — put it at the tool pipeline and provider boundary like dsh does |
| **Runtime invariants mounted by default** | cheap in JS | dsh itself omits them from `dsh-base` (production) and mounts them only in `sdk-minimal` (README) | Same policy: invariants behind a config switch / test profile; the `deriveMessages` equality check is O(history) per request |

---

## Top 8 absorbable mechanisms, ranked

1. **`register() -> Disposer`, owned by the plugin handle; unload = run the list in reverse** (`fiber.ts:418-561`, `reflect.ts:277-305`, `events.ts:254-260`)
   Why: it is the entire content of "temporal composability" and it is pure ownership discipline — no runtime magic. Makes skill/MCP/plugin unload correct by construction.
   Risk if ported naively: a Rust registry that stores `Box<dyn FnOnce>` disposers but lets some registration paths (prompt sections, event subscriptions, cron rules) bypass the handle recreates the leak; the guarantee only holds if **every** mutation of shared state goes through the handle (dsh enforces it because the kernel offers no other API).

2. **Boot-time `assertEntriesActivated`: every configured unit must reach ACTIVE; PENDING ones name their missing services** (`app-boot/src/index.ts:721-751`)
   Why: it is the direct detector for judgment §7 ("two ends, no wire") and §3, and it is ~30 lines.
   Risk: if Aleph's plugin/MCP/skill loaders report "ready" before their dependencies are actually resolved (e.g. MCP server spawned but `initialize` not answered), the gate turns 恒绿 (§2). The state must be the *terminal* state, and "waiting for X" must be derived from a declared dependency list, not a boolean.

3. **The `verify-package-invariants` rule set: an invariant that cannot call `fail` is rejected; an absent invariant must be an explicit README decision** (`scripts/package-invariants.ts:300-340`, `:201`)
   Why: it is judgment §2 ("when does this go red?") turned into a lint. Applies verbatim to Aleph's `census`/`qa` guards.
   Risk: syntactic "uses `fail`" ≠ semantic "can reach `fail`"; a `if false { fail() }` passes. Pair with mutation discipline (Aleph memory: guard-mutation-discipline).

4. **The Claude Code hook protocol trio: `runner` (spawn/exit-code) + `codec` (exit 2 = block, exit 0 + `{`-stdout = JSON, `hookSpecificOutput.hookEventName` guard, top-level decision limited to approve/block) + `merge` (deny>ask>allow, sticky `continue:false`, contexts accumulate)** (`packages/hooks/hook-protocol/src/{runner,codec,merge}.ts`)
   Why: ~340 lines of dialect-neutral pure functions that encode the real CC contract including its quirks; Aleph's hub/plugin work wants exactly this and the mapping to Aleph's approval gate is one table (§4.2).
   Risk: porting only the parser and mapping `ask` to `deny` because Aleph's approval seam is elsewhere (dsh does this when no approval service is mounted — `tools/src/index.ts:136-137`). Also the honest gaps (`updatedInput`, `continue:false`, Stop-loop guard) must be carried as documented gaps, not silently "supported".

5. **Runtime invariant "request sent to the model == `deriveMessages(log)`"** (`agent-loop/src/invariant.ts:18-59`)
   Why: it makes "model-visible ⟺ logged" a checked fact at the provider boundary instead of a claim; catches the class Aleph has hit repeatedly (history/transcript order, §6.9).
   Risk: O(history) per request and a second derivation path that itself can drift (§1). Keep it behind a test/QA profile switch; derive with the **same** function the production path uses, not a re-implementation.

6. **`concludesTurn` / `additionalContexts` on the tool result, and `agent.inject()` as the only plugin channel to the model** (`tools/src/index.ts:549-558`, `runtime-types.ts:232-241`)
   Why: removes two side channels from the loop (a "stop" special case and a "system reminder" injection API); the loop reads data, plugins write data; fits R10's five don'ts.
   Risk: `inject` semantics ("waits in the inbox until something else wakes the driver") interact with Aleph's busy-input rounds (§4.8); porting the field without the inbox-boundary model creates a third, unspecified queue.

7. **Generated producer/consumer matrix per event, committed and CI-verified** (`scripts/gen-doc-graphs.ts` → `docs/event-producer-consumer.md`)
   Why: judgment §6 "count first" as a machine artifact; an event with `-` listeners or `-` dispatchers is visible in review, and a mode mismatch fails.
   Risk: Aleph's event bus is stringly/enum-typed in places; a generator that only recognizes one registration shape reproduces §3 ("the guard recognizes the shapes it knows"). Start from the typed-key bus (item 8) so the generator has one shape to find.

8. **Typed events with a declared dispatch mode (`emit` / `parallel` / `serial` / `bail` / `waterfall`) and scope-filtered admission (listener context vs. dispatch carrier; events flow up the scope chain, never down)** (`events.ts:165-243`, `scope/src/index.ts:170-185`)
   Why: one bus serves N agents/teams without N buses; a plugin knows from the event whether it may veto; supervisor sees children by ancestry.
   Risk: this is the item most likely to grow into a framework. Limit to (a) mode as a const on the key type, (b) waterfall only at tool pipeline + provider boundary + prompt assembly, (c) scope filter = one `ScopeKey` compare against a parent chain. Do not bring `intercept`, `accessor`, `mixin`, or Proxy-based context.

Deliberately **not** in the top 8: patch-layer profiles (nice, but Aleph's TOML + session knobs already cover the use; the id-addressed override is the only piece worth stealing), `isolate` realms (Aleph's `scoped/` retain passes are enough until a second registry per agent is actually needed — judgment §19 "what did this widen"), HMR, typert.

---

## 7. dsh as an MCP host (what Aleph's MCP server face must look like to be a one-row mount)

Package: `packages/mcp/mcp-client` (4 source files, 1158 lines) on `@modelcontextprotocol/sdk` `^1.12.0`, lockfile-resolved **1.29.0** (`pnpm-lock.yaml:12951`). **Not mounted by any shipped bundle** — the user adds rows; shipped examples are `apps/cli/config/examples/mcp-memory/*.cordis.yml`.

### 7.1 The row (exact schema)

| field | stdio | streamable-http | where |
|---|---|---|---|
| `name` | `'@deepseek-ai/dsh-mcp-client'` (one plugin instance per server; N servers = N rows) | same | `src/index.ts:1-5` |
| `config.transport` | `'stdio'` | `'streamable-http'` — **no legacy SSE transport, no `sse` value** | `src/index.ts:113-134` (schemastery union), `src/transport.ts:31-49` |
| `config.serverName` | required, `[A-Za-z0-9_-]{1,32}`, unique per scope (global, or per agent scope when the row lives in an agent preset) | same | `src/index.ts:38`, `:154-168` (reservation as an effect; duplicate fails **this** row at load, earlier instance intact) |
| `config.command` / `args` / `env` / `cwd` | `command` required; `args: []` default, passed **without shell** to `StdioClientTransport`; `env` merged over a **scrubbed** parent env (credential-shaped and stale `DSH_*` names dropped); `cwd: ''` default | — | `src/index.ts:117-120`, `src/transport.ts:21-23`, `:34-39` |
| `config.url` / `headers` | — | `url` required; `headers: {}` → `requestInit.headers`. **Auth = static headers only** (`Authorization: Bearer …` via `!!js process.env.X`); no OAuth / auth provider / token refresh | `src/index.ts:128-129`, `src/transport.ts:45-48` |
| `config.toolCallTimeoutMs` | default 60 000 (per `tools/call`) | same | `src/index.ts:36`, `:121` |
| `config.failOnStartupError` | default `false` → a failed first connect logs and enters the reconnect loop; `true` → the fiber rejects (Cordis rolls the row back) | same | `src/index.ts:122`, `:184-187` |
| `config.reconnect` | `{enabled: true, initialDelayMs: 500, maxDelayMs: 30000, maxAttempts: 10}` | same | `src/connection.ts:40-45`, `:65-90` (unknown keys fail at load) |

Minimal row (from the README, `packages/mcp/mcp-client/README.md:34-50`):
```yaml
- id: mcp-aleph
  name: '@deepseek-ai/dsh-mcp-client'
  config: { serverName: aleph, transport: streamable-http, url: http://127.0.0.1:PORT/mcp,
            headers: { Authorization: !!js `Bearer ${process.env.ALEPH_TOKEN}` } }
```

### 7.2 Handshake and negotiated capabilities

| item | value | where |
|---|---|---|
| client info | `{ name: 'dsh-mcp-client', version: '0.0.1' }` | `src/connection.ts:238-241` |
| **client capabilities** | **`{}`** — no `sampling`, no `elicitation`, no `roots`; the server must not require any of them | `src/connection.ts:240` |
| protocol version | whatever SDK 1.29.0 negotiates (its `LATEST_PROTOCOL_VERSION`; node_modules is not present in this checkout, so the literal is not cited here) — dsh sets nothing explicitly | — |
| notifications handled | **only** `notifications/tools/list_changed` → full re-sync (`tools/list` drained again, generation swap) | `src/connection.ts:257-270` |
| requests the server can expect | `initialize`, paginated `tools/list` (uncached, **cursor loop detected** → invalid list), `tools/call {name: rawName, arguments}` with abort signal + timeout | `src/tools.ts:73-96`, `:154-184` |
| **not consumed** | `prompts/*`, `resources/*` ("Tools are the only bridged MCP capability — Resources and Prompts have no harness consumer mechanism"), server→client `sampling/createMessage`, `elicitation/create`, `roots/list`, logging, progress | README `:191`; `rg` over `src/` finds no `prompts|resources|sampling|elicit|roots` |
| MCP **tasks** extension | a tool with `execution.taskSupport === 'required'` is registered but **throws at call time** | `src/tools.ts:171`, `:322-324` |

### 7.3 How tools reach the model

| aspect | rule | where |
|---|---|---|
| public name | `mcp__<serverName>__<rawName>`; chars outside `[A-Za-z0-9_-]` → `_`; if normalization or the 64-char cap changed the name, truncate to 51 and append `_` + 12 hex of `sha256(serverName\0rawName)`. The public name is **never parsed back**; the raw name is closed over per definition | `src/tools.ts:47-56`, `:112-118` |
| description | `tool.description ?? ''` passed **verbatim** — no truncation, no rewriting | `src/tools.ts:168` |
| input schema | `tool.inputSchema` passed **verbatim** as `parameters`; `ToolRuntime.register` validates only the **output** schema (`assertSupportedJsonSchema(output.schema)`), not `parameters` — so a schema DeepSeek's API rejects surfaces at request time, not at mount | `src/tools.ts:169`, `:266-270`; `packages/core/tools/src/index.ts:1027-1035` |
| output | canonical value `{ content: JsonValue[], structuredContent? }`; `outputSchema` honored only if it is within the harness JSON-schema subset, else `structuredContent` falls back to unconstrained `JsonValue` | `src/tools.ts:230-239`, `:285-301` |
| model-visible text | `text` blocks joined with `\n`; `image` (png/jpeg/webp/gif, canonical base64) admitted **only** after exact model-capability proof + a mounted attachment store, else a diagnostic line; `resource_link` → `Resource link: <name> (<uri>)`; `audio`/`resource`/unknown → bracketed placeholder; empty → `(<tool> returned no model-visible content)` | `src/tools.ts:507-569`, `:389-401`, `:409-420` |
| `isError: true` | executor **throws** with the extracted text → registry produces a failed tool result for the model | `src/tools.ts:353-356` |
| args | `JSON.parse` of model args; non-object → `{}` so the server itself returns the "missing param" error | `src/tools.ts:325-329` |
| PTC / `run_code` mode | programmatic callers get the full `McpResult` (all blocks + structuredContent) | `src/tools.ts:40-44`, README `:130` |

### 7.4 Disconnect / reconnect / disposal — is the mount a disposable fiber? Yes.

| mechanism | where | guarantee |
|---|---|---|
| `apply` is `async`; activation **blocks** on first connect + initial `tools/list` sync so dependents see the tools the moment the fiber is ACTIVE | `src/index.ts:138-188` | fits §5.1 `assertEntriesActivated`: a server that never answers `initialize` holds the row in LOADING → boot audit (SDK default 60 s request timeout, README `:192`) |
| two effects on the fiber: namespace reservation and `connection.dispose()` | `src/index.ts:154-168`, `:175-177` | unloading the row (HMR, patch removal, agent disposal for a scoped row) stops reconnection, closes the client, awaits in-flight attempt + queued syncs, unregisters every tool it owns |
| **generation** model: each connect attempt = fresh `Client` + transport; `isCurrent(generation)` guards every callback; `syncChain` serializes all syncs so a swap never double-disposes/leaks | `src/connection.ts:137-170`, `:237-305` | |
| tool sync is **two-phase**: fetch the whole next generation first (any failure leaves the old generation registered), then dispose-old/register-new; a registry conflict on the `mcp__<server>__` namespace rolls back to **zero** tools (never partial) | `src/tools.ts:120-203` | model sees the full generation or none |
| disconnect trigger = transport `onclose` (stdio child death). **Streamable HTTP failures do not trigger the supervisor**; they surface per request via the SDK transport | `src/connection.ts:248-254`, README `:193` | for an HTTP-mounted Aleph, "reconnect" is per-call — Aleph should keep the endpoint up rather than expect respawn |
| backoff: doubles from 500 ms to 30 s; **one outage shares one budget** of 10 attempts; uptime ≥ `maxDelayMs` resets it; exhaustion **unregisters tools and stops** ("disposal (including HMR) is the only way back") | `src/connection.ts:1-13`, `:192-225` | |
| failed generation that does not close within 5 s → reconnect **stopped** (fail-closed against overlapping child processes) | `src/connection.ts:47-50`, `:288-293` | |

So: temporal composability holds for MCP mounts — an MCP server is a Cordis plugin row whose tools are effects on that row's fiber; remove the row and the tools vanish atomically; the model observes `tools/change`.

### 7.5 Hooks × MCP tools

| question | answer | where |
|---|---|---|
| Do CC-style `PreToolUse`/`PostToolUse` fire for MCP tools? | **Yes** — they are ordinary `tools/pre-execute` / `tools/post-execute` waterfalls; the bridge is not special-cased | `packages/hooks/hooks-claude-code/src/index.ts:237-264` |
| `tool_name` shape in the stdin payload | the **public** name `mcp__<serverName>__<rawName>` (`exec.name`), `tool_input` = parsed args, `tool_use_id` = `exec.callId`; `tool_response` = text projection only | `hooks-claude-code/src/index.ts:337-342` |
| matcher semantics | CC dialect: a pattern of only `[A-Za-z0-9_|]` is a literal pipe-list (`mcp__aleph__memory_search`); anything else (e.g. `mcp__aleph__.*`) is an **unanchored regex** | `packages/hooks/hook-protocol/src/matcher.ts:17-18`, `:57-65` |
| what a `deny` does to an MCP call | `{kind:'deny', reason}` before dispatch → error result to the model; the MCP server is never called | `tools/src/index.ts:581-584`, `hooks-claude-code/src/index.ts:240` |

### 7.6 Consequences for the Aleph MCP server face (one-row mount)

- **Transport**: implement **Streamable HTTP** (single `/mcp` endpoint, POST + optional SSE stream) with static-header auth; do not rely on legacy SSE or OAuth. stdio (`aleph-server mcp --stdio`) also works but then dsh owns the process lifetime and respawns it on crash — conflicts with Aleph's singleton `flock` (PROCESS_MANAGEMENT.md); HTTP against the running daemon is the right shape.
- **Capabilities**: advertise `tools` (+ `listChanged: true` — dsh honors it); prompts/resources are ignored, sampling/elicitation/roots must not be required.
- **Tool names**: keep raw names within `[A-Za-z0-9_-]` and short enough that `mcp__aleph__<name>` ≤ 64 chars, otherwise dsh hashes and truncates (still unique, but ugly for the model and for hook matchers).
- **Descriptions**: verbatim — the size discipline is Aleph's (R9 `prompt-size`), dsh will not trim.
- **Schemas**: `inputSchema` is forwarded untouched to DeepSeek; `outputSchema` only pays off if it stays inside dsh's supported JSON-schema subset (`assertSupportedJsonSchema`).
- **Results**: return `text` blocks; images only if PNG/JPEG/WebP/GIF canonical base64; avoid `resource`/`audio` (placeholders); use `isError: true` for failures (becomes a failed tool result, not a transport error); never require the tasks extension.
- **Liveness**: dsh blocks row activation on `initialize` + full `tools/list` with a 60 s SDK timeout; a slow `tools/list` delays dsh boot. Emit `notifications/tools/list_changed` when Aleph's catalog changes (skills/MCP/plugins loaded) — dsh will re-sync atomically.
- **Hooks**: Aleph tools will be gated by the user's CC hooks under the `mcp__aleph__*` name; document that shape for matcher authors.
