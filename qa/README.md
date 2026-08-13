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
```

`browser_managed` is the one scenario that needs **no mock provider**: it drives
`tools.invoke`, which runs a tool without an agent turn, so nothing in the run
needs a model. It does need a real `playwright-cli` (pinned via config so the
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
