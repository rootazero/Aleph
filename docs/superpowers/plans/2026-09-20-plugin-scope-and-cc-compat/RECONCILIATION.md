# Cross-phase reconciliation rulings (lead, 2026-09-20) — every writer applies these to their own plan file

Phase order is fixed: P0 → P1 → P2 → P3 → P4 → P5 → P6 → P7 → P8 → P9(merge). **A later phase's anchors must be
against the shape the earlier phases leave behind**, not against `3ddc1f2e7`. Where your task anchors on code that
an earlier phase deletes or moves, re-anchor to that phase's plan code (the plan files quote the full new code:
`plan-P1.md` for P1, `plan-P2P3.md` for P2/P3, `plan-P4.md` for P4). Keep the `3ddc1f2e7` cite as history in
parentheses so the implementer can still recognise the old lines if any survive.

All five plan files live in this scratchpad directory. Edit YOUR file in place; do not touch the others.
Repo stays read-only.

## G — global rulings (all writers)

- G-1 `PluginId` = bare `String`; primitives take `&str`.
- G-2 `PluginStatus` **keeps today's names**: `Loaded / Disabled / Blocked(String) / Error(String)`; P3 adds
  `Pending { waiting_on: Vec<String> }` and removes `Overridden`. The contract's `Active` / `Failed{step,reason}`
  rename is DROPPED (a rename with no behaviour and a wire+Panel ripple). Mount failure writes
  `Error("<step>: <reason>")` at P1's single site (`lifecycle.rs::write_failed_row`). Any text that says
  `Active` / `Failed { .. }` must be corrected in your file.
