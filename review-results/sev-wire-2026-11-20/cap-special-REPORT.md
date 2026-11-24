# Severed-wire audit — `src/capability` special kinds & tool adapters

- Repo: `/home/zou/data/workspace/Aleph-wt-sev3` @ HEAD `d57791367` (static audit, no diff)
- Slice: `src/capability/special.rs` (1104 L), `special_dispatch.rs` (49 L), `tool_adapters.rs` (990 L), `facade.rs` (621 L); intent evidence `special_tests.rs` (802 L)
- Method: full read of slice + `rg -n --glob '*.rs' SYMBOL` over `src/ desktop/ plugins/ crates/ tests/`; every candidate below was grep'd for a live production caller. `graphify` not used (graph file 185 MB; `rg` answers every "who calls X" question in this slice).
- Documented deferrals (AGENTS.md "Capability Phase 4 入口": EffectClaim producers, Safe Replay, external-effect exactly-once, universal scheduler, complete ACP server gate, **remaining CapabilityKind backends**) are **not** reported as defects; see §4 for the two places code contradicts that documentation.

**Summary:** 8 candidates + 4 non-wire static defects. The Tool/Terminal path is genuinely live end to end (mount → registry → domain policy → gateway); the severed ends cluster on the *capability-discovery / per-principal* face and on stub payloads.

---

## 1. Candidates

### C01 — `CapabilityInvocationOrigin` is threaded through the call chain and then discarded
- Form: stub-far-end
- Producer: `src/capability/special_dispatch.rs:22` — `pub(crate) enum CapabilityInvocationOrigin { CanonicalIngress, GatewayDomain }`
- Consumer: `src/capability/zahir_facade.rs:961` — `match origin { CapabilityInvocationOrigin::CanonicalIngress | CapabilityInvocationOrigin::GatewayDomain => {} }` (both arms empty; the value is never read again)
- Live caller search: `rg -n 'CapabilityInvocationOrigin' src/` → producers `tool_adapters.rs:686`, `zahir_facade.rs:605`, `zahir_facade.rs:725`, `gateway/capability_ui.rs:274`; consumers: only the empty match above. No other read of `origin` in `invoke_special` (body `zahir_facade.rs:946-1080`).
- Symptom: four call sites pay to declare which ingress a call came from (canonical Tool dispatch vs gateway domain RPC) and the only consumer does nothing with it. Any future rule that must distinguish the two ingresses (policy facts, approval, accounting, replay) has no hook: the enum looks like a live discriminator, so a reader assumes the distinction is already enforced.
- Hypothesis: DECIDE — either delete the parameter (it is pure noise today) or implement the arm; as written it is a classifier with no scoring site.
- Confidence: HIGH (code fact); the *intended* behaviour of each arm is unknown.

### C02 — `aleph/tools/ui_capability` special record's handler is a permanent `Unavailable` stub
- Form: stub-far-end
- Producer: `src/capability/tool_adapters.rs:849-852` — `Arc::new(UnavailableHandler { source: (kind == CapabilityKind::Terminal).then(|| registry.terminal_source_slot()) })`; for `CapabilityKind::Ui` `source` is `None`.
- Consumer: `src/capability/tool_adapters.rs:630-644` — `impl CapabilityHandler for UnavailableHandler::invoke` returns `Err(CapabilityDispatchError::Unavailable)` when `source` is `None`.
- Live caller search: `rg -n 'terminal_source_slot|bind_handler\(' src/` → the only slot accessor is `special.rs:481 terminal_source_slot()` (Terminal-only); the only binder is `tool_adapters.rs:124 TerminalCapabilitySource::bind_handler` (`mod.rs:1183` chain). No `bind_ui_source`/UI slot exists anywhere, so nothing can ever make the Ui record callable. Dispatch-side: `rg -n '"ui_capability"|aleph/tools/ui_capability' src/` → resolver only *names* it as the stable policy source for accounting (`zahir_facade.rs:989-990`), then dispatches the per-surface id `aleph/ui/surface-<uuid>`; the real UI path is the `ui_capability` **Tool** (`tool_adapters.rs:683`), which is wired.
- Symptom: `capabilities.describe` advertises `aleph/tools/ui_capability` with `facets.callable: true` (`src/gateway/handlers/capabilities.rs:53`), but any caller that resolves it gets a descriptor whose handler is a standing `Unavailable`. Today no client resolves it (the only generic invoke RPC is session-scoped `capabilities.terminal.invoke`), so the defect is latent — the first generic invoke/ACP client that trusts `describe` hits a permanent failure. `special_tests.rs:450` asserts the Terminal pre-bind `Unavailable` and never asserts the Ui record, so nothing pins the intent.
- Hypothesis: DECIDE — either register the Ui adapter with a handler that resolves the per-surface record (or with `facets.callable = false`), or document the stub; right now the record is advertised as callable and structurally cannot be.
- Confidence: MEDIUM (the code fact is HIGH; "intentional placeholder until UI gains a generic dispatch" is plausible but is written down nowhere).

