# Severed-wire audit — Capability facade and projection host

- Date: 2026-11-20
- HEAD: `d57791367`
- Slice: `src/capability/zahir_facade.rs`, `src/capability/projection_host.rs`; only directly connected startup, registry, provider, and live-consumer files were followed.
- Method: static/read-only inspection; no source edits and no build/test execution. `rg` was used for every reported live-caller decision. Test modules are not counted as production callers.

## Production entry path

`src/bin/aleph-server/commands/start/mod.rs:311-322` constructs `ProjectionHost::mount_with_runtime_and_events`, installs it in `StartupResources` and the process-global slot, and awaits readiness. The same startup path registers terminal/UI special adapters through `register_special_tool_adapters_with_terminal_source` (`src/bin/aleph-server/commands/start/mod.rs:1183`). Live snapshot consumers are `src/gateway/execution_engine/run_loop/inner.rs:1836` and `src/gateway/mcp_face/mod.rs:229`; live facade discovery is `src/gateway/handlers/capabilities.rs:30`. Source registration and applier lifecycle are connected; the important failures are below.

## Findings

### PH-01 — Mixed registry/special cursor suppresses valid publications
- Form: other-static-defect
- Producer: `src/capability/zahir_facade.rs:1197` — `registry_cursor = Cursor(snap.revision().max(self.specials.revision()))`; special registrations independently advance their own revision at `src/capability/special.rs:516` and are produced dynamically at `src/gateway/capability_ui.rs:211`.
- Consumer: `src/capability/projection_host.rs:1618-1650` — `publication_already_reflected` and `process_registry_publication` compare a registry-only publication cursor with `ConsumerState.last_cursor`, although `enqueue_snapshot`/`enqueue_invalidation` store the mixed `snapshot.registry_cursor` (`src/capability/projection_host.rs:301-352`). Live consumers: `src/gateway/execution_engine/run_loop/inner.rs:1836`, `src/gateway/mcp_face/mod.rs:229`.
- Live caller search: `rg -n --glob '*.rs' 'current_snapshot\(' src/gateway` found the two production callers above. `rg -n 'registry_cursor|last_cursor|publication_already_reflected' src/capability/projection_host.rs src/capability/zahir_facade.rs` confirmed the mixed-vs-registry-only comparison. Tests cover ordinary replacement and special publication separately, but not a special-revision-ahead followed by a lower tool publication.
- Symptom: after a special/UI publication raises the mixed cursor above the tool registry revision, subsequent ordinary tool publications at or below that cursor are skipped. The default run-loop/MCP snapshot can therefore retain stale tools until the tool revision surpasses the special revision or an invalidation arrives. The reverse also holds: while the tool revision is ahead, special-only changes reuse the same `max` cursor and are dropped as duplicates. `ZahirFacade::drain_registry` (`src/capability/zahir_facade.rs:1344-1395`) has the same mixed-cursor comparison for scoped subscribers.
- Hypothesis: CUT — two independent revision domains are being compared as if they shared one sequence, and the default snapshot path is live.
- Confidence: high

### ZF-01 — Tool unregistrations are filtered out of scoped change streams
- Form: other-static-defect
- Producer: `src/tools/registry.rs:1896-1937` removes the current registry entry and publishes `RegistryChange::Unregistered`; `src/capability/zahir_facade.rs:1284-1291` converts it to `CapabilityChange::Unregistered`.
- Consumer: `src/capability/zahir_facade.rs:1303-1317` — `change_in_scope` first calls `self.adapter.lookup(id)`. `ToolBackendAdapter::lookup` (`src/capability/backend.rs:339-341`) reads the current registry descriptor, which is already absent after unregistration. The event is consequently removed by `process_registry_publication` (`src/capability/projection_host.rs:1594-1598`) and by `drain_registry` (`src/capability/zahir_facade.rs:1356-1359`).
- Live caller search: `rg -n --glob '*.rs' '\.attach\(' src desktop plugins crates tests` and `rg -n --glob '*.rs' '\.subscribe\(' src/capability src/gateway src/bin` found no production scoped-stream consumer; only tests and unrelated subscription types. Thus the filter is on a real producer/consumer arm, but that arm is presently test-only in production terms. Existing unregister tests (`src/capability/projection_host.rs:4021`, `src/capability/zahir_facade.rs:2661,3062`) check fresh snapshots, not delivery of the unregister change.
- Symptom: an attached/scoped subscriber advances its cursor without receiving removal or a replacement snapshot, leaving the removed capability in its held view.
- Hypothesis: CUT — scope filtering requires the very current entry that the unregister event has just removed; impact is dormant until a scoped stream consumer is wired.
- Confidence: high

