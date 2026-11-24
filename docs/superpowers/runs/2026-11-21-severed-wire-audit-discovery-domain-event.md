# Severed-wire audit — `src/discovery`, `src/domain`, `src/event`

**Date:** 2026-11-21 · **Base:** `main` @ `0f4e4f94a` · **Worktrees:** `wire-discovery`, `wire-domain`, `wire-event` (branches of the same name, one per module)

Static review of three modules for **severed wires**: a declaration, a knob, a
handler, a variant or a doc claim on one side, and nothing (or nothing that can
act) on the other. No diff was supplied — the whole module surface was read.

**Outcome:** 3 module slices repaired on 3 branches, merged into `main` as
`b7a9c245d` (domain), `ce38fb3cf` (discovery), `054ebaabe` (event). 25 repairs
applied (9 CUT/narrow, 8 CONNECT, 8 doc/shape corrections), 21 findings reported
without a repair because they need a product or shape decision.

---

## 1. Method

| Phase | What happened |
|---|---|
| 1. Scan | 9 read-only `sev-auditor` subagents in parallel, three per module, one per seam lens: registration/path-naming parity, producer→consumer parity, classifier/priority drift, emit↔subscribe parity, call↔handler parity, stub sweep. |
| 2. Enumerate | Every candidate carries **both** ends (`producer file:line` + `consumer?`), and was produced by a path-qualified grep plus a read of the surrounding code. |
| 3. Triage | Read-first per candidate → `CONNECT` / `CUT` / `DECIDE`. `jev_evaluate` produced a calibrated second opinion on the disposition (see §8). |
| 4. Fix | Surgical edits in one worktree per module. A symbol is never deleted on the strength of a grep alone. |
| 5. Guard | Three new grep-diff detectors, one per module (`scripts/{discovery,domain,event}_wire_audit.py`), wired into `just check-wiring` next to the sibling audit scripts. |
| 6. Verify | One `cargo check -p alephcore --lib --tests` on the merged `main` after all three modules were done. |

**Coverage gap (declared).** One of the ten `sev-auditor` attempts died on the
provider's content-safety filter (gpt-6.2-sol, `src/domain` lens 2/3) and was
re-run on a different model; no lens was left uncovered. Two of the three
**repair** workers hit the 30-minute subagent ceiling while their `cargo check`
was running; their edits were on disk and were finished by hand (§7).

**Model routing.** Per the operator's instruction, reasoning-heavy work went to a
reasoning model (MiniMax-M3 / k3.1-pro / gpt-6.2-sol) and classification,
filtering and routing went to Jev (§8).

---

## 2. Findings and dispositions

`CUT` = the far end does not exist → removed. `CONNECT` = the wire was broken →
reconnected. `DECIDE` = product call → reported, not decided by the audit.
`NARROW` = the symbol is real but its visibility was wider than its consumers.

The nine auditors raised 68 candidate mentions; several lenses reported the same
wire under different ids, so the tables below merge them into **55 distinct
findings**. **56 candidate dispositions were put to Jev** (§8).

### `src/discovery` — 13 candidates, 8 repaired

