# `qa/` — manually-invoked real-machine fixtures

Scenarios here boot a **real `aleph-server`** against a **deterministic mock
provider** and drive it through a real surface. They are not part of
`cargo test` and must never be: they take minutes, they bind ports, and they
are timing-shaped. Run them by hand when a round's claim is about *runtime*
behaviour that a unit test can only assert about a pure function.

```bash
./qa/busy_input/run.sh burst-drain   # §4.8 Round-9 wake edge, RPC face
./qa/busy_input/run.sh interrupt     # §4.8 Round-8 ①, channel inbound
./qa/busy_input/run.sh queue         # channel inbound, nothing cancelled
KEEP=1 ./qa/busy_input/run.sh queue  # keep the scratch dir for post-mortem

./qa/plan_handoff/run.sh handoff     # §3.16 refuse -> card -> approve -> unlock
./qa/plan_handoff/run.sh deny        # a declined plan leaves the floor engaged
./qa/plan_handoff/run.sh floor       # explicit `allow` + `full` tier still lose

./qa/browser_managed/run.sh open     # managed driver actually reaches a browser
./qa/browser_managed/run.sh ambient  # a planted cwd cli.config.json is ignored
./qa/browser_managed/run.sh headed   # headless=false really launches headed
./qa/browser_managed/run.sh tools    # every remaining browser verb, asserted by EFFECT
./qa/browser_managed/run.sh frames   # a genuinely cross-origin iframe (second port)
./qa/browser_managed/run.sh reap     # the idle reaper really closes a session (~3 min)
./qa/browser_managed/run.sh pdf      # pdf_generate's browser engine, CLI off PATH
./qa/browser_managed/run.sh existing # the OTHER driver (Chrome DevTools MCP)
./qa/browser_managed/run.sh exec-offload # browser_exec's spill, inside a real turn
./qa/browser_managed/run.sh attach   # Aleph starts Chrome; playwright-cli joins over CDP
                                     # (unix only: pgrep)

./qa/file_search/run.sh floor   # deny_read_globs from a CONFIG FILE binds grep/find,
                                # and no_ignore=true does not lift it
./qa/file_search/run.sh page    # the window reports the whole; pages are disjoint
./qa/file_search/run.sh reach   # a real turn's grep output reaches the model
./qa/file_search/run.sh steer   # shell `grep -r` is steered to the builtin; a
                                # bounded `grep` and `rg` are not. Every arm
                                # proves it searched before it claims it was
                                # not steered — `rg` is absent on some machines
                                # and the driver reports SKIP, not PASS.

./qa/web_search/run.sh reach    # `recency:"week"` from the tool face arrives as
                                # time_range=week in the backend's query string
./qa/web_search/run.sh order    # a backend that can carry the asked-for dimension
                                # is asked first — with a control arm that asks
                                # for nothing, so a green is not "exa always fails"
./qa/web_search/run.sh degrade  # a dimension no configured backend can express is
                                # reported in the answer's notes, not dropped
./qa/web_search/run.sh empty    # a zero-result answer does not end the chain
./qa/web_search/run.sh fanout   # naming two backends asks both and merges the two
                                # answers; its fallback chain is deliberately empty
                                # so failover could not produce the same green
./qa/web_search/run.sh demote   # a backend that failed on the previous search is
                                # not asked again on the next one. Asserted on the
                                # dead backend's request count, not on the answer:
                                # ask-and-fail-over and don't-ask read identically

./qa/generation_timeout/run.sh cap        # a configured `timeout_seconds` really cuts a real
                                          # HTTP request, asserted on the PROVIDER's connection
                                          # (the mock records that the server hung up, and when)
                                          # rather than on the tool's error string, which four
                                          # unrelated failures produce identically
./qa/generation_timeout/run.sh auto       # "unset" lets the request outlive an 8s window —
                                          # the negative arm, asserted on a request STILL OPEN,
                                          # not on an absent log line. It falsifies "unset
                                          # collapsed into a short cap"; it does NOT separate
                                          # 120s from the provider's own default (that would
                                          # cost two minutes for one bit, and the `None` arm of
                                          # `WithRequestTimeout` carries it in-process)
./qa/generation_timeout/run.sh deploy     # `~/.aleph/defaults.toml`'s generation timeout still
                                          # reaches the client after the round MOVED that
                                          # override out of `#[serde(default = …)]`. Nothing
                                          # else in the suite watches that wire
./qa/generation_timeout/run.sh precedence # an explicit provider timeout outranks the deployment
                                          # override — without it, `deploy` green is also
                                          # consistent with "the override always wins"
./qa/generation_timeout/run.sh panel      # boot + hold: the Auto checkbox in a real browser.
                                          # `just wasm` proves the form COMPILES to wasm32; only
                                          # this shows the checked box omits the field from the
                                          # payload and the key leaves config.toml
#   DRIVEN 2026-09-06 (chrome-devtools MCP, all four steps): cold boot renders `超时: 自动` with
#   the box checked and the slider disabled (its DOM value 60 is `unwrap_or(60)`'s parking spot,
#   NOT a saved value — read the label, not the slider). Unchecking enables it; 180 + Save put
#   `"timeout_seconds":180` on the wire and `timeout_seconds = 180` in config.toml. Re-checking
#   Auto + Save sent a frame with NO `timeout_seconds` key at all (tap on
#   `WebSocket.prototype.send`, installed via initScript before the app connects) and the key
#   VANISHED from config.toml — not 0, not the old number. Reload comes back Auto, with the
#   slider parked at 60 rather than the value just set (no key in config ⇒ `unwrap_or(60)`).
#   A second pass carried the chain past config.toml to the SOCKET, both directions: saving the
#   slider minimum (10s) then firing one `tools.invoke` produced THREE aborts at 10014/10001/
#   10016 ms; re-checking Auto and firing the same call left it uncut — the mock held it the full
#   60s and answered (`aborted:false, held_ms:60020`). All four boundaries between "the number a
#   human dragged" and "what happened on the wire" are covered, with a measurement on each side.
#   Gotcha for whoever drives this next: the page has THREE `input[type=range]` and TWO buttons
#   reading 保存更改. Selecting either by text silently hits the wrong one (the first 保存更改
#   belongs to 生成设置 and sends `generation_config.update`). Pick the button by nearest common
#   ancestor with the timeout slider (depth 3, vs 7 for the settings one).
#   MEASURED 2026-09-06 (12/12 assertions green, four phases, on a binary built from the tree
#   it measured): `timeout_seconds` bounds each ATTEMPT, not the call. A 2s cap produced THREE
#   aborted attempts of ~2s and a tool call that settled at ~7s, so an operator's wall-clock
#   wait is about `timeout_seconds × attempts + backoff`. `cap`/`deploy` therefore assert the
#   shape of EVERY attempt and never the retry count: the count is provider policy that may
#   change, the per-attempt bound is the contract. The Panel's seconds field does not say any of
#   this — an unfixed labelling question, not a defect in the knob.
#
#   FOUR harness lies were fixed before these greens counted, and every one of them made a
#   WORKING server look broken:
#     1. the boot precondition grepped a `tracing::info!` the default log filter never emits —
#        it now reads the unconditional `println!` count, whose absence really does mean zero;
#     2. the config carried no chat provider, so the server chose simulated mode, where
#        `tools.invoke` is a `-32099` placeholder and every phase measured the boot mode;
#     3. `kill` does not stop a native Windows child from Git Bash, so runs leaked servers that
#        the next run then contended with — cleanup now uses `taskkill //F //T`;
#     4. **the binary was not the code.** With `SKIP_BUILD=1`, six consecutive runs reported
#        `cap`/`deploy` RED — no request ever cut — against a stale `target/debug/aleph-server`.
#        Rebuilding, changing nothing else, turned all four phases green. That stale binary also
#        produced two convincing phantom "product defects" (a hot-reload watcher on the real
#        `~/.aleph`, and an unset timeout written back as `120`), neither of which reproduces on
#        a current build — 120 was the serde default this feature's own round had REMOVED, so
#        the old binary was reporting its own age. The fixture now refuses to run when any
#        source file is newer than the binary (`HARNESS_STALE_BINARY`).

./qa/announce/run.sh outlive     # a background bash job outlives its run -> a fresh run is driven
./qa/announce/run.sh collected   # the model collected it itself -> no turn is spent
./qa/announce/run.sh midrun      # the run is still alive -> absorbed as steering, ONE run

./qa/run_halt/run.sh crash    # a failed run's receipt: `failed`, and the work it did
./qa/run_halt/run.sh cap      # a capped run: the umbrella token AND the cap in `terminate_detail`
./qa/run_halt/run.sh receipt  # the same crash through real `aleph ask`, once per LC_ALL
./qa/run_halt/run.sh panel    # boot + hold; the halt badge is a LIVE projection, so the
                              # browser has to be attached BEFORE the run ends
                              # ⚠️ `crash` reports 4 failures today — a real defect it
                              # found (one terminal frame per retry attempt, last one
                              # zeroed). See the header of its run.sh.

./qa/resume_boundary/run.sh crash      # a dangling tool call (`kill -9` mid-dispatch) gets
                                       # an OUTCOME UNKNOWN repair, and the boot-scan resume's
                                       # NEXT request to the model actually carries it — the
                                       # oracle is the mock's request log, not the event log,
                                       # because a repair synthesised but dropped by
                                       # `build_prompt` would pass every unit test here.
./qa/resume_boundary/run.sh attribute  # a dangle left by an EARLIER interrupted run is not
                                       # blamed on THIS restart: two dangling calls from two
                                       # separate crashes, repaired in the same boot scan, must
                                       # read two different sentences. Run against the pre-round
                                       # tree this must FAIL — both misattributed to "the server
                                       # restarted" — which is the falsifying arm for the design
                                       # spec's §1.4 claim.