### C03 — `capabilities.describe` can never describe the per-principal records it exists for (visibility is compared as a literal key, not as a wildcard)
- Form: name-path-drift (key mismatch between the registration key and the query key)
- Producer: `src/capability/tool_adapters.rs:147-149` — `register_session` builds `VisibilityScope { principal: created_by.map(str::to_owned), ..Default }` and registers `aleph/terminal/session-<id>` with it (`:161`), binding the same scope in the ownership tree (`:174 facade.bind_terminal_session`). Same shape for UI surfaces: `src/gateway/capability_ui.rs:191-204` registers with `visibility.principal = caller`.
- Consumer: `src/gateway/handlers/capabilities.rs:20-24` — `let scope = Scope { namespace, ..Scope::default() }` and `facade.describe(scope)`.
- Live caller search: `rg -n 'fn describe' src/capability/` → `special.rs:903` (filters **only** `kind` and `namespace`; `scope.visibility` and `scope.facet` are ignored there) and `zahir_facade.rs:1945-1994` (the real filter: `self.tree.generation(&descriptor.id, &scope.visibility).is_some()`, `:1964-1968`). `rg -n 'fn generation|fn generations_for' src/capability/ownership.rs` → `:568-576` and `:718-734` both do `inner.bindings.get(&Self::key(capability, visibility))` where `key` (`:357-362`) is **plain struct equality** — no wildcard. `rg -n 'impl VisibilityScope' src/` → **no impl at all**: the "`None` = unrestricted … `default()` is visible everywhere" contract in the module doc `ownership.rs:74-79,82-93` is unimplemented.
- Symptom: an operator calls `capabilities.describe` and sees only the two global adapter records; every live terminal session and every attached UI surface is invisible, because they were registered under `principal: Some(..)` and the query asks with `principal: None`. Conversely a session spawned with `created_by == None` is registered under the all-`None` key and therefore shows up in **every** caller's describe. The discovery RPC's whole per-principal dimension is dead in both directions.
- Hypothesis: CUT — the wire exists on both ends and the two keys can never meet. Fix is one of: give `VisibilityScope` a real `covers()`/`admits()` and use it in `generation`/`generations_for`, or derive the query scope from the caller at `handlers/capabilities.rs:20-24`. (Either way it is a `src/capability/ownership.rs` change, outside this slice.)
- Confidence: HIGH on the mechanism; MEDIUM on which side is meant to change (the doc says the *scope* is the wildcard, the code says the *binding* is exact).

### C04 — the live capability change stream (`Zahir::subscribe` / `subscribe_events`) has no production consumer
- Form: no-consumer
- Producer: `src/capability/facade.rs:397` — `fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream`, `facade.rs:399` — `fn subscribe_events(...)`; implemented at `zahir_facade.rs:2056` and `zahir_facade.rs:1858`.
- Consumer: NONE FOUND — `rg -n '\.subscribe\(' src/ | rg -v 'specials\.subscribe|registry\.subscribe|bus\.subscribe|broadcast'` → every remaining hit is a tool registry / MCP manager / config watcher / browser hub receiver (`tools/registration_scope.rs:242,300`, `start/mod.rs:341,1596`, `browser/live/*`, `tools/registry.rs:2163,3876`). The only `Zahir::subscribe` callers repo-wide are tests: `zahir_facade.rs:2605/2729`, `resolver_tests.rs:75`, plus the test-only `AuditedZahir` delegation in `events.rs:888/896`.
- Symptom: `CapabilityChangeStream` (history limit 256, `invalidate_and_resync`, cohort append — `facade.rs:213-320`) is a fully built live-change contract that no production component consumes. Clients are told capability state only when they poll `capabilities.describe`; a capability registered/unregistered at runtime (a pty session opens, a UI surface attaches) produces no pushed change. The one *internal* change path that is wired is the projection host's own `specials.subscribe()` (`projection_host.rs:704`, consumed by `source_worker`), i.e. the facade-level stream is one layer above what production actually uses.
- Hypothesis: DECIDE — this is a public trait method on the type AGENTS.md advertises as the mounted facade (`ZahirFacade`); either wire it to a gateway subscription (there is no `capabilities.subscribe` RPC — `src/gateway/handlers/mod.rs:331-334` registers only `capabilities.describe` + `capabilities.terminal.invoke`) or mark it `#[cfg(test)]`-only so it is not read as a shipped feature.
- Confidence: HIGH.