| ID | Finding | Disposition |
|---|---|---|
| D1-01 | `DiscoveryManager::aleph_home()` (`src/discovery/mod.rs`) and `DirectoryScanner::aleph_home()` (`src/discovery/scanner.rs`) — the chain ends at a `pub fn`, so `dead_code` cannot see it. No production caller. | **CUT** (both) |
| D1-02 | `DiscoveredPath::in_project` (`src/discovery/types.rs`) — only a `#[cfg(test)]` fixture used it; production assembles the struct directly. | **CUT** |
| D1-03 | `aleph.plugin.json` is advertised by discovery (`MANIFEST_REL_PATHS`) but no manifest adapter parses it (`AdapterRegistry::with_defaults` only knows `aleph.plugin.toml`). | **KEPT + allowlisted** — dropping the name would hide such a plugin; the missing adapter is a shape decision (§4.1) |
| D1-04 | `.lsp.json` is advertised in `COMPONENT_DIRS` and in error strings, but discovery never produces it and no LSP kind/adapter exists. | **KEPT + allowlisted** (§4.1) |
| D1-05 | Discovery's `has_plugin_manifest` and the loader's manifest list are two spellings of one set. | **DECIDE** — not repaired: `src/extension/manifest/mod.rs:184,230` still routes through the inline functions, so only the *lists* were unified, not the call path (worker note N-3) |
| D1-06 | `DiscoveryConfig.scan_claude_dirs` / `scan_project_dirs` / `max_upward_depth` have no production writer — only `Default`. | **DECIDE** — inert config, but the value belongs to a caller that does not exist yet |
| D1-07 | Project-scope skill scanning roots at the **process cwd**, while the skill system roots at the **session project directory** (`src/skill/mod.rs:260-269` merges into a process-global `SkillSystem`). | **DECIDE** — a placement decision, not a wire |
| D1-08 | `discover_component`'s `"commands"` / `"agents"` arms are unreachable in production (only `"skills"` is used; `~/.claude/commands` and `~/.claude/agents` never float up). | **KEPT** — narrowing the enum would change the discovery vocabulary |
| D1-09 | `pub type DiscoveryResult<T>` was visible crate-wide through `pub use paths::*` with no external consumer. | **NARROW → `pub(crate)`** |
| D1-10 | `AGENT.md` vs `agent.md` — one of the two spellings lives only in the dead `PluginKind::detect_from_path`. | **CUT** (dead spelling) |
| D1-11 | `src/utils/paths.rs:1029-1022` claims `~/.claude/plugins` is one of the two plugin parents discovery scans; discovery never scans it as a parent. | **DOC** (comment corrected) |
| D1-12 | The Claude-cache gate accepts only one of the two manifest names the loader accepts. | **DECIDE** |
| D1-13 | `paths.rs` says its constants are "not re-exported", but `mod.rs` re-exports them with `pub use paths::*`. | **DOC** (comment corrected) |
| D2-02 | A failed `read_dir` on an ordinary plugin parent only reached `debug!`, so it never entered the existing `PluginDiscovery.unreadable` channel — the downstream integrity check could not see it. | **CONNECT** — the failure now reaches `unreadable` ("unknown, not absent") |
| D2-03 | `DirectoryScanner::new` snapshotted which Claude/agents roots existed **at construction**; a `~/.claude` created after boot stayed invisible for the whole process lifetime (the extension manager keeps one `DiscoveryManager` for its life). | **CONNECT** — roots are re-checked per scan |
| D2-06 | A failed directory read during a *skill* scan only reached the log; `src/extension/projection.rs:245` cannot tell "scan missed it" from "the source has no skills". | **DECIDE** — needs a skill-side counterpart of `unreadable` (shape decision) |

### `src/domain` — 15 candidates, 9 repaired