./qa/resume_boundary/run.sh claims     # ONE reduction, three faces, and they must agree: the wire
                                       # (`chat.history` → `session.last_run`, the exact field the
                                       # Panel sidebar and TUI picker render), the operator
                                       # (`aleph-server resume --json` → every `ResumeReceipt`
                                       # key), and the effect (`last_run` settles to `clean`).
                                       # Boots with resume OFF so the receipt is the ONLY pass
                                       # over the log — a boot scan that already repaired it
                                       # makes every counter read zero, which looks like the
                                       # feature working. Also prints the two uncapped reads
                                       # A10 deferred (`load_all_events`, `load_run_markers`) as
                                       # NUMBERS: a cost on record is not a cost until measured.
./qa/resume_boundary/run.sh denied     # a dangling call the approval gate DENIED must be repaired
                                       # "NOT EXECUTED", never "OUTCOME UNKNOWN" — the model must
                                       # not go looking for side effects that cannot exist. The
                                       # denial row is written by the fixture with the server
                                       # down, because that is the only half of this shape
                                       # reachable from outside: a statically denied call is
                                       # answered in the same turn (never dangling), and the
                                       # crash window between a denial and its receipt is inside
                                       # one process. Everything downstream — reduction, the
                                       # `denied` flag on the wire, the repair text, the receipt
                                       # — is the product reading its own log.
./qa/resume_boundary/run.sh rewind     # a rewind that cuts a run's tail away and leaves its
                                       # `RunStarted` behind must end with the marker tail
                                       # BALANCED. Without it the log SAYS a run is still open, and
                                       # every later boot re-classifies the session `Interrupted`,
                                       # re-runs a turn the user deleted, and does it forever —
                                       # nothing else ever closes that marker. The rewind is aimed
                                       # ONE ROW PAST the open `RunStarted` on purpose: aimed AT it,
                                       # the opening half is retired too, `close_open_run_after_retire`
                                       # returns `Ok(None)` without appending anything, and the stage
                                       # is green on a build with no balancer at all (that was the
                                       # first-round arrangement; the tail then read `never_ran` and
                                       # the receipt `no_runs`). So the stage asserts the effects the
                                       # balancer alone can produce: the `RunStarted` still live, a
                                       # `RunFinished{cancelled}` appended after it, the wire face
                                       # reading `clean` before AND after a restart, and a parsed
                                       # `aleph-server resume --json` receipt reading
                                       # `already_finished` with `scanned > 0` (parsed, not grepped:
                                       # every counter of `ResumeReceipt` is serialised
                                       # unconditionally, so grepping for one of their keys matches
                                       # any well-formed receipt). Resume is OFF:
                                       # `balance_run_markers_after_retire` deliberately leaves a
                                       # RUNNING session alone, so a stage that let the boot scan
                                       # resume first would be green over a session it never tested.
./qa/resume_boundary/run.sh knobs      # the resumed run follows the SNAPSHOT its `RunStarted`
                                       # carried, not the session's current row. The crashing
                                       # turn carries an explicit per-turn directive for model
                                       # A (without one the marker records `model: None` — the
                                       # agent's CONFIGURED model is not a routing directive —
                                       # and there is no snapshot to replay); the session is
                                       # then moved to model B with the server DOWN, because no
                                       # RPC can move it (`session.update` does not exist, and
                                       # the metadata modify path refuses `model_pin` on
                                       # purpose — the legal writer is the `select_model` TOOL).
                                       # Three anti-vacuity checks come first: the marker really
                                       # snapshotted A, the row really moved to B, and the
                                       # restarted server really reads B. One model on the
                                       # provider could not tell those apart — the assertion
                                       # would pass for a build that dropped the envelope
                                       # entirely.
                                       # The stage carries the SECOND knob too, and in the
                                       # opposite direction on purpose: the crashing turn runs
                                       # at exec tier `ask`, the row is opened up to `full`
                                       # with the server down, and the resumed run's OWN
                                       # `RunStarted` envelope must still read `ask`. Snapshot
                                       # `full` over a session since pulled down to `ask` —
                                       # the arrangement that reads naturally — is green for a
                                       # build with NO ceiling at all, because the session rung
                                       # already answers `ask`. Only the loosening direction
                                       # separates `resolve_exec_tier_with_ceiling` from
                                       # `resolve_exec_tier`, and it is the direction that
                                       # costs something when it is wrong: a resume that ran
                                       # too loose executes, unattended, at a tier the operator
                                       # revoked while the daemon was down.
./qa/resume_boundary/run.sh holes      # a burst of tool calls in one turn must not lose a row
                                       # and must not bill the run twice: `chat.history`'s
                                       # server-reported total >= projectable events in
                                       # `session_events`, and the session's token total is
                                       # UNCHANGED across a restart (a heal pass that re-stamps
                                       # an already-stamped row bills the same run twice, and a
                                       # counter that grew while nobody ran anything is the only
                                       # outside evidence of it). The burst run is made to
                                       # FINISH before the kill — `dangle` returns on the first
                                       # durable dispatch, and killing there would leave dangling
                                       # calls whose resume adds a turn's usage. Two measured
                                       # bounds the stage prints rather than hides: the 4096
                                       # projector queue never fills (0 deferrals at both 40 and
                                       # 900 calls), and above the store's compaction bound the
                                       # projection is trimmed ON PURPOSE — at `QA_BURST=900`,
                                       # 1803 projectable events, history total 69,
                                       # `compaction_count 34`. So the comparison is guarded by
                                       # an explicit `compaction_count == 0` precondition:
                                       # raising the burst turns THAT red, not the row count,
                                       # which would otherwise read like data loss.
#
# Two things hold the five r2 stages themselves honest, because a QA fixture is
# also code that can stop working without saying so:
#
#   * **Every stage has an assertion FLOOR** — the case block at
#     `qa/resume_boundary/run.sh:234-240` holds the measured counts;
#     deliberately not copied here — the rewind copy already drifted. Each
#     `drive` call is its own node process with its own counters, so the last
#     line a green stage prints is whichever phase ran last; for `claims` that
#     is the cost probe, which asserts nothing and prints `0 passed, 0 failed`.
#     A phase whose assertions all vanished prints the SAME line and still
#     exits 0. run.sh sums the counts and turns a stage RED when it passes
#     while measuring less than its floor. Adding an assertion raises the floor
#     in the same commit.
#   * **The round-1 `crash` / `attribute` stages refuse to run without a real
#     python3** (exit 78, with the reason named). On this Windows host `python3`
#     and `python` are both the `WindowsApps` stub — no output, exit 49 — so they
#     have NOT been re-measured this round; the r2 stages above are the coverage
#     that exists here. Do not read their absence as a pass.
./qa/session_order/run.sh        # the transcript's order and `session.truncate`, on BOTH
                                 # backends. Drives one conversation into a file-backed
                                 # server and a sqlite-backed one (separate scratch
                                 # ALEPH_HOMEs), stops each, rewrites its stamps
                                 # DESCENDING — the shape an import or a reconciler
                                 # produces, and the only shape that tells "recording
                                 # order" and "stamp order" apart — restarts, and asserts
                                 # the served order did not move, that `session.truncate`
                                 # reached the database (it answered INTERNAL_ERROR to
                                 # every call ever made on sqlite: two transactions, the
                                 # first shadowed rather than committed), that it kept the
                                 # HEAD, and that both backends destroyed the same rows.
                                 # Unit tests build both stores in one process and cannot
                                 # see the config key that picks one — which is how
                                 # `default_session_store_backend()` came to return "file"
                                 # under a doc saying `"sqlite" (default)`.

./qa/leftovers/run.sh            # converged tool DESCRIPTIONs + relocated-ALEPH_HOME hooks + [agents.defaults] roots

./qa/picker_nav/run.sh           # keyboard walk + conditional bottom fade + phone add-a-provider,
                                 # at three widths (1440 / 700 folded / 390 phone)

./qa/multiuser_audit/run.sh      # §5.22 round-6: the security trail is readable end to end,
                                 # a deactivation receipt reports what it measured, and revoking
                                 # a device credential names whose it was. Binds 0.0.0.0 for the
                                 # remote-pairing half — a loopback peer is authorised before
                                 # `bootstrap_ticket` is ever read, so a ticket redeemed over
                                 # 127.0.0.1 creates no device row, silently and successfully.
                                 # Node, not Python, and it shares `teamchat_rooms`'
                                 # `patch_config.mjs` rather than keeping a patcher of its own.
                                 # Round-10 added the freeze's fourth leg to the same stage, and
                                 # specifically its DECLINED arm: the patcher sets
                                 # `[heartbeat] enabled = false`, so this run proves the receipt
                                 # says the heartbeat leg did not RUN instead of reporting a zero —
                                 # a boot-time `decline(because)`, an absent wire field, and a CLI
                                 # sentence, none of which a unit test reaches together. The same
                                 # stage also pins the deactivation's bootstrap-ticket leg: the
                                 # count is zero here (the ticket was redeemed by the pairing
                                 # driver), and zero is the point — only a deactivation carries
                                 # that field, so the sentence appearing at all proves the server
                                 # measured it, it crossed the wire, and the CLI rendered it.
                                 # Round-10 also added stage 3b, `aleph users show`: the SAME
                                 # devices/spend/background-work join read while she is still
                                 # active. Until `users.get` existed the only way to learn what a
                                 # principal held was to deactivate them and read the receipt, so
                                 # the pairing between 3b's assertions and the receipt's is the
                                 # claim. It pins two fail-closed renderings the receipt cannot:
                                 # an unrecorded spend prints a sentence and never `0.00`, and the
                                 # declined heartbeat leg prints "NOT counted" and never a number.
                                 # Its cost warning is asserted family by family (background work,
                                 # devices, bootstrap tickets, channel senders), because a preview
                                 # that names two of the four effects the receipt below reports is
                                 # read by the operator as coverage.
                                 #
                                 # FLOOR: run.sh's own `[ "$FAIL" -eq 0 ] || exit 1`
                                 # — zero failures, and deliberately no number
                                 # here (the header carries no claim count
                                 # either, and says so). ⚠️ Like its two room
                                 # siblings, it enforces no MINIMUM assertion
                                 # count: a phase whose precondition stopped
                                 # holding fires nothing, shrinks the total and
                                 # still exits 0. The only defence is comparing
                                 # totals across runs, so record what a run
                                 # printed — 2026-09-04 cold: 38 passed.