### ZF-02 — Provider Resource metadata bypasses ownership visibility
- Form: other-static-defect
- Producer: `src/memory/runtime/mod.rs:246-252` supplies a production provider registration; `src/capability/zahir_facade.rs:1620-1659` appends `provider_adapter.enumerate(scope)` directly, and `src/capability/zahir_facade.rs:1927-1929` returns `provider_adapter.entry(id)` directly from `lookup_descriptor_for_cache`.
- Consumer: `describe` at `src/capability/zahir_facade.rs:1934-1957` is live at `src/gateway/handlers/capabilities.rs:30`; provider-backed `snapshot_entries_in_scope_from` feeds the projection host. In contrast, invocation does enforce ownership at `src/capability/zahir_facade.rs:1526-1565` (`resolve_provider`).
- Live caller search: `rg -n --glob '*.rs' '\.describe\(' src/gateway src/bin src/capability` found the live gateway/default-scope call and test scoped calls, but no production custom-visibility describe caller. `rg -n --glob '*.rs' '\.lookup_descriptor_for_cache\(' src` found only the trait wrapper and `src/capability/lazy_load.rs:36`, whose resolver has no production constructor caller. `rg -n 'resources_in_scope|resolve_provider|provider_adapter\.(enumerate|entry)' src/capability/zahir_facade.rs` confirmed that neither provider metadata arm performs the tree check used by tool/special/legacy-memory arms.
- Symptom: a scoped describe or metadata lookup can expose a provider Resource (including `memory.runtime`) under a visibility scope with no ownership binding, while `resolve` of that same descriptor fails closed as not visible. Discovery and resolution therefore disagree.
- Hypothesis: CUT — the provider backend filters kind/namespace/facet only (`src/capability/provider_backend.rs:25-49`), so the facade must apply the ownership visibility authority but does not. Current production custom-scope reach is limited; the default-scope gateway call does not by itself demonstrate the leak.
- Confidence: high

### PH-02 — Scoped `attach` stream and `ProjectionHandle` have no production consumer
- Form: 1
- Producer: `src/capability/projection_host.rs:955` — `ProjectionHost::attach`; handle operations at `src/capability/projection_host.rs:1278` (`recv`), `:1299` (`cancel`), and `:1306` (`close`).
- Consumer: no consumer found (grepped: `\.attach\(`, `ProjectionHandle`, `ProjectionEvent::`).
- Live caller search: `rg -n --glob '*.rs' '\.attach\(' src desktop plugins crates tests` returned only projection-host tests for this API; `rg -n --glob '*.rs' 'ProjectionHandle|ProjectionEvent::' src desktop plugins crates tests` found the definitions and tests, not a production consumer. The live run-loop/MCP paths consume only `current_snapshot`.
- Symptom: the scoped, live-updating projection stream is dark outside tests; production cannot observe its snapshot/update/close semantics, including ZF-01.
- Hypothesis: DECIDE — the implementation is substantial and tested, so this may be an intentionally exported future API rather than an accidentally omitted registration.
- Confidence: high

### PH-03 — Audit collector accessor has no production reader
- Form: 1
- Producer: `src/capability/projection_host.rs:869` — `ProjectionHost::audit_collector`; event-stream accessors at `src/capability/projection_host.rs:865` and `src/capability/zahir_facade.rs:478`; `AuditCollector::new` at `src/capability/audit.rs:148`.
- Consumer: no consumer found (grepped: `\.audit_collector\(`, `AuditCollector::new`, facade/host `event_stream`).
- Live caller search: `rg -n --glob '*.rs' '\.audit_collector\(' src desktop plugins crates tests` returned only projection-host tests; `rg -n --glob '*.rs' 'AuditCollector::new' src` returned only audit tests. Host/facade `event_stream` accessors only reach this test-only audit path. This is distinct from the live unscoped event bridge, which subscribes directly to `InMemoryEventStream` during startup.
- Symptom: capability audit records are collectable in tests but no production diagnostics/tool/server path reads them.
- Hypothesis: DECIDE — either register an audit consumer or explicitly retain this as embedding/test-only API; the separate diagnostics tool does not consume `AuditCollector`.
- Confidence: high