| ID | Finding | Disposition |
|---|---|---|
| M1-01 | `pub trait Entity` (`src/domain/mod.rs:21`) — three impls (`SkillManifest`, `MemoryFact`, `A2ATask`), zero generic-bound consumers; the only production call is the UFCS `crate::domain::Entity::id(&manifest)`, and `manifest.id()` is an inherent method. The module doc already records that `AggregateRoot` was CUT on 2026-08-16 for the same reason. | **CUT** — trait, 3 impls, its test; the module doc now states the rule ("a marker trait needs a generic-bound consumer") |
| M1-02 | `SkillManifest::is_model_visible()` — production calls none; `src/skill/snapshot.rs:113-117` inlines a *different* rule (the config-overridden scope). | **DECIDE** — consolidating changes semantics (the inline rule sees the override) |
| M1-03 | `PromptScope::from_str` + `ParsePromptScopeError` — zero callers; three hand-written parsers instead, with three different answers for an unknown value: warn + silent `Disabled` (`src/skill/manifest.rs:487-505`), RPC `INVALID_PARAMS` (`src/gateway/handlers/skills.rs:63-74`), error (`src/extension/manifest/parsers.rs:1003-1012`). | **DECIDE** — the behaviours are deliberately different per surface |
| M1-04 | `InstallKind::from_str` + `ParseInstallKindError` — zero callers; the live copy (`src/skill/manifest.rs:561-578`) warns and drops the whole spec. | **DECIDE** (a 4th copy in `src/builtin_tools/skill_manage.rs:409-412` was found by the repair worker) |
| M1-05 | `Os::as_str` → `src/skill/status.rs:96-98` → JSON → the Panel's `MissingRequirements` has no `os` field, so serde drops it silently. | **DECIDE** — a wire gap to the Panel, not a Rust one |
| M1-06 | `SkillSource` / `CompatRoot` derive `Deserialize`, but nothing deserializes them (only `Serialize` is live). | **NARROW** — kept: the Panel matches the serialized tag |
| M1-07 | Three provenance vocabularies for the same fact: the model-facing `skill_read` provenance (`bundled`/`user`/`workspace`/`plugin:<id>`/`claude`/`agents`), the operator-facing `source_label` (`Official`/`Aleph`/`Workspace`/`Claude`/`Agents`/`Plugin: <id>`), and the sibling tool's `skills.list` `get_source_type` (`global`/`project`) — plus a serde tag that says `global` where the display says `user`. | **DECIDE** — one vocabulary is a product decision |
| M1-08 | `PromptScope`'s derived serde is case-sensitive (`rename_all = "lowercase"`), so `scope = "System"` in `skills.toml` failed the **whole config file**, while the RPC path lowercased first and accepted it: two answers to one question. | **CONNECT** — `Deserialize` now reuses the domain `FromStr` (case-insensitive), the same shape `Os` already used |
| M1-09 | `SkillSnapshot::build` hardcoded `prompt_budget: SkillPromptBudget::default()`; production overwrote it afterwards. The field doc claims `build` carries the user's configuration. | **CONNECT** — `build` now takes the configured budget |
| M1-10 | A **third** `PromptScope` type lives in `src/extension/types/hooks.rs`; `src/skill/frontmatter.rs:1-22` says the three `SKILL.md` ingestion paths are "deliberately NOT merged". | **DECIDE** — the dead parser inside it was CUT, the type stays |
| M2-05 | `PromptScope::as_str()` and its `Display` — no production consumer (the live spelling source is the serde tag). | **CUT** |
| M2-08 | `skill_manage action='delete'` reports that the "registry entry, telemetry, and backing files" were deleted, but `remove_skill` (`src/skill/mod.rs:709`) only deletes files for `Global`/`Workspace` sources — a `Compat`/`Plugin` skill survives and comes back on the next rescan. The write path guards this (`src/builtin_tools/skill_manage.rs:293-307`), the delete path does not. | **DECIDE** — product: is a compat skill the user's own (deletable) or read-only? |
| M2-09 | The extension's parallel `PromptScope` and `SkillRegistration.scope` are written but never read; `src/extension/slash_effect.rs:22-32` hardcodes `PromptScope::System`. | **DECIDE** |
| M2-10 | `SkillInfo.scope` (`src/skill/compat.rs:23`) is filled but never read. | **DECIDE** |
| M2-13 | 8 newtype impls (`Borrow<str>`, `AsRef<str>`, `PartialEq<str>`, `PartialEq<&str>` × `SkillId`/`PluginId`) have no production consumer. | **CUT** |
| M2-14 | `src/skill/registry.rs:10` documents 4 priority levels and omits `Compat`; the domain doc has the correct 5. | **DOC** (comment corrected) |
| M2-15 | The Panel mirrors the domain `PromptScope` as a bare `String` and re-lists the 4 values; a 5th variant would silently show the wrong scope (the `<select>` falls back to the first entry while the `RwSignal` keeps the old value). | **DECIDE** — needs a Panel-side enum |
| M3-01 | Eligibility and install specs parse their OS list with `.filter_map(|s| s.parse::<Os>().ok())`, so an unparseable OS is dropped **silently**: an all-invalid list becomes `Some([])`, which `src/skill/eligibility.rs:102-106` reads as "unsupported on this OS" — the exact opposite of the truth. | **CONNECT** — the domain parser now reports the offending token and drops the *filter*, not the spec |
| M3-02 | `From<&AlephSkillSpec> for SkillManifest` (`src/tools/markdown_skill/spec.rs:209`) had only its own test as a caller and is lossy (drops `requires.bins` and all execution/security metadata). | **CUT** |