./qa/teamchat_rooms/run.sh       # §5.22 round-8: three humans in one project room. A model's
                                 # `team_create` inside a room lands room-scoped; the activation
                                 # gate flips on the SECOND human (plain message observed,
                                 # @-mention dispatches, both broadcast live to the other socket);
                                 # a member run's approval card is addressable by the human who
                                 # SPOKE and not by the room's other member; `<room_context>` names
                                 # both members including the silent one; a child that room run
                                 # DELEGATES inherits the same block one spawn later; and each
                                 # project-page tab has a server-side effect. Same 0.0.0.0 + LAN-leg
                                 # reason as `multiuser_audit`. Node, not Python — see its run.sh
                                 # header.
                                 #
                                 # FLOOR: `drive_rooms.mjs`'s `report()` —
                                 # `process.exit(FAIL === 0 ? 0 : 1)`. Zero
                                 # failures, no minimum count; both driver
                                 # report paths add FAIL+1 on an exception, so
                                 # an abort is caught, but a phase that quietly
                                 # never fires is not. This one has a
                                 # cross-check the other two lack: its run.sh
                                 # header records the total an earlier
                                 # measurement saw, and a 2026-09-04 cold run
                                 # printed exactly that again.

./qa/agents_viz/run.sh claims    # §4.11 / §5.13 / §3.13 tasks+agents visualization: the two
                                 # severed wires the round fixed, each asserted by effect on a
                                 # real socket. `run.subagent_tree` reaches an UNFILTERED (TUI-
                                 # shaped) connection in its double-nested envelope; a filtered
                                 # connection gets it only after subscribing (a filtered-but-not-
                                 # subscribed socket gets NOTHING — the negative arm); the node
                                 # carries child_session and chat.history opens it. Plus the plan's
                                 # three carriers (live frames / RunSummary.plan / chat.history).
                                 # First run found RunSummary.plan had never left the server —
                                 # FEATURE_LOCATOR 附录 D.4.35. Reuses teamchat_rooms' mock
                                 # (QA-DELEGATE-BG / QA-PLAN arms) and config patcher. No pty:
                                 # aleph-tui is never launched (same call as btw_tui).
./qa/agents_viz/run.sh panel     # boot + hold with a minted session, prints the browser URL and
                                 # the one-line `delegate` / `plan` commands — the tree is a LIVE
                                 # projection, attach the browser BEFORE triggering.

./qa/terminal/run.sh identify    # §6.11/§6.12 the embedded terminal's agent panel. A session
                                 # spawned as `sh` with `claude` TYPED INTO IT afterwards reaches
                                 # the wire as program+agent+state — the shape phase 1 could not
                                 # produce, because it read the spawn label. A control session
                                 # that ran no agent is the falsifying arm, and a fourth check
                                 # ("the probe answered") keeps that arm from being green because
                                 # nothing ever looked.
./qa/terminal/run.sh wait        # terminal{wait} blocks on the table's watch and answers
                                 # `reached`; a state the session never enters answers `timeout`
                                 # with the CURRENT entry. Both arms carry a DURATION check — a
                                 # wait that never waited answers `timeout` too.
./qa/terminal/run.sh quiet       # ~90 s. 30 s of silence publishes `quiet_since` and does NOT
                                 # move `state` (spec R2-3). Checks the row is not-quiet first,
                                 # that the mark lands on the clock rather than instantly, and
                                 # that a later frame CLEARS it — a sticky flag reads the same.
./qa/terminal/run.sh cwd         # OSC 7 › foreground probe › spawn dir, over three directories
                                 # that actually differ. A second session emits no OSC 7, so its
                                 # answer can only be the probe's — without it "OSC 7 won" and
                                 # "the probe said nothing" are the same green.
./qa/terminal/run.sh real        # UNIX ONLY (probe_alive.py needs pty.fork; SKIPS loudly elsewhere).
                                 # A REAL agent binary found on PATH, run directly AND behind a
                                 # real `npx`. The fake is a Node script NAMED `claude`, so it
                                 # can only ever cover the arm a stand-in covers by construction;
                                 # this covers the ones only a real install has — a node CLI the
                                 # kernel calls `node`, a CLI that rewrites `process.title`, and a
                                 # launcher that stays the pgrp leader with the agent as its child.
                                 # SKIPS loudly (asserting nothing) when no agent is installed.
./qa/terminal/run.sh tui         # the REAL `aleph-tui` binary in a pty against this server.
                                 # Three observations, two flips: the program name is absent, then
                                 # `/agentpanel` puts it AND the header on screen, then toggling
                                 # again removes it. One observation could not tell "the panel
                                 # works" from "that text was on screen anyway".
./qa/terminal/run.sh panel       # boots, sets the board and WAITS for a browser. Tabs, agent-panel
                                 # row click, paste and cursor visibility are not reachable from the
                                 # wire — a tab title is a rendering, "Cmd+V is left to the browser"
                                 # is a claim about the browser, and the cursor is a rect on a
                                 # <canvas>. Needs `just wasm`; probes in `panel_probe.js`.
./qa/terminal/run.sh all         # the six non-interactive stages, one server each (not `panel`)

./qa/channels/run.sh             # both phases below
./qa/channels/run.sh reach       # feishu / line / qq really come up; msteams is the control.
./qa/channels/run.sh errors      # Lark throttle / refusal, via mock_lark.py's /__inject queue
                                 # exit code = failure count; the fixture prints every
                                 # assertion it ran. Deliberately no count here: the
                                 # first one drifted (16 in prose, 18 on screen).

./qa/plugins/run.sh manifest     # Claude Code manifest + marketplace unions through a real
                                 # load_all; ${CLAUDE_PLUGIN_ROOT}; per-plugin config across
                                 # a server restart
./qa/plugins/run.sh scaffold     # `aleph plugin init --type <rt>` output really installs and
                                 # loads — the CLI and the server are two authors
./qa/plugins/run.sh trust        # owner trust: default posture, enforce, vouch, restart,
                                 # withdraw. Three restarts, because the policy is a LOAD gate

./qa/memory_curated/run.sh       # the curated hot tier's three verbs, the note window's
                                 # load-more, and the partition contract every enumerating
                                 # memory reader resolves through (note list, stat cards, fix
                                 # queue, retrieval x-ray). Seeds through `remember` /
                                 # `note_manage` / `flag_user_correction` over `tools.invoke`,
                                 # then answers each checkpoint out-of-band with `probe.py`
                                 # (the FILE on disk and the TOOL face — never the RPC being
                                 # driven).

./qa/webview_compat/run.sh macos # br negotiation + Range + the macOS shell's own facts.
                                 # `marker-origin` is the automated one: it points a
                                 # panel-only shell at a fake Gateway bound to this
                                 # machine's LAN address — a genuinely foreign origin to
                                 # the webview — and asserts `data-shell` is already set
                                 # when the served page's FIRST inline <script> runs. The
                                 # comments used to claim the opposite, unmeasured, and a
                                 # later round drew a wrong conclusion from them. Needs
                                 # `(cd desktop/shell && cargo build --no-default-features)`;
                                 # a full-app binary or a missing LAN address is a SKIP,
                                 # never a PASS.
./qa/webview_compat/run.sh linux # the WebKitGTK half; its `flat-on-linux` step is manual and
                                 # that platform's SHELL_MARKER_JS arm is still unrun

./qa/winshell/run.sh all         # ~28 s. WINDOWS ONLY (loud SKIP elsewhere, never a pass). The
                                 # seven facts the pwsh-as-Windows-shell design rests on, asked of
                                 # the HOST's PowerShell. No server, no config, no port, nothing
                                 # under ~/.aleph — the only fixture here that boots nothing. It
                                 # exists because those numbers were measured once in a chat
                                 # window, and a number nobody can re-derive is a number nobody
                                 # can check (判据 §18). Stages: resolve / encoding / exit /
                                 # comment / length / profile / env, or `all`.
                                 #   Nothing is copied: the prologue, the epilogue, the argv
                                 # flags, the two separators joining them and the environment
                                 # allowlist are DERIVED from src/utils/shell.rs and
                                 # src/builtin_tools/code_exec.rs (`derive_ps_contract.mjs`; run it
                                 # alone to see what it read). A hand copy would be a second
                                 # statement of one fact. The ONE thing deliberately not derived
                                 # is where pwsh lives — `resolve` walks PATH with PATHEXT itself,
                                 # because reading the path out of the product would make the
                                 # fixture agree with it by construction.
                                 #   Every stage can be made to say NO:
                                 # `QA_WINSHELL_FALSIFY=prologue|epilogue|join|length|profile|env|
                                 # resolve` breaks exactly one input and the named stage goes red.
                                 # All seven were run that way on 2026-09-05 and all seven did.
                                 #   FLOOR: `FLOORS` in probe_pwsh.mjs — 18 gated checks for `all`;
                                 # below that the run goes red even with zero failures, because a
                                 # stage whose checks vanished prints a green summary too. OBSV
                                 # lines are observations and are deliberately not counted. The
                                 # floor earned itself on its first day: one run in eight came
                                 # back 16/18 (a `profile` spawn died, and that stage aborts its
                                 # arm rather than average over a failed spawn). Not reproduced in
                                 # six runs since. `profile` now retries a broken spawn ONCE and
                                 # prints how many needed it — a retry that is not counted is a
                                 # failure hidden rather than survived.
                                 #   ⚠️ Two instrument traps it is shaped around, both of which
                                 # silently flip an answer the SAFE-LOOKING way: (1) a literal
                                 # 中文 in the argv confounds the encoding measurement, so the
                                 # non-ASCII string is built from code points; (2) Node's
                                 # `spawnSync({env})` is NOT Rust's `env_clear()` — libuv copies
                                 # eleven names (TEMP among them) out of the parent, so the `env`
                                 # stage drives both arms through a pwsh launcher that calls
                                 # `ProcessStartInfo.Environment.Clear()`. Measured: 3 variables
                                 # the product's way vs 13 through Node, and only the first
                                 # reproduces the empty TEMP.
                                 #   Its first run corrected the tree it was written against: the
                                 # `PS_PROLOGUE` doc comment says the code page is "a property of
                                 # the invocation form, not of the host" (65001 via -Command, 936
                                 # via stdin). Measured here, BOTH forms answer 936 without the
                                 # prologue and both follow the console's code page. The
                                 # conclusion survives — the prologue is more necessary, not less
                                 # — but the stated reason does not reproduce. Stage 2's `2d`
                                 # line reports that and does not gate on it: a wrong reason in a
                                 # comment is a documentation defect, and gating it would turn a
                                 # host that MATCHES the comment red for being correct.

