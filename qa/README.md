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
```

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

**Server logs are not on stdout when stdout is not a TTY.** They go to
`$ALEPH_HOME/logs/aleph-server.log.<date>`; the redirected stdout holds only the
startup banner. "No output" is not "nothing happened".

**`ALEPH_HOME` points at the `.aleph` directory itself**, not its parent. Off by
one level and you silently build a whole empty state tree, and every assertion
that reads it passes vacuously.

## What each scenario has actually proved on real hardware

Last run 2026-08-11, debug build, mock provider, isolated HOME.

| Scenario | Result | The evidence, not the verdict |
|---|---|---|
| `burst-drain` | PASS | Steer #2 parked at `pending burst at cap; deferring to busy-queue backpressure, pending=1, cap=1`, then `injected user message into running loop` **6 ms** after the draining `assistant_message` committed, with the run still alive. The fallback tick was 600 s, so 6 ms can only be the drain edge. |
| `interrupt` | PASS | `busy-input interrupt: cancelled running sibling and any delegated children`, and `run_finished{outcome: cancelled}` for the run that predated the arrival. The mode travelled config → `ChannelConfig` → run metadata → engine busy branch. |
| `queue` | PASS | Zero cancellations, and `session busy; message queued for FIFO delivery, ticket=2` — proving the arrival was *queued*, not dropped. (Check that log line: "no cancellation" alone would also be satisfied by silently discarding the message.) |

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
