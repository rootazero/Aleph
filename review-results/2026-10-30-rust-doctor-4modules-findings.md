# Findings index — rust-doctor review of `src/bin`, `src/browser`, `src/builtin_tools`, `src/bundled`

Generated 2026-10-30 from the reviewer subagents' JSON deliverables (see `.review/rd-batch2/`).
`jev` is the Jev `noul` score used for triage; `disposition` is the routing decision
(`fix-now` ≥ 0.80, `verify-then-fix` 0.60–0.80, `defer` < 0.60). P3 items were not scored by Jev.

| # | severity | disposition | jev | location | title |
|---|---|---|---|---|---|
| 1 | P1 | fix-now | 0.84 | `src/bin/aleph-migrate/export.rs:126` | --end-date filters on <= midnight of the named day, so the end day itself is dropped |
| 2 | P1 | fix-now | 0.88 | `src/bin/aleph-migrate/index.rs:335` | INSERT OR REPLACE on the FTS5 table appends a duplicate copy of every transcript on each re-index |
| 3 | P1 | fix-now | 0.83 | `src/bin/aleph-migrate/verify.rs:553` | --remove-orphans deletes attachment files whose session manifest is missing or unparseable |
| 4 | P1 | fix-now | 0.91 | `src/bin/aleph-server/commands/plugins.rs:180` | plugins uninstall joins an unvalidated CLI name onto the plugins dir and recursively deletes it (path traversal) |
| 5 | P1 | fix-now | 0.88 | `src/bin/aleph-server/commands/update.rs:233` | update writes the installer to a predictable temp path (symlink-following) and then executes that path |
| 6 | P1 | fix-now | 0.86 | `src/bin/aleph-server/daemon.rs:174` | handle_stop resolves the PID file before the IPC endpoint, so a stale PID file makes stop a no-op that deletes a live server's discovery file |
| 7 | P1 | fix-now | 0.88 | `src/browser/cdp_backend/navigate.rs:237` | The post-navigation SSRF audit re-reads the URL navigate itself wrote, so it cannot fail when the landed URL is unknown |
| 8 | P1 | fix-now | 0.92 | `src/browser/chrome_mcp.rs:331` | read_proc_starttime parses the wrong /proc field (vsize instead of starttime), so the PID-reuse guard never matches and the bootstrap-launched Chrome is never SIGKILLed |
| 9 | P1 | fix-now | 0.87 | `src/browser/playwright_launch.rs:234` | sanitize_session_key is lossy, so two distinct profiles can share one browser data dir / Chrome --user-data-dir |
| 10 | P1 | fix-now | 0.81 | `src/browser/wait_probe.rs:106` | A fixed-delay wait is truncated to the unrelated wait budget, yet the tool reports the requested delay |
| 11 | P1 | fix-now | 0.89 | `src/builtin_tools/agent_manage/create.rs:425` | agent_create silently drops the TOML row for ids of 33-64 chars and still reports success |
| 12 | P1 | fix-now | 0.89 | `src/builtin_tools/browser_tools/exec.rs:1500` | browser_exec's input-side secret gate scans only top-level actions; type/fill/select/dialog text nested in repeat/if is typed into the page unblocked |
| 13 | P1 | verify-then-fix | 0.79 | `src/builtin_tools/browser_tools/exec.rs:1501` | browser_exec never applies the input-side secret gate to evaluate scripts, so it is a path around the check browser_evaluate enforces |
| 14 | P1 | fix-now | 0.9 | `src/builtin_tools/code_check.rs:338` | Timed-out check reports ok:true when partial output has no parseable diagnostics |
| 15 | P1 | fix-now | 0.91 | `src/builtin_tools/user_profile.rs:186` | Email redaction is bypassed whenever ≥22 chars of text follow the address, so profile PII reaches the model verbatim |
| 16 | P1 | fix-now | 0.89 | `src/bundled/extractor.rs:764` | Filesystem-source skill copy writes through nested destination symlinks (symlink escape out of ~/.aleph/skills) |
| 17 | P2 | fix-now | 0.89 | `src/bin/aleph-migrate/export.rs:149` | The timestamp column is read as a non-Option i64, so one NULL created_at aborts the whole export |
| 18 | P2 | verify-then-fix | 0.64 | `src/bin/aleph-migrate/main.rs:71` | --include-attachments can never be turned off (bool + clap SetTrue + default_value "true") |
| 19 | P2 | fix-now | 0.92 | `src/bin/aleph-migrate/transform.rs:361` | Attachment write has no containment check when the target path does not exist, so a symlink escapes the migration root |
| 20 | P2 | fix-now | 0.87 | `src/bin/aleph-migrate/verify.rs:57` | --fix can never make verification pass because report.errors is never decremented |
| 21 | P2 | fix-now | 0.93 | `src/bin/aleph-migrate/verify.rs:308` | --fix overwrites the stored content hash with the hash of the current file, laundering a corrupted transcript |
| 22 | P2 | defer | 0.28 | `src/bin/aleph-server/commands/start/builder/agent_init/common_handlers.rs:277` | gateway.identity.get reports a registered_method_count captured mid-boot, before most handler families are registered |
| 23 | P2 | fix-now | 0.8 | `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:284` | tools.invoke allowlist gate and tools.effective surface are built from a different agent catalogue than the one the agent loop uses |
| 24 | P2 | verify-then-fix | 0.77 | `src/bin/aleph-server/commands/start/helpers.rs:385` | Bundled-content extraction falls back to the shared world-writable /tmp/.aleph when the Aleph home cannot be resolved |
| 25 | P2 | fix-now | 0.82 | `src/bin/aleph-server/commands/start/mod.rs:4138` | Shutdown path unwraps the process-wide markdown-skill Mutex: a poisoned lock panics inside start_server and skips the rest of the teardown |
| 26 | P2 | defer | 0.1 | `src/bin/aleph-server/commands/update.rs:131` | update.rs still documents a checksum soft-fail default that the constant has already turned off |
| 27 | P2 | fix-now | 0.92 | `src/bin/aleph-server/daemon.rs:202` | stop signals a PID read from a file with only a liveness check, so a recycled PID receives SIGTERM/SIGKILL |
| 28 | P2 | fix-now | 0.88 | `src/browser/cdp_backend/actions.rs:170` | A failed browser_select deselects every option, wiping the element's previous selection |
| 29 | P2 | verify-then-fix | 0.63 | `src/browser/cdp_backend/actions.rs:1172` | A failed click/fill/type leaves the effect probe installed in the page (listeners + data-aleph-probe attribute) |
| 30 | P2 | fix-now | 0.88 | `src/browser/cdp_backend/migration.rs:322` | localStorage restore writes origin A's payload into the origin the page actually landed on, and books it under A |
| 31 | P2 | fix-now | 0.9 | `src/browser/cdp_backend/navigate.rs:336` | A history move that misses the frameNavigated event keeps the PREVIOUS (allowed) url in the tab table, so the read-time SSRF guard passes on a tab that has moved to a blocked origin |
| 32 | P2 | fix-now | 0.8 | `src/browser/cdp_backend/recording.rs:924` | An explicit recording path is handed to ffmpeg with -y, so an existing file is silently replaced |
| 33 | P2 | verify-then-fix | 0.61 | `src/browser/chromium_resolve.rs:461` | The operator's [general.browser.playwright_cli] binary_path pin is honoured on the managed route but silently ignored on the ChromiumEngine (cdp) route |
| 34 | P2 | verify-then-fix | 0.61 | `src/browser/chromium_resolve.rs:553` | Playwright-managed Chromium is resolved without any executability check, unlike the pinned route |
| 35 | P2 | verify-then-fix | 0.64 | `src/browser/engine/mod.rs:438` | attach_tab never detaches the CDP session it created when a domain enable fails, so the engine keeps a half-wired session for a tab Aleph refused |
| 36 | P2 | defer | 0.4 | `src/browser/engine/process.rs:320` | The engine sidecar registry is created world-readable and stores the live DevTools port, which no reader ever uses |
| 37 | P2 | defer | 0.38 | `src/browser/engine/readiness.rs:156` | The readiness-gate error says "The process is running but unusable", but the registry stops that browser before the error escapes |
| 38 | P2 | fix-now | 0.88 | `src/browser/engine/registry.rs:121` | Relaunch proceeds after the displaced engine refused to die, so the still-running browser's only record is overwritten and it becomes permanently unreapable |
| 39 | P2 | verify-then-fix | 0.76 | `src/browser/live/control.rs:697` | run_agent polls the agent future while holding every permit's non-reentrant control state mutex; only one cooperative yield_now() prevents a self-deadlock |
| 40 | P2 | fix-now | 0.81 | `src/browser/live/resource.rs:316` | A same-binding publish can shorten the resource expiry, but the lifecycle passes previous_expiry = None so already-issued grants are never revoked |
| 41 | P2 | verify-then-fix | 0.77 | `src/browser/live/viewer_backend.rs:264` | Backend-wide `controls` mutex is held across the `bind_control` await, so one in-flight agent mutation stalls every viewer handshake |
| 42 | P2 | verify-then-fix | 0.7 | `src/browser/manager.rs:1575` | arc_swap guard is held across the SSRF await, so a hot policy apply blocks a worker thread until the in-flight check returns |
| 43 | P2 | fix-now | 0.82 | `src/browser/manager.rs:1707` | store-flag stripping ignores the `/flag` spelling Chromium accepts on Windows, so a principal's copy can inherit the operator's browser store |
| 44 | P2 | defer | 0.54 | `src/browser/mod.rs:137` | home_guard_census's brace matcher is string/comment-blind, so a truncated test body yields a false green |
| 45 | P2 | fix-now | 0.81 | `src/browser/page_state/fetch_chromium.rs:552` | A child frame that fails to attach/capture is dropped silently and then escalates into a whole-page refusal |
| 46 | P2 | fix-now | 0.81 | `src/browser/page_state/fetch_chromium.rs:1031` | Grandchild frame ids from a CHILD renderer's node space are merged into the same actionable `NotCaptured` carrier as parent-space ids |
| 47 | P2 | fix-now | 0.84 | `src/browser/page_state/fetch_obscura.rs:382` | walk() recurses unboundedly over engine-controlled DOM depth; a deeply nested page overflows the stack |
| 48 | P2 | fix-now | 0.87 | `src/browser/playwright_cli.rs:910` | parse_page_meta matches `[Snapshot](` on any stdout line, so echoed page content can redirect the local file the server opens |
| 49 | P2 | verify-then-fix | 0.72 | `src/builtin_tools/agent_identity.rs:374` | agent_identity export does blocking std::fs + full-chain serialization inside an async tool call |
| 50 | P2 | fix-now | 0.88 | `src/builtin_tools/agent_manage/create.rs:285` | agent_create overwrites an existing SOUL.md / AGENTS.md on disk without checking the filesystem |
| 51 | P2 | fix-now | 0.83 | `src/builtin_tools/agent_manage/delete.rs:266` | agent_delete reports "workspace archived" and emits workspace_archived: true even when the archive rename failed |
| 52 | P2 | verify-then-fix | 0.69 | `src/builtin_tools/agent_manage/list.rs:163` | agent_list instantiates every lazily-registered agent as a side effect of a read |
| 53 | P2 | fix-now | 0.84 | `src/builtin_tools/agent_manage/update.rs:257` | agent_update applies the live allowed_users write before the TOML write, then reports a bare error on TOML failure |
| 54 | P2 | defer | 0.19 | `src/builtin_tools/bash_exec.rs:578` | Background-slot rejection message misattributes the fleet-wide 32-job cap to the per-session 8-job cap |
| 55 | P2 | fix-now | 0.85 | `src/builtin_tools/browser_tools/exec.rs:1115` | browser_exec checks its 600 s wall clock only between steps, so a single step can push it past the 630 s harness budget and lose the partial results |
| 56 | P2 | verify-then-fix | 0.78 | `src/builtin_tools/browser_tools/mod.rs:709` | bound_screenshot_png decodes the whole capture before applying its caps, and returns the unbounded original when decoding fails |
| 57 | P2 | verify-then-fix | 0.74 | `src/builtin_tools/browser_tools/qa.rs:649` | browser_qa has no BUILTIN_TOOL_BUDGETS_MS row, so the harness kills the run at 300 s although the tool's own worst case is 540 s |
| 58 | P2 | fix-now | 0.9 | `src/builtin_tools/browser_tools/qa.rs:867` | browser_qa performs its read-time SSRF check once, before a wait that can run for minutes, so a page that navigates itself to a blocked origin is still read |
| 59 | P2 | verify-then-fix | 0.78 | `src/builtin_tools/decompress.rs:309` | LLM-supplied max_tokens is unbounded, defeating the documented read_backstop_tokens ceiling |
| 60 | P2 | verify-then-fix | 0.79 | `src/builtin_tools/desktop/mod.rs:224` | Run-boundary cleanup releases the NEW run's session instead of the ended run's, contradicting its own comment |
| 61 | P2 | verify-then-fix | 0.6 | `src/builtin_tools/desktop/native.rs:1432` | scroll swallows the coordinate validation error, so a malformed point silently degrades to scrolling at the real cursor |
| 62 | P2 | fix-now | 0.82 | `src/builtin_tools/goal.rs:315` | Cross-session pause persists an uncapped note into another session's goal row |
| 63 | P2 | verify-then-fix | 0.77 | `src/builtin_tools/goal.rs:867` | `set` stores a blank gate_command that always passes, while `update` treats blank as "clear" |
| 64 | P2 | fix-now | 0.85 | `src/builtin_tools/mcp_resource.rs:173` | mcp_read_resource returns unbounded MCP content with no size cap, inline to the model |
| 65 | P2 | verify-then-fix | 0.76 | `src/builtin_tools/memory_search.rs:488` | A `workspace:` the caller may not read silently falls back to the caller's own partition, and the returned facts are indistinguishable from the requested workspace's |
| 66 | P2 | defer | 0.49 | `src/builtin_tools/memory_search.rs:636` | Smart Recall expansion is unreachable on the default (no-`workspace`) search path, so `smart_recall_triggered` is always false there |
| 67 | P2 | defer | 0.57 | `src/builtin_tools/note_schema.rs:14` | note_schema Write content has no size cap (same unfixed shape as BTS-7 note_manage) |
| 68 | P2 | fix-now | 0.82 | `src/builtin_tools/note_schema.rs:20` | note_schema Write with expected_hash=None is an unconditional force-overwrite, silently bypassing the hash guard the module exists for |
| 69 | P2 | fix-now | 0.88 | `src/builtin_tools/process_journal.rs:475` | Deserialized row fields are joined onto the store path without validation (traversal read, write and remove_dir_all) |
| 70 | P2 | verify-then-fix | 0.63 | `src/builtin_tools/runtime_manage.rs:309` | install spawns unconditionally: no dedupe against an already-running install of the same capability |
| 71 | P2 | verify-then-fix | 0.65 | `src/builtin_tools/tool_usage.rs:128` | forget_orphans reports N rows deleted even when the sidecar store is missing or the delete failed |
| 72 | P2 | fix-now | 0.87 | `src/bundled/extractor.rs:244` | Reconcile failure before extraction is only warned about, so a user skill can be overwritten and pruned by the bundled skill of the same name |
| 73 | P2 | verify-then-fix | 0.65 | `src/bundled/extractor.rs:258` | One failing skill permanently blocks the bundled_version bump, so every startup re-extracts and re-prunes all official skills |
| 74 | P2 | defer | 0.17 | `src/bundled/extractor.rs:609` | The 'destination may be a directory' fix (BUNDLED-R4-02) is dead code on Unix: rename-over-directory reports IsADirectory, not AlreadyExists |
| 75 | P2 | verify-then-fix | 0.67 | `src/bundled/extractor.rs:732` | copy_tree_with_prune prunes only the top level of dst, unlike the embedded path's recursive prune — nested stale official files survive forever |
| 76 | P2 | verify-then-fix | 0.71 | `src/bundled/extractor.rs:741` | Prune removal failures are silently discarded while the skill is reported as successfully synced |
| 77 | P2 | fix-now | 0.81 | `src/bundled/manifest.rs:121` | Windows rename fallback deletes the manifest and can leave it deleted, losing all skill provenance |
| 78 | P3 | not-triaged(P3) |  | `src/bin/aleph-migrate/index.rs:369` | The FTS5 'token count' is fabricated as row count x 100 and printed as 'FTS5 index: N tokens' |
| 79 | P3 | not-triaged(P3) |  | `src/bin/aleph-migrate/transform.rs:55` | The rayon build_global() error is swallowed, so --parallel can be silently ignored |
| 80 | P3 | not-triaged(P3) |  | `src/bin/aleph-migrate/verify.rs:28` | Connection::open creates a missing metadata.db (and the original db), so a wrong path yields 'no such table' instead of 'not found' |
| 81 | P3 | not-triaged(P3) |  | `src/bin/aleph-migrate/verify.rs:687` | The report prints a checkmark for the session count even when metadata.db and the filesystem disagree, and repeats the attachment error list |
| 82 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/identity.rs:383` | identity export writes the evidence document non-atomically (truncate-in-place, silent overwrite) |
| 83 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/identity.rs:389` | identity export prints the pin advice to stdout although the comment mandates stderr on both branches |
| 84 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/secret.rs:68` | secret set --value skips the empty-value check that the interactive prompt path enforces |
| 85 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/service/descriptors.rs:11` | service descriptor path validation is debug_assert-only: released binaries skip it and the release-profile unit tests fail |
| 86 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/start/builder/handlers/config.rs:15` | config.reload / config.validate / config.path are never registered when the config file does not exist |
| 87 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/start/builder/handlers/config.rs:142` | Config-watcher failure path is unreachable: the spawned task can never finish, so a dead file watcher is never reported |
| 88 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/start/builder/subsystems.rs:558` | The `enabled` channel boot request is honored only for feishu, although the comment states it as a general registry contract |
| 89 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/start/helpers.rs:390` | JoinHandle of the bundled-extraction blocking task is discarded, so a panic inside the extractor is invisible |
| 90 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/start/mod.rs:1838` | build_http_provider failure is silently dropped, so a misconfigured provider vanishes from the OpenAI-compatible map |
| 91 | P3 | defer | 0.4 | `src/bin/aleph-server/commands/start/mod.rs:2849` | Skill hot-reload task unwraps the markdown-skill Mutex: a poisoned lock kills the task with no log line |
| 92 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/start/runtime_warmup.rs:52` | runtime capability probe (subprocess work) runs synchronously inside an async fn, blocking a tokio worker at boot |
| 93 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/commands/start/runtime_warmup.rs:88` | Test mutates the process-global HOME inside a #[tokio::test], so parallel tests observe a temporary home |
| 94 | P3 | not-triaged(P3) |  | `src/bin/aleph-server/main.rs:420` | A SandboxInit path that returns instead of exec'ing falls through to an unreachable!() panic |
| 95 | P3 | not-triaged(P3) |  | `src/browser/cdp_backend/actions.rs:1355` | type_text clears the verdict latch AFTER resolve_target although the comment says "Cleared at ENTRY" |
| 96 | P3 | not-triaged(P3) |  | `src/browser/cdp_backend/actions.rs:1390` | expect() on a key-table lookup inside an async verb, where the sibling verb handles it gracefully |
| 97 | P3 | not-triaged(P3) |  | `src/browser/cdp_backend/actions.rs:1531` | fill's "the element did not accept a value" diagnosis is unreachable — the real failure blames the page |
| 98 | P3 | not-triaged(P3) |  | `src/browser/cdp_backend/recording.rs:476` | Blocking filesystem calls inside the async start, under the registry's start lock |
| 99 | P3 | not-triaged(P3) |  | `src/browser/cdp_backend/recording.rs:489` | The 500MB disk floor is fail-open: a failing available_space probe skips the check silently |
| 100 | P3 | not-triaged(P3) |  | `src/browser/cdp_backend/recording.rs:809` | Receipts of recordings that end on their own pile up in `finished` until someone stops that exact tab |
| 101 | P3 | not-triaged(P3) |  | `src/browser/cdp_backend/screenshot.rs:39` | ScreenshotOutput.png_bytes can carry JPEG bytes while every consumer assumes PNG |
| 102 | P3 | not-triaged(P3) |  | `src/browser/chrome_mcp_backend.rs:762` | classify_wait_error folds any ChromeMcpError containing "exceeded" into Ok(false), reporting a limit/quota error as "the text never appeared" |
| 103 | P3 | not-triaged(P3) |  | `src/browser/chrome_mcp.rs:92` | chrome_launch_args resolves the default user-data-dir unconditionally, so an explicitly configured user_data_dir still fails when the Aleph home cannot be resolved |
| 104 | P3 | not-triaged(P3) |  | `src/browser/chrome_mcp.rs:594` | Blocking std::fs read of /proc inside the async ensure_chrome_running |
| 105 | P3 | not-triaged(P3) |  | `src/browser/engine/mod.rs:414` | attach_tab ignores the dead map, so a tab that died gets a raw CDP attach error instead of the TabGone answer ensure_tab gives for the same fact |
| 106 | P3 | not-triaged(P3) |  | `src/browser/engine/mod.rs:469` | A budget already spent by the attach is reported as "stalled at Page.enable" — a step that was never sent |
| 107 | P3 | not-triaged(P3) |  | `src/browser/engine/process.rs:268` | `argv_names_dir`'s doc says obscura launches with the two-token `<flag> <path>` form, but obscura builds `--storage-dir=<dir>` as one token |
| 108 | P3 | not-triaged(P3) |  | `src/browser/engine/process.rs:343` | A crashed launch leaves a `<name>.json.tmp` file behind forever; the sweep skips it and nothing ever deletes it |
| 109 | P3 | not-triaged(P3) |  | `src/browser/engine/process.rs:927` | The sweep's test fixture writes sidecars under the old `<profile>.json` naming, so the production `<engine>-<profile>.json` key derivation is never exercised |
| 110 | P3 | not-triaged(P3) |  | `src/browser/engine/readiness.rs:158` | not_ready names browser_session{action:"switch_engine"}, a verb that answers NoSession in exactly the state this error describes |
| 111 | P3 | not-triaged(P3) |  | `src/browser/engine/readiness.rs:160` | The readiness error hardcodes browser_open{engine:"chromium"} as "the other engine" although the gate never learns which engine failed |
| 112 | P3 | verify-then-fix | 0.69 | `src/browser/live/control.rs:229` | The viewer expiry timer wakes at most every 60s, so an expired viewer keeps receiving screencast frames for up to a minute |
| 113 | P3 | fix-now | 0.83 | `src/browser/live/control.rs:339` | The InterventionSink is invoked while the control's non-reentrant state mutex is held |
| 114 | P3 | not-triaged(P3) |  | `src/browser/live/lifecycle.rs:200` | The Expired branch of decide() replaces the Expired error with the sweep's own error |
| 115 | P3 | fix-now | 0.84 | `src/browser/live/mod.rs:1219` | A deliberate owner retirement is reported as a dispatcher EventGap boundary |
| 116 | P3 | not-triaged(P3) |  | `src/browser/live/owner_registry.rs:269` | No owner-ended path retires an owner's route registrations, so the registrations keep the engine handle pinned |
| 117 | P3 | defer | 0.57 | `src/browser/live/session.rs:79` | A panicked or cancelled frame-ack task is reported as a successful CDP ack |
| 118 | P3 | not-triaged(P3) |  | `src/browser/live/viewer_backend.rs:403` | Two viewer-backend tests duplicate the entire fixture setup for opposite halves of one contract |
| 119 | P3 | not-triaged(P3) |  | `src/browser/page_state/fetch_chromium.rs:517` | The parent capture is fully parsed twice on the out-of-process-iframe path |
| 120 | P3 | not-triaged(P3) |  | `src/browser/page_state/fetch_chromium.rs:1044` | `decode` deep-clones the entire snapshot JSON before parsing it |
| 121 | P3 | not-triaged(P3) |  | `src/browser/page_state/fetch_obscura.rs:427` | rect_from's finiteness guard only checks min_x/min_y; huge finite or +INF quad coordinates saturate to i32::MAX rects instead of None |
| 122 | P3 | not-triaged(P3) |  | `src/browser/post_nav.rs:108` | The quarantined tab and the audited URL can be two different tabs |
| 123 | P3 | not-triaged(P3) |  | `src/browser/profile.rs:294` | playwright_cli nav/action timeouts are unbounded and unvalidated, unlike cdp_command_timeout_secs |
| 124 | P3 | not-triaged(P3) |  | `src/browser/secret_guard.rs:53` | One percent-decode pass lets a double-encoded secret bypass the URL secret scan |
| 125 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_identity.rs:384` | Chain export written non-atomically — a crash mid-write leaves a torn JSON that fails verify_export |
| 126 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/create.rs:310` | Blocking std::fs calls run inside the async agent_create tool |
| 127 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/create.rs:339` | The same system_prompt is written to both SOUL.md and AGENTS.md and is injected twice every turn |
| 128 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/delete.rs:216` | Archive path built with Path::with_extension("archived") breaks on the second delete of the same id |
| 129 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/error.rs:70` | Three AgentManageError variants are never constructed and two variants share one Display string |
| 130 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/info.rs:180` | agent_info looks up bound_channels under the requested id, not the resolved id, and swallows store errors |
| 131 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/switch.rs:144` | agent_switch reports "Agent '' not found" — the requested id is dropped from the error |
| 132 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/test_utils.rs:49` | instance() drops the session store's TempDir before returning, leaving the returned SessionStore backed by a deleted directory |
| 133 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/unbind.rs:113` | agent_unbind calls the sync SQLite seam in an async fn, and the seam swallows the binding read error |
| 134 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/update.rs:236` | The authority-change audit entry is written even when the live allowed_users write was refused |
| 135 | P3 | not-triaged(P3) |  | `src/builtin_tools/agent_manage/validation.rs:23` | validate_agent_id measures bytes but reports the count as characters, and checks length before characters |
| 136 | P3 | not-triaged(P3) |  | `src/builtin_tools/bash_exec.rs:424` | Background timeout is silently clamped to 4h; the model is never told its argument was rewritten |
| 137 | P3 | not-triaged(P3) |  | `src/builtin_tools/bash_exec.rs:424` | Background timeout clamp is upper-only, so `timeout: 0` yields a job that is killed instantly |
| 138 | P3 | not-triaged(P3) |  | `src/builtin_tools/bash_exec.rs:687` | A killed background job is reported with exit_code 0 / success true, contradicting the tool's own exit-code contract |
| 139 | P3 | not-triaged(P3) |  | `src/builtin_tools/browser_tools/companion/manager.rs:518` | Process-global MANAGERS Vec<Weak<CompanionManager>> grows monotonically; dead weak refs are never pruned |
| 140 | P3 | not-triaged(P3) |  | `src/builtin_tools/browser_tools/qa.rs:788` | browser_qa writes the raw evidence PNG to disk with no size bound |
| 141 | P3 | not-triaged(P3) |  | `src/builtin_tools/browser_tools/resize.rs:58` | browser_resize accepts unbounded width/height (zero check only); values are forwarded verbatim to Emulation.setDeviceMetricsOverride / playwright `resize` |
| 142 | P3 | not-triaged(P3) |  | `src/builtin_tools/channel_manage.rs:154` | The one-time channel pairing code is written to the info log |
| 143 | P3 | not-triaged(P3) |  | `src/builtin_tools/desktop/ax_compress.rs:51` | Elision treats `settable: Some(true)` as information, contradicting the module's own default-assumption doctrine |
| 144 | P3 | not-triaged(P3) |  | `src/builtin_tools/desktop/mod.rs:310` | execute_batch propagates a sub-action hard error and discards the already-delivered batch results |
| 145 | P3 | not-triaged(P3) |  | `src/builtin_tools/desktop/native.rs:1993` | restart_app fallback path omits the "verified" key that the system-capability path always emits |
| 146 | P3 | not-triaged(P3) |  | `src/builtin_tools/desktop/native.rs:1994` | Hand-rolled restart uses a fixed 500 ms settle sleep instead of waiting for the app to exit and reappear |
| 147 | P3 | not-triaged(P3) |  | `src/builtin_tools/desktop/perm.rs:121` | First failing permission kind aborts the whole check, discarding statuses already collected |
| 148 | P3 | not-triaged(P3) |  | `src/builtin_tools/desktop/vision_bridge.rs:218` | Cache key omits `format`, so identical bytes with a different declared ImageFormat share one entry |
| 149 | P3 | not-triaged(P3) |  | `src/builtin_tools/goal.rs:679` | inert_resume_reason rejects every update on a cap-spent goal, so unrelated edits fail with a misleading "Not resumed" |
| 150 | P3 | not-triaged(P3) |  | `src/builtin_tools/goal.rs:703` | gate_command is unbounded and token_budget / timeout_minutes are unclamped, so the documented soft guards can be made inert |
| 151 | P3 | not-triaged(P3) |  | `src/builtin_tools/goal.rs:847` | `set` silently drops `args.lesson` although the field is documented for both `set` and `update` |
| 152 | P3 | not-triaged(P3) |  | `src/builtin_tools/governance_metrics.rs:151` | Counting corrections loads every matching raw-memory row with usize::MAX, so memory scales with a year of rows |
| 153 | P3 | not-triaged(P3) |  | `src/builtin_tools/hooks_manage.rs:197` | Approval Deny drops the policy's reason from the model-visible error |
| 154 | P3 | not-triaged(P3) |  | `src/builtin_tools/memory_search.rs:253` | `default_workspace` / `default_session_key` handles are written every run but never read by the tool, while their docs claim they scope searches |
| 155 | P3 | not-triaged(P3) |  | `src/builtin_tools/memory_search.rs:477` | `workspaces: []` (empty list) resolves to an empty partition set and the search reports "nothing found" instead of falling back |
| 156 | P3 | not-triaged(P3) |  | `src/builtin_tools/memory_search.rs:585` | Comment states `max_results` is LLM-supplied and unclamped, contradicting the clamp added 50 lines above |
| 157 | P3 | not-triaged(P3) |  | `src/builtin_tools/note_schema.rs:76` | Conflict detection relies on substring matching of SchemaStore's error text |
| 158 | P3 | not-triaged(P3) |  | `src/builtin_tools/plugin_manage.rs:566` | Marketplace add/remove read-modify-write config race can lose a concurrent update |
| 159 | P3 | not-triaged(P3) |  | `src/builtin_tools/plugin_manage.rs:612` | marketplace_update with name="" updates ALL marketplaces but reports "Marketplace '' updated" |
| 160 | P3 | not-triaged(P3) |  | `src/builtin_tools/process_journal.rs:917` | The boot reconcile (directory walk plus per-row process probing) runs synchronously on the async executor thread |
| 161 | P3 | not-triaged(P3) |  | `src/builtin_tools/process_journal.rs:1623` | Journal persistence failures are logged only at debug level, so a lost row is invisible |
| 162 | P3 | not-triaged(P3) |  | `src/builtin_tools/scratchpad.rs:1035` | derive_default_project_id's doc comment is split across two functions |
| 163 | P3 | not-triaged(P3) |  | `src/builtin_tools/scratchpad.rs:1072` | Persisted directory name derived from std DefaultHasher (unstable across rustc releases); comment falsely claims BLAKE3 |
| 164 | P3 | not-triaged(P3) |  | `src/builtin_tools/scratchpad.rs:1074` | format!("{:x}", hash)[..8] panics when the u64 hash renders in fewer than 8 hex digits |
| 165 | P3 | not-triaged(P3) |  | `src/builtin_tools/self_config.rs:309` | `write_file` maps filesystem failures (dir creation / write errors) to `ToolError::InvalidArgs` |
| 166 | P3 | not-triaged(P3) |  | `src/builtin_tools/self_config.rs:368` | `update_config` accepts an unbounded model-supplied JSON patch and writes it verbatim into config.toml |
| 167 | P3 | not-triaged(P3) |  | `src/builtin_tools/self_config.rs:502` | `list_backups` performs a blocking `read_dir` on the async runtime thread |
| 168 | P3 | not-triaged(P3) |  | `src/builtin_tools/self_manage.rs:119` | Frontmatter stripping searches for the first '---' anywhere in the document, not a line-anchored delimiter |
| 169 | P3 | not-triaged(P3) |  | `src/builtin_tools/self_manage.rs:139` | self_manage returns success: true for the degraded fallback and caps nothing on the manual it injects |
| 170 | P3 | not-triaged(P3) |  | `src/builtin_tools/session_complete.rs:199` | session_complete stores unbounded LLM-supplied outcome/key_learnings into a durable memory row |
| 171 | P3 | not-triaged(P3) |  | `src/builtin_tools/system_tool.rs:254` | launch_app/quit_app/restart_app arms are three near-identical 22-line bodies that can drift |
| 172 | P3 | not-triaged(P3) |  | `src/builtin_tools/system_tool.rs:493` | "Valid actions" error string omits the real action list_installed_apps |
| 173 | P3 | not-triaged(P3) |  | `src/bundled/extractor.rs:135` | last_err is overwritten by the plugins branch, so the reported sync error names only one of the two failures |
| 174 | P3 | not-triaged(P3) |  | `src/bundled/extractor.rs:191` | The first-run remote bootstrap is skipped whenever the user has any pre-existing skill directory |
| 175 | P3 | not-triaged(P3) |  | `src/bundled/extractor.rs:213` | A partially successful remote bootstrap is silently reverted by the embedded snapshot extraction |
| 176 | P3 | not-triaged(P3) |  | `src/bundled/extractor.rs:448` | swap_dir_into_place is documented as an atomic move but is a remove-then-rename with a window where the cache does not exist |
| 177 | P3 | not-triaged(P3) |  | `src/bundled/extractor.rs:520` | prune_stale_entries silently drops read_dir errors while manifest.reconcile fails closed on the same condition |
| 178 | P3 | not-triaged(P3) |  | `src/bundled/extractor.rs:685` | Skills cloned from the remote repo are stamped with version = BUNDLED_VERSION although their content comes from the remote main branch |
| 179 | P3 | not-triaged(P3) |  | `src/bundled/sync.rs:21` | CHECKOUT_LOCKS grows one entry per distinct checkout directory and is never pruned |
| 180 | P3 | not-triaged(P3) |  | `src/bundled/sync.rs:129` | Non-force fetch refspec leaves the checkout permanently stale after an upstream force-push |