./qa/rooms_channel_bind/run.sh   # §5.22 round-9: a channel GROUP conversation bound into a project
                                 # room. Three real identities, a webhook channel (the binding key is
                                 # (channel_id, peer_kind, peer_id) and the mechanism does not know
                                 # which transport it is — webhook is the only kind a fixture can
                                 # drive with one signed POST). Its three oracles deliberately never
                                 # ask the server: the memory partition is read out of memory.db, the
                                 # session row out of sessions.db, and <room_context> plus the speaker
                                 # prefix out of the mock model's own request log. Carries the three
                                 # negative arms that make the positive ones mean something (a paired
                                 # non-member stays personal; an unpaired stranger runs on bare `main`
                                 # with no room block; a member's projects.channel.bind is refused),
                                 # and one addendum where a genuine store failure must answer
                                 # "unknown" rather than "nothing to move".
                                 #
                                 # FLOOR: `drive_bind.mjs`'s `report()` —
                                 # `process.exit(FAIL === 0 ? 0 : 1)`. SKIPPED is counted and printed
                                 # but NOT gated, so a skipped scenario still exits 0 — read the skip
                                 # count, it is not part of the floor. 2026-09-04 cold: 52 passed, 0
                                 # skipped. ⚠️ Its addendum D (does the room-settings channel section
                                 # survive a narrow viewport) is a BROWSER claim; a shell run neither
                                 # makes it nor breaks it, and it has not been looked at since the
                                 # fixture shipped.

./qa/spend_budget/run.sh         # §5.22 round-7's per-principal dollar ceiling. Every assertion reads
                                 # an EFFECT (a ledger row on disk, a wire error code,
                                 # a CLI table, a survived restart) rather than counting an RPC's
                                 # "it returned 200"; two of them read `spend_ledger` with
                                 # `sqlite3` directly rather than through `spend.query`, which is
                                 # what makes them evidence about the LEDGER and not about the
                                 # handler that reads it back. This is the fixture the root
                                 # CLAUDE.md `src/spend/` row routes to.
                                 #
                                 # FLOOR: run.sh's own `[ "$FAIL" -eq 0 ] || exit 1`.
                                 # Deliberately no count here — an inline number
                                 # for a set the script itself owns is the shape
                                 # the five rewind/claims/denied/knobs/holes
                                 # floors were removed for.
                                 #
                                 # NEEDS A REAL python3, and therefore does not run on a
                                 # Windows host, where the only `python3` on PATH is the
                                 # WindowsApps stub: it prints nothing and silently does
                                 # nothing, so a heredoc leg no-ops and the run dies far
                                 # from its cause. (No exit code is written down here —
                                 # it is stub-version dependent, and the operative half
                                 # of the sentence is the silence.) Its sibling
                                 # `multiuser_audit` was ported to Node; this one was not,
                                 # deliberately: `spend_rpc.py`, `mock_anthropic.py`, the
                                 # float comparisons and the `jf` helper are a much larger
                                 # surface than a port should carry in one round. On such a
                                 # host it is UNRUN, which is not the same as passing.
```

`plugins` uses a **short scratch root under `/tmp`** rather than `$TMPDIR` like
its siblings. The hook inventory elides action labels at 80 characters — a
documented "what is wired up" listing — and macOS spells `$TMPDIR` as a
48-character path, so under it the elision lands mid-path and cuts off the very
plugin id that distinguishes "expanded to this plugin's root" from "expanded to
something". Don't tidy that back without re-reading phase C.

Two of this fixture's own assertions were wrong before they were right, and both
mistakes are the kind worth naming. Phase C first read `commands.list`, which
serves a name/description tree and never a body: "no unexpanded
`${CLAUDE_PLUGIN_ROOT}` survives" passed there because the string cannot appear
either way. And the trust scenario first asserted "something is still loaded"
after enforcement, read 0 of 91, and looked like a regression — but 88 of those
rows are bundled *skills*, which are not plugin manifests and report `error`
before and after alike. The claim that actually discriminates is that
enforcement moves exactly the plugin rows and nothing else.

`picker_nav` needs **no mock provider** at all: every item is Panel-side
interaction, so nothing in the run needs a model. What it does need is three
widths, because the desktop master-detail folds at `max-width: 720px` while the
switch to the phone UI is at 640px — 641–720px renders the desktop screens in
their stacked form, and a round that only tested 1440x900 tested neither that
band nor the phone screens at all.

`browser_managed` needs **no mock provider** in every scenario but
`exec-offload`: it drives `tools.invoke`, which runs a tool without an agent
turn, so nothing in the run needs a model. It does need a real `playwright-cli` (pinned via config so the
run never triggers the network install path) and a browser it can launch —
which is the entire point. It exists because four defects survived four rounds
of unit tests: the managed driver, which is the DEFAULT driver, never issued
`playwright-cli open`, so every tool answered "the browser is not open";
`--headed` was prepended to `tab-new`, which rejects it outright; no line of a
real `tab-list` parsed, so the post-navigation SSRF audit ran over an empty
listing; and the PDF engine drove the same never-opened session. Every one of
them is invisible to a fake backend.

The second round of it (`tools` / `frames` / `reap` / `pdf` / `existing`) found
seven more, and the shape repeats: **a fake backend answers the question the
code hoped for.** `browser_type` passed a ref as an extra positional to a CLI
verb that takes one (`type <text>`, unlike `fill <target> <text>`) and had never
once succeeded; `wait_for` searched `evaluate`'s output for a sentinel that is a
literal inside every probe it builds, and the CLI echoes the script it ran, so
**every wait on the default driver reported "found" on its first poll**;
`playwright-cli` reports runtime failures with **exit code 0**, so
`browser_pdf` answered "Saved PDF to <path>" for a file it had been refused
permission to write; naming `outputDir` (the round-1 fix for cwd litter)
silently narrowed the CLI's allowed write roots, breaking screenshot / pdf /
session-save / upload at once; `browser_upload` never opened the file chooser it
needs; a persistent profile closed by the idle reaper phrased its "not open"
refusal differently from an unknown one, so the lazy relaunch never fired and
the reaper bricked what it reclaimed; and on the *other* driver
`browser_wait_for(text=…)` sent a string where the MCP schema has always
required a list.

The third round widened `existing` from three verbs to every verb the Chrome
DevTools MCP driver has, and found four more — all of the same shape, all
invisible to a fake backend, because **a wire contract with an external server
is settled by that server's schema and by nothing else**:
`browser_fill_form` sent its array under `fields` where the schema requires
`elements`, so it had never once filled a form; `browser_evaluate` handed back
the server's prose (`Script ran on page and returned:\n\`\`\`json\n…`) instead of
the value, which is the same defect the managed driver was fixed for in round 2
and which left the two drivers answering one call with two shapes;
`browser_select` routed through the MCP `fill`, which cannot interact with a
`<select>` at all (`fill_form` fails identically — same locator), so no dropdown
had ever been set on that driver; and `browser_upload` failed for **every path
outside the OS temp directory**, because chrome-devtools-mcp v1.6.0 added a path
guard that applies to clients which do not negotiate MCP `roots`, and Aleph
declares `sampling` only.

That last one also cost a false conclusion worth remembering: the guard was read
in a cached copy of the server picked with `ls | tail -1` — version **1.3.0**,
where the guard really is inert — while `run.sh` picks the newest by `sort -V`,
which is **1.7.0**. The source said "no restriction", the machine said "Access
denied". *Check the version you are actually running, not the one you happened
to open.*

The same round added `exec-offload`, which exists because a branch can be
unreachable from a whole surface: `browser_exec`'s spill is keyed by a tool call
id the harness Act phase mints, and `tools.invoke` has none, so over that surface
the tool always takes its other branch. It is the only browser scenario with a
mock provider, and its oracle is the mock's **request log** — turn 2 carries
turn 1's `tool_result` verbatim, which is what the model saw; the tool's RPC
reply is a different thing on a different path.

Three of the five scenarios exist to make a specific claim non-vacuous:
`frames` proves the iframe is cross-origin **before** asking whether the
snapshot reaches into it (a same-origin frame would satisfy every later claim);
`reap` runs a second profile with a far-future timeout, because "the idle one
was closed" and "everything was closed" are otherwise the same observation; and
`pdf` starts the server with `playwright-cli` **off its PATH**, because
otherwise "the engine honored the operator's pinned `binary_path`" and "it found
a binary on PATH" pass identically.

Knobs (all optional): `KEEP=1` keeps the scratch dir, `SKIP_BUILD=1` reuses the
binary already at `target/debug/aleph-server`, `QA_ROOT=<dir>` fixes the scratch
location, `GATEWAY_PORT` / `MOCK_PORT` move the ports (use distinct ones if you
run two scenarios at once).

Each run creates its own scratch `HOME` / `ALEPH_HOME` and deletes it on exit.
Nothing touches your real `~/.aleph` — which matters more than convenience: two
processes on one vault is a documented way to lose vault data
(`PROCESS_MANAGEMENT.md`).

## `announce` — and the wall it hit on its first run

The background-`bash` completion announce is a *runtime* claim a unit test
cannot reach: a job that finishes AFTER its run ended used to finish into an
empty room. The cure spends a provider turn nobody's client asked for, so the
oracle is the mock's `observations.jsonl` — a request whose conversation carries
`[system] Background process N finished`, arriving after `run_finished`.

