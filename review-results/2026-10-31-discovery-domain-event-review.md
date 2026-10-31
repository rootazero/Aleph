# Rust Review Report — `src/discovery`, `src/domain`, `src/event`

**Modules**: `src/discovery/` (6 files), `src/domain/` (2 files), `src/event/` (7 files) — 5,492 LOC
**Date**: 2026-10-31
**Branch**: `rd-review-modules` (merged into `main`, no PR)
**Mode**: static review, no diff scope (full-module review)

## Method

1. **Machine scan** — the 2026-10-29 full-workspace `rust-doctor` scan was reused as the
   machine baseline (the three modules are byte-identical to that scan: last change
   2026-10-09). It reported **11 findings** for these modules, all structural
   (`oversized_unit`, `complex_function`, `duplicate_function_body`,
   `near_duplicate_function_body`) — no correctness or security rules fired.
2. **Human/agent review** — 6 parallel sub-agent batches (`minimax-cn/MiniMax-M3`), each
   owning one file group, reading the real source and the whole-workspace grep for every
   claim. 51 findings, each with `file:line` evidence, a before/after and a confidence.
3. **Triage** — every finding was routed through **Jev** (`noul`, calibrated) against one
   policy: *safe to apply now* = behavior-preserving, or a local behavior change whose
   tests are updated in the same change, with no product decision and no public-API
   break. Findings below 0.5 on that axis were deferred with a reason.
4. **Fix** — applied in an isolated git worktree, then `cargo check -p alephcore
   --all-targets -j 2` and the affected modules' test suites.

**Jev was used for**: severity re-rating of every candidate, "safe to apply in this pass"
routing for all 51 findings, and the fix-batch assignment. It was **not** used for root-cause
analysis or for code decisions — those went to the MiniMax-M3 reviewers and to the final
human review.

## Summary

| Disposition | Count |
|---|---|
| Fixed | 34 |
| Deferred (needs an owner / product decision / public-API change) | 17 |
| Rejected after review | 0 |

## Fixed

### `src/discovery/scanner.rs`

- **[P1] A symlinked project root became a discovery root.** `collect_project_dirs` accepted
  `<project>/.aleph` on `is_dir()` semantics (follows symlinks), so a cloned repository
  shipping `.aleph -> /elsewhere` had its target scanned as a **project** root — and
  `guess_source` classifies `.aleph/skills/<x>` as `SkillSource::Workspace`, the highest
  priority tier. The injected skill outranked the user's own copies and its body was fed
  into the model prompt. The project root and the component dir under a project scope are
  now screened with `is_existing_dir_no_follow`; global scopes keep the link-following
  check (a dotfiles layout may legitimately be a symlink).
- **[P1] The precedence documentation contradicted the code.** The numbers are a **scan
  position**, not a precedence: the list is sorted ascending and the registry keeps the
  first registration, so the *lowest* number wins an equal-source tie — while the old doc
  said the opposite ("deeper = higher priority", "20+/30+/40+ beat 0/1/10"). The doc now
  states the rule the code implements, including the two consequences (a global `.claude`
  copy beats a project `.claude` copy on an id tie; the `.aleph` family is separated by
  `SkillSource`, not by these numbers). No number, sort or test changed — the *direction*
  of the compat precedence is a product decision (see Deferred).
- **[P2] The `SKILL.md` marker gate disagreed with the loader.** The gate was
  case-sensitive, the loader (`skill/mod.rs::is_skill_file`) is not: a skill stored as
  `skill.md` existed on macOS and did not exist on Linux. The gate now mirrors the loader
  per component type (`SKILL.md` case-insensitively, `agent.md` exactly, because the
  agent loader looks for that exact name).
- **[P2] Same-root same-id skill collisions were decided by `read_dir` order** —
  filesystem-dependent, so the winner varied between machines. Both sorts now tie-break
  on the path, making the order total.
- **[P2] A stray `.md` file in a skills root was reported as a skill component.** The
  consumer scanned a *file* path and produced an empty manifest set, and the bogus path was
  merged into a dir list that only ever grows. Direct `.md` files are now accepted for
  commands/agents only.
- **[P2] One swallowed `read_dir` failure** (the monorepo branch of `scan_plugin_parent`)
  was the only one of four in that function with no log. It is now logged like its three
  siblings.
- **[P3] `scan_plugin_parent` decomposed** (cyclomatic 17 / cognitive 27 → two helpers);
  the discovered set, order, scope and priority are unchanged.
- **[P3] The 700-line inline test module moved** to `src/discovery/scanner/tests.rs`,
  clearing the rust-doctor `oversized_unit` finding for both the file and the module.
- **[P3] The two near-identical scanner tests were merged** and now assert what their
  comments claim (the Aleph global scope, a project scope, `my-skill` *and*
  `project-skill`) — the shared fixture created `project-skill`, which neither test
  asserted.