### ZF-03 — Facade scoped subscriptions have no production subscriber
- Form: 1
- Producer: `src/capability/zahir_facade.rs:1858` — `Zahir::subscribe_events`; `src/capability/zahir_facade.rs:2056` — `Zahir::subscribe`.
- Consumer: no consumer found (grepped: `\.subscribe_events\(`, facade `\.subscribe\(`).
- Live caller search: repository-wide `rg` found only tests and `AuditedZahir` forwarding (`src/capability/events.rs:896` and the corresponding subscribe wrapper). `AuditedZahir::new` itself has no production construction. Startup's live event bridge uses `capability_events.subscribe` directly, not `Zahir::subscribe_events`; host workers use registry publication subscriptions, not `Zahir::subscribe`.
- Symptom: both scoped capability-event and scoped snapshot-change subscription faces are unused by production; cursor and filtering behavior is not exercised by a live subscriber.
- Hypothesis: DECIDE — the live unscoped bridge means events as a whole are not dark, but these exported scoped faces are unconnected.
- Confidence: high

### ZF-04 — Transport projection and provider activation have no production caller
- Form: 1
- Producer: `src/capability/zahir_facade.rs:2072` — `Zahir::project`; `src/capability/zahir_facade.rs:2082` — `Zahir::activate`.
- Consumer: no consumer found (grepped: `\.project\(`, `\.activate\(` across `src desktop plugins crates tests`).
- Live caller search: the only facade-method hits are tests and `AuditedZahir` forwarding; no live server/run-loop/tool-runtime call was found. `TransportTarget::AcpJson` projection and provider activator execution therefore have no production path through `ZahirFacade`. Memory lifecycle can still be managed directly elsewhere; this finding is limited to the facade activation arm.
- Symptom: ACP/JSON transport projection and facade-driven provider activation are present and tested but dark in production.
- Hypothesis: DECIDE — no missing handler or stub was found; the missing arm is a caller/registration decision.
- Confidence: high

### ZF-05 — Metadata revision/cache lookup face is only reachable through a test-only resolver
- Form: 1
- Producer: `src/capability/zahir_facade.rs:1869` (`descriptor_revision`), `:1888` (`lookup_descriptor`), `:1893` (`lookup_descriptor_for_cache`).
- Consumer: `src/capability/lazy_load.rs:21,36` and `src/capability/resolver.rs:24,112`, but no production `CapabilityResolver::new` caller found.
- Live caller search: `rg -n --glob '*.rs' '\.descriptor_revision\(|\.lookup_descriptor\(|\.lookup_descriptor_for_cache\(' src desktop plugins crates tests` found only lazy-load, forwarding, and tests. `rg -n --glob '*.rs' 'CapabilityResolver::new' src desktop plugins crates tests` found only resolver tests. Live discovery calls `describe` directly.
- Symptom: the bounded metadata cache/live-revision freshness path is not used in production; the facade's three lookup methods have no live consumer.
- Hypothesis: DECIDE — this is an unwired optimization/API face, not a failure of the live describe path.
- Confidence: high

### ZF-06 — Alternate public constructors/mounts have no production caller
- Form: 1
- Producer: `src/capability/zahir_facade.rs:262` (`new_with_providers`), `:289` (`new_with_specials`), `:326` (`new_with_memory`), `:343` (`new_with_memory_and_policy_and_specials`), `:390` (`new_with_memory_and_policy`); `src/capability/projection_host.rs:571` (`mount_with_diagnostics`), `:586` (`mount_with_memory`), `:605` (`mount_with_policy`), `:796` (`mount_with_capacity`).
- Consumer: no production caller found for these direct entry points (tests and mutual delegation only).
- Live caller search: `rg -n --glob '*.rs' '\bZahirFacade::(new_with_providers|new_with_specials|new_with_memory|new_with_memory_and_policy_and_specials|new_with_memory_and_policy)\(' src desktop plugins crates tests` and equivalent `ProjectionHost::mount_with_*` searches found tests or delegation among these alternatives. Production uses `mount_with_runtime_and_events` and `new_with_memory_and_policy_and_specials_and_events`. Base `new`, `mount`, and `mount_with_runtime` are **not** included: they are reached internally by the live production mount.
- Symptom: several public embedding/configuration combinations are test-only; no production path validates their exact construction semantics.
- Hypothesis: DECIDE — the canonical production constructor is connected, so this is public-surface pruning/ownership rather than a dark canonical feature.
- Confidence: high