**On real hardware the announce mechanism works.** The session log shows exactly
the claimed shape: `run_finished` for the spawning run, then a `user_message`
carrying the notice, then a **second `run_started`**. Nobody's client sent that
message. All three scenarios now pass on real hardware — `outlive` 7/7,
`collected` 4/4, `midrun` 5/5.

### The bigger thing the first run found (fixed 2026-08-16)

The first run of this fixture never got as far as an announce claim, because
`bash` never executed at all:

> `exit_code: -1`, `stderr: "Capability denied: cwd outside workspace root"`

Two subsystems answered *"where does this session work"*, and they never agreed:

* `tools/adapters/registry_adapter.rs::execute` injected `default_working_dir`
  into every `bash` / `code_exec` call that omitted `working_dir`. Its value was
  `effective_workspace` — the project override, else the **agent** workspace
  `~/.aleph/workspaces/<agent_id>`.
* `sandbox/workspace/mod.rs::for_session` put the session's cwd at
  `~/.aleph/workspaces/<sha256(session_id_json)[..16]>` and refused any cwd
  outside it.

A 32-hex directory name is never an agent id — nor a project path — so the
injected path was always outside. Verified on the observed run: the session dir
was `2f5185e22a04f821e25984a77d161ac3`, exactly
`sha256('{"type":"main","agent_id":"main","main_key":"main","epoch":1}')[:32]`,
while the agent workspace was `workspaces/main`. Sandbox `enabled = true` is the
generated default (and disabling it swaps in a `NoopSandbox` that refuses
everything), so this was not a fixture artefact: on a default install, every
shell call that omitted `working_dir` was refused.

Four rounds of unit tests missed it because each drove one half against a
stand-in for the other: sandbox tests build a `SandboxCommand` by hand (no tool,
so nothing injects), tool tests run against a fake sandbox (injection, but no
containment check). **Only a real run puts both halves in the same process** —
which is what this fixture does, and what `tests/exec_workspace_jail.rs` now
does hermetically.

**The fix**: the authorised workspace no longer travels through the tool's
model-writable `working_dir` argument. `run_agent_loop` publishes it on
`sandbox::context::EXEC_WORKSPACE` — a channel nothing on the model's side can
write — and `WorkspaceSandbox` uses it as the jail root. `working_dir: None` now
means "wherever this run works"; a relative `working_dir` resolves *under* that
root instead of being silently replaced by it; an absolute one outside it is
still refused. Callers with no run in scope (cluster node file commands, direct
callers) keep the per-session hash directory.

### `getcwd` noise under `$TMPDIR` is the fixture's own, not the product's

On macOS `$TMPDIR` is `/var/folders/…`, a symlink to `/private/var/…`. The
seatbelt profile is built from the lexical path, so `bash`'s shell-init `getcwd`
walk up the *resolved* parents is refused and every command prints

> `shell-init: error retrieving current directory: getcwd: cannot access parent
> directories: Operation not permitted`

to stderr while still exiting 0 with correct stdout. Re-running the same
scenario with `QA_ROOT` on a non-symlinked path gives an empty stderr and the
same 7/7, which is why this is recorded as an artefact of where the fixture puts
its scratch HOME rather than as a defect: a real `~/.aleph` is not behind a
symlinked prefix. If you ever see it outside `$TMPDIR`, it is a different bug.

### Why this fixture asserts a control first

Its first version did not, and the first run was unreadable: the background job
came back "failed with exit code -1" and there was no way to tell *the
background path is broken* from *bash cannot run here at all*. Every plan now
opens with a foreground `bash` probe in the same process, and the driver refuses
to evaluate any announce claim until that probe's output has reached the model.
On failure it prints the diagnosis above rather than a bare `[FAIL]`.

Two more traps this scenario paid for:

**`session_events.session_id` is not the `session_key` `chat.send` returns.**
The column holds a serialized `SessionId` JSON blob
(`{"type":"main","agent_id":"main",...}`); the RPC returns `agent:main:main:s1`.
Scoping `SessionLog` by the latter matches nothing, so every `wait_for` times
out reporting "the run never finished" about a run that finished in 80 ms.

**"The last user message" is never the announce.** The harness appends a
`<system-reminder>` as its own user message, so the newest user text is that
reminder on every single turn. The first oracle read it and reported
`announce=no` for a request that was carrying the announce three messages up.
Membership questions go to the whole request, not to one message.


## `channels` — the only end-to-end evidence that a channel *works*

`feishu` got a factory on 2026-08-18 and became configurable end to end. The
evidence for that was one line in `picker_nav`'s manual checklist, and that
line's first version asserted the wrong thing: it looked for a
`Failed to create channel` message which that code path never prints, on a
stream (stdout) it never reaches. Nobody could have noticed — a paragraph a
human is asked to read and obey has no failure mode that announces itself.

This fixture is that item, made executable, and it is deeper than the item was:

* **Construction.** `feishu`, `line` and `qq` each appear as
  `Registered channel: <id> (<type>)`.
* **The control.** `[channels.msteams]` is configured too, and must be dropped
  by `resolved_channels()` **with a named warning**. Without it the three
  assertions above prove nothing: an empty log and a probe pointed at the
  wrong stream look identical. (That is exactly how the original item failed.)
* **The flat QQ spelling.** `[channels.qq]` is written the way the Panel card
  writes it — flat, no `accounts` array — so `QQConfig::from_wire` is exercised
  on the real boot path. It has no mock, so `start()` must fail; the assertion
  is on *where*: an auth failure means the config parsed, a config error would
  mean the spelling was rejected.
* **`start()` against a real socket.** `FeishuConfig.domain` takes an arbitrary
  URL, so `mock_lark.py` stands in for the Open Platform and the channel runs
  its real startup: fetch an app access token, fetch bot info, latch the bot's
  open_id, spawn the refresher, bring up the webhook server. Every request is
  recorded, so the assertions read what the channel *sent*, not what it logged.
* **The whole loop.** A signed `im.message.receive_v1` event is POSTed to the
  channel's own webhook; the reply must come back out through the real Feishu
  send path as `POST /open-apis/im/v1/messages`, addressed to the chat the
  event came from.

### What its first two runs found

Both were green everywhere else — 16k unit tests, both reconciliation suites.

1. **feishu was not constructed at all.** `validate()` demands an `encrypt_key`
   in webhook mode. That one was the fixture's own config bug, but it is worth
   keeping in mind that the failure surfaced as a channel silently missing from
   a list, not as an error anyone would see.
2. **`require_mention = false` was ignored.** The router decides with
   `ChannelConfig::default()` for any channel with no
   `From<&*Config> for ChannelConfig` bridge, so five policy fields the Feishu
   card collects died between the form and the decider. Fixing it uncovered a
   third: the gating arms parse the *raw* config block, and the vault migration
   removes `bot_token` / `app_secret` from it — so **Telegram's bridge had been
   dead since that migration landed**, reverting to the very default it exists
   to prevent, with one `warn` and nothing red. Both arms now take their config
   from one shared `gating_config` closure.

3. **The streaming emitter had never been reachable.** Falsifying the
   "shared client" assertion produced *no change* — which was the finding.
   `try_create_feishu_emitter` rebuilds `FeishuConfig` from `Config.channels`,
   hits the same missing `app_secret`, and returns `None` through an `.ok()?`
   that says nothing at all. The reply still goes out (plain `ReplyEmitter` →
   channel → `MessageOps`), so the symptom is zero. The fixture now asserts a
   `POST /open-apis/cardkit/v1/cards`, which only happens when the emitter
   exists.

   The methodological half is worth more than the bug: **when a mutation does
   not turn a guard red, suspect your model of the code before you suspect the
   guard.** The mutation could not reach the line it targeted.

### Why webhook mode

It is the mode that can be driven from localhost. Note that the Panel's Feishu
card cannot produce it — the card offers no `connection_mode` or `webhook_*`
field — so a Panel-configured feishu is always the websocket mode, which this
fixture does not cover.

## Why a mock provider rather than a real one

The scenarios are about **timing**, and a real provider will not tell you when
an assistant turn commits or hold a run open on request. `mock_anthropic.py`
scripts both. Its `api_key` is inlined in the QA config, which works because
`ProviderConfig.api_key` is `skip_serializing` but still *deserializes* — so
the QA server never opens the real `secrets.vault`, and a run costs nothing and
reaches no network.

## What a scenario has to carry to be worth running

**A control group, in the same process.** Every "X is absent while planning"
claim is satisfied just as well by "X was never offered in this session mode at
all", and the fixture cannot tell those apart from the inside. So
`plan_handoff` opens every plan with one ordinary `building` turn and reads the
absence claims against it: 179 tools offered to the control, 51 to the planning
turn, and `file_write`/`bash` in the first set and not the second. Without that
first number the whole scenario is a tautology with a green verdict.

**Evidence the code under test cannot fake.** `mock_plan.py` records the
`tools[]` array of every request. That array is assembled by the real server
and is the only place the "hide the tool / rebuild the surface after the latch
lifts" half of the floor is observable at all — a unit test can assert about
`PlanPhase::hides` and about `cache_generation`, but not that the two meet on a
live run.

**A claim about wording, not just about effect.** Refusals reach the model as
prose. `plan_handoff` asserts on the tool_result text, which is how it caught
the one real defect this fixture has found so far — see below.

## Traps — each of these cost a debugging session

**A scratch dir under `$TMPDIR` cannot test a path restriction.** `QA_ROOT` is a
`mktemp -d`, so anything the fixture writes there is *inside* the OS temp
directory — exactly the region chrome-devtools-mcp's path guard permits. An
upload from `QA_ROOT` therefore passes whether the guard is armed or inert, and
proves nothing either way. `existing` plants its payload under `target/` instead
and asserts the file really is outside `tempfile.gettempdir()` before using it.

**Element names are not portable between the drivers.** A plain
`<div aria-label="Drag source">` is `Drag source` in playwright-cli's tree and
`StaticText "DRAGSRC"` in chrome-devtools-mcp's, which prunes the label off a
non-interactive node and keeps the text. A fixture that knew only one of them
reported "this control is not addressable" for an element that was right there
— a fixture bug wearing a product bug's costume. `ref_for_any` takes both names.