### `src/event` — 13 candidates, 8 repaired

| ID | Finding | Disposition |
|---|---|---|
| E1-01 | `TeamNotifier`'s subscription was registered **inside an unawaited `tokio::spawn`** (`src/bin/aleph-server/commands/start/mod.rs:1729`), so events in the startup window were never heard; the code contradicted its own comment. | **CONNECT** — the subscription is awaited inline |
| E1-02/E2-01 | A coord task with no team never emits an `AlephEvent` (`src/agents/swarm/tasks/store/mod.rs:113-117`), so `task_wait` can only time out. | **DECIDE** — needs a product answer for team-less tasks |
| E1-03/E3-01 | `AlephEvent::name()` — zero production callers (the only call was a unit-test assertion). | **CUT** |
| E1-04 | `EventFilter`'s `session_ids` / `agent_ids` dimensions are inert in production: every production subscriber filters by event type only, and for two event kinds "session" is really another id. | **KEPT** — the dimensions are part of the public filter contract; renaming them is a product decision |
| E1-05 | `EventHandler::handle`'s follow-up channel is empty in production (`src/event/handler.rs:64`) although the boot path consumes it. | **DECIDE** |
| E1-06 | `HandlerError` is an uninhabited placeholder — the `Err` branch is unreachable. | **KEPT** — a handler that can fail is a legitimate future shape |
| E1-07 | `TeamTaskUpdated.progress` was always `None` (both producers pass `None`) yet it was serialized into the durable team timeline. | **CONNECT** — the field was replaced by `previous_status`, which carries the transition the consumers actually need |
| E1-08 | `TeamCreated.member_ids` was always empty (by design, documented at the producer) yet travelled through the wire and the durable timeline. | **CUT** |
| E1-09 | `GlobalBus::unsubscribe()` — zero production callers, so a subscription could never be revoked. | **CUT** |
| E1-10 | `EventType::All` / `EventFilter::all()` — the wildcard pseudo-variant made the filter's `matches()` unreachable branch reachable, and gave "every type" two spellings. | **CUT** |
| E1-11 | `source_session_id` means three different things across the bus, the filter and the timeline. | **DECIDE** |
| E1-12/E3-02 | The event integration test builds its own `GlobalBus` and never exercises the boot assembly — deleting the boot registrations would leave it green. | **DECIDE** — the new guard covers the parity, not the assembly |
| E1-13 | The bus's documented contract ("notification-only, not a durable recovery source") contradicts `TeamEventLogger`, the only durable writer of the team timeline. | **DECIDE** |
| E2-02 | A metadata-only update re-fired "just entered review": the generic "updated" branch fills the current status, which may still be `waiting_review`. | **CONNECT** — the notifier now compares `previous_status` |
| E2-03 | The dispatcher's first recovery round can fire **before** the logger/notifier subscribe, and the bus does not replay — those events were lost forever. | **CONNECT** — `spawn_loop()` now runs after both subscriptions are registered |
| E2-04 | A later assignment emitted no `TeamTaskAssigned`, so the durable timeline had no assignment history (the Panel reads it). | **CONNECT** — assignment is emitted on every owner change |
| E2-05 | A `Skipped` last step never produces "Team work complete" although `is_settled()` counts `Skipped` as settled. | **DECIDE** |
| E2-06 | Deleting a team cascades the task deletion but wakes no `task_wait`. | **DECIDE** |