- G-3 `Disposer = Box<dyn FnOnce() -> BoxFuture<'static, Result<(), String>> + Send>` (P1's flavour).
- G-4 `try_mcp_face() -> Option<&'static Arc<McpFace>>`.
- G-5 `ExtensionManager::scopes` is `crate::sync_primitives::Mutex<HashMap<String, EffectScope>>`; transitions
  serialise on the existing `load_guard`; `after_transition()` is the ONLY view-recompute site and runs once per
  public primitive.
- G-6 Docs: all phases stay in the worktree until P9 — **no cherry-picks**. But a code task that falsifies a
  code comment or a `docs/reference` line edits that line in the same commit (判据 §1). The rows marked
  "**P<n> writer**" in plan-P5P8's doc-code 同笔 matrix are yours to absorb (listed per phase below).
- G-7 User rulings received (2026-09-20, all four "recommended" options):
  (U-a) the nine tool-face-less `mcp.*` RPCs are ALL CUT (so all eleven `mcp.*` go);
  (U-b) hook stdin `hook_event_name` = **the spelling the hook was registered under** (`PreToolUse` for a hook
  registered as `PreToolUse`, `before_tool_call` for one registered as `before_tool_call`) — one field on the
  registration, one derivation, Aleph-native scripts unchanged; the alias table maps CC→Aleph for dispatch only;
  (U-c) `/cmd` body: raw `/cmd args` is persisted, the expanded body is delivered transiently (as planned);
  (U-d) MCP-face attended approval cards: accept the dangling-card shape and document it; "retire the card
  when the handler future drops" is recorded as a follow-up, not built.
- G-8 `ExecTier::Auto → "auto"` (a real value in CC's 6-value `permission_mode` enum, scan-cc-plugin-format.md:446).

## P1 (plan-P1.md) — foundation; changes other phases depend on

- R1.1 **`register_transient_servers` returns the receivers instead of spawning log-only watchers**:
  `Result<(Disposer, Vec<(String /*server_id*/, oneshot::Receiver<Result<(), String>>)>), String>`.
  The watcher moves into `lifecycle.rs` as `ExtensionManager::watch_server_starts(&self, plugin_id: &str,
  receivers: Vec<(String, oneshot::Receiver<…>)>)`: one spawned task per plugin awaiting all receivers, logging each
  outcome exactly as your current watcher does. The manager must also expose
  `pub async fn activation_settled(&self)` — completes when every watcher spawned so far has finished (mechanism
  is yours: e.g. a `tokio::sync::watch<usize>` of outstanding watchers, or a `Mutex<Vec<JoinHandle<()>>>` drained by
  the awaiter; no timer — the actor's own handshake cap bounds every receiver). P3.3 later turns
  `watch_server_starts` into the `Pending` writer and P3.4 awaits `activation_settled()` at boot. Update the P1.4
  doc comment ("the activation gate (P3) attaches Pending to these same receivers" → "P3 turns the watcher in
  `lifecycle.rs` into the readiness writer").
- R1.2 P1.10 keeps the `plugins.{load,unload}` CUT (P5.3 drops them). P1.10's commit ALSO carries the doc edit
  P5.3 lists for the namespace-inequality paragraph (`PLUGIN_SYSTEM.md:481-483`) — copy the replacement text
  from plan-P5P8.md Task P5.3 and put it in P1.10's Step 3 + `git add`.
- R1.3 P1.7: the `SkillInfo` for a plugin command must be built by ONE named `pub(crate)` function in
  `slash_effect.rs` (e.g. `plugin_command_skill_info(cmd: &SkillRegistration) -> SkillInfo`) and listed under
  "Produces" — P2.8 adds `plugin_id` to it and P4.7b projects `argument_hint` / `allowed_tools` / `model` into it.
  Say explicitly in P1.7 that this replaces `tool_catalog_init.rs:214-247` (which P1.10 deletes).
- R1.4 P1.9: name (under "Produces") the exact function + line where `mount` consults `plugins.toml`
  (`is_enabled(id)` today) and where the legacy `.disabled` migration runs — P4.10 changes both to
  `is_enabled_for(id, origin)` / origin-gated. Likewise name the one site where `load_all` builds the
  `PluginRecord` (P2.3 stamps `record.scope_key` there) and `write_failed_row` (P2.3's error-row key).
- R1.5 `after_transition`: P6.4 adds the `notify_tools_list_changed` line and extends the G3 `PINNED` list. Make
  P1.14's pinned list a `const` slice P6 can extend by one entry; say so in P1.14 "Produces".
- R1.6 Your open question 3 (CC `plugin.json` without `aleph.runtime="mcp"` parses `Static`, so `mcpServers`
  never mount) is assigned to **P4** (new task P4.15). Leave a one-line pointer in P1.9 where `mount` keys the
  `mcp_server` step on `kind == Mcp`.
- R1.7 Open questions 4/5/7/8: accepted as planned (extism lazy link; two mocks; eager WASM;
  `sync_runtime_snapshots` kept). Open question 6: G-2.
- R1.8 Your Contract delta 10 (`unregister_skills(&[String])`, ids owned by the disposer): kept — P2.8's
  `plugin_id` field is for visibility, not for unregistration. Remove any sentence saying P1 needs that field.

## P2/P3 (plan-P2P3.md)

- R2.1 Re-anchor P2.3: the `record.scope_key` stamp goes at P1.9's record-construction site in
  `lifecycle.rs::load_all`/`mount` (P1 will name it — until then cite "plan-P1 P1.9 `load_all`, the
  `PluginRecord` construction"); the error row → P1's `write_failed_row`; DROP the `mod.rs:1320-1323`
  (`reload_plugin` preserves the key) edit — P1.11 deletes the narrow `reload_plugin`; the new one is
  unmount+mount and re-derives the key from discovery.
- R2.2 Re-anchor P2.8: `tool_catalog_init.rs:227` (plugin-command `SkillInfo` literal) → P1.7's
  `plugin_command_skill_info` (R1.3). Check plan-P1 P1.8/P1.10 for what remains of `tool_catalog_init.rs` before
  citing `:178` / `:282-291` line numbers; cite by function name.
- R2.3 `commands.list` gets **no** `project_root` param (zero clients, 判据 §9). Keep the server-side
  derivation (`VisibilityCtx::for_session()` / `from_project_root`) and the manager-backed closure only.
- R2.4 `ScopeKey::from_discovery`: make `(Project, None)` **unrepresentable** — the Project arm takes the root
  by value; `DiscoveredPath::in_project(root, …)` is the only way to obtain `source: Project` (make `new` reject
  `Project`, or remove that route). No `debug_assert` + release fail-open.
- R2.5 P2.3's matches over `PluginOrigin` / `DiscoverySource` must be wildcard-free so that P4.10's new
  `PluginOrigin::ClaudeCache` variant is compiler-forced to get its `=> ScopeKey::Global` arm (P4.10 owns adding it).
  Say so in P2.3.
- R3.1 CUT the `runtime:<name>` arm of `derive_readiness` (zero producers: `.mcp.json` never sets
  `requires_runtime`; 判据 §7). `waiting_on` derives only `mcp:<server_id>` (+ `mcp:manager` when the MCP handle
  is absent, i.e. `scope.skipped()` contains `"mcp_server"`). Remove `requires_runtime` reads from P3.2 and its tests.
- R3.2 Re-anchor P3.3 (`sync_mcp_plugin_servers` and the boot catch-up task at `start/mod.rs:1429-1459` no longer
  exist after P1.4/P1.10): `refresh_readiness(&SyncOutcomes)` is fed from P1's
  `lifecycle.rs::watch_server_starts` (R1.1). P3.3 modifies that function: before spawning, write
  `Pending { waiting_on: ["mcp:<id>", …] }` via `with_pending`; after all receivers settle, build `SyncOutcomes`
  from their results and call `refresh_readiness`. `load_all` writes `mcp:manager` only in the no-handle case.
  Quote the P1 code you change (from plan-P1 P1.9 / P1.4), not `mod.rs:462-524`.
- R3.3 Re-anchor P3.4's boot call site: after P1.8 the first `load_all` (`ensure_loaded`, `agent_init/mod.rs:600`)
  is handle-complete and the catch-up task is gone. The gate runs on the boot path right after `ensure_loaded()`
  returns, in a spawned task: `manager.activation_settled().await` (R1.1; no extra timer) →
  `activation_gate::assess(&registry)` → `tracing::info!/warn!` → `ALEPH_ACTIVATION_GATE=fatal` ⇒ `exit(78)`.
  Never writes status. Accepted: `Pending` counts as `is_active()`; doctor builder shape; exit from a spawned task.
- R3.4 P3.1's `PLUGIN_SYSTEM.md:149-171` edit stays the minimal truth fix (P8.3(a) rewrites the table later,
  against your text). Also absorb from plan-P5P8's matrix: FL §5.9 Doctor — add the `extension/plugins-activated`
  line in P3.4's commit; FL §5.10 — add one sentence pointing at `visibility.rs` in P2.2's commit.
- R3.5 G-2: keep `Loaded/Error/Blocked`; remove any "if P1 renames…" hedges.

## P4 (plan-P4.md)

- R4.1 Open Q1: P0 stays a pre-step of P4.3 (as planned). Q3: `Auto → "auto"` (G-8). Q4: transient (U-c).
  Q5: accept `permissionDecision: "block"` as `Block` — one arm + one test in P4.2 (or P4.1, whichever owns
  `json_output.rs`). Q6: agent `permissionMode` = DEVIATION this round (record in P4.8 + the P4.14 table).
  Q7: mock request-log oracle is fine. Q8: no follow-up round is scheduled; keep the table as the record.
- R4.2 Open Q2 → U-b: `hook_event_name` echoes the registered spelling. Implement in P4.4 (event alignment):
  one field on the hook registration carrying the declared spelling (e.g. `HookRegistration.declared_event:
  String`, filled by the parser from the key as written), emitted by the payload builder in P4.3; the alias
  table maps CC→Aleph only for dispatch. Unit test: a hook registered as `PreToolUse` receives
  `"hook_event_name":"PreToolUse"`, one registered as `before_tool_call` receives `before_tool_call`.
  Flip acceptance row 42 from DEVIATION to CONNECT.
- R4.3 Re-anchor P4.10: `mod.rs:630-662` (enable gate + legacy `.disabled` migration) → P1.9's
  `lifecycle.rs::mount` (P1 names the exact function; cite "plan-P1 P1.9 mount admit gate" + the quoted code).
  Also: adding `PluginOrigin::ClaudeCache` will break P2.3's wildcard-free matches in `visibility.rs`
  (`ScopeKey::from_discovery`) and any other exhaustive `PluginOrigin` match — add an explicit step
  "add the `ClaudeCache => ScopeKey::Global` arm" with the file list (read plan-P2P3 P2.3).
- R4.4 Re-anchor P4.7b: `tool_catalog_init.rs:225-240` → P1.7's `slash_effect::plugin_command_skill_info`
  (R1.3); P1.10 deletes `tool_catalog_init.rs:214-247`. P4.7d's CUT of `plugins.executeCommand` + cascade
  (`execute_plugin_command`, `PluginLoader::execute_command`, `DirectCommandResult`, `ExecuteCommandParams`) also
  carries the `EXTENSION_SYSTEM.md:613-709` "Direct Commands" section edit — copy the replacement text from
  plan-P5P8 Task P5.3 into P4.7d Step 3 + `git add`.
- R4.5 New task **P4.15**: a CC `plugin.json` that declares `mcpServers` (or ships `.mcp.json`) without
  `"aleph": {"runtime": "mcp"}` parses as `PluginKind::Static` (`cc_plugin_json.rs:217`), so its servers never
  mount (P1.9 keys the `mcp_server` step on `kind == Mcp`). Make the CC adapter infer `Mcp` from the presence of
  `mcpServers` / `.mcp.json`; unit test on the adapter; extend the `cc-cache` QA stage to assert the fixture's
  server shows in `tools.catalog`. Cite `plant_plugins.py`'s `qa-inline` fixture (works today only by accident).
- R4.6 Absorb from plan-P5P8's matrix: FL §5.10 gains the exit-2 sentence in P4.1's commit;
  `PLUGIN_SYSTEM.md:499` ("source 分类只有一个答案") gains one sentence in P4.9's commit.
- R4.7 G-2: any `PluginStatus::Active`/`Failed` mention → today's names.

## P5/P8 (plan-P5P8.md)

- R5.1 P5.3 shrinks: `plugins.{load,unload}` → P1.10; `plugins.executeCommand` + cascade → P4.7d. Rewrite P5.3 as
  the post-condition census: grep proves `plugins.load|unload|executeCommand`, `execute_plugin_command`,
  `load_runtime_plugin`, `unload_runtime_plugin`, `DirectCommandResult`, `ExecuteCommandParams` are gone from
  `src interfaces shared qa crates docs/reference`; `method_census` rulings present; any leftover doc line.
  Keep the two doc replacement texts in P5.3 but mark them "carried by P1.10" / "carried by P4.7d".
- R5.2 Open Q1: `command.execute` CUT stands (zero clients). Q2 → U-a: P5.5 deletes all eleven `mcp.*`;
  P5.6 cascade includes `AggregateTools` / `aggregate_tools` (Q6) — pick a new positive control for the pin.
  Q3: P1.11 deletes `load_runtime_plugin` (verify in plan-P1 P1.11; if absent, keep it in P5.3's census as a
  must-be-gone symbol and tell the lead). Q4 → P4.7d. Q5: CUT `ToolCatalog::{is_namespace,list_namespace_children}`
  in P5.4 with their own tests. Q7: ONE archive at `docs/archive/` — P8.10 also moves
  `docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md` there, deletes `docs/reference/archive/`, and greps for
  links. Q8: no cherry-picks (G-6). Q9: delete `media-video`.
- R5.3 P8.3: (a) status table uses today's names + `Pending` (G-2), written against P3.1's text; (d) DEVIATION
  list = hook timeout 300 s, agent `permissionMode`, `~/.claude/settings.json` not read, hooks file at
  `~/.aleph/hooks.json`; `hook_event_name` is CONNECT (U-b), not a deviation. P8.5: version set
  `["2025-11-25","2025-06-18","2025-03-26"]` (`2026-07-28` sessionless dialect not spoken); one paragraph on the
  dangling attended card (U-d); `expose` live-apply exists (P6.9, below) — no restart caveat.
- R5.4 P8.7: add a D.0 entry for P6's finding "`OperatorApprovalRequester`'s zero-subscriber deny never fires
  on a real server (17 internal bus subscribers) — presence must be probed on the connection table"
  (number it after your D.0.198), with an E.4 trigger line.
- R5.5 P8.8 routing rows: `src/extension/` row cites `EXTENSION_SYSTEM.md` + `PLUGIN_SYSTEM.md` + FL §3.10 /
  §5.27, QA `qa/plugins/run.sh {scope,visibility,command,exit2,cc-cache}`; `src/gateway/mcp_face/` row cites
  GATEWAY.md MCP 面 + FL §5.27, QA `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`.

## P6/P7 (plan-P6.md)

- R6.1 **P6.4 adds the call**: `if let Some(face) = try_mcp_face() { face.notify_tools_list_changed(); }` inside
  P1's `lifecycle.rs::after_transition` (quote the P1.9 function from plan-P1.md) and extends P1.14's G3 `PINNED`
  const by `("notify_tools_list_changed(", "lifecycle.rs")` (read plan-P1 P1.14 for the exact shape). Files list
  gains `src/extension/lifecycle.rs` and the G3 test file; test: G3 stays green + a unit test that
  `after_transition` reaches a test-installed face (or, if the slot is install-once, a source-level pin).
- R6.2 Open Q1: `2026-07-28` deferred (not spoken; `server/discover` → -32601 so `auto` probes fall back).
  Q2 → U-d: accept + document (P6.4 module doc, P6.8 README "Approvals and timeouts", P8.5). Q7: flag only.
- R6.3 Open Q3 → new task **P6.9**: live-apply for `[mcp_server].expose` — `ReloadImpact::Live` arm for the
  section, `ArcSwap<BTreeSet<String>>` (or the crate's existing swap primitive) on the face, re-run the G5
  runtime validation, then `notify_tools_list_changed()`; test: change → `tools/list` differs + one notification
  per live session. Spec §3.7 promised it.
- R6.4 Open Q4: add a private limiter for **remote** `POST /mcp` (loopback exempt) in P6.6, same shape as the
  artifact route's bucket; one test (N+1th remote request → 429).
- R6.5 Open Q5: keep the `MCP_APPROVAL_HINT` append; structural `ToolError::ApprovalUnavailable` is a follow-up.
- R6.6 Open Q6: subtract `config_audit`, `node_list`, `user_profile` from the default exposure by name in a
  `DEFAULT_EXPOSE_EXCLUDES: &[(&str, &str /*reason*/)]` const (reason each), pinned by the P6.1 name-by-name test;
  the predicate stays the rule for everything else.
- R6.7 P7.1 `list_changed` stage: depends on P1 + R6.1 — after R6.1 it is P6's own wire, so the stage is PASS
  once P6 lands (drop the "FAIL until P1" note; P1 precedes P6).
- R6.8 Absorb from plan-P5P8's matrix: `GATEWAY.md:1154-1163` HTTP route bullet `- MCP face (`/mcp`)` in P6.7's
  commit; the pi-aleph pointer sentence in EXTENSION_SYSTEM "Node plugins run as MCP stdio servers" (post-P5.7
  text, see plan-P5P8 P5.7) in P6.8's commit.

## Reply format (each writer)

Edit your plan file in place, then reply in ≤ 12 lines: which rulings you applied (by id), which task ids changed,
new task ids, and anything a ruling asked for that the code makes impossible (say why; do not silently skip).