**The session log is the clock, not the wall.** `count_pending_steering` reads
the session event log, so anything paced by `time.sleep` is paced by something
the code under test cannot see. `assistant_message` landing in the DB is *not*
the same event as a provider call, and in practice the first one arrived ~30 s
into a run. The first version of the Round-9 driver sent both steers inside the
window where the log still held no assistant turn (`pending == 0`), so it never
reached the backpressure branch at all — and it looked perfectly healthy doing
it, because both messages *were* accepted. Wait on
`SessionLog.wait_for("assistant_message", ...)`, never on a duration.

**Build before redirecting `HOME`, not after.** cargo's registry, git cache and
rustup toolchain all live under the real `HOME`; a build launched with the
scratch one silently degrades into a full network fetch and then times out
("curl failed ... Timeout was reached"). `run.sh` keeps `REAL_HOME` for exactly
this. The signature of getting it wrong is a `.rustup/toolchains/` tree
appearing inside your scratch dir.

**The redirect itself is `qa/lib/scratch_home.sh::qa_redirect_home`, and it
pins `RUSTUP_HOME`/`CARGO_HOME` as well.** The paragraph above was true and
still insufficient, which is the interesting part: it names the exact signature
(`.rustup/toolchains/` inside the scratch dir) and the fixtures all guarded
their own cargo lines correctly — every `HOME="$REAL_HOME" cargo …` call site
right — and the leak happened anyway, three times, 1.3 GB each. A
per-invocation guard covers the line it is written on; `export HOME=` stays in
force for the whole process, so any *other* rustup-shimmed command (a drive
script, a `bash`-tool call a scenario makes the agent run, a command typed into
a shell that inherited the export) re-bootstraps the toolchain. The fix is
environment-level and lives in the same function as the redirect that creates
the hazard, so a fixture cannot take the isolation without the protection:

```bash
. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"      # exports HOME, ALEPH_HOME, RUSTUP_HOME, CARGO_HOME
```

`tests/qa_fixture_hygiene.rs` enforces it, deriving the fixture list by walking
`qa/` rather than from a list in the test — a newly added fixture that
hand-rolls `export HOME=` is named on its first run. There is no allowlist:
obeying the rule is free, and an allowlist would be a second source of truth
about who may hand-roll a scratch home. A per-command `HOME=… cmd` prefix is
still fine once the pins are in the environment (`browser_managed` needs two,
for a playwright-cli whose session store is HOME-scoped); only the
process-wide `export HOME=` is refused.

**The frame envelope has exactly one reader: `qa/lib/ws.mjs::normalizeFrame`.**
Four Node drivers each held a byte-identical copy of it, and what had already
decayed was the *comment*: one carried the three-shape list plus the incident it
cost, one a two-line abbreviation with both dropped, two nothing at all — a copy
born as a weakened version of another. The full prose now lives on the shared
function, which names `src/gateway/server/handler.rs::extract_topic_and_data` as
the producer-side owner it mirrors, so the two surviving representations are
*linked*, not merely fewer. `node --test qa/lib/ws.test.mjs` pins all three
shapes plus a counter of the frames that yielded NO topic, and every fixture
assertion that reports a missing frame prints its tap through `frameDigest`,
which renders that count — so a fifth server envelope reads as `unclassified: N`
in the fixture output instead of as the product-shaped lie ("no frame arrived").
Deliberately **not** shared: the `Conn` classes around it — their pending maps,
`attempt()` return shapes and poll budgets differ per fixture, and lifting those
would change what each one asserts. Deliberately **no** `qa/lib/ws.py`: the
Python fixtures as a family read only the single-shape `stream.*` JSON-RPC
notifications and never observe a bus `event` frame at all, so a future Python
fixture that needs a topic must port `normalizeFrame` first.

**Debug builds need `RUST_MIN_STACK=268435456` (256 MB).** The 32 MB floor in
`main.rs::worker_stack_size` is not enough for a debug-built agent run with
tools; it aborts with `tokio-rt-worker has overflowed its stack`. `run.sh`
exports it. Release builds do not need it.

**A stale build-script cache can break the link step** with a path into a
worktree you deleted (`build.rs` bakes `{manifest_dir}` into link args, and
each feature combination caches its own fingerprint). Cure: `touch build.rs`.
Note that `cargo build --bin aleph-server` succeeding does **not** mean
`--features test-helpers` will link — different features, different cache entry.

**A config section name copied off a unit test is not the section name.**
`config/types/general.rs` has a doc test that deserializes `[browser.policy]`
straight into `GeneralConfig`, but a *generated* config nests everything under
`[general]`. The first `browser_managed` patcher wrote `[browser.*]`, which is a
table nothing reads — so `block_private` stayed `true` and the browser was
refused the fixture's own page on 127.0.0.1. Read a generated config, not a
unit test's fixture.

**An oracle that shells out needs the real PATH.** `playwright-cli` is a node
script. The first version of the fixture's `playwright-cli list` oracle passed a
hand-made `PATH` without `node`, so it returned `env: node: No such file or
directory` — which does not contain `status: open` and therefore *passed* the
"no session is open" check. An oracle that cannot run is not an oracle that
says no.

**A control group can pass for the wrong reason.** The same fixture's control
("a non-launching verb must not open a browser") sent `{"action": "goto", "url":
...}` when `NavigateAction` is externally tagged (`{"goto": {"url": ...}}`). The
call was refused — for failing deserialization — and the control went green
having proven nothing about browsers. Assert the *reason* a control was
refused, not just that it was.

**An absence claim is a vacuous pass unless it is paired with presence.**
"the server's cwd has no `.playwright-cli/` litter" is also satisfied by a CLI
that wrote nothing at all, i.e. by a browser that never rendered. The claim only
means something next to "…and the snapshots did land under `~/.aleph` instead".

**A detail string computed before the check will lie on a pass.** The same
assertion first printed `[PASS] … — found /…/.playwright-cli` because the
detail was formatted unconditionally. A reader scanning the log cannot tell that
apart from a failure — build the detail from what was actually observed.

**Server logs are not on stdout when stdout is not a TTY.** They go to
`$ALEPH_HOME/logs/aleph-server.log.<date>`; the redirected stdout holds only the
startup banner. "No output" is not "nothing happened".

**`ALEPH_HOME` points at the `.aleph` directory itself**, not its parent. Off by
one level and you silently build a whole empty state tree, and every assertion
that reads it passes vacuously.

**`$REPO/target` may not exist.** This repo resolves a shared target directory,
so a build launched from a git worktree lands in the MAIN checkout's tree.
`busy_input/run.sh` hardcodes `$REPO/target` and works only because it has
always been run from the main checkout; `plan_handoff/run.sh` asks
`cargo metadata` instead. The failure is loud (`no binary at …`), which is the
only reason it is a trap and not a bug.

**A generated config already contains
`[policies.tool_permissions.overrides]`** — an empty table. Appending your own
copy of that header produces `duplicate key 'overrides'` and the server refuses
to boot. It refuses correctly, but it prints its startup banner *first*, showing
the DEFAULT gateway port, so the symptom reads as "it came up on the wrong
port". `plan_handoff/add_overrides.py` inserts into the existing table.

**`file_ops list` with `path: "."` fails** (`Directory not found: .`) — a run's
tool calls do not resolve relative paths against the repo. That failure is a
call that got *past* every gate, so a scenario asserting "the floor admitted it"
must assert on the absence of the floor's refusal, not on the tool succeeding.
Asserting success is asserting something the claim never said.

## What each scenario has actually proved on real hardware

Last run 2026-08-11, debug build, mock provider, isolated HOME.

| Scenario | Result | The evidence, not the verdict |
|---|---|---|
| `burst-drain` | PASS | Steer #2 parked at `pending burst at cap; deferring to busy-queue backpressure, pending=1, cap=1`, then `injected user message into running loop` **6 ms** after the draining `assistant_message` committed, with the run still alive. The fallback tick was 600 s, so 6 ms can only be the drain edge. |
| `interrupt` | PASS | `busy-input interrupt: cancelled running sibling and any delegated children`, and `run_finished{outcome: cancelled}` for the run that predated the arrival. The mode travelled config → `ChannelConfig` → run metadata → engine busy branch. |
| `queue` | PASS | Zero cancellations, and `session busy; message queued for FIFO delivery, ticket=2` — proving the arrival was *queued*, not dropped. (Check that log line: "no cancellation" alone would also be satisfied by silently discarding the message.) |

Last run 2026-08-12, debug build, mock provider, isolated HOME.

| Scenario | Result | The evidence, not the verdict |
|---|---|---|
| `handoff` | PASS (13/13) | Control turn: 179 tools, `file_write` and `bash` both present. Planning turns: 51 tools, neither present. The card carried `allowed_decisions=["allow-once","deny"]` — no standing grant even though the connection was operator — and its text named the plan, not the scratchpad. After `allow-once`, **the very next turn of the same run** was offered 179 tools again and the `file_write` returned `Wrote 17 bytes to …`. `sessions.list` then reported `plan_phase: "building"`. |
| `deny` | PASS (7/7) | Same card, answered `deny`. The tool result was `The user did not approve running 'scratchpad' (Denied)`, the next turn was still 51 tools, the `file_write` after it was still refused by the floor, and the session still read `plan_phase: "planning"`. A declined plan is not a lifted latch. |
| `floor` | PASS (8/8) | Config carried `[policies.tool_permissions.overrides] bash = "allow"`, `file_write = "allow"`, session ran at `exec_tier: "full"` — the two things that beat the tier, and neither put either tool back in the 51-tool surface. `bash` refused by the floor. Same turn sequence: `file_ops list` admitted, `file_ops delete` refused — the argument-aware half, on one tool, on real hardware. |

### The defect this fixture found

`handoff` failed its first run on one claim, and the claim it failed was about
*wording*: a `file_write` that reached dispatch anyway came back as