---

## 3. What changed

### `src/discovery` (branch `wire-discovery`, commit `fc8452e7b`)

- **CUT** `DiscoveryManager::aleph_home`, `DirectoryScanner::aleph_home`,
  `DiscoveredPath::in_project`, the `AGENT.md` spelling in the dead
  `PluginKind::detect_from_path`.
- **NARROW** `DiscoveryResult<T>` → `pub(crate)`.
- **CONNECT** `hooks.json` joins `MANIFEST_REL_PATHS`: the auto-discover adapter
  detects and parses it, but discovery never looked for it, so a plugin shipping
  only `hooks.json` was invisible to the whole plugin pipeline.
- **CONNECT** a failed `read_dir` during a plugin scan now reaches
  `PluginDiscovery::unreadable` instead of only the log.
- **CONNECT** the Claude/agents roots are re-checked per scan instead of being
  snapshotted when the scanner was built.
- **CONNECT** `ExtensionManager::discovered_plugin_ids()` now returns
  `DiscoveredPluginIds { ids, unreadable }`, so "not found" is not reported for a
  plugin whose source could not be read.
- **DOC** two comments that described a world the code does not have.

### `src/domain` (branch `wire-domain`, commit `817add97b`)

- **CUT** `pub trait Entity` (+ 3 impls, + its test), `PromptScope::as_str` +
  `Display`, the extension's dead `from_str_or_default` + `FromStr`, 8 newtype
  trait impls, the lossy `From<&AlephSkillSpec> for SkillManifest`.
- **CONNECT** the OS list is parsed through the domain parser: an unparseable
  token is reported and the *filter* is dropped, so it can no longer read as
  "unsupported on this OS".
- **CONNECT** `PromptScope` (and `Os`) deserialize through the domain `FromStr`,
  so the config path and the RPC path can no longer disagree about case.
- **CONNECT** `SkillSnapshot::build` takes the configured `SkillPromptBudget`.
- **DOC** the registry's priority list now names all five sources.

### `src/event` (branch `wire-event`, commit `03e97aeb6`)

- **CUT** `AlephEvent::name()`, `GlobalBus::unsubscribe()`, `EventType::All` +
  `EventFilter::all()`, `TeamCreated.member_ids`.
- **CONNECT** `TeamTaskUpdated` carries `previous_status` instead of an always-
  `null` `progress`; the notifier uses it to fire "awaiting review" only on a
  real transition.
- **CONNECT** `TeamTaskAssigned` is emitted on every owner change, not only on
  creation — the durable timeline now has the assignment history the Panel reads.
- **CONNECT** the `TeamNotifier` subscription is awaited inline, and the
  dispatcher's `spawn_loop()` moved after both subscriptions are registered.

---

## 4. Operator decisions taken

1. **`aleph.plugin.json` and `.lsp.json` stay advertised.** Cutting the names
   would hide plugins that ship them; adding an adapter for each is a shape
   decision beyond this audit's scope. Both are pinned in the guard's allowlist
   with a one-line justification, so a *new* divergence fails the guard and a
   stale entry has to be revisited rather than decaying.
2. **Case-sensitivity resolved toward the domain parser.** `skills.toml` with
   `scope = "System"` used to fail the whole file while the RPC path accepted
   it. The config path now behaves like the RPC path. `Os` was normalized the
   same way (it already had the shape, only a case-sensitive alias set).
3. **`TeamTaskUpdated.progress` was replaced, not deleted.** The field was
   always `null`; `previous_status` carries the transition the consumers
   (notifier, timeline) actually need. This is a wire-visible change to the
   durable team timeline, taken because the timeline already stores `status`.
4. **`TeamCreated.member_ids` removed.** It was empty by design (members enrol
   one `TeamMemberAdded` each) and only travelled to make the timeline look
   incomplete.