### ZF-07 — Two public convenience wrappers have no direct production caller
- Form: 1
- Producer: `src/capability/zahir_facade.rs:1128` — `reconcile_registry_bindings`; `src/capability/projection_host.rs:881` — `register_special_tool_adapters` (without terminal source).
- Consumer: no direct production caller found; live equivalents are `reconcile_snapshot` during host snapshotting and `register_special_tool_adapters_with_terminal_source` at `src/bin/aleph-server/commands/start/mod.rs:1183`.
- Live caller search: `rg -n --glob '*.rs' '\.reconcile_registry_bindings\(' src desktop plugins crates tests` returned facade/host tests only. `rg -n -F --glob '*.rs' '.register_special_tool_adapters(' src desktop plugins crates tests` returned `tests/gateway_ui_ingress.rs:128`; production calls the `with_terminal_source` variant.
- Symptom: the exact public wrappers are unused, although their underlying reconciliation and special-registration behavior is live through other methods.
- Hypothesis: DECIDE — do not misclassify the underlying features as severed; only the direct public wrapper symbols lack callers.
- Confidence: high

## noConsumer census summary

| Exported item/group | Production consumer | Disposition |
|---|---|---|
| Global `set_projection_host`, `projection_host`, `projection_host_slot` | startup, diagnostics, gateway/runtime | CONNECT |
| Canonical `mount`, `mount_with_runtime`, `mount_with_runtime_and_events`; base `ZahirFacade::new` | startup through internal mount helpers | CONNECT |
| Alternate constructors/mounts listed in ZF-06 | tests/delegation only | DECIDE (ZF-06) |
| `wait_until_ready`, `current_snapshot`, `facade`, `specials`, `provider`, `policy`, `terminal_source` | startup, gateway, tool adapters | CONNECT |
| `bind_terminal_source`, terminal invocation/session helpers, UI/special resolution | PTY/UI gateway and adapters | CONNECT |
| `register_special_tool_adapters_with_terminal_source` | startup | CONNECT |
| No-source registration wrapper; public reconciliation wrapper | tests only, equivalent live paths exist | DECIDE (ZF-07) |
| `close_and_await`, diagnostic status/hold/release/close | startup cleanup and diagnostics tool | CONNECT |
| `attach`, `ProjectionHandle::{recv,cancel,close}`, `ProjectionEvent` | tests only | DECIDE (PH-02) |
| Host/facade `event_stream`, `audit_collector` | audit tests only | DECIDE (PH-03) |
| `Zahir::{describe,resolve,validate_lease}` | gateway, run-loop, tool adapters | CONNECT |
| `Zahir::{subscribe,subscribe_events}` | tests/unused audited wrapper only | DECIDE (ZF-03) |
| `Zahir::{project,activate}` | tests/unused audited wrapper only | DECIDE (ZF-04) |
| `Zahir::{descriptor_revision,lookup_descriptor,lookup_descriptor_for_cache}` | test-only resolver/lazy-load path | DECIDE (ZF-05) |
| `HostSnapshot`, `ProjectionShutdownOutcome`, `DiagnosticPlane`, pending-capacity constants | live snapshot/cleanup/diagnostic paths | CONNECT |

## Negative sweep

- No `TODO`, `todo!()`, `unimplemented!()`, or non-test feature-gated implementation arm was found in the two slice files.
- No inert config field or route/name literal is owned by these two files; terminal/UI IDs and special registration names on the followed production path matched.
- The facade trait methods are implemented rather than stubbed; the no-consumer items above are wiring/API-surface findings, not claims that their bodies are empty.

## Not covered

- No exhaustive audit of `src/capability/{ownership,backend,provider_registry,special,special_dispatch,policy,events,audit}.rs`, gateway tool registry, or PTY/UI policy internals beyond the directly followed wires.
- No external/downstream crate callers outside this repository; “no production caller” means no in-repo non-test caller.
- No runtime/build/test confirmation, per the read-only static brief. In particular, PH-01's stale-snapshot sequence and ZF-02's custom-scope provider exposure are code-path proofs, not observed runtime failures.
- The remaining `ZahirFacade` methods classified CONNECT were not fully re-audited for semantic policy correctness after confirming their live callers; their connection status is not a proof that every invocation path is correct.