> `file_write` is denied by `default` in the merged tool permission policy
> (`[policies.tool_permissions]`, global → agent → channel, most restrictive wins).

The effect was right and every word of the explanation was invented. No policy
entry decided it — the read-only floor did — and the knob the sentence names
would not have changed the outcome, while the one action that would
(`request_build`) went unmentioned. The floor resolves a hidden tool to `Deny`
at the chokepoint, an explicit policy entry produces the same `Deny`, and the
consumer downstream had exactly one story to tell about both. Fixed by
`GateRule::PlanFloor` + the attribution fork in `deny_rule`; pinned by
`gate_chain::tests::a_floor_deny_names_the_floor_and_not_the_policy` (proven RED).

Worth noting *why* only a live run could find it: the code comment at that gate
asserted these calls "never reach here at all — they are absent from the
surface". Absent from the surface is not unreachable, and a real model proved it
in the first minute.

## Pairing is not optional

Generic channels are hardcoded to `DmPolicy::Pairing` — the flat-key
`ChannelPolicyConfig` parsed for non-Telegram/iMessage channels carries only
`permission_level`, `default_workspace`, `busy_input_mode` and
`tool_permissions`, so `dm_policy` and `allow_from` keep their defaults whatever
you write in the config. The first message from a new sender therefore always
returns `Permission denied: Pairing required`. `drive_channel_busy.py` performs
the real operator handshake (`channel.pairing.list` → `channel.pairing.approve`)
rather than working around the gate.

## The coalescer is a floor on inter-arrival time

`gateway::coalescer` sits in front of the busy lane with an 800 ms debounce and a
200 ms early flush, so two messages from one conversation cannot reach the engine
closer together than that. Bursts sent faster arrive as ONE merged message —
which is why `--spacing` defaults to 1.2 s.

The consequence is worth knowing before designing a scenario: the tight burst
that Round-8 ① is about (interrupts arriving faster than a run can be admitted)
is **not reachable from a channel surface**. Each channel-borne interrupt targets
a run that genuinely was already going, so cancelling it is correct behaviour.
That case stays unit-covered by
`steering::tests::a_burst_of_interrupts_does_not_eat_itself`. The observed run
was 2 cancellations for 3 arrivals at 1.2 s spacing.

## Why `leftovers` configures roots it could have left unset

`[agents.defaults] agents_root / workspace_root` are the two keys whose bug is
invisible on any install that does not set them: unset, provisioning and the
resolver both fall back to `$ALEPH_HOME/agents`, agree for the uninteresting
reason, and the divergence only appears on a relocated layout. So the scenario
sets both — to roots outside `$ALEPH_HOME`, not merely elsewhere inside it, so
a sloppy `starts_with($ALEPH_HOME)` check could not pass by accident.

Two of its claims are deliberately **gated on the create having happened**. A
refusal leaves the default layout empty too, so `nothing was provisioned into
the default layout` is a vacuous green whenever the tool did not run — which is
exactly what the first draft of this scenario reported before
`ALEPH_GATEWAY_TOOLS_ALLOW` let `agent_create` past the `tools.invoke`
transport floor.

The hooks half drives `hooks.add` / `hooks.reload` rather than the
`hooks_manage` tool: the tool's `add` raises an approval this surface has no
transport for (correct behaviour, and not the thing under test), while
`hooks.add` is the writer that used to resolve its own home-rooted path while
`load_user_hooks` read `ALEPH_HOME`.

## Known wiring quirk the channel scenarios work around

`register_plain_channel!`'s generated creator takes `_config: ChannelConfig` and
**discards it**; every such factory then hardcodes its runtime channel id to the
channel *type* (`WebhookChannel::new("webhook", …)`, `DiscordChannel::new("discord", …)`,
and so on). Meanwhile `subsystems.rs` registers the per-channel policy under the
configured **instance id**. The two only agree when the instance is named after
its type, so `patch_config.py` defaults `--channel-id webhook`. Pass a different
one to reproduce the divergence: the policy block (`busy_input_mode`,
`permission_level`, `tool_permissions`, `default_workspace`, `slash_access`)
silently does nothing.

Related: `Config.channels` is a **map keyed by instance id**
(`[channels.webhook]`), not an array of tables. The `[[channels]] id = "webhook"`
form shown in `webhook/mod.rs`'s own module doc does not parse — the server
refuses to boot with `invalid type: sequence, expected a map`.

## Known gap: tab identity does not survive a re-attach

`browser_managed/attach` drives Aleph through `close` (a DISCONNECT under
`attach --cdp` — Chrome and its tabs survive) and back through a re-attach that
`playwright_cli.rs`'s `run` performs when `LaunchPolicy::Refuse` gets
`NoSession` and the browser is still alive (Piece 4, `59dc20cce`). That
re-attach reaches the SAME OS process — `attach`'s claim `"a later tool call
re-attaches to the SAME browser process (not a relaunch)"` asserts exactly
that, and it is green.

What it does not assert, on purpose, is that the re-attached session finds the
SAME tab it had before. Measured with a real Chrome and a real marker page:
the fresh `attach --cdp` session's own `(current)`/listing-order idea of which
tab is active is **not** inherited from before the disconnect, and the CLI's
tab-listing order was observed to differ between the first attach and the
re-attach in the same run — the original `about:blank` (always present, from
`ChromiumLaunchSpec::argv`) and the profile's actual page traded places. So
"pick whichever tab the CLI calls current" is neither reliably right nor
reliably wrong; a candidate fix — select the LAST-listed tab, the fallback
`tab_registry::active_tab`'s own doc endorses for a listing with no marker —
was implemented, run against this exact repro, and picked the WRONG tab. That
doc now carries this as a known exception.

Fixing this needs Aleph's OWN persistent record of which tab a profile was
last using (`ProfileManager::tab_registry`, in `manager.rs`) consulted at
re-attach time — one layer above `playwright_cli.rs`, where the re-attach
itself lives. Not attempted here: it is a real design question for the
live-view round (plan 2), whose entire premise is a human and an agent
sharing one browser, and reaching for it from inside this fix would have
widened a narrowly-scoped change into that question. Tracked in
`docs/reference/FEATURE_LOCATOR.md` §3.12 (附录 D.9.19).

---

## 每个装置在证明什么（由根 `CLAUDE.md` 子系统路由表迁入，2026-08-30）

根 `CLAUDE.md` 的路由表现在只写「改 X 前跑 `qa/Y/run.sh`」。**为什么非真机不可、以及每个阶段
挡的是哪一类假绿**，母本在这里。判据全文见 [FEATURE_LOCATOR 附录 E](../docs/reference/FEATURE_LOCATOR.md)。

- **`file_search`** — 改 walk / 上限 / deny 绑定前跑 `{floor,page,reach,steer}`。`floor` 证明
  `[sandbox] deny_read_globs` **从配置文件**绑住 `grep` 与 `find`，且 `no_ignore` 掀不掉它——单测把
  `denied_paths` 手递进去，证明的是**谓词**不是**接线**。`reach`/`steer` 读 mock provider 的 request log，
  那是「模型收到了什么」唯一的 oracle。耗时与峰值堆另用 `cargo bench --bench file_search_scan`
  （`ALEPH_BENCH_ROOT` 可指向任意树）。
- **`channels`** — 改 `interfaces/<channel>/` 或通道接线前跑。三阶段：
  - `reach` — 三个通道**真被构造**（msteams 作对照组）· qq 扁平拼法过了配置解析 · feishu `start()` 对
    mock Lark 真拨号 · webhook 事件 → agent 回合 → 回复打回 `im/v1/messages`。
  - `errors` — 旧版 `400 + 99991400` 限频被重试且退避读 `x-ogw-ratelimit-reset` · `403` 报状态码且不重试 ·
    无限频码的 `400` 是终态。
  - `approval` — 通道腿的「通知 + 永久等待」：卡带无过期哨兵**且该键真在 wire 上**（否则断言恒真）·
    真发到 Feishu · **过了旧的 120 s 死线仍在 pending** · `/approve` 文本回复仍能结掉一张已超死线的卡。
    ⚠️ **自带一次 reboot**：`policies` 是 `ReloadImpact::Restart`，运行时 patch 会被保存并忽略，
    而那会让这一阶段对着一道**从未武装的闸**作断言。
- **`session_order`** — 改转录顺序、删除边界或任一 idle sweep 前跑。两个 backend 各驱动同一段对话，
  停机后把戳改成**降序**再重读——**生产数据上两序恒合，所以不打乱就等于没测**。断言：服务序不变 ·
  `session.truncate` 真的到达数据库 · 它留下的是**头部** · 两个 backend 销毁同一批行。
- **`browser_managed`** — 改 `src/browser/` 或 `src/builtin_tools/browser_tools/` 前跑。
  `{open,ambient,headed,tools,frames,reap,pdf,existing,exec-offload,attach}`——**两个 driver 的每个动词都有效果断言**。
  `attach` 证的是 Aleph 自己启动 Chrome、`playwright-cli` 只 `attach --cdp` 上去；已知缺口见下方
  "Known gap: tab identity does not survive a re-attach"。
  **改启动链（`chromium_launch` / `chromium_resolve` / `playwright_launch` / `playwright_cli`）必须跑
  `attach`**——它是唯一证明「**Aleph 起的**浏览器」而不是「某个浏览器」的阶段。它用 `pgrep -f`，所以
  和这个目录里其它场景一样**只在 unix 上跑得动**；Windows 上它不是坏了，是没覆盖。
  ⚠️ **这套装置的绿有一部分是环境属性，不是代码属性。** 2026-09-05 的启动链翻转发货了一个
  `--use-mock-keychain` 缺失、因而**每个页面的第一次导航都永不派发**的 Chromium；整套装置之所以抓到它，
  纯属 `qa/lib/scratch_home.sh` 重定向 `HOME` 的副作用——**用真 HOME 跑，没修的 argv 0.62 s 就导航完了**
  （实测）。所以**去掉或收窄那个 HOME 重定向，整套装置会在缺陷完好无损的情况下变绿**。今天挡住这一类的
  是 `attach` 里那条 argv 断言（`--use-mock-keychain` 在场、且在 `extra_args` 之后）与
  `chromium_launch.rs` 的 `rposition` 单测，**不是任何场景自己的绿**。全文见
  `docs/reference/FEATURE_LOCATOR.md` §3.12 第七轮 ①③。