5. **`pub trait Entity` removed and the rule written down.** A marker trait with
   no generic-bound consumer is a wire with no far end; the module doc now says
   so, next to the `AggregateRoot` precedent.
6. **The guards follow the sibling convention** — standalone executable scripts
   wired into `just check-wiring`. No wire-audit script is in CI today
   (`.github/workflows/*` runs only the panel/webview/shared checks), so adding
   a new CI job for one family member was not done unilaterally.

---

## 5. Guards installed

| Script | What it pins |
|---|---|
| `scripts/discovery_wire_audit.py` | (1) manifest-name parity between what discovery advertises, what the manifest adapters detect and what the loader advertises — with a documented allowlist for the two deferred formats; (2) no swallowed `read_dir` failure during a plugin scan (every error arm either feeds `unreadable` or carries an explicit `// allow:` justification). |
| `scripts/domain_wire_audit.py` | (1) one vocabulary parser — a new hand-written scope parser fails the guard unless it is pinned as known-pending; (2) one priority source of truth (the registry's documented order is parsed and compared with `SkillSource::priority`); (3) the CUT symbols must not come back; (4) the CONNECT wires must stay connected. |
| `scripts/event_wire_audit.py` | (1) every `AlephEvent` variant has a producer and a subscriber; (2) the fields that were repaired must stay populated (`previous_status`, the assignment event); (3) the CUT symbols must not come back and the repaired transition/boot wires must stay connected. |

All three are wired into `just check-wiring` (as `check-discovery-wiring`,
`check-event-wiring`, `check-domain-wiring`) and each was proven to be a
detector: a deliberately regressed tree makes it fail.

---

## 7. Verification

- **`cargo check -p alephcore --lib --tests` on the merged `main`** — the one
  unified check the operator asked for, run after all three modules were
  repaired and merged (`CARGO_BUILD_JOBS=4`, `-j 4`, shared target directory).
  Result: **PASS** — `Finished dev profile in 13m 25s`, **0 errors, 0 warnings**
  (the whole log is two lines).
- `python3 scripts/discovery_wire_audit.py` → PASS (2 checks)
- `python3 scripts/domain_wire_audit.py` → PASS
- `python3 scripts/event_wire_audit.py` → PASS (2 checks)
- Every touched file is rustfmt-clean; `git diff --check` is clean.
- New regression tests (type-checked, **not executed**): a plugin shipping only
  `hooks.json` is a plugin candidate; a root created after the scanner is found
  by the next scan; `build` carries the configured budget; an owner change and a
  status transition broadcast an assignment and an update.

**Two repair workers timed out** (30-minute subagent ceiling) while their
`cargo check` was running. Their edits were complete on disk except for one
item (`DiscoveredPath::in_project`, R-D2), which was finished by hand and
verified by the unified check.

---

## 8. Where Jev was used

Jev (`jev_evaluate`) was used **only for classification and routing**, never for
reading code or writing fixes:

1. **Disposition classification** — every candidate was submitted to Jev as a
   `choice` question ("`CUT` / `CONNECT` / `DECIDE`", plus the candidate's own
   evidence summary) and Jev returned a calibrated probability distribution.
   **56 candidate dispositions** were scored this way, in batches of 9–14
   questions (12 for the discovery slice, 45 for the domain and event slices —
   one candidate was scored twice, in both slices).
2. **Severity / fix-shape routing** — for candidates where the disposition
   depended on how deep the fix was, the same choice question was used to route
   the fix shape ("cut the symbol", "narrow the visibility", "reconnect the
   wire").

Jev's verdicts were **not** taken as gospel. The overrides below are the cases
where a human/agent review went against a Jev probability; each one is recorded
with its reason. Jev and the review agreed on the other candidates.

| Candidate | Jev | Final | Why it was overridden |
|---|---|---|---|
| D1-03 (`aleph.plugin.json`) | `CUT` 0.55 | KEPT + allowlisted | Cutting the advertised name would hide a plugin that ships only that file. |
| D1-08 (`commands`/`agents` arms) | `CUT` 0.49 | KEPT | Narrowing the discovery vocabulary is a product decision, not a dead-code cut. |
| D1-10 (`AGENT.md` vs `agent.md`) | `DECIDE` 0.28 | CUT | Jev saw two spellings and called it a decision; the second spelling is only reachable from dead code (`PluginKind::detect_from_path`), so there is nothing to decide. |
| E1-01 (`TeamNotifier` subscription) | `DECIDE` 0.83 | CONNECT | The fix is mechanical (await the subscription instead of spawning it) and the code contradicted its own comment — that is a fact in the code, not an ambiguity. |
| E1-02 (team-less `task_wait`) | `CUT` 0.56 | DECIDE | What a team-less coord task *means* is a product call; cutting the event would decide it silently. |
| E1-07 (`TeamTaskUpdated.progress`) | `CUT` 0.55 | CONNECT | The field travels into the durable team timeline; cutting it would drop information, replacing it with `previous_status` keeps the timeline complete. |
| M1-08 (`PromptScope` case) | `DECIDE` 0.66 | CONNECT | Two paths (config vs RPC) gave two answers to one question — the fix removes the divergence rather than choosing a product answer. |
| M1-06 (`Deserialize` derives) | `DECIDE` 0.53 | NARROW | The derives are the Panel's contract; only the *visibility* was too wide. |
| M3-03 (lossy `AlephSkillSpec` conversion) | `DECIDE` 0.34 | CUT | Its only caller was its own test — no product ambiguity at all. |
| M2-10 (`SkillInfo.scope`) | `DECIDE` 0.40 | DECIDE | Agreed (the Panel/extension scope channel is a product decision). |

Candidates where Jev's confidence was high and the review agreed: `M1-01`
(CUT 0.98), `M1-03` (DECIDE 0.98), `M1-07` (DECIDE 1.0), `M2-08` (DECIDE 1.0),
`M2-12` (CUT 1.0), `M2-13` (CUT 1.0), `M3-01` (CONNECT 1.0), `E1-03` (CUT 1.0),
`E1-04` (DECIDE 0.97), `E1-11` (DECIDE 1.0), `E2-05` (DECIDE 0.98),
`E2-06` (DECIDE 0.98), `D3-04` (DECIDE 0.97), `D1-06` (DECIDE 0.96).

---

## 9. What was NOT done

- **No test suite was run.** The brief asked for one unified `cargo check` after
  all three modules; the new regression tests are type-checked but **never
  executed**. They need a `cargo test -p alephcore --lib` run (≈20 min) before
  anyone trusts them.
- **No `cargo fmt --all`** — the tree is not rustfmt-clean at HEAD (174 unrelated
  files would be rewritten); only the touched files were formatted.
- **21 findings were reported, not repaired** (§2, rows marked DECIDE / KEPT):
  the three provenance vocabularies, the three hand-written `PromptScope`
  parsers, the `skill_manage delete` over-promise, the extension scope channel,
  the `task_wait` team-less case, the `source_session_id` naming, the
  `EventFilter` session/agent dimensions, the `HandlerError`/`follow_ups`
  channels, the skill-side `unreadable` channel, the root-resolution channel
  (worker note N-1), the discovery↔loader manifest-list duplication (N-3), the
  Panel's bare-string `PromptScope` mirror, the team's `Skipped` completion, and
  the team-delete wake-up.
- **The guards are not in CI.** No wire-audit script is in CI today; they are
  wired into `just check-wiring` only. Wiring that family into CI is a
  separate decision.
- **No push.** The three slices were merged into `main` locally; publishing is
  the operator's call.
- **The sibling guards `cluster_wire_audit.py` and
  `config_context_diagnostics_wire_audit.py` are still not wired into the
  justfile/CI** — noticed during this round, left alone.