### C05 — `CapturedSpecialEntry::canonical_schema()` has zero callers
- Form: no-consumer
- Producer: `src/capability/special.rs:333` — `pub fn canonical_schema(&self) -> &[u8]`
- Consumer: NONE FOUND — `rg -n 'canonical_schema' src/` → `special.rs:219` (field), `:231` (`PartialEq`), `:333-334` (this accessor), `:549/551` (registration), `:614` (store), `:659` (clone into the captured entry), `:1100` (`canonical_schema_bytes` helper). No read of the accessor anywhere, including tests.
- Symptom: the canonical (fingerprint-bearing) schema bytes are computed, validated on registration, and stored, but no dispatch, describe, or RPC path ever emits them — `capabilities.describe` emits `capture.schema()` (the raw JSON), not the canonical bytes (`handlers/capabilities.rs:56-57`).
- Hypothesis: CUT — dead accessor; the canonical bytes only matter for fingerprint comparison, which happens inside `register_record`.
- Confidence: HIGH.

### C06 — three `pub fn` adapter-registration entry points are test-only, and two of them silently drop the Tool projection
- Form: no-consumer
- Producer: `src/capability/tool_adapters.rs:805` `register_canonical_adapters`, `:814 register_canonical_adapters_with_tools`, `:821 register_canonical_adapters_with_tools_and_source` (all `pub`); `:829 register_canonical_adapters_with_tools_bundle` is `pub(crate)` and is the real one.
- Consumer: NONE FOUND in production — `rg -n 'register_canonical_adapters' src/` → production mount goes through `projection_host.rs:901 register_special_tool_adapters_with_terminal_source` (from `bin/aleph-server/commands/start/mod.rs:1183`). All other hits are tests: `special_tests.rs:247/450/485/562`, `projection_host.rs:1899/2002`, `gateway/capability_events_tests.rs:406/447/845/1660`, `ui_surface_tests.rs:93`, `capability_terminal_tests.rs:14/253`, `capability_init.rs:204` (inside `#[cfg(test)] mod tests`).
- Symptom: `register_canonical_adapters` passes `None` for tools (`:808`) and `register_canonical_adapters_with_tools` passes `None` for the source (`:818`), so a future caller of either gets special records with **no** model-facing Tool and a Terminal adapter whose handler slot is never bound. The API offers three plausible-looking entry points of which exactly one combination is correct, and the wrong ones fail silently (records register fine, dispatch returns `Unavailable`).
- Hypothesis: CUT — reduce to the single `pub(crate)` bundle (or `#[cfg(test)]` the thin wrappers).
- Confidence: HIGH.