- **`btw_tui`** — 改 `/btw` 的到达顺序或退休面前先读 FEATURE_LOCATOR §4.14 的机制图，再跑 `{frames,promote}`。
- **`agents_viz`** — 改 `run.subagent_tree` 的产地 / relay / 可见性分类、`events.subscribe` 的过滤语义、
  执行清单三载体（`tool_call_completed` snapshot · `RunSummary.plan` · `chat.history.plan`）或 TUI/Panel
  的 tasks/agents 面板前跑 `claims`；改 Panel `/dashboard/subagents` 的渲染再跑 `panel` 并挂上浏览器。
  三个 socket 三种订阅形状，D5 是负向臂——少了它 D4 对「订阅才是载体」什么也证明不了。
- **`picker_nav`** — 改 `interfaces/webchat/` 的键盘导航/渐隐前跑：键盘 walk · 条件渐隐 · 手机端加 provider，
  三档宽度各带效果断言。
- **`canvas`** — 改 `src/canvas/` 或 Panel canvas 视图前跑：九项清单每条带效果断言。
- **`terminal`** — 改 `src/gateway/pty/foreground.rs`、`src/gateway/runtime/`、`crates/agent-detect/`
  或 `src/builtin_tools/terminal.rs` 前跑 `{identify,wait,quiet,cwd,real,tui}`；动 Panel 终端视图或
  `components/sidebar/agent_panel.rs` 时另跑 `panel`（需要浏览器）。
  - `identify` — **本轮存在的理由**。第 1 期的面板在生产上从未识别过一个 agent：采样器拿到的是
    `PtySession::shell`（**spawn 时刻**的标签），而 Panel 的终端只发 `{rows, cols}`，所以标签恒为
    `zsh`，`identify_agent` 答 `None`，检测引擎在读第一条规则**之前**就早返回 `Unknown`——21 份
    manifest 与它们的单测全绿，因为**每一条都自己把 agent 名字递进去**（判据 §2：问的不是"规则对不对"，
    是"它什么时候会红"）。这里 spawn 的是 `sh`，agent 是**事后敲进去的**，唯一能把它变成
    `agent: "claude"` 的只有前台探测。负向臂是同一台服务器上一个没跑 agent 的 shell；
    第四条断言「**探测确实答了**」把那条臂从「什么都没看」里救出来——`program: null` 会让另外
    三条同样为真（判据 §8）。
  - `wait` — `reached` 与 `timeout` 两臂**各带一个耗时断言**：一个从不等待的 `wait` 同样会答
    `timeout`，光看 outcome 的绿和瞎的绿长得一样（M4 变异实测：outcome 那条仍绿，耗时那条红）。
  - `quiet` — 三次观测两次翻转：先证明它**当时不是 quiet**，再证明标记落在 **30 s 的钟上**而非立刻，
    最后证明**一帧能把它清掉**。少了任何一条，一个永不复位的黏滞标志都读起来一样。
  - `cwd` — 三层来源用**三个真的不同的目录**；第二个会话不发 OSC 7，所以它的答案只可能来自探测。
    只有一个会话时，「OSC 7 赢了」和「探测什么都没说」是同一个绿。**故意不证**的：spawn 目录那一层
    （要让探测**失败**才能到达，从 wire 上安排不出来）、`program: null`、Panel 的渲染、以及另外 20 份 manifest。
  - 装置的画面**不是手抄的**：`derive_chrome.mjs` 按 rule id 从 `claude.toml` 里取出字面量、拼成行、
    再用 manifest 自己的正则回验；它**故意不判定哪条规则胜出**（那要重写一遍 region 与优先级
    ——第二套引擎，判据 §1），胜者由运行时的 `terminal{explain}` 用**发货的引擎**报出规则 id 来断言。
    读 manifest 用的 `toml_min.mjs` 同理**不是 schema 的第二份表述**：它只答「这份文件说了什么」，
    「这份 manifest 合不合法」由 `agent_detect::manifest::parse_manifest` 在运行时回答。
  - **平台**（2026-09-05）：`identify` / `wait` / `quiet` / `cwd` 与 `panel` 布板在 **Unix 和 Windows
    上都跑**。在那之前整套装置在 Windows 上是 UNRUN，原因不是设计而是**语言的意外**——驱动是
    Python，而这台主机上**没装解释器**（PATH 上的 `python3` 是 WindowsApps 存根、exit 49；
    `uv` 装着但还没有 managed 解释器）。`run.sh` 的 `PY_CMD` 因此按 Aleph 自己的顺序找
    （真 `python3` → `uv run`，与 `bootstrap-runtime` 的 `DEFAULT_TARGETS` 一致），
    **刻意不替操作员 `uv python install`**——一个中途悄悄下载运行时的装置是它自己的隐患。
    **Windows 恰好是前台探测没有 `tcgetpgrp`、走树是全部答案的那个平台**，所以那是它最不该跑不了的
    地方。`real` 与 `tui` **没搬**，理由是结构性的：`probe_alive.py` / `drive_tui.py` 用 `pty.fork`
    驱动程序，Node 没有原生模块就没有 pty ⇒ 它们在跑不了的地方**响亮地 SKIP，不报 pass**（判据 §2）。
    ⚠️ shell 交互的两种拼写收在 `drive_terminal.mjs` 的**一个** `SHELL` kit 里，不是逐调用点的分支——
    逐点分支正是「Windows 那条臂安静地不再敲 agent、而阶段照样报出对照会话的行」的形状。
  - `real` — **假 agent 只能覆盖一条臂**：它是个**名叫** `claude` 的 Node 脚本，所以"按名字认出来"
    这条臂是它按构造必然覆盖的那条，也是唯一一条。真实安装才有的三种形状它碰不到——内核把
    `#!/usr/bin/env node` 的 CLI 报成 `node`；重写了 `process.title` 的 CLI 让标题占了 `argv[0]` 的位置，
    而 macOS 会把**环境变量**渗进它后面的 `cmd()`；包装器自己留在进程组组长的位置、真 agent 是它的孩子。
    2026-09-05 在本机量到：`npx pi` 的组长是 `npm exec pi …`，真 `pi` 是它的孩子；而一个值里带空格的
    导出变量会把**裸词**（`prefer` / `modern` / `like`）撒进命令行里程序名该在的位置。
    候选名单**从 `engine.rs` 派生**（`agent_label` 与 `interactive_agent_executable` 对四个 agent
    答案不同：`agy` / `copilot` / `cursor-agent` / `kiro-cli`，手写一份当天就是错的，判据 §1）；
    "装了"不等于"能跑"，所以每个候选先在 pty 里活 3 秒（本机的 `codex` 缺 vendored 二进制，一秒内就退）；
    排序**偏好带 shebang 的**，因为那才是替身伪造不了的形状。一台没装 agent 的机器上它**响亮地 SKIP**
    并明说自己什么都没断言——静默通过的绿是那种唯一没有意义的绿（判据 §2）。
  - `tui` — `interfaces/tui/` **从来没有对着活服务器跑过**。它的 agent 面板渲染
    `shared_ui_logic::entry_name`（`program ?? agent ?? label`），数据来自实时的
    `runtime.agents.list` + `events.subscribe`，而它现有的每一条测试都是把**手搓的**
    `AgentPanelData` 渲进测试 backend——渲染器永远是对的，没有任何东西检查送进去的值来自 wire，
    这正是第 1 期缺陷藏身的形状。这里跑的是**真的 `aleph-tui` 二进制**（在 pty 里，120x40）：
    三次观测两次翻转——先证明程序名**不在**屏幕上，再 `/agentpanel` 让它和表头一起出现，
    再切一次让它消失。只观测一次的话，「面板工作了」和「那行字本来就在屏幕上」是同一个绿（判据 §2）。
  - `panel` — 唯一一个**boot 完就等**的阶段（和 `canvas` 同形）。标签页 / agent 面板行点击 / 粘贴 /
    光标可见性四件事**从 wire 上够不到**：标签标题是一次渲染，"Cmd+V 交给浏览器"是一句**关于浏览器**的断言
    （没有任何单测有浏览器），光标是画在 `<canvas>` 上的一个矩形。装置把局面摆好——一个 spawn 成 `sh`
    然后跑起 agent 的会话，加一个什么都没跑的对照会话——再打印带效果断言的清单；探针在
    `panel_probe.js`（`qaTerm.tabs()` / `.route()` / `.inkCount()`），光标那条比**三次读数彼此之间**的
    差，不比字面量（判据 §18）。需要先 `just wasm`：debug server 从磁盘读 `dist/`，空 dist 会让每一条都
    "失败"在错误的原因上。
  - ⚠️ 它的**生成配置那一次 boot 带了 `--port`**——2026-09-05 起**每一个有生成 boot 的装置都带了**
    （此前只有它和 `channels`；`webview_compat` 没有生成 boot，不在其列）：那一次 boot 绑的是**内置默认端口**，机器上只要有别的 server 占着，进程在写出 config
    之前就退出了，症状是 `no config generated at …`——读起来像路径或权限问题，原因在日志下一行。
- **`rooms_channel_bind`** — 把一个通道群会话绑到项目房间（`Real-machine QA for binding a channel
  group conversation to a project room.`，见其 `run.sh:2`）。改 `projects.channel.*` handler、绑定的
  CLI 面、Panel 的项目通道段或 `rescope_attribution` 前跑。它挡的那一类假绿写在自己的 header 里：
  这条链上的每一件事——handler、CLI、Panel 段、arm 2 的名册闸、`rescope_attribution`——此前**全部**
  只有编译期与单测证据，没有任何一件对活网关说过话。路由入口在 [GATEWAY.md](../docs/reference/GATEWAY.md)，
  不在根 `CLAUDE.md` 的路由表里。