### `src/discovery/claude_cache.rs`

- **[P2] A plugin could live outside the cache root and still be labelled `ClaudeCache`.**
  Only the leaf version dir was screened no-follow; a symlink at the `<name>` level rooted
  the plugin physically outside `~/.claude/plugins/cache` while the trust policy read it as
  the user's own install. The `<name>` level is now screened too; the `<marketplace>`
  level may still be a symlink (a shared team cache is legitimate).
- **[P3] The index parse error was flattened to a `String` and then dropped**, so
  "schema version 3 (this reader knows 2)" never reached the surface that reports the
  source as unavailable. It is now a `thiserror` enum (`Json` with `#[source]`,
  `SchemaVersion`) that the `warn!` carries.
- **[P3] A malformed install entry was diagnosed as "it belongs to one project"** when it
  was actually an unknown *shape* — two failures with opposite remedies. Each now gets its
  own diagnosis.

### `src/discovery/paths.rs` / `src/discovery/mod.rs`

- **[P2] `validate_path_component` accepted `.`** — a self-reference component that roots
  the path one level *above* the name it was given (a `"version": "."` entry). Rejected.
- **[P2] `validate_path_component` accepted a Windows drive prefix** (`C:`); `PathBuf::push`
  *replaces* the base path for such a component, so the joined path left the Claude tree
  entirely. Rejected on Windows only (`cfg!(windows)`), so Unix behavior is unchanged.
- **[P3] `DiscoveryError::HomeDir` said "Aleph home directory"** but was also used for user
  home failures, sending the reader to `~/.aleph` when the cause was a missing `HOME`. A
  new `UserHome` variant (additive) carries the user-home failure.
- **[P3] `max_upward_depth` was documented as a depth** but counts directories examined
  (0 examines nothing, 10 = the working directory + 9 ancestors). Doc corrected; the code
  and the default are unchanged.

### `src/discovery/home_scan_census.rs`

- **[P1] The census's green could be false.** `calls()` matched a helper named `new`
  against *any* `X::new(` — `TempDir::new(`, `Arc::new(`, `Path::new(`. A live instance:
  `slash_command_body/tests.rs` defines a redirecting helper named `new` and also calls
  `TempDir::new(` / `Arc::new(`, so a test there was credited with the redirect on the
  strength of an unrelated constructor. A qualified call (`Owner::name(`, `owner.name(`) no
  longer counts — in Rust a qualified path never names a free function — which makes the
  census fail-closed where it cannot prove the owner.
- **[P3] Parameterized-test attributes** (`#[rstest]`, `#[test_case]`, `#[quickcheck]`)
  are now recognized, and the module doc lists the remaining blind spots instead of
  overstating its coverage.
- The duplication with `utils::source_scan::scan_test_bodies` is documented as a
  follow-up (the shared scanner lives in a file this change does not own).

### `src/domain/skill.rs` (+ one parse call site)

- **[P2] `InvocationPolicy`'s defaults were restated at the parse boundary**
  (`unwrap_or(true)`), so a change to the domain default would be silently overridden for
  the field the author did not set. The domain now owns the merge
  (`InvocationPolicy::from_optional`), and `EligibilitySpec::always` got the same
  treatment (`EligibilitySpec::from_optional`).
- **[P3] `PluginId::new` had drifted from `SkillId::new`** — the hand-copied guard lost the
  whitespace/control-character `debug_assert!`. Restored.