### C07 — the model-facing description of both adapters is a machine-generated placeholder
- Form: stub-far-end (the far end is the LLM's tool list)
- Producer: `src/capability/tool_adapters.rs:732-745` — `tool_definition` sets `description: format!("Canonical {name} capability adapter")`; `:746-767 tool_descriptor` sets `replay_policy: ReplayPolicy::Unsafe`, `concurrent_safe: false`, `idempotent: false`, `max_duration_ms: None`, `implementation_contract: None`.
- Consumer: `src/tools/registry.rs` catalog → the model's tool list (`start/mod.rs:1944-1947` passes the same registry to the live agent). The only description the model ever sees for `terminal_capability` is the string `"Canonical terminal_capability capability adapter"`; the enum's variants (`PtyToolParams` Spawn/List/Close) carry the real affordances but the prose does not.
- Live caller search: `rg -n '"terminal_capability"' src/` → no curated description table, no prompt section, no doc string anywhere overrides it (all other hits are tests or the stable-policy-source name at `zahir_facade.rs:989`).
- Symptom: the model is handed two tools it can discover but cannot understand from their description, and `terminal_capability` requires approval (`requires_approval = (kind == Terminal)`, `tool_adapters.rs:869`) — an approval prompt whose text is "Canonical terminal_capability capability adapter" is a UX and safety problem (the human cannot tell what they are approving). This is R9 territory ("智慧在 Prompt 中"): the intelligence that belongs in the description is absent.
- Hypothesis: DECIDE — write real descriptions (and an approval summary) for both adapters.
- Confidence: HIGH on the placeholder text; MEDIUM on whether a prompt layer adds guidance elsewhere (no evidence found, but prompt assembly is outside this slice).

### C08 — `TerminalCapabilitySource`'s `policy` constructor argument is accepted and never read
- Form: inert-config (constructor knob)
- Producer: `src/capability/tool_adapters.rs:44` — `_policy: Arc<crate::capability::policy::PolicyEngine>`, set at `:66`; production passes the real engine: `src/bin/aleph-server/commands/start/mod.rs:1157 Arc::clone(&capability_policy)`.
- Consumer: NONE FOUND — `rg -n '_policy' src/capability/tool_adapters.rs` → field declaration and initializer only. (Adjacent `policy_snapshot()` at `:103` reads the pty *manager* generation, not this engine.)
- Symptom: the mount site reads as "the terminal source enforces the capability policy"; the source enforces nothing — admission lives in `ZahirFacade` and `validate_lease`. Anyone debugging a denied/allowed terminal call looks at the source, sees a policy handle it "must" be using, and finds nothing. (No behavioural break: the facade holds the authoritative `Arc`.)
- Hypothesis: CUT — drop the parameter, or actually consult it and delete the duplicate authority.
- Confidence: HIGH.

---

## 2. 非连线类静态缺陷

1. **`VisibilityScope`'s documented wildcard does not exist** — `src/capability/ownership.rs:74-79,82-93` promise "`None` = unrestricted … `default()` is visible everywhere"; `:357-362 key()` is exact struct equality and `rg -n 'impl VisibilityScope' src/` finds **no impl**. Every doc that leans on wildcard semantics (`Scope::default()` at `handlers/capabilities.rs:23`, `ownership.rs:568/718`) is wrong. This is the root cause of C03 and the highest-value fix in the slice's neighbourhood. Two shipped shapes depend on opposite readings: registration uses principal-scoped keys, discovery uses the default key.
2. **The special record shadows the Tool descriptor of the same id** — tool `terminal_capability` maps to `aleph/tools/terminal_capability` (`zahir_facade.rs:56-61 tool_id` + `backend.rs:41 TOOL_NAMESPACE`), the special adapter registers the identical id (`tool_adapters.rs:30`). `describe` drops the Tool descriptor for any id present in the special set (`zahir_facade.rs:1982`) and `resolve` checks `specials.capture` first (`:1999`), so the Tool descriptor's `requires_confirmation` / facets / registry revision are unobservable through the capability face, and a lease built from the same id always carries `BackendSourceEvidence::Special`. Consistent (dedupe exists) but one wire has two descriptors — any future consumer that diffs "descriptor as advertised" vs "descriptor as leased" will see a revision mismatch by construction.
3. **`Scope.facet` is never set in production** — `facet` is filtered in `backend.rs:309` and `provider_backend.rs:33`, but `rg -n '\bfacet:' src/` shows every assignment lives in test code (`backend.rs:689+`, `resolver.rs:201`, `zahir_facade.rs:2217+`). The facet dimension is a knob nothing turns, and `special.rs:903 describe` ignores `facet` altogether — so even if someone set it, special records would not honour it.
4. **`dispatch_legacy`'s twin surface** — `special.rs:953 dispatch_legacy` is `#[cfg_attr(not(test), expect(dead_code))]` and `backend.rs:204 LegacyCapabilityHandlerRegistry::dispatch` is likewise annotated "no production caller is wired yet". Self-documented; listed only so it is not re-reported as new. The same holds for `src/capability/ambient.rs:223/232/242` (`register_process_handler` / `register_user_handler` / `register_environment_handler` — the latter two have **zero** callers, not even tests) and `src/capability/terminal.rs:236` / `src/capability/ui.rs:223 register_handler`: these are the documented-deferred "remaining CapabilityKind backends" (AGENTS.md), **not** defects.

---

## 3. Verified-live (explicitly not reported)

- Production mount: `start/mod.rs:1154-1190` → `TerminalCapabilitySource::new` → `facade.bind_terminal_source` → `terminal_source.bind_handler()` → `projection_host.register_special_tool_adapters_with_terminal_source(Some(&terminal_source))`; `projection_host.rs:901` is the only production path into `register_canonical_adapters_with_tools_bundle`.
- Tool chain: `register_domain_adapter` stamps `domain_policy` (`tools/registry.rs:1597-1599`) → `tools/scoped/dispatch.rs:1351/1484/1387` builds and scopes `DOMAIN_TOOL_CALL` → `CanonicalToolAdapter::call` (`tool_adapters.rs:660-690`) → `TerminalCapabilitySource::invoke_tool` (`:528`). Approval gate reads the descriptor's `requires_confirmation`, not a name list.
- Session lifecycle: `gateway/pty/manager.rs:830/861/1065/1501/1657` → `register_session` / `commit_session` / `withdraw_session_deferred` (owner-guarded by `unregister_owned_deferred`).
- UI surface face: `capability_ui.rs:204` registers `aleph/ui/surface-<uuid>` and `:243 handle_invoke` → `invoke_ui_gateway`; the five `capabilities.ui.*` RPCs are registered by `server/mod.rs:875-903`, reached from production `start/mod.rs:984 set_session_store`.
- SpecialPublication broadcast is consumed (`projection_host.rs:704/1511`); `CapabilityCreated/Invoked/Activated/Detaching/ConstraintViolation` all have production producers and a consumer in `audit.rs`.
- `ToolBackendAdapter::snapshot_capabilities()` (`backend.rs:321`) is CapabilityBackend-internal (`:359`) + tests — no severed external consumer.
- No ghost RPC: `rg -n 'capability_ui::handle_' src/` → all five registered; `capabilities.describe` / `capabilities.terminal.invoke` are `Class::Open`/`Admin` in `method_census.rs:122-123` and absent from `method_visibility.rs` by design (that table is explicitly **not** a dispatch gate, `method_visibility.rs:6-9`).

## 4. 代码与 AGENTS.md 文档不一致之处

- AGENTS.md says the phase-4 mount is "bounded Tool-only + Memory Resource composition" — consistent with C02/C04 (special kinds other than Terminal/Ui have no adapter, and the special set has no live-change consumer). **No contradiction**, except that `ZahirFacade::subscribe` is presented as implemented (`describe / resolve / validate_lease / subscribe / project are implemented`) while nothing consumes it (C04) — that sentence reads as a shipped feature.

## 5. 未覆盖的接缝

- **Prompt assembly / tool-catalog filtering**: I did not trace how the two adapter definitions reach the model (system-prompt rendering, mode/allowlist filters, sub-agent tool subsets). C07 assumes the registry description is what the model sees; a prompt layer could add guidance I did not find by `rg`.
- **`src/capability/policy.rs` / `policy_runtime.rs` internals**: I confirmed the facade consumes policy facts (`special_policy_facts`) but did not audit the fact vocabulary or the constraint compile path — a producer/consumer mismatch inside the policy engine would be outside this slice.
- **ACP / MCP projection (`Zahir::project`, `TransportTarget::{Mcp,AcpJson,StateDatabase}`)**: I read the trait and its `project` impl signature only; whether the MCP tool list is derived from the capability snapshot or directly from the tool registry (which would make the special records irrelevant to MCP) was not verified.
- **`graphify` semantic cross-check**: not used (185 MB graph; every question in this slice was answerable with exact `rg`). Consequently "no consumer" claims rest on textual grep — a consumer reaching these symbols through a macro, a re-export alias, or a `dyn` trait object registered by string name would be missed. The `Zahir` trait is object-safe and mounted as `Arc<dyn Zahir>`; I did check that no production `Arc<dyn Zahir>` holder calls `subscribe` (`rg -n 'dyn Zahir' src/`), but a `CapabilityChangeStream` consumer living outside `src/` (desktop shell) would not appear.
- **Runtime behaviour**: no `cargo`, no test execution (brief forbids it). C02's "latent" claim and C03's user-visible effect are static deductions; a single `capabilities.describe` call against a mounted host with one open pty session would settle both.
- **Re-verification against prior reports**: `review-results/severed-wire-2026-08-17/capability/REPORT.md` covers the `capability_wiring` diagnostic checker and `review-results/2026-10-30-rust-doctor-canvas-capability-clarification.md` covers `effect_claim`/`backend`/`provider` items — neither overlaps this slice's candidates, so nothing here is a 回归 of a previously reported item.