- **[P3] A comment pointed at a context that does not exist** ("see the wrapping context at
  `SkillContent::new`"). The trust note moved onto the constructor it describes, so the
  reason `Display` was removed stays attached to the access path.

### `src/event/`

- **[P2] The events a handler returns were dropped by the boot path.** `EventHandler::handle`
  returns "a list of new events to publish"; both call sites discarded the `Ok` value.
  They now publish the follow-ups on the bus (both implementors return an empty list
  today, so this makes the contract real rather than changing behavior).
- **[P2] The boot filter was a hand-written copy** of `TeamEventLogger::subscriptions()`,
  so a new team event type would be broadcast and never reach the durable team log — the
  one place the `TeamNotifier` registration already gets right. The filter is now derived
  from the handler's own contract.
- **[P2] The `TeamEventLogger` subscription was registered inside `tokio::spawn`,** so
  anything broadcast before the spawned task first polled was *permanently* missing from
  the durable `team_events` timeline (the bus is notification-only, with no replay). The
  registration is now awaited — the rule the repo already states in
  `gateway::announce_delivery`: "a subscriber that is merely *scheduled* is a subscriber
  that is not listening yet".
- **[P2] A panicking subscriber callback vanished.** Tokio records the panic on the
  `JoinHandle`, which the bus never keeps — no log line at all, and the event silently
  dropped (at least one production subscriber runs its work inline). The callback is now
  wrapped in `catch_unwind` and the panic is logged with the subscription id.
- **[P2] A test asserted a guarantee the module explicitly denies.** The ordering test
  asserted monotonically increasing delivery while the doc says delivery is *not* ordered
  — it passed only because `#[tokio::test]` defaults to a current-thread runtime. It now
  asserts the contract that holds (the sequences are unique and contiguous).
- **[P2] The unsubscribe test asserted bookkeeping, not the effect** (a no-op callback and
  `subscription_count()`). It now counts the callback invocations and asserts that no
  delivery happens after the unsubscribe.
- **[P3] `subscribe_broadcast`'s two obligations were undocumented** — it is *unfiltered*
  (the channel send happens before the filter is consulted) and *lossy* (`Lagged`). Both
  are now in the doc.
- **[P3] The `SubscriptionId` is the only handle to a subscription** and nothing said so.
  The type now carries `#[must_use]` with the reason, and the two call sites that dropped
  the id on the floor bind it deliberately.
- **[P3] Three duplicated test pairs merged** (`global_bus.rs`, `filter.rs`,
  `tests/integration.rs` — the rust-doctor `duplicate_function_body` findings), the
  `Box::leak` in every integration test removed, three stale `EventBus` doc references
  corrected, `EventType::All`'s exemption from the enum's own rule documented, and two
  `EventFilter` doc/behaviour mismatches fixed (the empty-agent-id rule and the
  add-vs-replace semantics of `with_session`/`with_sessions`).

## Deferred (with the reason)

| Finding | Why deferred |
|---|---|
| Global-vs-project precedence for the **compat** skill roots | The correct direction is a product decision (the file's doc implies "project wins", the code says "global wins"). The docs were corrected instead of the behavior. |
| `domain::Entity` marker trait removal | It has no generic-bound consumer (the same shape its own file documents as rot), but it is a public trait of a published crate with ~10 call sites. |
| `DiscoveredPath` fields → private accessors | Public-API shape change. |
| `SkillRegistration.source` / `AgentRegistration.source` removal | Never written, never read — but a persisted-row and public-type change. |
| `GlobalBus::new` → `pub(crate)` | Public-API narrowing of a published crate (the second-bus trap is real, but the API break is the owner's call). |
| `subscribe_async` callback → `Arc<GlobalEvent>` | Removes the N+1 deep clones per broadcast, but changes a public signature used by 7 call sites. |
| `is_model_visible()` vs `PromptScope::Standalone` | The domain predicate contradicts the variant's documented meaning, but the fix changes what is injected into the system prompt. |
| Empty skill description rejected at the parse boundary | Changes which user-written `SKILL.md` files are accepted. |
| `Os` parse errors logged instead of discarded | Small, but it is a change in another module's parse path. |
| `skill_list` provenance vocabulary | The model-facing tool's output text changes from `global`/`project` to the domain's `user`/`workspace`/`bundled`/`claude`/`agents`. |
| `TeamTaskUpdated.status` typed vocabulary | Cross-module change to two consumers. |
| `SubAgentCompletionEvent.summary` masking | The sibling `ProcessCompletionEvent` is masked by its producer; the sub-agent path is not. Masking it changes what the parent model reads. |
| `AlephEvent` payload field removals (`progress`, `agent_id`) | Public payload shape change. |
| `#[serde(default)]` shim on `request_ids` | Wire-compat change; nothing in the workspace writes that format today. |
| Empty/relative `HOME` → fail closed | The daemon would refuse to start instead of scanning CWD-relative roots. |
| Compat root names (`.claude`/`.agents`) in one place | A three-module refactor; one copy feeds the file-tool deny-write list, so it deserves its own change. |

## Verification

- `cargo check -p alephcore --all-targets -j 2` — clean. The 14 warnings it reports are
  pre-existing and all in `src/gateway/server/panel_asset_route.rs` (untouched).
- `cargo test -p alephcore --lib -- discovery:: event:: domain:: skill::` — **402 passed,
  0 failed** (including `discovery::home_scan_census::tests::no_lib_test_reaches_the_real_claude_home`,
  the census's own anti-vacuity assertion, which the file move would otherwise have broken).
- `cargo test -p alephcore --lib -- teams:: goal_wait extension::` — 1299 passed, 4 failed.
  **All four failures are pre-existing**: three fail identically on the unmodified `main`
  (`extension::hooks::registry::ownership_tests`,
  `extension::panel::tests::accepts_semver_build_metadata_in_the_canonical_entry`,
  `extension::projection::tests::publishing_plugin_projections_has_exactly_one_author`),
  and the fourth (`extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation`)
  needs the untracked `plugins/` directory, which a fresh worktree does not have.
