# Changelog

All notable changes to the Aleph project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [26.9.1]

Five days, 517 commits, 809 files, +98.4k/-11.4k (of which +58.2k/-10.5k is
source). Two features and one very large sweep. First, **the terminal becomes a
first-class surface**: a VT emulator lives in the gateway, holds the screen
server-side, and ships dirty-row patches to a canvas2d grid in the Panel —
which means the wire carries a *screen*, not a byte stream, and every reader
agrees on what geometry a frame describes. Second, **a project room can be
bound to a channel conversation**, so a Feishu/Slack thread and an Aleph room
are one place with one roster and one scope. Third, and largest by far: an
**audit sweep across every subsystem** — a 40-defect hardening round against
the openclaw reference, eight rounds of per-module logic audits, and 22
numbered fix rounds. Underneath the sweep runs one repeated shape: a predicate
that answered "I could not tell" with "no", and the fix is almost never at the
call site — it is at the type that let the two answers look alike.

### Added

- **An embedded terminal, end to end.** A character grid with wide-glyph and
  wrap semantics, straddled-spacer repair, scrollback with correct eviction,
  the alternate screen buffer, resize, cursor positioning, erase, C0 and SGR —
  parsed against a pinned `vte` API surface. The screen is held server-side and
  published as diffs on a 16 ms cadence; the raw byte topic is retired.
  `pty.attach` replays into a client that does gap detection, stale-frame
  discard and attach buffering. The Panel gets a Terminal nav mode, a canvas2d
  grid renderer, keyboard encoding, and a font stack whose shipped default
  actually has icon glyphs.
- **The terminal's authority is scoped, not ambient.** `pty.screen` / `pty.exit`
  delivery and the five addressed methods are gated on session ownership; spawn
  cwd is jailed to the registered workspaces (pinned to the resolver that reads
  configuration, not a second copy); geometry that would abort the daemon is
  refused; a `[policies.terminal]` session gate ships live and default-on, and
  the switch's writer is gated and recorded.
- **Project rooms bind to channel conversations.** `projects.channel.bind` /
  `unbind` / `list` with one protocol author for the wire shapes, a
  roster-gated channel-binding arm in room claiming, `session_store::rescope`
  taking a project id rather than a rendered scope, every agent's row rescoped
  for a bound conversation, and the room scope carried across a harness spawn.
  Reachable from `aleph projects` on the CLI, from room settings in the Panel,
  and from `project_manage bind_workspace`. Covered by a real-machine fixture
  (`qa/rooms_channel_bind`).
- **`grep` and `find` are tools.** Content search and filename discovery each
  get their own verb over the shared `.gitignore`-aware walk, and `bash` stops
  describing the web-search tool in its own description.
- **One implementation of "how do I search."** There were two. Now each provider
  *declares* what it can express and the claim is pinned to the wire; the
  request's capability bits are a function of the request; an explicit provider
  is honoured; the freshness vocabulary and the "what this result set is
  missing" sentences each have exactly one owner; include/exclude domain lists
  reach the backends that have them; one call can fan out to several backends.
  The search tool exposes the knobs that already existed and stops discarding
  four fields, and the Panel's provider cards are derived from the protocol's
  single list of which backends exist.
- **A history window that loads earlier across clients**, with the same
  vocabulary in the protocol, the CLI, the TUI and the Panel.
- **A repair pass for on-disk session damage.** Two already-fixed writer bugs
  left millisecond `last_active_at` stamps that outranked every healthy session
  in the descending sort, and torn `metadata.json` documents that made
  `list_sessions` warn on every poll. The pass normalizes the timestamps,
  rebuilds torn metadata from the transcript (defaults where the transcript
  cannot answer — a zero token count reads as "unknown", an invented one would
  read as measured), and quarantines what it cannot rebuild.
- **`TopologyDiff::EdgeUnknown`,** so an unparseable edge kind is reported as
  unknown rather than silently dropped from the diff.
- **Approval that survives inattention.** A still-parked approval is re-raised
  on a backoff schedule; an attended card waits instead of expiring; launching
  an app is ungated for an attended operator.
- **`SESSION_KNOBS.md`,** and a `CLAUDE.md` that stops carrying detail no single
  reference document could say — the criteria index now holds shape names, with
  triggers and full text in the FEATURE_LOCATOR appendices.

### Fixed

- **40 verified defects across five subsystems** in one openclaw-reference
  hardening round, plus eight rounds of per-module logic audits (teams,
  thinker, tool_metadata, tool_output, tools, utils, vision, wizard,
  verification, workflow, resilience, looping, mcp, media, memory, harness,
  hub, logging, loop_graph, group_chat, guardrails, generation, extension,
  clarification, cli, canvas) and 22 numbered fix rounds. Highlights below; the
  reports are archived under `review-results/`.
- **SSRF and rebinding, closed as a class rather than a list.** URL fetches in
  `generation`, `media_send`, `google_meet`, `a2a_agents` and `export` now
  carry the policy from config instead of each deciding for itself;
  `web_fetch` skips the provider path that reopened a DNS-rebinding TOCTOU;
  protocol-relative and percent-encoded bypasses in `export` are closed; the
  Discord token regex is anchored and joins the leak-detector assets; the PII
  email regex caps its TLD quantifier so it cannot backtrack unboundedly.
- **A failed run now reports that it failed.** The exit arm computed
  `HarnessError::class()` and fanned all four variants into `Cancelled`, so a
  provider auth failure rendered on every trace surface as `Info` labelled
  "cancelled" — softer than a `HitLimit` cap and indistinguishable from the
  user pressing stop, while the session log beside it said `Errored`. One halt
  vocabulary now spans protocol, CLI, TUI and Panel.
- **The transcript has one order, and it is the order the rows were recorded.**
  Two backends each answered "what order?" separately, and two of the disagreeing
  paths were DELETEs.
- **`require_operator_tier` was fail-open on an absent `TurnContext`,** and the
  justification comment for that was unconditional — it read as if the absence
  had been considered.
- **`caller_may_choose_directory` was constant-true.** A predicate that cannot
  return false is not a gate; five more claims of the same shape were narrowed
  at their own sites rather than at the readers.
- **A session that was stopped or ended can no longer be silently resumed**, and
  the FTS delete, the source delete and the count update happen in one
  transaction instead of three.
- **Idempotency keys are namespaced by `(principal, method)`,** and the connect
  rate limit stopped locking out an entire NAT because one client behind it
  misbehaved.
- **`crate::sync_primitives` is now the only door.** Locks, `Arc` and atomics in
  canvas, identity, loop_graph, extension, runtimes and the capability slots
  were reaching past it into `std::sync` — which is how a poisoned mutex
  becomes a panic in one subsystem and a recovered guard in the next.
- **`looping` refunds an iteration on `Active → Paused` with an unrun tick**, so
  pausing no longer costs a turn that never happened.
- **The macOS shell fork ban is lifted for shells,** which never bought security
  (`(deny process-fork)` does not stop `rm -rf`, only compound commands) and did
  break every multi-command invocation; the per-user `TMPDIR` is granted.
- **Panel search settings stopped acting on names it had not resolved** — an
  unresolved delete target was treated as permitted, and the default-provider
  refusal is now shown before the click rather than after it.
- **The release pipeline was gated shut, for the second time.** `0dc1ff85a`
  dropped the eight tracked files under `interfaces/webchat/dist/` with the
  commit message "release job to take over"; no release job was taught to build
  the WASM, and the workflow still says "pre-built and committed to git — no
  WASM build here". `panel-dist-check` is `needs:` of every build, so every
  release run would have failed at the first job — and, worse, `assets.rs`
  embeds that directory at *compile* time, so any clean checkout was building
  an `aleph-server` with an empty Panel inside it. The same removal shut the
  pipeline for two days at `033814185` on 2026-08-13. The files are tracked
  again and the root `.gitignore` no longer contradicts the twenty-line comment
  in `interfaces/webchat/.gitignore` that explains why they must be.
- **CI: the ubuntu test job stopped fitting in 16 GB.** Its own comment
  predicted this precisely — "if this dies again the sampler will show a
  decline rather than a plateau, and that is a different problem" — and that is
  what the sampler shows: 13.0 GB to 669 MiB in five minutes, then exit 143.
  16 GB of swap on the ephemeral volume, rather than another `-j` cap the
  earlier measurements had already shown to be inert.
- **Every continuation re-seeded its whole history.** `seed_history` probes the
  session log for non-emptiness with `get_events(id, from, to)` — a seq
  *range*, where `to` is exclusive and seq allocation starts at 1, so `Some(2)`
  is the one-event probe. A review pass read `to` as a limit and lowered it to
  `Some(1)`, which asks for `seq < 1` and matches nothing on any log: the guard
  answered "empty" every time and doubled the conversation context on every
  continuation run — the exact duplication it exists to prevent, introduced by
  the edit whose commit message says it fixed it. The bound is a named constant
  now, because the literal reads like a count.
- **Four Windows-only test failures** in `session_store::migration`: the fixture
  joined a session *key* to a path, and `:` is illegal in a Windows filename —
  production never does that, it routes every key through
  `sanitize_key_for_dir`.
- **The Rust Doctor workflow wrote its diagnosis only to the step summary,** so
  its failures — it has never been green since it landed — said nothing at all
  to anyone reading the logs. It also checked out without submodules, which
  `include_dir!` needs at compile time.

## [26.8.27]

Four days, 361 commits, 568 files, +61.5k/−5.8k (of which +40.5k/−5.7k is
source). Three threads and one correction. First, **project rooms grow a full
face** — a `project_manage` tool, Kanban / Workspace / Memory tabs in the
Panel, a roster-gated `projects.changed` push topic, and a prompt layer that
finally tells the model who else is in the room. Second, **waiting becomes a
wire concept**: a run sitting in its session's busy lane used to emit nothing
at all between `chat.send` returning an id and `RunAccepted`, so every client
painted "thinking" over a run the engine had never heard of. Third, **the
streaming renderer stops rebuilding everything on every token**, in the Panel
and the TUI both. Underneath all three runs a **process-global handle
migration**: 46 `OnceLock`s became `CapabilitySlot`s, where writing a value and
stamping that it was written are one action instead of a discipline — because a
handle that was never installed and one installed with a permissive default
read identically from outside, and half of this release's diagnostics work is
the same conflation in a different subsystem.

### Added

- **Project rooms, end to end.** `project_manage` is the conversational face
  (R8); the Panel gains Kanban, Workspace and Memory tabs over room-scoped
  teams, goals and loops, plus read-only workspace browse; `projects.changed`
  is a push topic whose delivery is gated on the roster. Authorization behind
  every `projects.*` gate is one shared derivation rather than a predicate per
  face, and a room-claimed session key always stamps the room's scope.
- **A room roster layer in the prompt.** The model is told who else is in the
  room, and a delegated child inherits the same `<room_context>` its parent
  had — a fan-out where each member re-derives the roster is a fan-out with N
  rosters. Covered by a real-machine fixture (`qa/teamchat_rooms`).
- **The waiting phase, as its own wire representation.** `StreamEvent::RunQueued`
  and its protocol twin, `TicketGuard::ahead` for lane position reported from
  the waiter's existing wake point, and the lane carried on `chat.history` —
  the attach-time authority, so a client that reconnects mid-wait rebuilds the
  queued phase from a snapshot instead of guessing. The busy-input queue itself
  is now crash-durable.
- **`CapabilitySlot`, and a doctor check that can see through it.**
  `install(v)` writes the value and stamps the outcome as one action; a
  conditional install's `else` arm must `decline("what was missing")` rather
  than silently skip. Each slot names its own `MissingSemantics` in its own
  module — derived from what the fallback branch actually hands the reader, not
  from the config default and not from the slot's name. `core/capability-wiring`
  reports three states, not two: not booted (this process cannot answer), booted
  and complete, booted with holes (named per slot, severity derived).
- **A streaming renderer that renders only what changed.** In the Panel,
  `TypewriterRenderer` mounts once and splits into two zones — a stable zone
  gated on the freeze boundary and a tail zone that re-renders at O(tail) — with
  the character reveal mapped to byte offsets by an incremental cursor instead
  of a per-frame `chars().take().collect()` rescan. In the TUI: windowed chat
  rendering with an `Rc`-shared streaming prefix, a per-message rendered-line
  cache, tail-only markdown conversion, queued gateway events coalesced into a
  single draw, and no `terminal.draw()` at all on ticks that change nothing
  visible.
- **Reconnect that reconciles instead of resetting.** The TUI joins the run in
  flight on an attached session, renders peer messages, backfills turn age, and
  stops holding other sessions' runs; the Panel registers a conversation on
  every chat surface and repairs reconnects at the root rather than inside a
  component with a mount condition; the `/btw` overlay settles across a
  reconnect by asking `agent.status` about the one id it still holds.
- **The protocol owns the receipt codes both sides classify by.** The Panel now
  reads the server's `error_code` instead of matching keywords in a message,
  and a Cancelled send stops being painted as a danger banner.
- **`note_manage` can read one note by address.** Retrieval returned ranked
  excerpts truncated to 4k each, and update replaced whole documents — so the
  model could overwrite a note it had never been able to read in full. The
  governance verdict archive also gets its first reader and a retention window.
- **Markdown streaming safety in shared UI logic.** CommonMark-compliant fence
  detection and reference-link-definition boundaries, so a partially-arrived
  document is never split inside a construct.
- **Model-aware summarizer budget, with a cheap-summarizer fallback retry.**
- **Closed-set effect-evidence grades on mutating desktop actions**, so a verb
  reports what it actually observed rather than that it was dispatched.
- **The full desktop app keeps its remote target across restarts**, and probes
  the gateway's `/ready` endpoint rather than a bare TCP connect before
  navigating — a socket that accepts is not a server that can answer.

### Fixed

- **Eight diagnostics checks answered "I could not look" with "there is nothing
  there."** `Path::exists()` returns `false` for two different worlds, and in a
  subsystem whose entire job is telling an operator what is true, that
  conflation is the job. Six sites read the error as "absent, and absent is
  fine" — including a vault that cannot be opened reporting "no secrets stored
  yet", the one sentence that stops an operator investigating, on a repo that
  already records vault data loss as an observed failure. A ninth of the same
  class was closed in `browser_runtime`, where a panicked probe task rendered
  as `[ok]`. The fix is a chokepoint plus a source-level guard, not nine edits:
  `Presence`/`DirListing` make the third answer an `Err(Finding)`, and
  `Unknown` is deliberately *not* a `Presence` variant, so spending it as
  absence has to be written out.
- **`core/capability-wiring` was registered in the offline registry.** Its cold
  branch fired on every call, on every machine, forever — so the exit code it
  documents as CI-gateable could never be 0. A check whose premise can never
  hold is not a gate.
- **The harness line ratchet had been disarmed by grant, not by drift.** A
  prior entry set `CEILING` to measured + 1024 and called the gap "headroom",
  while asserting in the next sentence that the budget was still a hard cap.
  Both cannot be true: R10's redline permitted ~20% silent growth for a day,
  and "src/harness/ delta 0" — the claim five comparison rounds rest on — was
  unprovable while it stood. Measured now: 5233, itemised.
- **rust-logic-audit and review batches applied across nine subsystems:**
  secrets/security (4 Critical), clipboard (4 Critical), config (3 Critical + 1
  Warning), context (2 Critical), clarification (2 Critical + 2 Warning),
  session (3 High), skill (3 High + 1 Medium), plus cli and canvas warnings.
  SSRF and hostname handling, the DuckDuckGo `uddg` URL decode, and
  `SECRET_SUFFIXES` were all widened.
- **Four streaming-render defects that only a real client shows.** The message
  bubble remounted on every token; the entrance animation played twice at
  stream completion; the unfrozen tail was baked into the markdown cache; and a
  cached prefix offset could outlive a content swap that shrank it.
- **A single command could take the daemon out through stdin.** `command_text`
  is now bounded. Separately, a truncated stdin keeps its head *and* its tail,
  cut on char boundaries — dropping the tail throws away the one part a reader
  is usually looking for, and the hot paths for file writes, line counts and
  note bodies are capped alongside it.
- **Windows reserved-name filenames were sanitized past the wrong boundary.**
  The stem rule now cuts at the first `.` or `:`; unsafe agent ids get named
  reasons rather than a silent rewrite.
- **Panel and phone hardening.** Attachment size and count are capped to bound
  base64 inflation; URL credentials are stripped and protocol-relative links
  blocked; a reconnect no longer clobbers in-progress edits; the phone memory
  tab no longer loses the URL fragment or panics on a disposed read.
- **iOS: four independent trust and storage defects.** Cert-pin saves are
  serialized against concurrent challenges, the Keychain pairing URL is scoped
  to the device that stored it, overlapping TLS trust prompts fail closed, and
  `isInspectable` is gated behind `DEBUG`. The project also moves to Swift 6
  language mode, and owns its IPv6 bracket rule instead of assuming
  Foundation's.
- **A refused transcription endpoint took the daemon down with it.**
- **A failed run was reported as a cancelled one.** Four error classes fanned
  into `Cancelled` at the exit arm, so a provider auth failure rendered in the
  trace as a neutral "stopped" — lighter than hitting the turn cap and byte-for
  byte identical to the user pressing stop, while the session log beside it
  said `Errored`. Every guardrail `Block` now settles through one path, so the
  second producer of that outcome stops leaving orphan `tool_use` calls behind.
- **`purge_all` blocked the async runtime**, and now falls back to inline
  execution when no Tokio runtime is in scope rather than panicking.
- **A missing session-service handle dropped work without a word.** Approval
  records, replayed messages on the OpenAI-compat history path, both branches
  of the slash-command fast path, both on `SimpleExecutionEngine`'s, and a
  skipped run-id/occupancy stamp now each name what they dropped.
- **CI told the truth about three of its own failures.** The ubuntu exit-143
  death names its resource (memory, not disk); the security audit's path
  filter now includes the code it lints; and `Info.plist` is staged into
  `OUT_DIR`, because a link-arg cached in `target/` naming a file in the source
  tree outlives what it names — and the failing case is by definition the one
  where the build script does not run.
- **This release's own gates were red before it started.** `cargo fmt --check`
  failed on 26 files across all three platforms, which — under `bash -e` —
  aborted the lint job before clippy and before the `--all-targets` check, so
  the two steps that catch real regressions had not run in days. A `panic!` in
  `build.rs` tripped `clippy::panic`; the lint exists to keep panics out of code
  that ships, and a build script is not that code, so the allow is scoped to
  that one file.

## [26.8.23]

The largest release to date — 1,461 commits over 23 days, 2,513 files,
+378k/−85k (of which +286k/−83k is source). Four threads. First, **three new
subsystems**: a collaborative whiteboard canvas, a per-principal spend ledger,
and an explicit scope layer that finally gives every memory read and session
listing an answer to "who is asking". Second, **the Panel grows up** — a shared
searchable picker replacing 56 always-visible provider cards, the canvas gallery
moved into the left column, and every Chinese string literal a phone screen can
reach collapsed into a locale table under a crate-wide census. Third,
**multi-user reaches round 7**: an agent-axis admission gate, revocation that
takes effect without a restart, roster-mediated room visibility, and a durable
per-principal budget. Fourth, and the reason most of the fixes below exist at
all, a **real-machine QA discipline** — fourteen new fixtures under `qa/` that
drive a live server or a real browser and assert effects rather than calls.
Nearly every defect in the Fixed list was green across 16k in-process tests and
only visible once something real was on the other end of the wire.

### Added

- **Whiteboard canvas.** A collaborative drawing surface with a four-layer
  architecture, an optimistic-lock concurrency protocol (revision-keyed batches
  that rebase rather than clobber), a capability-URL asset face served through a
  shared byte-range parser, and an iframe sandbox boundary. The gallery lives in
  the Panel's left column as a titled list, and titles are the one stored string
  a human reads — so they pass a validation gate whose refusals are a closed enum
  rather than prose, and can therefore be localized. See
  [CANVAS.md](docs/reference/CANVAS.md).
- **Per-principal spend budget.** A new `[policies.spend]` section, a durable
  per-period ledger on the security store, and a single admission predicate
  metered at the one funnel every LLM call already passes through. Period
  boundaries follow the local calendar, and the policy and ledger handles are
  installed unconditionally at boot — a process-global handle whose "never
  installed" fallback happens to be a legal configuration value (no ceiling) can
  otherwise report `configured: false` truthfully while describing a machine that
  is in fact configured.
- **A `plan` execution tier, and the plan → build handoff.** A read-only planning
  tier: mutating tools are refused, and when a human approves the plan through
  `scratchpad(request_approval)` the gate flips to the restore tier within the
  same turn — no restart, no second parse. The refusal is a floor at the bottom
  of `effective_permission`, so a months-old `"bash" = "allow"` entry cannot
  hollow it out; the other three tiers keep explicit-entry precedence byte for
  byte.
- **`/btw` side questions.** Ask something mid-run without derailing the run. The
  gateway serves the promote crossing itself instead of asking the model what the
  word meant, promoted answers travel as a classifiable carrier, and both the TUI
  overlay and the channel face read the same derivation (`BtwTurn`).
- **Marketplace browse, and Claude Code plugin compatibility.** Install-by-name no
  longer requires already knowing the name. Upstream manifests parse in every
  shape their authors actually ship — inline `mcpServers` objects and the
  six-member `source` union included — because serde does not degrade field by
  field: one field typed too narrowly makes the whole document unreadable, and a
  single `{source:"github"}` row used to make an entire marketplace invisible.
- **`workspace_manage` and a drift-proof workspace CLI.** A tool face for
  workspaces plus `workspace get|update|list --include-archived|unarchive`, with
  the request and response shapes owned by `aleph_protocol::workspace` so a
  renamed wire key is a compile error rather than a column of dashes.
- **Provider preset pickers in the Panel.** The 56 preset cards collapse into a
  searchable disclosure that opens in place; the left column keeps only what you
  have actually configured. Keyboard walk, filtering, and highlight movement are
  one shared implementation across four surfaces.
- **Browser parity round.** `browser_batch`, `TextGone` and `Time` wait
  conditions, and two drivers — a managed `playwright-cli` profile and the
  chrome-devtools MCP server — held to the same contract: each verb returns a
  value, not a transcript of the call.
- **Per-origin extension usage tracking.** Usage badges in the Panel, an
  `idle-extensions` doctor check, and a cleanup report that distinguishes
  never-used from idle and marks the rows it cannot actually remove — a cleanup
  list that invites an action must only list rows that action can succeed on.
- **Persistent approval grants with a list-and-revoke surface.** The negative half
  of the approval system had a circuit breaker; the positive half had no list at
  all, so "allow for this session" could neither be seen nor taken back. Grants
  now record the sentence the human was shown, because a list of fingerprints is
  not a list.
- **An `<environment_context>` XML envelope.** The runtime facts the model always
  saw (cwd, repo, branch, model, time) now sit in a tag-delimited region that
  downstream prompt splitters can match on; `TurnEnvelope` carries a sub-agent
  binding, and the operating envelope echoes the sandbox network line and
  permission profile. Primary dispatch stays byte-identical and cache-keyed the
  same.
- **A retrieval x-ray for memory, and fourteen real-machine QA fixtures.**
  `memory.retrieve_with_trace` explains why a recall returned what it did;
  `qa/{canvas,browser_managed,channels,spend_budget,btw_tui,busy_input,
  plan_handoff,picker_nav,plugins,multiuser_audit,memory_curated,announce,
  webview_compat,leftovers}/run.sh` each stand up a live server or a real browser
  and assert effects.

### Fixed

- **Every `bash` call that omitted `working_dir` was denied on a factory
  install.** The run loop injected the run's effective workspace through the
  model-writable `working_dir` field, and the sandbox — which pins cwd to
  `workspaces/<sha256(session)[..16]>` — could not tell that authorized value
  from one the model invented, so it refused the authorized one. The value now
  travels on a channel the gateway owns and the model cannot write, which also
  stops relative paths from being replaced instead of resolved.
- **The managed browser driver never launched a browser.** It issued 28
  subcommands and never `open`, while `playwright-cli` requires it first; the
  matching `NoSession` error was constructed and had zero consumers. Diagnostics
  the CLI writes to stdout with exit 0 are now classified alongside stderr, so a
  PDF write refused by a path gate stops reporting "Saved PDF to <path>".
- **Slash commands never carried their arguments.** The TUI palette executed each
  entry's bare `full_command`, so four session knobs, `/tools` and
  `/compress` were only ever invoked with no argument — and their no-argument
  behaviour (print the current value) reads like a feature. `/model <id>` was
  worse: it had no arm in the payload builder at all and failed validation on
  every slash surface, while the guard that checked shorthand targets kept
  passing because the target had always existed.
- **Ten tools were advertised but not dispatchable.** `plugin_manage` was
  registered on three faces — each enough to put it on the tool table and bill
  for its description on every request — and the dispatcher's hand-written match
  had no arm for it, so every call answered `Unknown tool`.
- **Telegram and Feishu policy configuration was dead on any install that had
  ever saved a channel.** Secret migration moves required fields into the vault
  and out of `config.toml`; the gating path parsed the original block, failed,
  warned, and silently fell back to defaults — which was the entire reason the
  bridge existed. Feishu's streaming emitter was unreachable by the same route
  and said nothing at all.
- **Feishu rate limiting was classified as permanent failure.** Lark reports it on
  two channels — a modern `429` and a documented legacy `400`, both carrying
  `code: 99991400` — and only the status code was read, so replies were dropped
  silently. The backoff now reads the header the server actually sends.
- **Memory reads answered from the wrong partition on eight gateway faces.**
  Writers compose a partition id; readers used the bare persona, so "list my
  notes" answered "none" about a note written a minute earlier. This is not
  multi-user-only — a loopback Panel session is already a personal partition.
- **A frame that carried its own attribution was rendered into whatever
  conversation you happened to be looking at.** A second tab, a room teammate, a
  CLI turn or any cron tick could paint a full turn into the active thread and
  overwrite its session key. Frames now carry attribution end to end, and only
  provably-elsewhere frames are dropped.
- **`metadata.json` corruption from non-atomic read-modify-write.** `fs::write` is
  create + truncate + write, and sixteen unlocked call sites could interleave, so
  a torn file made a session simultaneously absent from the list, not-found in
  history, and unpatchable — with the transcript intact beside it, and a restart
  no help. Writes are atomic and the read-modify-write is a critical section by
  construction, not by discipline.
- **The dream daemon's activity sensor had no producer.** `record_activity()` was
  cut as unused on the same day another merge restored every consumer, so
  `idle_seconds()` measured process uptime: yielding to the user was vacuously
  false after fifteen minutes and vacuously true before, spending the single
  nightly budget on a cycle that aborted in its first stage.
- **Test suites that were never compiled, and one that never ran.** A 325-line RPC
  test file no `mod` statement referenced; a `--lib` suite that had not compiled
  since a missing parenthesis, during which a ratchet number was raised by
  arithmetic rather than measurement; and a `-c` short flag claimed twice, which
  clap validates with a debug assertion that killed the TUI binary before `main`.
- **A temp-directory leak of 7,623 stray trees and 4.0 GB.** Guards dropped before
  the tree was used, `Drop` impls hung on `static`s that never drop, and stray
  `aleph-server` processes still holding ports. Cleanup is an `atexit` hook now —
  and the first measurement was wrong because `ls -1` does not list dotfiles,
  which is what `tempfile` creates.
- **macOS packaging broke on the install floor.** `minimumSystemVersion` doubles
  as `MACOSX_DEPLOYMENT_TARGET`; at ≥ 12.0 chained fixups left proc-macro dylib
  symbol tables misaligned and dyld refused to load them, surfacing as
  `E0463 can't find crate`. It looked intermittent only because cargo does not
  fingerprint that variable, so a warm cache hid it.
- **Panel refusals were rendered as absence.** An admin `Err` read as "nothing
  configured" — confidently telling a user with a working provider to go
  configure one — and the write path still answered with a raw protocol string
  after the read path had been fixed. Classification is one crate-wide chokepoint
  with no per-page allowlist. Phone screens also rendered Chinese copy through
  shared modules a directory-scoped guard could not see, and every iOS-style
  switch displayed as off because `attr:aria-pressed` on a native element sets
  nothing — so the first tap on a healthy provider turned it off.
## [26.7.31] 

Bigger again — 400 commits over ten days, 1,506 files, +115k/−56k. Three
threads. First, **Linux becomes a first-class desktop target**: a full AT-SPI2
accessibility layer, native EWMH window management, and a single source for
session type, clipboard, and app launching — while Windows coordinates were made
to mean exactly one thing and macOS stopped reporting success for things it
cannot do. Second, **the model gets a way to hand over finished work**: a durable
artifact store, an `artifact_publish` tool, a session HTML exporter, and a Panel
right rail rebuilt around deliverables and attachments instead of a redundant
tool inspector — alongside a per-agent Ed25519 identity with a signed operation
ledger, a dual-era MCP client for the 2026-07-28 stateless protocol, and an
end-to-end Aleph Hub install chain. Third, a **sustained correctness sweep**: a
five-batch adversarial static review, a new severed-wire audit skill and its
first pass over the tree, and the discovery that several finished features were
never actually reachable in production.

### Added

- **Linux AT-SPI2 accessibility layer.** The four AX tools, set-of-marks, and the
  password gate now work on Linux. The connection is shared (a fresh one costs a
  measured 424 ms) but liveness-probed before reuse, since a zbus connection is
  driven by the runtime that built it and silently hangs forever across runtimes.
  Node reads are bounded by both a node cap and a wall-clock budget, because each
  read is a D-Bus round trip into another process — and a hung application is the
  headline reason a user reaches for the agent at all.
- **Native Linux window management and desktop plumbing.** EWMH via `x11rb` plus
  sway/Hyprland IPC replace the shell-out window layer; session type, clipboard,
  and app launch each collapse to one source in `desktop/shared`. Every shell-out
  that waits on a desktop service (`xclip`, `notify-send`, `swaymsg`, `hyprctl`,
  `pactl`, `ffmpeg`) now carries a deadline.
- **`artifact_publish` and the deliverables surface.** The model can publish a
  finished work product as its own rendered document; deliverables pin to the top
  of the Panel right rail and open in the system browser rather than downloading.
  Inbound and outbound media are harvested into a durable artifact store, and the
  whole session can be exported as a self-contained zero-`<script>` HTML page.
- **Per-agent identity and a signed operation ledger.** Each agent gets an Ed25519
  keypair; tool executions are recorded on a signed hash chain with delegated-role
  chains, in-chain key lifecycle, and an offline `aleph-server identity` verifier.
  Records that were never written are reported as `lost` rather than silently
  passed.
- **Dual-era MCP client.** The 2026-07-28 spec removes the `initialize` handshake,
  protocol sessions, `ping`, and server-initiated requests. Era is probed once per
  server and latched; sampling/elicitation/roots move to the MRTR retry flow, which
  incidentally makes sampling work over HTTP for the first time.
- **Aleph Hub install chain, wired end to end.** Catalog ingest validates entry
  counts, duplicate ids, and the reserved `local:` namespace before anything
  enters the cache; git checkouts are pinned and digest-verified before the first
  write; `update_available` is backed by an install-provenance ledger; remote MCP
  secrets travel as `headers` resolved at dial time.
- **Dynamic webhook mount table.** Channel webhooks are admitted deterministically
  at boot from a shared table owned solely by the channel registry, so
  `channel.create` / `stop` / `delete` take effect without a restart.
- **Panel memory vault, rebuilt.** Dual-track search, a card list with a
  three-state shell, deep links, a pager with a page-size selector, a bulk-action
  bar, markdown export, and an evidence chain that renders `notes_citing`.
- **Loop and goal pause/resume.** Both autonomous continuation chains gained a
  `Paused` state, an atomic transition primitive, and cross-session lifecycle
  control — with the rule that cross-session operations may only *lower*
  activity (stop/pause/clear), never arm.
- **Voice as context.** The `[voice]` vocabulary list is dual-consumed — as ASR
  bias and as a prompt-side misrecognition hint — and TTS gained provider
  fallback plus a voice-catalog join point.
- **`severed-wire-audit` skill.** Finds features where producer and consumer are
  both complete but the registration, dispatch arm, or subscription between them
  is missing — the class of defect dead-code lints structurally cannot see.

### Fixed

- **Windows desktop coordinates meant two different things.** A DPI-unaware
  process reads virtualized `GetWindowRect` / `GetCursorPos` / UIA rectangles
  while screenshots come from the display driver unscaled — a 1.5× mismatch
  between where the model sees a button and where the click lands, on a default
  150%-scaled laptop. Process DPI awareness is now latched at both entry points.
  Absolute pointer moves no longer go through `enigo`, which normalizes against
  the *primary* monitor and omits `MOUSEEVENTF_VIRTUALDESK` — every click aimed
  at a secondary display was landing on the primary one. Window geometry now
  compensates for the DWM extended frame, so `move` no longer drifts and `resize`
  no longer shrinks by two borders per call.
- **macOS reported success for three things it cannot do.** `NSEvent` global
  monitors never fire in a daemon with no `NSApplication` — the Escape emergency
  stop had been dead while every layer above it reported it armed; it now runs a
  listen-only `CGEventTap` on its own `CFRunLoop`. `focus_window` polls and
  reports honest failure instead of trusting `activate`'s `true`. Ten
  `SUGGESTED_TIMEOUT_MS` constants had zero consumers, so every RPC used the 60 s
  fallback — including the 3 s focus check on every batch of keystrokes.
- **macOS bridge could be killed by a 2-pixel image.** Vision invoked the
  completion handler and then threw, resuming a Swift continuation twice —
  process suicide, which surfaced in tests as "helper stdout closed."
- **Three private AX node budgets silently pruned the tree** at 10,000 / 4,000 /
  1,500 nodes per platform; the model saw a truncated tree and concluded the
  control did not exist. The budget is now protocol-level, and results
  distinguish a truncated *list* from an unfinished *walk*.
- **`key_button` bypassed the desktop input gate.** With `allow_global_pointer`
  off, `key_combo` was refused while `key_button {press_action:"click"}` delivered
  the same keystroke into the user's foreground window, and reported no
  `delivery` field. Held inputs are now released on the rail they were pressed on.
- **Manual `/compact` did nothing to the model's context.** It deleted rows from
  the `messages` projection — which the prompt is not built from — and reported an
  invented token saving while truly deleting the user's scroll history. It now
  summarizes, checkpoints, and soft-retires a prefix of `session_events`, deleting
  nothing and leaving FTS intact so compacted detail stays recallable.
- **The busy-queue ticket was held for the entire agent run**, so a waiting
  message never reached the front of the lane — which made `Steer` and `Interrupt`
  structurally unreachable and silently degraded both to `Queue`. One root cause
  with three faces: the `/stop` receipt also over-counted by one and the backlog
  gauge counted an already-running message.
- **Content-aware tool-output cleaning ran after flattening.** `Value::to_string()`
  compacts a result to one line, so the log/search/diff/json reducers never fired
  for any builtin tool, and the "inline the key error" digest displayed the JSON
  envelope header instead of the compiler error. Cleaning is now field-level, at
  ingress, before flattening.
- **The environment envelope lied about the working directory.** The prompt layer
  read `std::env::current_dir()` — the daemon's directory — so a single request
  carried three contradictory paths and the model would issue `bash(working_dir=…)`
  against a directory the sandbox then refused. All of `cwd`/`os`/`arch`/`shell`/
  `git` now come only from `RuntimeContext`, partitioned by volatility so per-run
  bytes stay out of the cacheable prefix.
- **The model pick never reached the wire.** Judging "is this the primary slot" by
  `tier == Unknown` discarded a pinned model on chained providers and made every
  fallback dial with the primary provider's model id; the composer's model pill
  reached the "switched to X" banner and nothing else. Route status now reports
  the actual next dial order, and `Retry-After` is parsed in one place across both
  formats.
- **Provider health had a table nothing dialed from**, and the failover chain
  described a topology it did not use.
- **Command-policy bypasses on Windows.** The unconditional backslash fold turned
  `\\?\C:\` into `\?C:`; encoded PowerShell payloads were a blind spot; and a
  full-line rule gap stitched two unrelated statements into one unclosable false
  positive. Matching now runs over both a POSIX-folded and a path-preserving view,
  decodes `-EncodedCommand` before layering, and uses segment-scoped gaps.
- **`--config` was honoured by one consumer out of nine.** Settings were written to
  a file nothing read; the path is now pinned once in `main()`.
- **Panel remote auth: `devices` is one table shared by Panel and cluster nodes**,
  both self-reporting their `device_id` — claiming a row by id could mint an
  operator credential invisible to the roster and unreachable by revoke or token
  rotation. Namespace guards are now symmetric, per-device revoke is immediate,
  pairing URLs are built server-side, and the login wall accepts pairing codes.
- **A published deliverable was served as a download** — precisely defeating the
  purpose of the tool — and the "new items" badge counted tool calls, a leftover
  from the inspector era, so it lit for things the panel does not contain and
  stayed dark for things it does.
- **Standing goals could be resurrected by a stale tool snapshot.** Field commits
  now carry a status CAS; crash recovery exempts every wait barrier, since a
  timer-parked goal woken by the service would otherwise block forever; and
  workspace is inherited across the three hook-less wake paths.
- **Hook defects.** Injected context is bounded, a timeout leak is closed, consent
  binds to script content, and `hooks_manage(only_unreachable=true)` reports all
  three silent non-firing causes from the runtime inventory.
- **CI was not running every integration test**, which had been masking real
  defects; `-D warnings` propagation, a red fmt gate that prevented clippy from
  ever running, and 19 tests that were never isolated from the developer's real
  `~/.aleph` were all fixed. Chinese source comments were translated to English
  across every module.
- **Five batches of adversarial static review** across ~40 modules closed several
  hundred findings — auth and lock defects across channels, WASM timeouts and SSRF
  redirect handling in extensions, TOCTOU in the tool handler registry, blocking
  I/O inside async, unbounded channels, silently discarded errors, and a
  `set_config_patcher` no-op that shipped `self_config` and `moa` without a
  patcher. The severed-wire pass cut dead abstractions across `a2a`, `acp`,
  `agents`, `approval`, `arena`, `browser`, and `clawhub` rather than reconnecting
  them.
- **Tool argument names unified on `timeout_seconds`** (`bash_exec`, `code_exec`,
  `code_check`, `task_create`, `team_delegate`, workflow steps, MoA advisors), with
  serde aliases preserving the old spellings.

## [26.7.21]

The largest release since the harness dissolution — 282 commits across three
days. Three threads run through it. First, a new user-facing dial: the
**chat / work / code session mode**, the third twin of exec tier and think
level, which statically partitions the tool *presentation* surface (never
permissions) and cuts per-turn token cost sharply in chat. Second, a
**governance topology** — the loop-graph layer gains a persistent store, a
nine-action tool, victory-claim watchers, an independent-evidence auditor
agent, and a team node kind that fuses it with the multi-agent system. Third,
a broad **entropy-reduction and hardening sweep**: the Telegram and Discord
channels were gap-analyzed against reference implementations and shed several
thousand lines of dead parallel layers while their access control was unified
onto the real router, the macOS desktop rails were corrected against real
hardware, and a multi-module adversarial review pass fixed dozens of
correctness, concurrency, and security defects across the gateway, providers,
extensions, tasks, teams, sessions, and skills.

### Added

- **`chat` / `work` / `code` session mode.** A user-selected static partition of
  the tool presentation surface, orthogonal to permissions (approvals stay with
  the exec tier). Single source in `config/types/policies/session_mode.rs`;
  request > session > global resolution with stamp-on-carry; the partition lands
  on the two existing presentation mechanisms (progressive-disclosure core set +
  deferred-tool tier) so `src/harness/` gains zero lines. Picked from a Panel
  composer pill, from Settings → Policies as a global default, or
  conversationally via the new `session_set_mode` tool. Each mode also drives a
  distinct right-panel behavior (chat = badge, work = plan surface, code = tool
  detail) and adds one cache-stable prompt line.
- **Mode v2 semantics.** Family tables match on `_` word boundaries;
  MCP-qualified `{server}__{tool}` names are exempt from the builtin tables;
  `media_understand` stays listed in chat and code; chat's core subtraction can
  no longer drain to empty; subagents inherit the parent mode with a short prompt
  line.
- **Loop-graph governance layer.** A governance topology store, a nine-action
  tool, an audit ring, objective ACLs, a prompt layer, and a root gate — the
  closed-set governance edges that give the four single-loop failure modes
  (Goodhart, reference blindness, ring conflict, measurement decay) a
  topological answer. Documented in `GRAPH_LAYER.md`.
- **Graph × multi-agent fusion.** A `team` node kind, live-joined teams in
  graph status, team pair targets, victory-claim triggers that poke watchers on
  team disband, and a built-in **`loop-auditor` agent** so audit and watch
  templates default to independent-context evidence.
- **`governance_metrics` in-core probe.** Replaces the sandbox-walled `sqlite`
  shell probes that the audit sensor could not legally reach, plus notes-era
  activity counters on `dream_reports`.
- **Discord interactive approval buttons.** A two-way `ApprovalCallbackSink`
  flow, plus edit-based reply streaming.
- **iMessage inbound reactions as context.** Add-tapbacks arriving from the user
  are surfaced to the model as context rather than dropped.
- **macOS window targeting via Accessibility.** A public AX window resolver
  (CGWindowID → AX by geometry) so focus raises the *specific* target window and
  bounds are set on the exact window, with an osascript fallback.
- **Grounding evidence gate on team task review.** `task_review` approvals can
  now require grounding, with acceptance metadata helpers.
- **Daemon-computed session cost.** `session.usage` computes cost via core
  pricing and the duplicate price table in the shell was deleted; `plugin.install`
  gained a unified handler that classifies the source (the CLI stops guessing).

### Fixed

- **Every Panel send re-carried the composer's cached mode**, silently reverting
  a `session_set_mode` switch made by the model — all four send paths now carry
  mode on the first message only, and `session_updated` syncs mode and exec tier
  back into Panel state so the pill and right rail read store truth.
- **`tool_search` could be collapsed by progressive disclosure**, pointing the
  model at a guaranteed-miss lookup — it is now pushed into the request core set
  at registration, and a mode or tier pick riding a slash-command message is no
  longer dropped.
- **Telegram ignored the operator's configured access policy.** The inbound
  router fell back to defaults for Telegram, and the channel-local pairing store
  was never written to, so it never paired anyone; access, pairing, and
  allowlists are now single-sourced on the router. ~2.8k lines of dead parallel
  layers (an uncompiled webhook server, an unconstructed session manager, a dead
  draft API, duplicated status reactions) were removed, along with the parallel
  nested-config / account-pool / resolver layers in Discord.
- **Gateway leaks and drops.** A per-connection event-forward task leaked on
  every WebSocket disconnect; the channel forwarder died permanently on
  `broadcast::RecvError::Lagged`; webhook backpressure silently dropped inbound
  messages instead of returning 503; and the raw error chain leaked to the Panel
  instead of a single-sourced user receipt.
- **Command injection on desktop `open`/`launch`** — now dispatched through
  `ShellExecuteW` instead of a shell; symlink planting during bundled-content
  extraction is refused via `symlink_metadata`; skill download path traversal is
  closed and the scanner file size capped; cluster `file.write` no longer has a
  TOCTOU window between existence check and write.
- **Certificate approval could be authorized without a fingerprint match**, and
  the webview microphone grant was not origin-restricted on Linux or Windows.
- **Approval failed open on a missing policy file**, and `permissive_default`
  disagreed with `Default::default()` — both now fail closed; the guardian judge
  payload masks secrets; heartbeat probes refuse dangerous and
  confirmation-gated tools.
- **Prompt-boundary injection.** Transcript, focus, and prior-summary content now
  escape boundary markers before entering the prompt; `ToolAwareChunker` asserts
  a non-zero token ratio instead of computing `usize::MAX`.
- **Provider failover cloned the entire conversation on every request** and
  published route state non-atomically (torn config reads); ~2,000 lines of
  unreachable provider code were deleted.
- **Extension lifecycle races** — concurrent `reload` is now serialized under the
  load guard, malformed plugin results are no longer masked as
  `ServiceResult::ok`, and `AuthorInfo` parsing uses the leftmost separator.
- **Cluster node keys were byte-sliced**, breaking CJK node names — normalization
  is now Unicode-aware.
- **macOS capture and recording rails.** Screen recording is cropped to the
  requested region via `setSourceRect` (the region is in physical pixels, not
  points), completion is verified to have produced output before reporting
  success, typed bridge errors are preserved on the OCR, window-capture,
  media, input, and screenshot rails instead of being flattened, and
  `screen_record` serialization errors propagate instead of becoming `null`.
- **Harness ratchet lowered to 5008** by removing the `DiminishingReturnsDetector`
  hard stop — a deterministic completion judgment inside the loop, which R10
  forbids. Stuck runs are bounded by `max_iterations`, the tool-loop verifier, and
  the model's own stop instead.
- **Session and team integrity.** `retire_from` is atomic and shutdown timeouts
  surface; active team names are unique via a partial index; team protocol text
  and artifact content are size-capped; `sessions` and `tasks` gained presence
  opt-in, a hashed carryover filename, and deterministic template env ordering.
- **Wire request ids no longer use UUIDs** (replaced by an atomic counter,
  dropping the dependency from `aleph-protocol`); a custom regex that fails to
  compile no longer discards the valid ones alongside it.

## [26.7.18]

A remote-access release that closes the loop on **self-signed TLS**, building
directly on 26.7.17's in-process gateway TLS. Two halves meet in the middle.
On the server, the self-signed certificate's SAN now **auto-discovers the
machine's non-loopback interface IPs** — a `wss://` connection to a LAN or
public address no longer fails hostname verification, retiring the
localhost-only SAN gap — with a drift-tracking sidecar that reuses the existing
cert when the interface set is a subset and regenerates it (never bricks) when
it drifts or the sidecar is corrupt. On the client, the Panel gains **in-app
trust-on-first-use (TOFU) certificate trust**: a self-signed gateway cert is
reviewed and pinned from inside the app via a fingerprint + SAN approval splash
with a TOFU/change warning, backed by a shared decision core and a pinned trust
store. The macOS WKWebView cert-challenge adapter is the reference platform —
proven end-to-end against a real self-signed remote — with the iOS Keychain
trust store and decision mirror wired alongside.

### Added

- **Self-signed cert SAN auto-discovery.** A new `[gateway.tls] san` field plus
  automatic discovery of the host's non-loopback interface IPs are assembled
  into the self-signed certificate's SAN and threaded through the server mirror
  to `load_or_generate`, so remote `wss://` connects verify cleanly.
- **SAN-drift sidecar.** The cert's SAN set is tracked in a sidecar: an unchanged
  or subset SAN reuses the existing cert, a drifted one regenerates it, guarded
  by an atomic regen marker and a shrink-reuse test.
- **Panel in-app certificate trust (TOFU).** A shared decision core with a pinned
  TOFU trust store, SHA-256 fingerprint + SAN/subject parsing, an approval splash
  page (fingerprint + SAN + TOFU/change warning), and pending-cert state with
  approve/reject Tauri commands let a self-signed cert be trusted from inside the
  app.
- **macOS WKWebView cert-challenge adapter + install dispatch.** The reference
  platform for in-app trust, driving the challenge through the shared decision
  core.
- **iOS Keychain trust store + decision mirror.** Unit-tested Keychain-backed
  trust for the iOS Panel, mirroring the shared decision state.

### Fixed

- **Panel token wall buried by the boot gate** on a remote unauthorized connect —
  the token wall now surfaces instead of being hidden behind the boot gate (WASM
  dist rebuilt).
- **macOS WKWebView `respondsToSelector` cache** was stale, so the injected
  challenge handler never fired — the cache is now busted on install.
- **macOS trust approval navigated by a relative path** and could miss the page —
  it now navigates to the approval page by absolute URL.
- **`credentialForTrust` returning nil** could panic at the FFI boundary — it now
  fails closed.
- **Corrupt `sans.txt` sidecar** is treated as best-effort and regenerates the
  cert rather than bricking boot.
- **Lite supervisor relocation** is suppressed while a trust prompt is pending, so
  the approval flow isn't interrupted.
- **Malformed-DER fingerprint parsing** never panics (regression-tested).

## [26.7.17]

A security and hardening release. The headline is **native gateway TLS for
remote connections** — the gateway now terminates TLS in-process (a provided
cert or an auto-generated self-signed one), fails closed on plaintext to any
non-loopback bind, refuses insecure transport to a remote client by default,
and resolves the real client IP behind a trusted reverse proxy so caps, rate
limits, audit, and connect-auth all key on the true peer; the Panel forces
`wss://` to remote hosts. Alongside it: an **MCP discovery convergence** that
retracts the eager resource/prompt index layer in favor of on-demand discovery
tools (the model stops `cat`-ing files), **plugin-bundled skills folded into the
skill mechanism** with the agent catalog revived, deep **file-op path safety**,
and a broad hardening sweep across the harness, tools, context, cache, loop,
teams, and memory subsystems — much of it driven by adversarial review and
cross-implementation gap analysis.

### Added

- **Native in-process gateway TLS.** New off-by-default config
  (`[gateway] tls` / `trusted_proxy` / `allow_insecure_remote`) turns on
  `axum-server`-terminated TLS, using a provided certificate or an auto-generated
  self-signed one, plus reverse-proxy trust for TLS-terminating front ends.
- **Trusted-proxy real-client-IP resolver.** A spoof-safe resolver reads the
  true client address behind a trusted proxy, so capacity limits, rate limiting,
  the audit trail, and connect-auth no longer see the proxy's IP.
- **MCP resource/prompt discovery tools.** `mcp_list_*` discovery tools let the
  model find MCP resources and prompts on demand instead of blindly reading
  files, and route it to native MCP / skill / plugin reads via a `cat`-guard
  read-steer.
- **Plugin-bundled skills are visible to the model.** Installed-plugin skills are
  folded into the skill index and `skill_read`, and the agent catalog is revived.
- **`call_id`-exact approval correlation + completion-order live tool events.**
  The tool-concurrency scheduler ties each approval to an exact call and emits
  live tool events in completion order.
- **Workflow interop for `.mjs` / dynamic workflows.** Executable-step options
  and a `.mjs` export path for cross-tool workflow compatibility.
- **Gateway dead-letter forensic trail.** Undeliverable messages can be
  inspected and safely redriven.

### Changed

- **Fail-closed remote transport.** The gateway refuses plaintext on a
  non-loopback bind at boot and refuses insecure transport to a remote client
  (`allow_insecure_remote = false` by default); the Panel forces `wss://` for
  remote hosts and refuses plaintext to a remote gateway.
- **Dropped `aws-lc-rs` for a `ring`-only TLS provider** (`axum-server`
  `tls-rustls-no-provider`) to keep the Windows build clean and the core light
  (R3).
- **MCP discovery converged to one on-demand channel.** `McpResourceIndexLayer`
  is retracted — the eager index layer (single-prefix ids that didn't round-trip)
  is gone, and on-demand discovery tools replace the prompt index.
- **Harness R10 line-budget paydown.** Dead prompt-layer surfaces pruned
  (net ≈ −3.4k LOC), a proactive context reminder and a reasoning-gate fix wired,
  ceiling 5035 → 4988.
- **Deep context-layer hardening.** Sixteen items from an
  openclaw / hermes / codex / pi gap analysis, plus a context-cache hardening
  round (content-addressed packing, cross-run fingerprint carryover, watchdog
  grain, prefix-stability contract).
- **Loop layer, rounds 5–7.** Wake/resume channel deny layer, tree budget for
  teams/swarm, Guardian cache/breaker, a goal wait-barrier, and lifecycle welds
  across strategy tiers.
- **Teams coordinated-tasks, rounds 4–5.** Snapshot protocol restore, a
  pause/resume gate, live escalation leader and config, notifier re-arm, and a
  filter single-source-of-truth.
- **Memory management deepened (D1–D4).** Ack-inversion fix, session-end wiring,
  compression fixes, and a batch `remember`, with adversarial-review fixes across
  the note layer.
- **File-op path safety.** Symlink-safe delete/move, glob/deny re-checks,
  collision de-dup, per-path locks, a hardened path deny gate (`%APPDATA%`
  expansion, system/proc guards), and an `apply_patch` forward cursor that fails
  honestly on unplaceable additions.
- **Subsystem hardening rounds.** Agent switching (binding seam, three-registry
  sync, ghost self-heal), the hook system (Stop-event gate, consent/wiring,
  fail-closed output), the voice conversation loop, the desktop bridge
  (§7.1–7.6), permission-hierarchy round 4, event-driven waits for background
  subagents, and wedged reverse-RPC teardown (slow-consumer eviction).
- **Provider model presets refreshed** to the latest per vendor, with the catalog
  realigned.

### Fixed

- **`skill_read` refused symlinked/duplicate skills.** It now resolves them by
  precedence instead of reporting an ambiguity error.
- **Gateway remote-auth rotation kick + device resurrection**, with the audit
  trail wired and dead auth code pruned.
- **`serde_json` map-insert key type** in `message_history`.
- **Two pre-existing `alephcore` lib-suite test failures** unblocked the suite.
- **Workspace-wide clippy / rust-doctor cleanups** to keep `-D warnings` green.

## [26.7.15]

A hardening and safety release. The headline is a **three-tier execution
permission model** (Ask / Auto / Full) with an action-aware approval gate — the
human sees the exact command and the grant keys on it — surfaced as inline
approval and `ask_user` clarification cards in Panel and a polled HITL overlay
in the TUI. Alongside it: a matured **knowledge-memory note layer** (graph
path-finding, `[[wikilink]]` supersession, CJK full-text search,
contradiction→supersession closure), **truthful cost & token accounting** (both
were partly fabricated), a repaired **cluster enrollment / reverse-RPC** path,
and a large **harness R10 paydown** that deletes zero-consumer abstractions and
an unreachable ~3,800-line tool-hydration stack.

### Added

- **Three-tier execution permissions (Ask / Auto / Full).** A single
  operator-facing knob picks how much tool execution is auto-approved. It reads
  each tool's declared metadata (idempotent / destructive), not its name;
  unknown tools fail closed in `Ask`; the `[sandbox.command_policy]` hard-floor
  can't be lowered by any tier. The gate is action-aware — the human sees the
  actual command and the grant fingerprint keys on it — enforced at the single
  choke point `src/tools/scoped/`, with the slash fast-path, `tools.invoke` and
  background-continuation bypasses all closed. Panel gains a composer tier pill
  plus inline approval / `ask_user` clarification cards; the TUI gains a polled
  Ask-tier approval overlay.
- **`note_graph_query` read-only tool + note-graph path-finding.** The
  knowledge-memory note layer gains a graph query surface with bidirectional-BFS
  path finding between notes, typed relation edges, and `[[wikilink]]`
  supersession that force-surfaces a correcting note when an outdated one is
  recalled — backed by a crash-safe index and CJK trigram full-text search.
- **In-window restart-to-update banner.** The desktop shell injects an update
  banner on stage with its own sentinel control channel, intercepts the banner's
  control links, and re-injects it across Panel reloads, so an available update
  is one click to restart-and-apply.
- **TUI §5.12 maturity.** A `/sessions` picker, a live context-window gauge in
  the status bar, a polled Ask-tier tool-approval overlay (HITL), and a
  connection status dot; `mod.rs` / `app` split by responsibility.
- **Live cluster fleet feed.** The Panel cluster view is wired to a live feed
  with a corrected contract, and node identity now persists on the connect
  verdict.

### Changed

- **Truthful cost & token accounting.** The gateway session now carries a real
  spend ledger (both cost *and* token counts were previously fabricated),
  prompt-cache discipline is restored, thinking depth is wired end-to-end, and
  every assistant message carries its real per-message token cost without
  double-counting.
- **Non-intrusive computer-use.** Desktop apps are driven through a pid-targeted
  input rail with window-scoped capture, so automation no longer moves the
  user's cursor. Six P0 defects were fixed (escape trap, key-combo block,
  press-hold, scroll units, paste, degenerate frames), macOS PIM and Chromium AX
  were wired, and background mouse acts macOS can't actually deliver are refused
  up front.
- **Knowledge-memory note layer, rounds 2–4.** Category names canonicalized
  (singular/plural split-brain fixed), the relation vocabulary cleaned of
  entity-name pollution, connected communities via Leiden refinement, the
  contradiction→supersession loop closed, stale notes archived, and a
  default-off governance gate wired.
- **Memory Tier-3 hardening.** Agent-scoped recall signals, severity-gated
  archival, a maturity cohort, and several retrieval bug fixes; dead knobs
  pruned.
- **Cluster enrollment repaired.** Dead cold-start enrollment fixed,
  deregistration made to stick, reverse RPC hardened, and the `connect`
  handshake now precedes the method in `gateway call`.
- **Deferred tools are callable.** Progressive tool disclosure no longer strands
  deferred tools; CJK tool search is unbroken, and an `off` reasoning setting no
  longer silently buys reasoning.
- **Sandbox hardening.** Invisible-character SSOT, symmetric breaker purge, and a
  dead-code sweep.
- **Goal subsystem.** A fail-closed continuation gate and an atomic continuation
  claim.

### Removed

- **Four zero-consumer harness abstractions (R10 line-budget paydown).**
  - `TraceSink::on_init_seam` + `harness_bridge::emit_init_seams` — a trait
    method with an empty default body. Seven events fired per run, five
    production sinks forwarded them, and **both leaf sinks fell through to the
    empty default**: the channel terminated in `{}`. The same facts were already
    on the live `tracing` channel five lines later, which is what operators
    actually read. (The 26.5-era entry below advertises "9 events" and the
    deleted `src/orchestrator/tests/init_audit.rs` said "eight" — three numbers
    for one dead channel. All three are gone with the code.)
  - `HarnessCallback::on_tool_call` — the name-only tool hook. The one real impl
    (`BroadcastCallback`) deliberately never implemented it, because the
    synthetic `ToolCallStart { id: "legacy" }` it used to emit could never be
    paired by a `ToolCallDone` (those only ever carry the real call id).
    `on_tool_call_start` is now the single tool-start signal.
  - `HarnessCallback::on_complete` — fired from eight places in the loop and
    overridden to an explicitly empty body by the only production impl. The real
    terminal event has come from `on_complete_with_outcome` since P4.
  - The `Harness` trait itself. Its only production impl (`AgentHarness`)
    overrode `run()`, so the trait's default loop body executed nowhere — not in
    production and not in tests — and `dyn Harness` appeared only in a doctest.
    `AgentHarness::run` / `run_turn` are now inherent methods; the real
    polymorphic seams remain `SessionDriver` and `Arc<dyn HarnessRunner>`.
    `TurnState` / `TurnStep` / `HarnessError` / `TurnPhase` are unaffected.
  - `ChainContext::with_max_depth` and its `Display` impl — the former had only
    `#[cfg(test)]` callers, the latter only its own formatting test.
- **Unreachable tool-hydration stack (~3,800 lines)**, plus a cache framework
  for caching that was never performed and a configuration knob that reported a
  value it did not honor — deleted in the cost-efficiency sweep.
- **The phantom 4,900-line harness target** — retired in favor of the
  `budget.rs` ratchet as the actual R10 redline (now measured, not hand-counted;
  currently 5,043 lines).

### Fixed

- **Dreaming daemon burned provider quota nightly.** A retry storm in the
  nightly dream run is now bounded, so it no longer exhausts the provider quota.
- **Panel auto-update 404.** Manifest URLs normalize the space in the version to
  a dot, so the Panel updater resolves the release asset.
- **`channels.set_agent` accepted non-existent agents.** The gateway now
  validates agent existence before binding a channel (AS-1).
- **Provider error reads could hang on a stalled proxy.** Error-response body
  reads are now bounded.
- **Instance-lock holder was unreadable cross-platform.** The holder PID is now
  recorded in an unlocked sidecar for readback.
- **Spawned subagents inherit the operator's `[execution] max_iterations`.** The
  `HarnessRunner::default_max_iterations` hook existed and the spawner consumed
  it, but `AgentHarnessRunner` never overrode it, so every child fell back to
  `FALLBACK_MAX_ITERATIONS` (200). The loop was never uncapped; the number was
  just not the configured one.
- **A subagent can find its own offloaded tool output again.** The child harness
  was handed the process-wide, *unscoped* `ToolResultStore`, so its Layer-3
  spills landed outside any session directory while its `ctx_search` looked in
  the (parent-scoped) session index — a silent zero-hit recall. The child now
  gets a handle scoped to the parent session, which is the scope its tools
  already run under. Failed safe before (a child saw nothing, never another
  session's data), so this is a recall fix, not an isolation fix.

## [26.7.7]

A large capability release. The headline is **Mixture-of-Agents (MoA)
continuous advisory** — any agent turn can now be shadowed by a panel of
advisor models whose views are aggregated into the reply — landing alongside
**progressive tool disclosure** (per-turn token savings from collapsing
non-core tool schemas behind an on-demand `tool_search`), **true multi-session
parallelism** (the run gate moved from per-agent to per-session), a
**`[[wikilink]]` note lifecycle** with rename cascades and provenance, a
deepened **memory-galaxy canvas**, a **narration-led chat stream** with a
live-follow workspace, and a rebuilt **session single-source-of-truth**
event log under the hood.

### Added

- **Mixture-of-Agents (MoA) continuous advisory** — a new virtual
  `MoaProvider` fans a turn out to a configurable panel of advisor models in
  parallel (with per-advisor timeout, prompt-cache breakpoints and a signature
  cache), then an aggregator model synthesizes the reply. Presets are authored
  and switched entirely by conversation through the new `moa` tool (session
  activation + preset CRUD), a `/moa` one-shot command (operator-gated), and a
  Panel visual-config page. Advisor token spend is tracked in its own usage
  bucket and VESR records the aggregator as the acting model, so MoA never
  pollutes the primary model's accounting.
- **Progressive tool disclosure + `tool_search`** — non-core tool schemas are
  now collapsed to just their name + description (a lightweight catalog),
  cutting a large share of per-turn tool-schema tokens, and the model pulls a
  full schema back on demand via `get_tool_schema`. A new self-contained
  `tool_search` meta-tool (BM25 ranker, no dependencies) lets the model
  discover deferred tools, and MCP tools can be pushed into that deferred tier
  via `[tools] defer_mcp_tools`. A byte-identical escape hatch keeps the old
  behavior when disclosure is off.
- **True multi-session parallelism** — the run gate moved from a per-agent
  lock to a per-session `SessionRunRegistry`, so one agent can run in several
  sessions at once while any single session stays mutually exclusive. A
  run-lifetime `ConcurrencyLimiter` enforces global + per-agent caps that
  hot-reload from `[execution]` config, and a server-authoritative running-set
  broadcast drives unified "running" dots and a run-slots gauge across CLI,
  TUI and Panel.
- **`[[wikilink]]` note lifecycle** — notes now parse `[[wikilinks]]` (with
  aliases), resolve them through a strategy chain that records provenance
  (`resolved_by` / `status` / `label`), cascade on rename (including typed
  relations), and use tombstone delete semantics with targeted inbound
  back-fill. The dreaming daemon materializes unlinked-mention soft edges, and
  wikilinks render as clickable anchors across the galaxy, drawer and phone
  note views.
- **Deepened memory-galaxy canvas** — the graph view now clusters by
  community-centroid gravity, brightens nodes by recency and confidence, tints
  edges by relation kind (semantic / related / co-recalled / keyword vs. the
  wikilink backbone), and adds pan (shift / middle-drag) to the orbit camera.
  Node detail shows backlinks and a path breadcrumb, with a truncation badge
  when the node cap is hit; the dead 2D canvas engine was removed and pure
  graph→galaxy transforms extracted.
- **Narration-led chat stream + live-follow workspace** — the transcript now
  reads as a narration with compact single-line tool rows (status glyphs, live
  elapsed) and merged "explore" groups for consecutive reads. The workspace
  pane becomes a tool-detail viewer that live-follows the foreground
  conversation (with pin), and the typewriter reveal and markdown rendering
  were improved.
- **Session single-source-of-truth event log** — `session_events` is now the
  canonical log; a `MessageProjector` materializes events into the `messages`
  table, a boot-time `ProjectionReconciler` back-fills transcripts, and
  messages carry `tool_call_id` / `tool_name` plus an `AssistantRunMeta`
  stamp. Legacy sessions are back-filled and read behind a locked invariant.
- **Computer-use semantic interaction** — new `set_value` / `ax_action`
  accessibility actions with write-verification via stateless locator scoring,
  an `observe` parameter that fuses act→observe (returning post-state /
  screenshot), and recovery hints appended to tool failures so the model can
  self-correct. Computer use is now hard-blocked inside password managers, and
  the Windows UIA backend implements value read/write and action dispatch to
  reach macOS parity.
- **Message-stream assembler** — a single `MessageAssembler` reducer now owns
  the assembled message, stripping inline `<think>` blocks live as the stream
  drains and routing both streaming and final text through one shared
  sanitize atom.
- **Live-hot-reloadable execution caps** — the `[execution]` run-cap section
  is now Live: changing `max_runs_global` / `max_runs_per_agent` takes effect
  without a restart via an arc-swap global semaphore.

### Fixed

- **Session sidebar showed a raw `<system-reminder>`** — per-turn reminders
  (working directory, etc.) are now delivered as transient recall context each
  turn instead of being prepended into the persisted user message, so derived
  session titles no longer leak reminder text.
- **MoA presets on the default provider never activated** — the primary
  provider key is now mapped into `named_providers`, and the `moa` tool's args
  schema is hand-written flat (the Anthropic adapter was stripping a root
  `oneOf` union, leaving an arg-less tool) — both found in runtime QA.
- **`file_ops` glob path-escape** — search / batch-move now reject absolute and
  `..` glob patterns, re-check every match against the deny list, and reject
  Windows drive-relative / UNC-prefixed globs (`C:foo`).
- **Session epoch parsing on Windows** — fixed a Windows-only failure parsing
  the session epoch.
- **Sandbox stdin / timeout hardening** — stdin EOF handling and foreground
  timeout clamping in the command sandbox were hardened.
- **Windows standalone-server self-update** — the desktop shell now stops the
  running daemon before replacing its own binary on Windows.
- **Panel "running" dot correctness** — the red running dot is now purely
  server-authoritative with a sequence guard and cold-load seed guard, and the
  run-slots gauge refreshes on every running-set change (was connect-time
  only).
- **Reconciler token undercount** — the projection reconciler no longer
  undercounts tokens on turns that straddle the compaction watermark.
- **Structural entropy reduction** — several oversized modules (coordinated-task
  store, dispatcher schedule, and others) were split into focused directory
  submodules with no behavior change, and a swath of zero-consumer dead code
  was removed per the thin-harness R10 diet.

## [26.7.1]

A harness-reliability and polish release. The headline is a **context
"never-break" guarantee** — the agent loop now always compacts and continues
instead of ever cutting a turn short — alongside a **real, reload-proof
context-usage gauge**, **launch-at-login landing on all three product forms**,
and a flattened, collapsible redesign of tool calls in the chat stream.

### Added

- **Context "never-break" guarantee** — pressure situations that previously
  could end a turn early via `FinalReply` (critical/strict budget breaches,
  exhausted reactive-compaction retries, split-session fail-soft) now always
  route through `compact_to_fit` and continue instead. Adds a deterministic
  `truncate_to_fit` floor as a last resort — hardened against orphaned
  `tool_result` messages — so a turn can never silently stop mid-session.
- **Real per-model context-usage gauge that survives reloads** — the chat
  context-usage percentage is computed core-side (honoring any
  `context_window` config override) and persisted on the assistant message
  itself, so it survives tab switches and history reloads instead of
  resetting. Brand-new sessions show an `≈N%` estimate from a local budget
  dry-run (zero LLM calls) that self-corrects to the real figure after the
  first turn.
- **Launch-at-login lands on all three product forms** — the full desktop App
  and the lite Panel shell both get a working "Launch at Login" toggle
  (Settings → General / tray menu), and the standalone `aleph-server` gets a
  new `service` subcommand plus an installer default so it starts as a system
  service (launchd / systemd-user / Task Scheduler) out of the box.
- **Flattened, collapsible chat tool-call display** — tool calls and results
  in the chat stream now render as a compact, single-changing-line step strip
  while running (`✓N steps` once done), with a full-detail side panel for
  anything that overflows instead of nested `<details>`/JSON-tree widgets.
- **`system.open_path` desktop capability** — a new cross-platform primitive
  to open a file or URL with the OS's default handler (`open` / `xdg-open` /
  `start`), so the agent can hand the user a document it just wrote instead of
  failing to find a way to open it.
- **Broader runtime-detection coverage** — install-dir probing now also
  checks Homebrew's `rustup` keg, MacPorts, asdf shims, and Nix profile paths
  (macOS/Linux), plus extended Windows search paths for `node` /
  `playwright-cli`; the agent is now told not to re-verify paths the probe
  already confirmed.

### Fixed

- **Standalone server self-update on Linux** — the installer now downloads the
  new `aleph-server` binary to a temp file and atomically renames it into place
  instead of writing straight over the destination. Overwriting the
  currently-running binary in place failed with `ETXTBSY` ("Text file busy") on
  Linux — which is exactly what `aleph-server update` does when it re-runs the
  installer to replace itself — so in-place updates of a running server now
  succeed. (Also back-ported to the `install.sh` asset on the v26.6.29 release.)
- **Launch-at-login toggle never appeared in the full desktop App** — the
  panel was calling app-level Tauri commands that can't be authorized from a
  detached/remote origin; switched to the `tauri-plugin-autostart` plugin
  commands (with the matching capability grant), which work from any origin.
- **Updater signing key rotated** — the previous Tauri update-signing keypair
  was unusable, breaking in-app auto-updates on both product forms; a new
  keypair is now wired through CI and both product forms.
- Cross-platform session-directory naming is healed on read in the file
  backend, and chat-sidebar async reads now guard against a disposed signal,
  fixing intermittent panics/no-ops when switching sessions quickly.

## [26.6.29]

A big multi-day release. The headlines are a brand-new **iMessage channel via
BlueBubbles** (so Aleph can reach you over iMessage on any OS), the **iOS Panel
growing into a real iPhone + iPad app** (native pairing, iPad multitasking,
TestFlight), and **VESR dynamic model routing** — Aleph now learns which model
actually performs from its own verified track record instead of a hard-coded
router. Plus first-class **web-fetch providers**, a much richer **multi-agent
teams / kanban** surface, and the ability to **type into a chat while it's still
running**.

### Added

- **iMessage channel via BlueBubbles** — a new messaging channel that reaches
  you over iMessage on any OS (BlueBubbles transport, with local macOS send
  gated separately). Outbound text and attachments, inbound webhook + catch-up
  polling with offset reconciliation and GUID dedup, group/chat sends, and
  reactions / typing / read-receipts (gated on the BlueBubbles private API).
- **iOS Panel is now a real iPhone + iPad app** — a native pairing screen
  (shake-to-reconfigure, Keychain-backed connection store, load-failure
  fallback), full iPad support (device family, full-screen, orientations,
  Split View / Stage Manager multitasking, tablet-specific layout, touch
  ergonomics), and internal **TestFlight** distribution. The phone UI is split
  into tabs — Chat lands directly on the conversation (history behind a button),
  with Dashboard / Teams / Extensions drill-down menus under "More".
- **VESR dynamic model routing** — instead of a deterministic router, Aleph
  records every completed run's outcome and recalls a per-model track record at
  run start (k-NN over sqlite-vec), so the model sees which models have actually
  performed for this kind of work. Includes per-agent / per-model lifetime
  aggregates, USD-cost enrichment, and subagent routing capture.
- **Web-fetch provider category** — `crawl4ai` and `firecrawl` are now
  first-class, UI-configurable fetch providers (URL → markdown) with
  vault-stored keys, automatically falling back to the built-in fetch on
  failure. Firecrawl shares its configuration with the search side.
- **Richer multi-agent teams / kanban** — the kanban drawer now drives the full
  task lifecycle (approve / reject, plus waiting-review / paused / skipped
  columns so tasks stop vanishing), a live subagent-tree (node identity, live
  events, tree RPC), operator-tunable broadcast storm-prevention guards, and
  per-task execution-timeout overrides.
- **Type into a chat while it's still running** — queued messages render as
  in-stream "ghost" bubbles and are flushed at the next turn boundary via
  steering, or force-inserted immediately (Esc + ⚡). Available on both the wide
  and phone layouts.
- **Sticky single-chat Todo / plan panel** — an always-visible plan widget above
  the input box; when the plan changes, the previous plan sinks into the chat
  stream as a capsule.
- **Real per-model context-usage gauge** — core-authoritative occupancy and
  window (honoring any `context_window` config override), persisted across tab
  switches.
- **MCP enhancements** — advertises the sampling capability, negotiates the
  protocol version, wires `max_failures`, adds a `post_install` field, and ships
  `zhipu-vision` and `unreal-engine` presets.
- **Tool improvements** — `file_read` now supports images (`image_read`), and
  unchanged file reads are de-duplicated across turns via a read-cache.
- **Soul archetype selector** in Panel agent creation, and a 3D memory-galaxy
  visual/perf pass (per-node twinkle, curved bezier edges, highlight chains,
  bloom retune, vertex-shader idle drift, render-skip when hidden).

### Fixed

- **"Model returned empty response" when steering mid-run** — injecting a
  message between a tool call and its result no longer produces an illegal
  message sequence; tool results are kept contiguous so OpenAI-compatible
  proxies don't silently return empty responses.
- **macOS Local Network privacy** — the daemon now embeds a stable
  `CFBundleIdentifier` Info.plist and is re-signed on build, so it appears in
  the Local Network privacy list and self-hosted SearXNG / Firecrawl stop
  failing with "Network error".
- **Routing failover attribution** — frozen model + provider are attributed from
  the model directive rather than the `failover` wrapper name, fixing collapsed
  availability/attribution.
- **firecrawl is never persisted as a `[fetch]` backend** (it is derived from
  the search config), and the gateway closes remote sessions on token rotation
  while leaving loopback intact.
- **iOS out-of-range port crash** is guarded, and the iMessage group reply now
  routes through `send_to_chat`.

## [26.6.24]

A small, focused follow-up centered on **faster remote-panel cold-load** and
**Windows runtime polish**.

### Changed

- **Remote panel cold-load is dramatically faster** — the control-plane now
  gzip-compresses its static assets on the wire (the ~15.5 MB panel WASM ships
  as ~3.7 MB) and serves every asset with a content-hash `ETag` +
  `Cache-Control: no-cache`. A repeat open revalidates with `If-None-Match` and
  gets a body-less `304`, turning a multi-MB re-download into a tiny round-trip
  while still guaranteeing a fresh panel after every deploy. This cuts the
  lite-shell-over-LAN blank-screen wait from ~40 s to a few seconds.

### Added

- **Homepage link in Settings → Help** — the Help section now links out to the
  Aleph homepage (with `en` / `zh` strings).

### Fixed

- **Windows WebView2 connect page** — the desktop shell now resolves the
  connect-page URL per platform, so the connection-setup screen loads correctly
  under Windows WebView2.
- **`aleph-server stop` on Windows / foreground** — `stop` now resolves the
  target PID from the IPC endpoint, so stopping a foreground or Windows server
  instance reliably targets the right process.

## [26.6.23]

A focused follow-up to the Aleph Hub release. The headline is a **3D nebula
memory-canvas** — the memory graph is now a WebGL2 galaxy you fly through —
alongside the **Aleph Hub convergence**: official MCP servers, skills, and
plugins now all flow through a single cold-start primer so the Hub is the one
place to discover and install them.

### Added

- **3D nebula memory-canvas (WebGL2 galaxy view)** — the memory graph is
  re-rendered as an interactive 3D galaxy: an orbit camera with damping,
  fly-to, and idle rotation; instanced billboard node sprites; a batched
  additive line edge renderer; an FBO bloom pipeline for the nebula glow;
  3D force-directed layout with animated settling and idle drift; screen-space
  node picking; theme-palette category colors with HDR boost; and LOD edge
  density driven by the fold threshold. Pick / select / hover wire through to
  fly-to + highlight + detail panel, and search / agent-switch / list
  cross-links all retarget the galaxy. The legacy Canvas2D renderer, radial
  navigation engine, 2D minimap, and dead prefetch/excerpt subsystem are
  retired.

### Changed

- **Aleph Hub is the single source for official MCP / skills / plugins** — a
  unified cold-start primer projects the bundled official MCP presets, skills,
  and plugins into the `aleph-hub` catalog slot at boot (only when that source
  is empty; a live fetch overwrites it). MCP discovery/install converges onto
  the Hub: the standalone preset install engine and the
  `mcp.list_presets` / `mcp.install_preset` RPCs are retired (catalog.json is
  now Hub seed data), the Settings → MCP recommended section is dropped
  (discovery moves to the Hub), boot migrates off retired preset installs, and
  an stdio MCP install now fails fast when its command is missing.
- **First-party SiliconFlow + t8star MCP presets** — SiliconFlow ships as a
  built-in preset (Aleph-mcp self-built, rewritten to TypeScript and launched
  via `npx`) and t8star is added as the 6th default preset (npx/node); both are
  pinned to `@0.2.2`.
- **Content-type-aware tool-result reduction (context §2.7)** — a deterministic
  cheap preflight pass classifies tool output as log / search / diff and
  compresses it structurally — preserving head/tail and error lines with
  stack-frame context, per-file first/last grep matches, and diff context
  trimmed to ±2 — instead of a flat first-line placeholder, and never grows the
  context.

### Fixed

- **openai_compat video** — the video request body now also carries a
  top-level `prompt`, which OpenAI-compatible aggregators (e.g. T8star) require
  alongside the Ark-style `content` array.
- **macOS Dock icon oversized** — the `.icns` is padded to Apple's 824/1024
  grid so the Dock icon no longer renders ~12% larger than native apps (PNG/ICO
  stay full-bleed for the Windows taskbar).
- **Lite Panel window drag on remote core (macOS)** — the frameless lite Panel
  window is draggable again when connected to a remote Gateway: a runtime
  capability grant authorizes `start_dragging` for the remote origin and the
  platform marker is injected so the drag band renders.
- **Panel polish** — a distinct lite Panel app icon (cyan "P" badge) so it is
  visually separable from the full app on one machine, and the Help card links
  on the welcome settings page are wired up (support link points to the contact
  email).
- **Windows build** — `ListenerState` now compiles on all targets.

## [26.6.22]

A large release. The headline is **Aleph Hub** — a single federated marketplace
for MCP servers, plugins, and skills — alongside multi-agent **Teams**, a
**Strategy** planner, a self-paced **Loop** tool, an associative **memory
graph**, **streaming voice**, and project workspaces.

### Added

- **Aleph Hub — one federated extension marketplace** — a new top-level Hub
  unifies MCP servers, plugins, and skills into a single browsable catalog
  (replacing the separate ClawHub / extensions tabs). Browse by category with
  featured shelves, type/trust filters, and full-text search, then open a
  detail drawer with up-front permission disclosure.
- **Trust-gated install flow** — a trust modal plus a schema-driven config
  wizard (`json_schema_form`) collects required env/secrets, routes the install
  (MCP add / plugin / skill), runs post-install verification, and exposes an
  Installed view to toggle or remove items.
- **Source federation with provenance** — the catalog aggregates multiple
  sources (Aleph Hub, marketplace, MCP registry, Docker-MCP), dedups across
  them by priority, stamps a "via {source}" provenance badge, and refreshes via
  background periodic sync with a last-good cache.
- **LLM-driven extension install** — operator-gated Hub builtin tools (catalog
  sync, resolve spec, fetch docs, trust-gated install + verify) let the agent
  install extensions through conversation.
- **Multi-agent Teams (group-chat orchestration)** — create a team with an
  explicit leader + members and run them as a group chat. The leader
  orchestrates tracked tasks (assign → member submits → leader reviews
  accept/reject) behind a leader-first gate and a fire-once team planner, with
  bounded task retry (exponential backoff + jitter) and cascade hard-delete on
  disband. Panel gains attributed bubbles with emoji avatars, a participants
  popover, `@`-mention autocomplete, live-refreshing 任务/deliverables tabs,
  durable history replay, and rename/disband controls.
- **Mixture-of-Agents synthesis** — subagent batch fan-out can synthesize
  multiple agent answers into one.
- **Strategy planner** — a tool-free, fail-soft planner fires once at goal-set,
  loop-start, and team-chat, writes a Strategy artifact, and welds the
  run-global strategy into spawned subagents. Surfaced through `<strategy>` /
  `<strategy_reminder>` prompt layers and configurable under `[strategy]`.
- **Loop tool (self-paced recurring runs)** — a `loop` builtin
  (start/stop/status/update) with a default soft iteration cap, token-budget
  enforcement, fail-closed tick handling, and continuation-hook re-fire.
- **Associative memory graph** — 4-signal community-aware recall on the primary
  retrieval path, backed by a hand-rolled Louvain community detection (no
  external crate), graph snapshot/cache/insights tables, and graph-health
  insights (isolated / sparse / bridge / surprising) exposed to the LLM.
- **Obsidian-compatible vault** — emit/parse frontmatter (type/title/aliases)
  for byte-compatible round-trip, auto-generated `.obsidian` config,
  frontmatter `aliases` now resolving inside `[[wikilinks]]`, and
  LLM-maintained `overview.md` / `purpose.md` orientation files.
- **Memory Hub & governance (Panel)** — the graph canvas and the vault table
  merge into one Memory Hub with a shared toolbar, search, and forward/reverse
  links between nodes and rows; new Dream Insights and Corrections governance
  views; and a retrieval-trace debug panel backed by `memory.retrieve_with_trace`
  with real per-stage scoring telemetry.
- **Streaming voice** — a streaming speech-to-text contract with Deepgram and
  WhisperLive adapters, `voice.stream.{start,audio,stop}` RPC + delta topic, a
  `voice.format` fast-model regularization pass, an immersive streaming session
  (wave-wipe lock, interim/committed/locked captions), and echo-aware barge-in.
- **Project workspaces** — a session can bind a project folder as CWD with its
  own project `CLAUDE.md`, persisted across restarts; the sidebar gains pinned
  / recent / projects sections with pin/unpin.
- **Firecrawl search provider** — a 9th search provider over `/v2/search` with
  date-range mapping and Test Connection support.
- **Cost-aware route load balancing** — LiteLLM lowest-cost and RouteLLM
  cost-axis routing across a model pool.
- **MCP presets & secrets** — a built-in preset catalog with
  `mcp.list_presets` / `mcp.install_preset` planning
  (NeedsKey/AlreadyInstalled/NoRuntime/Ready) and per-server vault secret
  injection at spawn.
- **Soul archetypes** — three-layer soul composition (Base + Archetype +
  Personalization) with four built-in archetypes (Expert / Companion /
  Assistant / Maker) and an agent-creation interview.
- **Bundled official content** — official skills/plugins are sourced from git
  submodules and embedded at build time, with first-run clone-with-fallback,
  git2 hard-reset sync, `aleph skills sync` / `aleph plugin sync`, and a
  `bundled.sync` RPC; `skill-creator` ships as a built-in skill.
- **Generation presets** — Volcengine Ark image/video presets (Seedream /
  Seedance) + veImageX MCP mount, speaking-speed honored in ElevenLabs / Azure
  / Cartesia TTS, plus Cohere rerank and Jina/Mistral embed presets.
- **/doctor self-repair** — a `/doctor` command and an `f`-hotkey that run
  doctor + LLM repair, backed by a WebRich-gated repair-hint prompt layer.
- **Platform & ops** — a Windows `install.ps1` server installer, Windows
  `screen_record` via ffmpeg gdigrab, console child-window suppression, a
  standalone `aleph-server update [--check]`, a third Panel appearance axis
  (紧凑度 density knob) with a redesigned mode-nav sidebar, glob-pattern
  tool-name permission overrides, and an `agent_switch` tool.

### Changed

- **Multi-model robustness** — per-model `ModelRobustnessProfile` loop
  thresholds with distinctness-based loop detection: legitimate fan-out is
  allowed and a thrashing loop is steered instead of halted. `resolve_behavior`
  is the single source of truth driving per-family coaching deltas; Kimi/Minimax
  ship anthropic-primary presets; grace salvage covers per-turn/stall timeouts
  and a majority-failure soft-landing precedes the hard cap.
- **Context management** — per-model compaction thresholds tuned to each
  model's context window, a cheap-tier summarization provider from
  `[context_budget]`, a preventive-band gate for cheap preflight passes, and
  proportional CJK/code token blending in the pressure sensor.
- **Panel auth** collapses to a single-tier Gateway-token model with a 2-tier
  device permission (Chat / Config).
- **Routing** dissolves the vestigial regex intent classifier (R7/P8) —
  semantic routing only.
- Large modules (>1000 lines) split into directory submodules; `gateway.log`
  rotates on start with 7-day retention; console logging attaches only on an
  interactive TTY.

### Fixed

- **Panel blank/stuck-connecting on remote (hotfix re-release)** — the committed
  `interfaces/webchat/dist/` shipped a js-only rebuild whose `aleph_panel.js`
  referenced wasm-bindgen closure trampolines absent from `aleph_panel_bg.wasm`.
  The panel rendered but the connect coroutine invoked a missing trampoline
  (`TypeError: …closures…invoke is not a function`) and aborted before opening
  the WebSocket, so it hung on "connecting" / blank against any remote core
  (CI embeds the committed dist verbatim — no WASM build — so the broken pair
  shipped). Rebuilt so js + wasm are a matched pair.
- **Stale panel embed in production builds** — `build.rs` now emits the
  `interfaces/webchat/dist/` rerun-if-changed triggers unconditionally. They were
  gated behind the `control-plane` feature, which production server builds
  (`just build`, `default = []`) don't enable, while the panel embed
  (`rust_embed` in `control_plane/assets.rs`) is unconditional — so an incremental
  build after `just wasm` reused the cached embed and served a stale panel. The
  trigger now matches the embed, so a fresh `dist/` always re-embeds.
- **Lite Panel shell ↔ remote core** — three fixes so the panel-only app works
  against a remote core over the public internet: (1) lifted App Transport
  Security (`NSAllowsArbitraryLoads`) so the macOS WKWebView can load a
  user-chosen remote core over cleartext HTTP (default ATS silently blocked it →
  blank webview; the trust boundary here is network + Gateway token, not TLS);
  (2) Settings → 服务与集群 now reflects the **actual** connected core, derived
  from the document origin (`location.host`) instead of the shell's loopback-only
  IPC — a remote-origin panel could never read that IPC and always mislabeled the
  connection as local; (3) the connection target is now fixed by build form — the
  full app is always local-only, the lite shell always remote — so 服务与集群 drops
  the local/remote switcher for a read-only indicator and the one-way
  `data-shell-variant` shell marker was removed (the panel reads `location.host`
  directly, eliminating the "stale shell binary → wrong mode" failure class); and
  (4) namespaced the lite shell's
  connection/autostart markers under `~/.aleph/.desktop-shell-panel-*` (the full
  app keeps the historical `.desktop-shell-*`) so both can run on one machine
  with independent targets instead of clobbering each other's connection state.
- **Security sweep** — anchored `sk-` secret-leak patterns at word boundaries
  (killing false positives like `elon-musk-`), a PII email-detection leak, path
  traversal in notes / extensions-uninstall / exec, SSRF allowlist + metadata-IP
  hardening, OAuth callback HTML escaping, and lock-poisoning recovery across
  subsystems.
- **Cross-platform** — Windows daemon lifecycle / process-liveness,
  `atomic_write` fsync, the NSIS installer icon, and a broad
  check/test/clippy clean-up; UTF-8 char/byte truncation panics across model-id
  parse, ACP/cluster windows, and CJK content.
- **web_fetch** now preserves link URLs and `<time>` timestamps in the selector
  fallback (fixes "页面未提供" on index pages).
- **Voice** — round-2 silent TTS, a dropped final sentence in the splitter, and
  stale keep-alive stalls on OpenAI-compatible TTS.
- **Auto-update** — the macOS path now applies staged updates; Linux deb/rpm
  degrade gracefully to the releases page.
- **Memory** — correction agent-id routing, wikilink re-resolution after a
  concurrent rebuild, and staged-write dedup to prevent commit ENOENT.

## [26.6.14]

### Added

- **BYO voice — OpenAI-compatible local voice endpoints** — voice mode pivots
  to a bring-your-own-endpoint model: STT and TTS can target any
  OpenAI-compatible server (local or cloud) the user runs, dropping the
  self-built sidecar. Adds a `[voice]` config section and a voice-mode model
  override so voice turns can run on a different model than chat.
- **Immersive voice session (Siri-style)** — a full-screen voice mode with an
  animated orb, energy-threshold VAD, an incremental sentence-splitting TTS
  pipeline, barge-in, and a composer mini-orb entry point (tap = immersive,
  hold = dictate, plus a hotkey).
- **Dedicated MiniMax TTS provider** — a hex-decoding `minimax_tts` provider
  replaces the broken `openai_compat` routing (MiniMax T2A returns hex-encoded
  audio inside a status envelope), defaulting to `speech-2.8-turbo`.
- **Volcengine TTS provider** — a new TTS provider over Volcengine's legacy
  openspeech base64 API.
- **SiliconFlow STT/TTS presets** — config-only presets for SiliconFlow's
  Whisper-compatible STT and OpenAI-compatible TTS.
- **Desktop screen-vision loop** — `desktop screenshot` results are now fed to
  the model as native image blocks, so vision models can actually see the
  screen instead of receiving only flattened OCR text.
- **PRODUCT_TOPOLOGY.md** — documents the one-source → three-artifact product
  topology (full app / panel-only shell / standalone server).

### Changed

- **Autonomous goal continuations are observable** — `/goal` continuation runs
  now broadcast to the Panel and `aleph watch` and fan out final results to
  Telegram/Slack, instead of being collected and silently discarded.
  Continuation failures fail closed (Blocked + push) rather than stalling
  silently, and lesson-capture guidance is hardened against self-poisoning.
- **Sandbox bash policy hardened** — command-policy gains de-obfuscation
  normalization (strips zero-width/RTL characters, folds backslash and
  empty-quote evasions) and an undisableable hardline floor: fork-bomb, `dd`,
  `mkfs`, `rm --no-preserve-root`, and redirect rules can no longer be turned
  off even with enforcement disabled.

### Fixed

- **Cohere endpoint + empty-default presets** — fixes the Cohere API endpoint
  and fills 6 LLM presets that shipped with unusable empty default models.
- **Round-2 silent TTS + stuck "正在思考"** — the immersive voice session no
  longer goes silent or hangs on "thinking" after the first exchange.
- **Immersive TTS never firing** — the speak Effect captured zero reactive
  dependencies and never ran; reactivity is now tracked correctly.
- **Stale keep-alive stall on OpenAI-compatible TTS** — eliminated a hang
  caused by stale keep-alive connections, added bounded transient retry for TTS
  cold-start, and made immersive playback robust to incremental segments.
- **WKWebView mini-orb clipping** — the composer mini orb is masked so
  WKWebView clips its blend-mode children, removing a square rendering artifact.
- **Connection-switch hardening** — switching cores blocks a blank remote
  target and drops a redundant reload; "Reload Panel" now performs a
  cache-clearing hard reload; switching to a remote core notes that the local
  daemon stays resident.
- **Glass surface render cost** — glass surfaces use a fade-only entrance that
  kills per-frame re-blur, and overall style-recalc tax is reduced (visuals
  unchanged).

## [26.6.13]

### Changed

- **LAN-trust architecture (shell-core separation reverted)** — Aleph returns
  to a single integrated architecture where the trust boundary is the network
  boundary. The server binds `127.0.0.1` by default; setting
  `[gateway] host = "0.0.0.0"` opts the whole LAN in, granting any LAN device
  full control of the agent (including PTY/shell). Device pairing, token auth,
  the bootstrap/challenge handshake, guest invitations, and the per-method
  authorization gate are all removed — roughly 17k lines of auth and
  shell-separation code deleted. See `docs/reference/SECURITY.md` for the model.
- **Three-artifact distribution** — a single release now ships three
  deliverables across macOS / Windows / Linux: the full desktop App (with
  `aleph-server` bundled for zero-config single-machine use), the Aleph Panel
  lite shell (no daemon — connects to any `aleph-server` on the LAN), and the
  standalone `aleph-server` binary (installed via `curl | bash` for server /
  NAS deployment).
- **DNS-rebinding hardening on the WS origin guard** — with auth removed, the
  WebSocket Origin check is the sole protocol guardrail. Same-origin requests
  are now auto-allowed only when the `Host` is an IP literal or loopback
  (rebinding requires a domain, which IP/loopback cannot be); domain
  deployments must list their origin in `[gateway] allowed_origins`, or set
  `[gateway] allow_any_origin = true` to opt out.
- **CLI and node enrollment reworked for LAN-trust** — the `aleph auth`,
  `devices`, and `guests` subcommands are removed, the desktop "Open in
  Browser" action is de-nonced, clients use the no-token connect-first
  handshake, and `aleph-server` nodes enroll through `cluster.enroll` instead
  of token pairing.

### Added

- **Three-material glass theme system** — appearance gains an orthogonal
  material axis (Luxe / Liquid / Aurora) layered over brightness and accent
  colour, with a 3-up material row in the theme popover and Appearance
  settings, a SwatchButton primitive, and `aria-pressed` state on every
  appearance toggle.
- **Floating composer chrome** — the chat composer floats over the message
  flow with clearance tracking, overlay session tabs, and a top scroll fade.
- **Panel-only first-run connect setup** — the lite shell ships a deterministic
  connect flow with mDNS server discovery and a probe-gated reroute, reusing
  the splash/connect page with progressive enhancement.
- **Material persistence + reduced-transparency fallback** — the chosen
  material is persisted across sessions and every material primitive honors the
  reduced-transparency accessibility setting.

### Fixed

- **Crash safety: char-boundary-safe slicing** — security structural-marker
  detection and extension consent-approval now slice on character boundaries,
  avoiding panics on multi-byte UTF-8 input.
- **Error propagation over panics** — the builtin tool registry propagates
  goal-store errors instead of panicking, and the clipboard tool propagates
  base64 parse errors instead of `expect`.

## [26.6.11]

### Added

- **Autonomous goal-loop hardening (Loop Engineering rounds 2 & 3)** — the
  autonomous goal loop gained an objective stop-hook gate so the model's
  self-reported completion is checked against deterministic guardrails before a
  goal closes; per-goal gate commands (AND-combined with the global gate); a
  per-goal wall-clock deadline (`timeout_minutes`) that auto-retires exhausted
  pursuits; accumulated "lessons" that are rebacked into continuation prompts and
  promoted into long-term notes via a global-only Dream consolidate stage; secret
  redaction on unattended-run trace streams; and fail-closed behavior on
  confirm-gated tools during unattended autonomous runs.
- **Real-time memory (keyword linking + weaving)** — deterministic
  keyword-overlap note pairing with LLM keyword extraction, a keyword-first link
  contract (FTS fallback when embedding is empty), a NoteWeave Dream stage that
  relinks orphan notes by keyword overlap, a mandatory ingest link contract with
  a repair prompt, async session-end flush (compress + link) gated on a bounded
  per-agent readiness registry, and note `keywords` frontmatter.
- **Multi-agent: subagent completion announce + session deadlock lifecycle** —
  background subagent completions now announce back to the parent session, with a
  session-deadlock lifecycle (sweep + conclude) so stalled multi-agent sessions
  are reclaimed.
- **Teams orchestration wiring** — team lifecycle events on the event bus, a
  process-lifetime background-agent tracker, subagent guardrail inheritance, and
  an end-to-end `lead_review_required` verification gate on workflow review
  steps.
- **Aleph cluster node visibility** — `node_invoke_many` concurrent tag fan-out
  tool and `node_list`, node tags on the connect frame for fan-out selection,
  `last_seen` wiring, and an offline-fleet view that merges unreachable nodes.
- **CLI streaming fidelity** — `aleph watch` live activity board, model-fallback
  notice, colored history with markdown rendering, live response-body streaming,
  and provider-retry (`run_retrying`) visibility in CLI/TUI/Panel.
- **Panel glass language rollout** — chat message-flow glass refresh, unified
  `nav-tile` frosted sidebar material across all menus, glass round-2 for
  transient surfaces (modals/menus/drawers) with a 28%-smaller WASM bundle, and
  full-width assistant reply bubbles.
- **Tool permission policy end-to-end** — tool permission policy wired
  end-to-end with a per-channel permission layer and a three-tier
  (global → agent → channel) merge.
- **MCP protocol fidelity + OAuth** — wire-format fidelity fixes, Streamable
  HTTP session support (`Mcp-Session-Id`), OAuth bearer-token chain wiring with
  an `mcp_login` tool, and external MCP tools surfaced into the live LLM tool
  registry.
- **Provider capability surface** — a `list_models` tool surfacing capability +
  cost metadata, a model-metadata/pricing catalog refresh, live route/failover
  `route_status` observability, native ollama `/api/chat` migration
  (history + tools + vision), gemini parallel `functionResponse` merge with
  cache-billing fix, and OpenAI stale encrypted-reasoning strip-and-retry.
- **Voice as a context layer** — the previously dead voice-mode context layer is
  wired into prompt generation, with a Whisper hallucination filter on
  transcription and markdown/URL sanitization + length clamp on TTS.
- **Hook + plugin service lifecycle** — `[prompt.extra_files]` config wiring and
  `HookAction::Agent` delivery; plugin manifest `[[services]]` wired end-to-end
  with a full service lifecycle (autostart / disable / uninstall / shutdown /
  reload).
- **Message queueing / steering** — a non-lossy FIFO busy queue with a `queue`
  busy-input mode, a `/stop` channel command, steering teardown-race rescue, and
  a replayed interruption marker.
- **Compaction cache + process-topology slimming** — a compaction fingerprint
  cache that kills per-turn redundant LLM summarization and wires daily insights
  into memory gather; plus a single process-wide `AlephBridge` (3 → 1) with lazy
  spawn, cutting idle desktop subprocesses.

### Fixed

- **WhatsApp credential encryption** — WhatsApp auth data is now encrypted at
  rest in the vault instead of stored in plaintext.
- **Sandbox hardening** — fail-closed writable-symlink TOCTOU guard for the
  macOS seatbelt, and a head + tail command-policy scan that closes padded-tail
  evasion.
- **Security miscellany** — SOCKS5 `nmethods` allocation cap, PowerShell
  injection escaping in desktop mail ops, stronger filename validation and file
  permissions, constant-time A2A auth-token comparison, and A2A
  broadcast-channel leak fixes.
- **Memory incoming-link full-path matching** — NoteDecay incoming-link
  protection and NoteWeave orphan detection now match the full `to_note` path,
  preventing false-orphan archival.
- **Harness exit fidelity** — exit-point `terminate_reason` fidelity (including a
  `DiminishingReturns` variant) and parallel tool-batch partitioning into
  resource-disjoint groups.
- **Panic hardening** — UTF-8 byte-slice panic in device-timestamp formatting,
  saturating token totals to avoid overflow panic, and poison-safe mutex handling
  in webchat event dispatch and the ACP streaming callback.
- **macOS shell** — `perm_monitor` now spawns on the Tauri runtime (not a bare
  tokio runtime), and the daemon auto-restarts on permission grant.
- **Tool-call salvage** — malformed tool-call argument salvage, wider coercion,
  did-you-mean `NotFound` hints, and bounded error bodies.
- **Workflow run lifecycle** — run identity, status/cancel surface, sticky
  cancellation guard, and deterministic latest-run tie-break on same-second runs.
- **Initialization rollback protection** — `init_unified` now protects
  pre-existing data during an initialization rollback instead of clobbering it.

## [26.6.9]

### Added

- **Aleph 集群（One Core, Many Nodes）** — a single-center asymmetric node
  federation built on reverse-RPC. A center daemon can now resolve and invoke
  remote nodes over a persistent reverse channel: `node_invoke` (run a
  command-allowlisted tool on a node), `node_file` (push/pull files
  center↔node with per-file sha256 + 8 MB cap, never routed through the LLM
  context), and node-side approval routing that bubbles a node's capability
  upgrade back to the operator's existing Panel approval card. Landed across
  Phase 0a (reverse-RPC channel + `PendingInvokes`), 0b (`NodeRegistry`), and
  0c (node runtime, interactive 6-digit pairing enroll, command allowlist).
- **节点生命周期与多层解析** — multi-tier node resolution with operator-driven
  deregister and Panel wiring, plus `node.connected` / `node.disconnected`
  lifecycle events. Node disconnects now fail-fast: in-flight invokes are
  cancelled at the connect/disconnect seam instead of hanging up to the full
  timeout.
- **Panel Glass 主题** — a re-added translucent "Glass" theme with a controlled
  dark aurora backdrop and intensified chrome glass, tokenized blur/saturate
  behind CSS variables, a reduced-transparency opaque fallback for
  accessibility, and a command-palette quick-switch. Light and Dark were
  harmonized alongside it (Dark retuned to a calm darkened-Light; the drama
  moves to Glass).
- **一核多端会话连续性** — session origin-channel binding so a conversation
  started on one surface (Telegram / Panel / iMessage) carries its source
  through `sessions.list`, plus cross-surface reply fan-out and owner-DM
  `dm_scope` wiring that lets a single user share one Main session across all
  channels.
- **Channel 两层权限模型** — channels map onto Chat / Config device tiers:
  conversational access with a default workspace vs. configuration access with
  a free workspace, closing an over-authorization gap where every inbound
  external message was implicitly treated as operator.
- **Panel Network 配置页** — a connection-switch page for local/remote core
  selection with a cluster skeleton, a clickable connection-status chip that
  links to network settings, and full connection i18n (shell-core separation).
- **Standing-goal 子系统** — a persistent standing-goal subsystem that keeps a
  user-set goal active across turns until satisfied.
- **Memory 实体图与写时去重** — a memory entity graph (Gap A), write-time
  three-tier ADD / MERGE / NOOP dedup (Gap B), hot-recall wiring, memory↔context
  budget coordination, and memory-extension lifecycle hooks
  (MCP binding / on_delegation / on_pre_compress).
- **配置回滚与自管理** — config rollback / undo via `self_config`, plus the
  "service & cluster" rename with self-management guides so Aleph's own
  configuration is driven by natural-language tool calls (R8).
- **Panel 崩溃自报告** — symbolicated, self-reporting panel crashes: the WASM
  name section is preserved, a panic overlay surfaces a readable Rust stack,
  and the last 10 crashes are kept in a ring buffer for one-shot diagnosis.

### Fixed

- **Panel 前后端连线大扫除** — fixed frontend↔backend wiring across all six
  dashboard pages, wired runtime-install progress events through to the Panel,
  unified provider behavior (fill / save / test / verify / set-default /
  badges) with stable key echo, and removed large swaths of dead config items,
  i18n keys, and orphaned components.
- **停止回吐明文密钥** — provider, channel, and shared-token secrets are no
  longer echoed back in plaintext; presence is reported via `has_*` flags and
  re-entry uses pairing codes / reveal-once flows.
- **凭证沙箱隔离** — agent file tools are denied access to the vault and auth
  data; browser navigation / click / type / fill / evaluate now enforce the
  approval policy.
- **MCP 工具过滤与 slash 快路** — per-server MCP tool filters now apply, MCP
  slash commands no longer hard-fail on the L0 fast path, and dead router /
  tool-filter entropy was removed.
- **Provider 模型 ID 归一化** — endpoint-aware OpenAI model-id normalization
  fixes OpenRouter slug stripping.
- **Gateway chat.history 游标分页** — `chat.history` now paginates correctly
  before the cursor.
- **多端打字机/即时输出同步** — the global typewriter/instant output switch is
  synced across all channels (Telegram bypass fixed, behavior reclassified as
  Live).
- **中途转向注入** — mid-loop steering injection is now bounded and coalesced,
  and a mid-final-turn steering boundary race was fixed.
- **后台 bash 超时** — background bash jobs get a generous timeout instead of
  the 60 s foreground default.
- **删除操作二次确认** — all destructive Panel buttons gained an inline
  two-step confirm.

## [26.6.7]

### Added

- **Usage / rate-aware provider routing** — a load-balancing layer that folds
  per-provider RPM / TPM rate limits into routing decisions (LiteLLM
  usage-based parity): a lock-free 60s usage window meters in-flight requests
  and tokens, saturated providers are demoted toward the tail of their tier
  (never starved), and `pin` still overrides the gate. Empty `rate_limits`
  stays byte-identical to prior behavior.
- **Panel chat sidebar wiring** — the left session list gained a working
  client-side search filter, a live per-session "running" indicator driven by
  run lifecycle events, and a bottom status bar showing gateway connection
  state plus active-run count (reusing the existing `activity.stats` RPC).
- **Workspace tool-card rich rendering** — tool invocations now render as
  structured cards with per-kind bodies (diff / shell / write / patch / read /
  search) in both the left chat and the right workspace panel, replacing the
  flat activity rows; assistant turns are bubble-less for higher density with
  only the final answer surfaced as a bubble.
- **Workspace trace persistence** — a read-only `trace.by_runs` RPC plus a
  panel `replay_run` path rehydrate a run's chat bubbles and workspace payloads
  from persisted traces on reload, with a shared `apply_trace_event`
  projection used by both the live and replay code paths.
- **Cron agent selector & graceful degradation** — cron jobs can now be bound
  to a specific agent via a Panel selector (with default-id fallback), display
  their read-only delivery channel for awareness, and fall back to the `main`
  agent — with a fallback note — when the bound agent has been deleted.
- **Canvas hover & performance pass** — hover hysteresis with hover frozen
  during tweens, `requestAnimationFrame` parking when the canvas is hidden
  (resumed via `IntersectionObserver`), and a `ResizeObserver` replacing
  per-frame layout reads.
- **Session-end reflection & open-loop tracking** — a single session-end LLM
  pass distills first-person lessons into the note layer and tracks unresolved
  "open loops", injecting them back into later sessions; the `FileSessionStore`
  backend now actually emits `session_end` raws (a 13-day-dormant path).
- **Redundant-call loop guard** — a fourth sibling nudge that detects a
  non-idempotent tool (bash / code_exec / write / MCP) repeating the identical
  `(tool, args)` + byte-identical result ≥3 times and emits a soft
  `system-reminder`, closing the alternating-tool bypass of the prior guards.
- **Panel introspection & context tooling** — a context-budget gauge,
  transcript export, and empty-state suggestions in the panel, plus an offline
  `aleph-server prompt-size` subcommand that breaks down the system-prompt
  pipeline by layer.
- **Centralized injection threat library** — a single-source regex threat
  library (exfiltration / role-hijack / C2-promptware / persistence) wired into
  the external-content wrapping chokepoint so web / MCP / tool / browser content
  gains detection automatically.
- **Harness callback wiring** — `on_tool_call_start` and `on_tool_call_done`
  now fire at the real tool start / completion sites, so live clients receive
  matched tool-begin / tool-end events instead of a leaking pending-tool list.

### Fixed

- **Exec-class tools failing in Panel / cron sessions** — `code_exec` / `bash`
  / `code_check` returned `no active session context` and the model retried
  forever, because the dispatch chokepoint scoped only `TURN_CONTEXT` and never
  `SESSION_ID`. The chokepoint now scopes `SESSION_ID` from the turn's session
  key, fixing the WebChat exec infinite-loop class at its root.
- **GUI-launched daemon reporting runtimes as "not installed"** — a daemon
  started from the `.app` / launchd inherits a minimal `PATH`, so `fnm` /
  `node` / `cargo` / `uv` probes failed. The runtime probe now searches known
  install directories (cargo / local / homebrew / fnm alias bins) via a single
  chokepoint, without mutating the global environment.
- **Panel final answer trapped as a step + duplicated narration** — a
  terminating turn that carried both text and a tool call rendered its summary
  as a streamed step instead of a final bubble, and two async pipelines raced
  the same bubble; the final answer is now authoritatively finalized and
  narration de-duplicated.
- **Memory panel correctness** — memory views are paginated, raw-memory delete
  is fixed, and tool telemetry is hidden from the memory dashboard.
- **Compound-ingest dropping knowledge** — the L0→L1 planner omitting the
  `kind` discriminator no longer silently burns raws; ops recover their kind by
  field shape and empty-plan batches defer young raws for retry instead of
  marking them processed.
- **Provider resilience under 429 / slow first byte** — Kimi 429s are now
  ridden out in place with paced re-requests and same-protocol fallback
  preference, a time-to-first-byte timeout bounds stalled LLM requests, and 429
  is treated as transient with a two-tier tool-loop halt.
- **Tool-loop halt salvages a deliverable** — hitting a loop halt now closes
  the orphan `tool_use` and salvages a partial deliverable rather than aborting
  empty, with added Tier-2 loop detection; a per-tool budget overrun is now a
  recoverable tool error, not a whole-run abort.
- **Run-failure & orphan-task receipts** — the gateway now surfaces
  user-facing receipts for run failures and for tasks orphaned by a restart,
  and round-trips provider `context_window` through the CRUD DTOs.
- **Context compaction sizing** — the compaction budget is derived from the
  active model's window (and the chain-minimum window across agents), instead
  of a single primary, so large-context runs compact correctly.
- **Status-bar reconnect leak** — the panel status-bar poll loop is guarded
  against accumulating stale reconnect effects.

## [26.6.5]

### Added

- **Terminal, shell & code tooling** — an embedded PTY terminal stream over a
  single multiplexed gateway port (`portable-pty`); `bash` background-process
  execution with a poll / kill / list registry; and a new `code_check` tool
  that auto-detects the project's typecheck / lint and returns structured
  diagnostics.
- **Installed-plugin update lifecycle** — plugins can now be updated in place
  (crash-safe atomic swap with rollback, semver no-downgrade guard) via
  `plugin update [name] [--force]` and the `plugin.update` RPC, instead of
  being pinned forever at install time.
- **Intent-based tool discovery & repair** — a `search_tools` meta-tool surfaces
  relevant tools by intent, and a unified tool-name repair resolver fixes
  model-emitted typos / separator swaps before dispatch.
- **AI-assisted doctor repair** — `aleph doctor --fix` (or the interactive
  F-key) routes failing diagnostics through the LLM for guided repair instead
  of stopping at a static hint.
- **Provider routing & cost** — a pluggable load-balancing layer over failover
  routing with fair per-owner multi-agent scheduling; long-context tiered
  pricing (Gemini / Claude 1M-beta) that stops undercounting large-context
  runs; and a Kimi CN-region preset with a refreshed Moonshot lineup.
- **Desktop computer-use depth** — a vision bridge so text-only models can act
  on screenshots; native push-to-talk audio capture for the Panel voice
  channel; window move / resize geometry control on macOS, Windows
  (`SetWindowPos`), and Linux; a Windows UI-Automation accessibility backend;
  Linux `PermissionCapability` + real OCR bounding boxes; and a `ydotool`
  Wayland input fallback.
- **Matrix channel** — auto-join, per-room `@mention` gating, and inbound media
  download.
- **Gateway delivery & observability** — a durable outbound delivery queue
  (SQLite-backed retry), a request-latency histogram with a per-IP connection
  cap, effective-working-directory surfaced to the model each turn, and an
  `insights.tools` per-tool usage-introspection RPC.
- **Smarter memory** — write-time semantic dedup at compound ingest, Mem0-style
  reference-token indirection, dormant cross-encoder rerank and salience
  scoring wired into recall, and a dedicated recall slot for user-taught
  feedback rules.
- **Approval & security tiers** — a session-scoped approval tier
  (`AllowSession`) between once and always; a shared vendor-credential catalog
  that widens leak detection across ~15 providers; a per-path config
  reload-impact classifier; a `config_audit` posture tool; and a hard floor on
  dangerous tools for guest / remote surfaces.
- **Capability health & hooks** — capability health probes for browser and
  media-generation tools, `SubagentStart` / `SubagentStop` lifecycle hook
  events, and a `BeforeCompaction` interceptor that can pin context surviving
  compaction.
- **ACP & A2A** — bidirectional incoming-request support over ACP and an
  `a2a` `tasks/resubscribe` streaming method.
- **Panel polish** — an Appearance settings page with consolidated theme logic,
  an order-preserving substring filter on the chat model picker, and a
  day-separated message timeline.
- **Toolchain & dependencies** — pinned to Rust 1.96 (MSRV 1.95) with a broad
  dependency refresh (`schemars` 0.8 → 1.2, `rand` 0.8 → 0.10, `sha3`
  0.10 → 0.12, `sysinfo`, `teloxide`, `handlebars`, `tokio-tungstenite`,
  `printpdf`, and the consolidation onto a single `reqwest` 0.12).
- **Panel workspace as a live activity stream** — the right-hand pane now
  auto-renders the agent's tool activity as an inline-expandable timeline
  (args + results) plus a project file-tree drawer with read-only preview over
  a new scoped `fs.read_file` RPC, replacing the old click-to-open single-view
  JSON inspector.
- **Panel step cards & workflow echo** — multi-step Think→Act runs render as
  iteration-grouped step cards driven by `agent_trace`, with iteration labels
  and a cross-highlight ring linking each chat bubble to its workspace step
  (instead of collapsing into one concatenated paragraph).
- **Workflow clarify step** — a workflow run can pause to ask the user a
  question over any channel and resume on their reply, durable across daemon
  restarts (CoordTask-backed).
- **Failover model migration on rate limits** — transient 429s ride out with
  deeper in-place exponential backoff, and a per-model rate-limit cooldown
  migrates within a request to a sibling model (honoring `Retry-After`) before
  advancing the provider chain.
- **Human-like memory** — permanent core-knowledge exemption from decay,
  hot-surfacing + retrieval-time time-decay enabled by default, a
  model-perceivable four-layer cognitive taxonomy over the memory envelope, and
  local-first automatic embedding-provider resolution.
- **Workflow interop fidelity** — `.workflow.js` round-trips multi-line agent
  prompts as the `join`-array idiom, captures agent opts on the bare-scan import
  path, and reconstructs `parallel([...])` blocks into a DAG layer; plus
  auto-drafted, gated MetaSkill proposals mined from skill co-occurrence.
- **CLI session export** — `aleph session export` dumps a session transcript to
  Markdown or JSON.
- **Desktop shell external-link guard** — external links route to the OS browser
  while the webview stays pinned to the Panel origin.
- **Skill index graceful degradation** — the prompt skill index degrades in two
  tiers under budget pressure instead of hard-dropping low-priority skills.

### Fixed

- **Sandbox hardening** — Linux seccomp now denies the full socket-control
  surface and the FS-handle escape syscalls that bypassed path-based Landlock;
  macOS seatbelt adds `allow_local_binding` for loopback servers; Windows
  serializes workspace DACL read-modify-write across concurrent inits; and
  arch-absent syscalls are skipped instead of aborting.
- **Secret egress, fail-closed** — shell output now fails closed on
  catastrophic secrets (private keys) instead of only redacting, browser
  page-content egress redacts embedded credentials in both directions, and
  command output is sanitized of ANSI escapes and binary control bytes.
- **Large static-audit sweep** — logic, security, and UTF-8 bugs fixed across
  the gateway, memory, daemon, extension, generation, agents, a2a, context,
  components, and CLI subsystems, plus removal of several zero-consumer dead
  modules (R10 YAGNI).
- **Gateway streaming resilience** — the event-bus → client forwarder now
  survives transient `broadcast` lag instead of silently starving the socket,
  and streaming edits use adaptive flood-control backoff that honors the
  channel's `retry_after` hint.
- **CLI output & connectivity** — agent final-result text now falls back to
  `RunSummary.final_response` so reasoning / tool-only runs are no longer
  printed empty, the broken default gateway URL is fixed, `ask` accepts stdin
  piping, and version is sourced from the `VERSION` file.
- **Provider correctness** — typed Anthropic error-envelope classification with
  explicit thinking-disable honored, truncated-stream detection surfaced as
  retryable, Gemini schema fidelity (required reconciliation, `$ref` sibling
  preservation), and spec-conformant OpenAI array-form content + stream usage.
- **Approval & redaction** — the exec approval decision set is derived from
  command risk (no allow-always for danger), `LeakAction::Redact` masks raw
  secrets in redacted text, the `**` glob spans newlines to close a blocklist
  evasion, and `clawhub` slug paths drop dot segments to block traversal.
- **Memory & migration** — fixed a curated-store deadlock under concurrent
  writes, note-layer starvation under embedding degrade, and a stale
  embedding-dimension left after `StateDatabase` migration.
- **Sandbox denial ledger (否决账本)** — a session-scoped negative ledger trips a
  brute-force pause after repeated denials and surfaces an agent hint to the
  model, purges offloaded tool-result cache on circuit-break (防引用绕过),
  broadens the file-ops credential denylist, collapses the three security tiers
  onto an ordered `PolicyTier` single-source, neutralizes invisible-char
  injection, and XML-escapes `system-reminder` fences in untrusted hook context.
- **Config & build** — all state paths route through an `ALEPH_HOME`-aware
  resolver with a guard against provider-config erasure; custom non-preset
  providers now surface in the chat model catalog; an unconditional `warn!`
  import unbreaks the Windows build; and `Cargo.lock` workspace versions are
  synced to 26.6.5.

## [26.5.30]

### Added

- **Passwordless auth UX** — first daemon start auto-provisions the token; the
  desktop app does a silent keychain bootstrap, same-machine browsers open via
  `aleph open`, and remote/mobile devices pair with a 6-digit code or QR
  approved from the desktop NotificationCenter. The legacy `/login` token-paste
  form and `?token=` URL fallback are gone.
- **Proactive multi-channel push** — the new `channel_message` tool lets the
  assistant reach you on its own (send / react / edit / typing) across any
  connected channel, so help arrives in the channel you already use.
- **New and hardened channels** — native WhatsApp channel; Slack inbound
  file/image attachments; Feishu encrypted-webhook decryption; iMessage
  allow-list gating, attachment paths, and offline catch-up; opt-in group
  `@mention` gating for Telegram and Microsoft Teams.
- **`google_meet` tool** — join / create / leave / speak / status, relayed to
  an out-of-core meeting bridge.
- **Workflow templates + `.workflow.js` interop** — save named, re-runnable
  declarative workflows that compile to a coordinated task DAG, with lossless
  bidirectional import/export of the Claude-Code-compatible `.workflow.js`
  format.
- **Multi-agent teams** — team templates, snapshots (capture / restore / list),
  zombie-task detection, per-team token-usage aggregation, task-level control,
  a merged replay timeline, `@mention` parsing, ACP-backed members, and
  step-level workflow review.
- **Smarter retrieval** — new `ctx_search` (BM25 over offloaded tool output)
  and `recall_events` (BM25 over the session event log) tools, plus hybrid
  lexical memory recall (porter + trigram, RRF-fused).
- **Search & web reach** — Jina and DuckDuckGo search providers, a SERP-scrape
  fallback when every provider fails, an LRU URL cache for `web_fetch`, and
  language / region / date-range / safe-search wiring.
- **More model providers** — many new chat, TTS, STT, and image presets
  (Deepgram, Azure Speech, Cartesia, MiniMax, Suno, BFL FLUX, Fal.ai),
  per-family thinking-budget profiles, model-id normalization, discovery
  fallbacks, and DeepSeek cache-hit metrics.
- **Computer use** — `gui_locate` (natural-language → screen coordinate),
  `wait_visual` screen-settle detection, a browser-operator strategy, the
  UI-TARS coordinate contract, and ready-made recipes (book a flight / hotel,
  draw).
- **Panel** — ⌘K command palette, NotificationCenter, a workspace pane with
  per-tool renderers, a trace replay scrubber, a collapsible reasoning panel
  for streamed model reasoning, a multi-tab SessionMap, a plan/DAG view, and a
  rebuilt memory canvas with editable note cards (`graph.update_note`).
- **24/7 scheduling** — per-job timeouts with failure alerts, transient-error
  retry classification, a raised iteration cap (40 → 200) and wall-clock budget
  for long-running jobs, carry-over resume of interrupted runs, heartbeat
  active-hours, and periodic run-history reaping.
- **Gateway connect & observability** — the `connect` response now carries a
  `hello` snapshot (identity, uptime, presence, limits, capabilities, active
  workspace); a `connect.challenge` HMAC-nonce flow for replay hardening
  (`require_challenge`); optional idempotency-key enforcement
  (`require_idempotency_key`); field-level `events.subscribe` filters
  (`{topic, where:[{field, equals}]}`); and a `gateway.metrics.lanes`
  occupancy gauge.
- **Sandbox** — command-policy hard-filter for shell execution and a managed
  in-process proxy for allow-listed hosts.

### Fixed

- **Browser SSRF hardening** — content-read tools now re-validate the active
  tab's current URL against the SSRF policy (defeats redirect / JS / history
  reaching an internal origin), and hostname allow/deny checks normalize
  trailing dots.
- **Prompt-injection defenses** — untrusted channel/sender labels are sanitized
  before entering the system prompt, and injected memory recall is fenced as a
  non-authoritative reference with echoed fences stripped from the stream.
- **Auth brute-force & token leaks** — per-source-IP rate limiting on failed
  `connect` auth, live WebSocket disconnect on device-token revocation, and
  removal of the stderr token banner that leaked the token into logs and
  screencasts.
- **Provider error handling** — fixed Anthropic OAuth identity and the legacy
  thinking-budget `max_tokens` guard; Anthropic 429 overloads and
  OpenAI/xAI usage/quota-limit errors are now classified correctly; Ollama
  honors configured `top_p` / `top_k` and normalizes greedy decoding.
- **MCP tool quarantine** — MCP tools whose parameter schema is structurally
  unusable are now skipped with a warning instead of 400-ing every provider
  request while that server is connected.
- **Context compaction** — empty compaction is recovered when the model emits
  an analysis-only reply, so the context summary is never silently blanked.
- **Reply-storm protection** — a bot↔bot pair-loop guard plus group channels
  that respond only on `@mention` or reply-to-bot.
- **Gateway resilience** — a bounded rate-limiter (caps memory growth), unified
  jitter-free channel restart backoff, and an hourly health monitor that
  auto-restarts wedged channels.
- **Harness recovery** — reactive compaction rescue on `prompt_too_long` and
  structured tool-error hints with a fallback ladder.

## [26.5.24]

### Highlight — Aleph is now a native desktop app

This release marks a major repositioning. Aleph previously shipped as a
headless `aleph-server` daemon (openclaw-style) that users wired up to
channel bots from a terminal. **Starting with the 26.5.x line, the primary
delivery is a signed native desktop app for macOS, Windows, and Linux** —
the app bundles `aleph-server` via Tauri `externalBin`, runs in the system
tray, and exposes the full assistant through a polished Leptos chat panel.

What this changes for users:

- **One-click install** — `.dmg` / `.msi` / `.deb` installers, no Docker, no
  terminal, no `docker compose`, no port forwarding
- **System-tray daemon** with launch-at-login, global summon hotkey, native
  OS notifications, `aleph://` deep links, and signed background auto-update
- **In-app pairing wizard** and visual settings panel — `aleph.toml` editing
  is no longer required for normal use
- **Native macOS / Windows / Linux integration** — titlebar drag, accent
  scrollbars, theme toggle, sidebar collapse, single-instance enforcement,
  window-state persistence
- **Channel bots and WebChat keep working unchanged** — the same `aleph-server`
  inside the desktop app still serves Telegram / Discord / Slack / WhatsApp /
  iMessage / 15+ other channels plus the browser WebChat UI. The desktop app
  is an additional surface, not a replacement
- **Headless mode is still supported** — `cargo run --bin aleph-server start`
  on a VPS works exactly as before for users who only want remote channel
  access

See the rewritten [README](README.md) / [README_CN](README_CN.md) for the
full pitch. Hero screenshot: `docs/images/aleph-desktop.png`.

### Added — Desktop shell

- **Native desktop shell** (`desktop/shell/`, Tauri v2) — full lifecycle:
  window management, system tray, OS notifications, signed auto-update,
  `aleph://` deep links, global summon hotkey, macOS app menu, daemon
  supervision (`externalBin` boots `aleph-server` on app launch, restarts on
  crash, shuts down gracefully on quit). Single-instance enforcement +
  window-state persistence carry over across restarts.
- **In-app pairing wizard** — replaces the deleted `aleph pair` CLI
  bootstrap. PairingFlow + Leptos `PairingModal` drive `wizard.*` JSON-RPC,
  auto-triggers on `pairing_required`, reconnects with the issued token,
  and persists credentials to the OS keychain (`keyring` crate). Gateway
  auth allows unauthenticated `wizard.*` on loopback for the bootstrap
  hop only. Shell loads the gateway token from the keychain before any
  subsystem boots.
- **Hebrew aleph (א) glyph as the app icon** + brand identity artwork
  archived under `docs/brand/`.
- **Codex-style panel layout** — chat-first shell with bottom-left section
  menu, single-row composer (paperclip + textarea + send), glass theme set
  (Gemini light-field aesthetic + dark / contrast variants), macOS
  titlebar drag strip with right-aligned sidebar toggle, accent scrollbars,
  inline theme-toggle popover anchored inside the sidebar.
- **Cycle 3 desktop bridge / daemon trio** — `MicLevelMonitor` (opt-in,
  AVAudioEngine tap via Swift `MicMeterSession` actor), `screen.capture`
  via ScreenCaptureKit with xcap fallback on macOS 13, `PresenceReporter`
  daemon for Slack-style presence. Permission-kind enum unified across
  macOS / Windows / Linux into a single 14-variant `PermissionKind` with
  bridge-only kinds returning `Unknown` on unsupported platforms.
- **Cycle 3 hardening** — fixed Windows idle-time `u32` wrap, macOS
  `request_notifications` no-op, Linux Wayland silent fail; added Automation
  Finder probe + Location bridge gaps.
- **`just verify-build`** — CI build-only three-platform verification target
  (build + upload artifacts, no tag, no release) for pre-release sanity
  checks. Sibling to `just release`.
- **Per-channel-class concurrency** with a reserved Desktop lane in the
  gateway router so panel traffic never queues behind a slow Telegram
  poll or a long-running channel bot turn.

### Added — Core (memory canvas, hooks, security, providers)

- **Memory canvas: rich Markdown node cards (FULL / MINI / DOT modes)** rendered as a Leptos DOM overlay over Canvas2D edges. The center node renders as a 280px FULL card with stripe + title + lazy-fetched Markdown excerpt + tag chips; 1-hop nodes render as 140px MINI pills; 2-hop and orphan nodes render as 10px colored DOTs. Hovering or selecting a node promotes it one tier (DOT→MINI→FULL); zoom < 0.5 force-collapses everything to DOTs. rendered as a Leptos DOM overlay over Canvas2D edges. The center node renders as a 280px FULL card with stripe + title + lazy-fetched Markdown excerpt + tag chips; 1-hop nodes render as 140px MINI pills; 2-hop and orphan nodes render as 10px colored DOTs. Hovering or selecting a node promotes it one tier (DOT→MINI→FULL); zoom < 0.5 force-collapses everything to DOTs.
- **Memory sidebar: filled the shared left column** with agent picker, search input, fold-threshold slider, and a `<NodeDetailPanel>` that lists recently visited memories when nothing is selected. The four right-side widgets (agent selector, toolbar, breadcrumb, detail panel) were stripped; their state lives in a new `MemoryState` Leptos context provided at the `App` root.
- **Sidebar collapse**: ⇧ button at the sidebar footer + Esc key + 8px right-edge hover strip + peek-handle button + localStorage persistence (`aleph.sidebar.collapsed`). The whole sidebar slides out via CSS transform animation, giving the canvas full-width when in focus.
- **Edge labels**: free-form `label` and `kind` fields on graph edges (Obsidian JSON Canvas-compatible naming); labels fade in for edges adjacent to the hovered/selected node when zoom ≥ 0.7. Position pinned to Bézier midpoint, rotation clamped to `[-π/4, π/4]` so labels never read upside-down.

### Changed
- **Memory canvas layout**: replaced the strict concentric "religious totem" rings with deterministic-jitter perturbed rings (±17° angular jitter, ±15% radial jitter via FNV-1a hash) plus Poisson-disk-scattered orphans (20 candidates per orphan, 60% central exclusion rect, spill into outer band beyond 20 orphans). No force engine, no new crates. Snapshot-locked via `tests/fixtures/layout_baseline_30nodes.json`.
- **Memory canvas edges**: replaced straight strokes with α-gradient quadratic-Bézier curves (sag 0.12, both control points coincide), layered by hop (1-hop α 0.85 / width 1.8; 2-hop α 0.55 / width 1.2). Adjacent edges to a hovered/selected node highlight in gold (`#fcd34d`) at 1.5× width via a two-pass render that dims non-adjacent first then draws bright adjacent on top.
- **Phase 0 perf gate (soft pass)**: 300 DOM-overlay cards over Canvas2D, measured via Chrome DevTools — median 60 fps, p25 56 fps, p5 30 fps under synthetic stress (every frame mutates 300 transforms). Production load is much lighter (cards static 95% of the time, transform changes only on drag / selection / hop transition). Phase 8 final perf validation deferred to user manual verification.

### Removed
- `interfaces/webchat/src/views/canvas/agent_selector.rs`, `toolbar.rs`, `breadcrumb.rs` — their UI is now in the shared sidebar. Net −342 lines from this cleanup alone.
- Per-node `draw_node` circle rendering in `renderer.rs` (replaced by DOM-overlay `<NodeCard>` components). Old `r_orphan` ring helper + the sector/golden-angle orphan layout in `populate_orphans` deleted.

### Fixed
- **Extension hooks — production wiring closure (hermes-inspired)**: the
  `HookExecutor` snapshot that the gateway request loop already builds was
  being dropped on the floor (`let _hook_executor = ext_manager.hook_executor_snapshot()…`
  in `gateway::execution_engine::run_loop`). Combined with the fact that
  `build_request_tool_service` never accepted a hook executor, this meant
  `BeforeToolCall` / `AfterToolCall` / `AfterToolCallFailure` were registered
  end-to-end but never fired for a single production tool call. The snapshot is
  now plumbed through `build_request_tool_service` → `ScopedToolService::with_hook_executor`
  for both the main agent dispatch and the subagent parent-view path, with the
  current `SessionKey` flowing into `HookContext::session_id`. `BeforeToolCall`
  fires as an interceptor (block / deny / ask / update_input); `AfterToolCall`
  and `AfterToolCallFailure` fire as observers + interceptors with
  `update_output:` honoured. The legacy in-process `ToolHookDecorator` seam is
  preserved unchanged. 6 integration tests in `src/tools/scoped.rs` cover block,
  deny, update_input rewrite, success-observer side-effect, failure-observer
  side-effect, and the no-hooks regression guard.

### Added
- **Extension hooks — gateway, message, tool-persist & API-request events (hermes-inspired, Phase 3)**: wires the last deferred observer events. **Gateway lifecycle** — `GatewayStart` / `GatewayStop` fire (observers) from `aleph-server`'s boot and graceful-shutdown paths; the SIGTERM `process::exit` path bypasses `GatewayStop` (best-effort, documented). **Message events** — `MessageReceived` fires from `inbound_router::handle_message` after permission checks; `MessageSending` / `MessageSent` fire from the unified `ChannelRegistry::send` outbound chokepoint, carrying channel / conversation / size env vars (`MESSAGE_PREVIEW` is capped at 256 chars so message content never lands in an unbounded env var). **`ToolResultPersist`** — fires from `ScopedToolService` via the per-run `HookExecutor` when Layer-2 budgeting offloads an oversized tool result to disk. **`PreApiRequest` / `PostApiRequest`** — two new `HookEvent` variants fire around every LLM provider HTTP call in `http_provider`: `PreApiRequest` before the request, `PostApiRequest` after, carrying provider / model / protocol / token-usage / cost env vars; the streaming path wraps the delta stream so the post hook fires once with the accumulated `TokenUsage` on stream completion. All five events are observers (fire-and-forget, no blocking). The `ExtensionManager` process-global was relocated from `crate::gateway` into `crate::extension::manager_global` (re-exported for back-compat) so the Core providers layer reaches it without a reverse dependency; a shared `fire_global_observer` helper backs the gateway / channel / provider fire-sites. The in-process plugin handler (`HookConfig.handler`) and the `Notification` / `PermissionRequest` events were intentionally left out of scope (no dispatch surface / redundant with existing buses). 4 new unit tests plus extended `HookEvent` serialization/alias coverage for the two new variants.
- **Extension hooks — lifecycle, compaction & shell-hook consent (hermes-inspired, Phase 2)**: completes the hook surface left deferred after the Phase 1 tool-call wiring. (1) **Lifecycle hooks** — `gateway::execution_engine::run_loop` now splits `run_agent_loop` into a thin hook-firing wrapper plus `run_agent_loop_inner`: `BeforeAgentStart` fires as an interceptor (a `block:`/`deny:` aborts the run before any provider call), `AgentEnd` fires as an observer on every exit path with an `AGENT_OUTCOME` env var, and `SessionStart` fires (observer) on the first turn of a session with no prior history. `SessionEnd` fires (observer) from the `session.delete` handler once a session is removed. All fire sites live outside `src/harness/`, so the dumb loop stays free of lifecycle logic (R10). (2) **Compaction hooks** — `SessionCompactor::prepare_history` now takes the per-run `HookExecutor` snapshot and fires `BeforeCompaction` / `AfterCompaction` (observers) only when history is actually compacted, carrying `COMPACTION_*` stats (raw message count, summaries injected, post-compaction token estimate) via env vars. (3) **Shell-hook consent allowlist** — shell-command hooks (`HookAction::Command`) execute arbitrary code, so `HookExecutor` now gates them behind `~/.aleph/shell-hooks-allowlist.json` (`src/extension/hooks/consent.rs`): an un-approved command is skipped fail-safe and recorded `pending`; a per-hook `sha256(plugin\0command)` fingerprint means editing a hook revokes consent; the registry's `(mtime, len)` stamp is the cache fingerprint so a running server picks up CLI approvals without a restart; the file is guarded by an `fs2` lock + atomic rename. New `aleph hooks list|test|revoke|doctor` CLI surface manages it — `test` runs a hook command and offers to approve it, `doctor` flags pending entries and stale fingerprints. A `HookExecutor` with no consent gate (the default, used by tests) keeps running commands freely. 17 new unit tests across the consent module, executor gate, compaction firing, and CLI parsing/helpers.
- **REPL agent control panel** — six new TUI slash commands, hermes-agent-inspired, wired by connecting existing backend RPCs (zero new harness logic, R10). `/usage` shows session token totals plus a per-provider USD cost estimate; `/compress` triggers `session.compact` and reports before/after counts; `/stop` aborts the active run via `chat.abort`; `/undo` drops the last user+assistant turn; `/retry` undoes then re-submits the previous user message; `/tools off|new|all|verbose` switches the client-side tool-progress display filter. Aliases `/compact`→`/compress` and `/abort`→`/stop`. The status bar gains a `T:` tool-progress glyph. Provider pricing is a hardcoded 8-entry table (`interfaces/tui/src/tui/cost.rs`); unknown providers render `n/a` rather than a fabricated cost. Spec: [`2026-05-21-repl-agent-control-panel-design.md`](docs/superpowers/specs/2026-05-21-repl-agent-control-panel-design.md).
- **`SessionStore::truncate_messages` + `session.truncate` RPC** — new trait method that drops the tail of a session transcript, keeping only the first N messages (`keep_count=0` clears all, `keep_count>=total` is a no-op). SQLite (`SessionManager`) and file backends implement it; the default trait impl returns `Unsupported` so legacy/test stores need no change. The SQLite path deletes by `(timestamp, id)` threshold, syncs the FTS index, and keeps `sessions.message_count` consistent. RPC `session.truncate` sits in the Mutate gateway lane and powers the TUI `/undo` command. 6 boundary-case integration tests in `tests/session_truncate_messages.rs`.
- **Security wiring cycle** — `PiiSecretsGuardrail` is now a thin trait adapter over `Arc<RuntimeSecurityGuard>` (instead of a parallel security stack). Closes the four-month gap left by [`2026-04-16-runtime-security-orchestrator-design.md`](docs/superpowers/specs/2026-04-16-runtime-security-orchestrator-design.md), whose orchestrator was designed but never wired (the only prior production reference was a `let _ = …default_guard()` no-op). Three guardrail surfaces now map onto the orchestrator: `evaluate_input` → `process_outbound(None resolver)`, `evaluate_output` → `process_inbound`, `evaluate_tool_call` → `process_outbound(Some(resolver))`. Boot path constructs a vault-backed `VaultSecretResolver` over `SharedTokenManager` and threads it through `initialize_orchestrator` → `build_guardrail_registry`. Spec: [`2026-05-20-aleph-security-wiring-design.md`](docs/superpowers/specs/2026-05-20-aleph-security-wiring-design.md).
- **`{{secret:NAME}}` placeholder substitution at tool-call boundary** — LLM-generated tool args containing `{{secret:NAME}}` are now resolved by `RuntimeSecurityGuard::process_outbound` and the rendered JSON is returned via the existing `GuardrailDecision::Sanitize` variant (already honored by `src/harness/agent/guardrails.rs:82-87`). Placeholder substitution happens exclusively at the tool-call surface — never on user input or LLM-to-user output — so the LLM transcript shows the placeholder while the sandbox receives the resolved value. Unknown placeholder names produce `GuardrailDecision::Block` with the missing name in the reason.
- **`VaultSecretResolver`** (`src/secrets/vault_resolver.rs`) — `AsyncSecretResolver` impl wrapping `SharedTokenManager`. Maps `Ok(None)` → `SecretError::NotFound`, vault errors → `SecretError::Serialization`. 69 lines + 2 tests.
- **Byte-level secret scrub at sandbox edge** — new `scrub_secrets_bytes(&[u8], &[InjectedSecret]) -> ScrubResult` (`src/sandbox/scrub.rs`) runs `regex::bytes::Regex` patterns over raw stdout/stderr `Vec<u8>` before any `String::from_utf8_lossy` call, catching secrets surrounded by non-UTF-8 bytes that would otherwise be masked by `U+FFFD` replacement. Wired into `WorkspaceSandbox::execute` so every sandboxed command output is scrubbed. Whitelist via `InjectedSecret` hash comparison so intentionally-substituted placeholders bypass the scrub. Shares pattern source-of-truth with `LeakDetector` via the new `SECRET_PATTERN_SOURCES` constant. 3 sandbox-integration tests + 4 unit tests.
- **Persistent audit drain task** — new `spawn_audit_drain(rx, store) -> JoinHandle<()>` (`src/security/audit_drain.rs`) consumes the previously-discarded `mpsc::Receiver<AuditEntry>` from `RuntimeSecurityGuard::new_with_audit` and writes rows to the pre-existing `security_audit_log` SQL table via the new `SecurityStore::insert_audit_entry` helper. Spawned at boot, drained continuously, exits gracefully when the orchestrator drops. 2 unit tests verify persistence and graceful exit.
- **Input-side blocking — explicit 5-prefix coverage** — `PiiSecretsGuardrail::evaluate_input` now has explicit test coverage for pasted `sk-proj-`, `sk-ant-`, `AKIA`, `ghp_`, and `glpat-` keys (closes the historical worry that user paste might bypass the leak detector). `glpat-` was previously uncovered by str-side patterns; added to `LEAK_PATTERNS` to mirror the bytes-side `SECRET_PATTERN_SOURCES`.

### Fixed
- **aleph-server CORS origin parsing** — the WebChat static-file server's `tower-http` `AllowOrigin::predicate` callback receives the raw `Origin` header value (`&HeaderValue`), not a parsed URI; the existing `.host()` / `.scheme_str()` calls on it did not compile (`E0599`), breaking the `aleph-server` binary build. The header is now parsed into an `axum::http::Uri` before scheme/host inspection.
- **Fail-closed on `RuntimeSecurityGuard` internal error** — `PiiSecretsGuardrail::map_outbound`'s catch-all `Err(_)` arm previously fell through to `GuardrailDecision::Allow` (fail-OPEN on a security guardrail). Now returns `GuardrailDecision::Block { class: ErrorClass::Unexpected }` with `tracing::error!`. Operational issues like PII engine unavailable or lock poison can no longer silently weaken security.
- **TUI agent-trace events leaked debug decoration into chat content** — when a run streamed structured `AgentTrace` events, the TUI populated the user-facing assistant message via `present_agent_trace_event` (the protocol's *debug* presentation, `TuiDebug` preset). A `TextEmitted::Final` event therefore rendered in the chat body as `[Final text] iter 1: <model output>`, and `ToolSummary` rendered in the reasoning fold as `Tool summary: <summary>` — trace-panel labels, not primary-transcript text. `AppState::append_trace_debug_entry` now feeds the raw `text` / `summary` fields verbatim for `TextEmitted` and `ToolSummary`; turn/state/session lifecycle events still use the decorated presentation (they carry structured data, not authored prose). Regression introduced by the `cf2f8e236` trace-consumer rebuild.

### Removed
- Dead `let _ = RuntimeSecurityGuard::default_guard();` boot no-op in `src/security/mod.rs`. The function is still used by `PiiSecretsGuardrail::with_resolver` and is unit-tested.
- 4 vestigial "OpenClaw tool-policy" TODO markers in `src/executor/builtin_registry/{registry.rs, builder/constructor.rs}` (replaced with doc-comment pointers to SANDBOX.md and SECURITY.md describing the actual layered enforcement: `GuardrailRegistry` + `WorkspaceSandbox` + `ApprovalGate`).
- Old `PiiSecretsGuardrail::from_globals()` constructor (replaced by `with_guard_and_resolver` at the single production call site).
- 5 redundant `format_params_*` unit tests in `interfaces/tui` that re-tested the pure protocol function `aleph_protocol::summarize_tool_input` in isolation — fully covered by `shared/protocol`'s own `summarize_*` tests, and silently failing for ~6 weeks after the `cf2f8e236` protocol rebuild changed the function's output semantics without updating the cross-crate consumer tests. The TUI's actual use of the function stays covered by the `handle_agent_trace_*` / `handle_tool_lifecycle` integration tests.

### Added (continued)
- **OpenAI protocol — response_format wiring**: `ProviderConfig` now exposes
  `response_format: Option<ResponseFormat>` (variants `Text` / `JsonObject` /
  `JsonSchema { name, schema }`). Both Chat and Responses adapters honor it.
  Capability-gated by `ProviderCapabilities::supports_response_format` —
  enabled for OpenAI public and ChatGPT Codex endpoints, conservative `false`
  for all third-party OpenAI-compatible backends (opt-in flip in Cycle 3).
  Responses adapter's `text.format` slot fuses with config; variant verbosity
  preserved. Strict mode emitted automatically when endpoint supports it.
- **OpenAI protocol — parallel_tool_calls config knob**: `ProviderConfig` now
  exposes `parallel_tool_calls: Option<bool>`. When `None`, no `parallel_tool_calls`
  field is sent on the wire (server default applies).
- **Harness Stage 7 — Initialization Audit (#12)** — closes the 12-module roadmap. A static audit of `HarnessDeps` producers + consumers found 5 production wiring gaps on the gateway path: `harness_bridge.rs` was hardcoding `None` for `guardrails`, `fallback_llm`, `stall_config`, `consecutive_failure_cap`, and `turn_timeout` despite Stage 5a/5b/P0 rescue having shipped the seams. `AgentHarnessRunner` now exposes 5 new `pub` fields plumbing those values from boot to `HarnessDeps`; production behavior is unchanged (defaults stay `None`) but Phase-6 can now wire from `aleph.toml` without touching `harness_bridge.rs`. New `TraceSink::on_init_seam(stage, seam, configured)` trait method (default no-op, strictly additive — `NoopTraceSink` / `GatewayTraceSink` / test sinks compile unchanged) emits 9 events per session-start in declared order: `PromptBuilder`, `ChainContext`, `GuardrailRegistry`, `FallbackLLM`, `VerifierChain`, `StallConfig`, `ConsecutiveFailureCap`, `TurnTimeout`, `SkillPrefetcher`. `tracing::info!` adds production telemetry alongside. Three integration tests in `src/orchestrator/tests/init_audit.rs` lock the cold-start contract (event set, declared order, configured-flag truth table). Stage 6b (`JudgeVerifier` + `ComputationalVerifier`) now **permanently deferred** per R7 (LLM Sovereignty) + R8 (Everything-is-a-Tool) + R10 dumb-loop "5 nos" #3 + #4 — `src/verification/mod.rs` preamble hardened from "gated on waiver" to permanent prohibition. Master spec § Stage 7 / plan: `docs/superpowers/specs/2026-05-08-harness-stage7-init-audit-plan.md` / audit report: `docs/superpowers/specs/2026-05-08-harness-stage7-audit-report.md`.
- **Phase-6 — Stage 7 closure: config-driven harness assembly** — three new top-level `aleph.toml` sections (`[guardrails]`, `[stability]`, `[fallback_provider]`) finally light up the five `AgentHarnessRunner` Phase-6 placeholder fields that Stage 7 left at hardcoded `None`. Three private builders in `src/bin/aleph-server/commands/start/orchestrator_init.rs` (`build_guardrail_registry`, `build_fallback_llm`, `build_stability_triple`) read the config snapshot at boot and feed `guardrails: Option<Arc<GuardrailRegistry>>`, `fallback_llm: Option<Arc<dyn AiProvider>>`, `stall_config: Option<StallConfig>`, `consecutive_failure_cap: Option<usize>`, and `turn_timeout: Option<Duration>` end-to-end into `HarnessDeps` via `harness_bridge::run`. Behaviors: missing section ≡ `None` ≡ pre-Phase-6 main HEAD behavior; `[guardrails] enabled = true` wires the existing `PiiSecretsGuardrail::from_globals()` onto Input + Output + ToolCall surfaces (one struct, three traits); `[fallback_provider] provider = "<key>"` looks up `[providers.<key>]` and constructs the secondary via `create_provider`, with self-reference (ASCII-case-insensitive), unknown name, and `create_provider` Err all warn-and-disabled to `None`; `[stability]` independently controls `stall_timeout_secs` (defaults to `StallConfig::default().check_interval = 30s` when paired without `stall_check_interval_secs`), `consecutive_failure_cap`, and `turn_timeout_secs`. Activates Stage 5a guardrails, Stage 5b single-step `Transient` retry seam, and the P0 rescue trio (stall watchdog + failure cap + per-turn timeout) for the first time in production. R10: `src/harness/agent.rs` unchanged at 1520 lines. 13 builder tests in `commands::start::orchestrator_init::tests` (3 guardrails + 6 fallback + 4 stability) plus 3 cold-start `init_audit` non-regression tests lock the contract. Plan: `docs/superpowers/specs/2026-05-08-phase6-config-wiring-plan.md`.
- `ProviderConfig.stream_idle_timeout_secs` — per-event idle timeout for streaming responses (Anthropic protocol). Defaults to 60 seconds; `Some(0)` disables. Stalled streams now surface as `AlephError::Timeout` instead of hanging the request task.
- `CacheRetention { Off, Short, Long }` enum + `ProviderConfig.cache_retention: Option<CacheRetention>` field. Configures prompt-cache retention for the Anthropic protocol (other protocols ignore). Wired in the next commit; this commit only adds the type and threads `cache_retention: None` through the 5 production `ProviderConfig` literal sites.
- Anthropic protocol prompt cache wiring: `AnthropicProtocol::build_request` now injects `cache_control` at two breakpoints per request — the last text block of `system` and the last non-thinking block of the trailing user message. `ProviderConfig.cache_retention` controls behavior: `Off` skips injection entirely; `Short` (default for `api.anthropic.com`, off elsewhere unless explicit) uses Anthropic's 5-minute TTL; `Long` uses 1-hour TTL and appends `extended-cache-ttl-2025-04-11` to the `anthropic-beta` header. The `anthropic-beta` header is now an accumulator joining multiple beta tokens with `,` so OAuth + 1h cache coexist (`oauth-2025-04-20,extended-cache-ttl-2025-04-11`). Non-official hostnames with explicit `Long` opt-in are honored with a `tracing::warn!` audit log.
- **OpenAI Provider Cycle 1 — `stop_sequences` wiring** — `ProviderConfig.stop_sequences` (comma-separated, already consumed by Anthropic + template protocols) is now also forwarded to both OpenAI Chat (`body["stop"] = json!(vec)` inline in `build_request`) and OpenAI Responses (new `stop: Option<Vec<String>>` field on `ResponsesRequest` with `skip_serializing_if`). Parse: split-by-comma, trim, drop empty entries; absent / whitespace-only / comma-only configs produce no `stop` field on the wire. 5 Chat + 2 Responses tests cover happy path + edges.
- **OpenAI Provider Cycle 1 — SSE fixtures directory** — new `tests/fixtures/openai_sse/` containing plaintext SSE chunks captured for regression testing: `chat_completion_with_cache.txt`, `responses_with_cache_and_reasoning.txt`, `responses_with_reasoning_summary_parts.txt`. Fixtures `include_str!`d by unit tests so future wire-shape regressions are caught at the byte level.
- **OpenAI provider — `seed` parameter** — `ProviderConfig.seed: Option<u64>` is now wired into both Chat and Responses request bodies. Capability-gated: OpenAI, Codex, Azure, OpenRouter, and 6 OpenAI-compatible backends (DeepSeek, Groq, Mistral, Moonshot, Cerebras, xAI) emit the field; Local, Custom, and AnthropicPublic endpoints strip it.
- **OpenAI provider — `logprobs` and `top_logprobs` parameters** — `ProviderConfig.logprobs: Option<bool>` and `ProviderConfig.top_logprobs: Option<u8>` are now wired into the Chat request body. The Responses adapter surfaces `top_logprobs` on the request side. Capability-gated; emitted only on endpoints that document support. Response-side parsing of returned log-probability data is deferred to a future cycle.
- **OpenAI provider — `response_format` capability flip for 8 endpoints** — Azure, OpenRouter, DeepSeek, Groq, Mistral, Moonshot, Cerebras, and xAI endpoints are flipped to `supports_response_format = true`. `JsonSchema` variants degrade gracefully to `{type: "json_object"}` on endpoints that do not support strict schemas.
- **Anthropic protocol Cycle 4 — capability matrix module** — new sibling module `src/providers/protocols/anthropic/provider_policy.rs`, mirroring the OpenAI `provider_policy` pattern. Exposes `AnthropicEndpointClass` (`Official` for `api.anthropic.com`, `Custom` for everything else with a conservative URL-parse fallback), `AnthropicCapabilities` (7-bit profile: `cache_control` / `service_tier` / `metadata_user_id` / `output_config_effort` / `top_k` / `top_p` / `stop_sequences`), `AnthropicPolicy::apply` (single JSON-body mutation gate that strips capability-off fields and prunes emptied `metadata` / `output_config` parents), and `build_anthropic_policy` (one-shot builder). Official enables all 7 bits; Custom keeps the 3 protocol-standard sampling bits and drops the 4 Anthropic-only bits. Capability bits flow one-way (`base_url → AnthropicEndpointClass → AnthropicCapabilities`) — no `ProviderConfig` override field.
- **Anthropic protocol Cycle 4 — `metadata_user_id` + `effort` config fields** — `ProviderConfig.metadata_user_id: Option<String>` wires into `MessagesRequest.metadata.user_id` (Anthropic abuse-detection / rate-limit bucketing); `ProviderConfig.effort: Option<String>` wires into `MessagesRequest.output_config.effort` (`"low"` / `"medium"` / `"high"` / `"max"`). Both `#[serde(default)]` — old `config.toml` deserializes unchanged. Wired on Official endpoints, silently stripped on Custom by `AnthropicPolicy::apply`.
- **Anthropic protocol Cycle 4 — `MessagesRequest` field parity** — wire struct gains `top_p` / `top_k` / `stop_sequences` / `metadata`, all `Option<...>` with `skip_serializing_if` (absent fields = pre-Cycle 4 wire shape). New `Metadata` struct in `providers::anthropic::types` with a single optional `user_id` field — future-proof shape for additional metadata keys.
- **Anthropic protocol Cycle 4 — adaptive thinking for Claude 4.6/4.7** — `build_request` now emits `thinking: {type: "adaptive", display: "summarized"}` + `output_config.effort` for Claude 4.6/4.7 models instead of the deprecated `{type: "enabled", budget_tokens: N}`. The model picks its own per-turn budget; `ThinkLevel` maps to `effort` (`Minimal`/`Low → low`, `Medium → medium`, `High → high`, `XHigh → xhigh`). `xhigh` downgrades to `max` on 4.6 (which rejects `xhigh` with a 400). Pre-4.6 models keep the legacy `enabled` + `budget_tokens` path unchanged. Adaptive `effort` overrides any config-level `effort`.
- **Anthropic protocol Cycle 4 — `context-1m-2025-08-07` beta wiring** — new `AnthropicCapabilities.supports_context_1m` bit unlocks the 1M-context-window beta header. Auto-enabled for Azure AI Foundry (`*.azure.com`) and AWS Bedrock (`bedrock-runtime.*.amazonaws.com`) endpoints, gated to the Claude 4 model family. Default OFF on native `api.anthropic.com` — subscriptions without the long-context beta return HTTP 400 on ordinary short calls.

### Changed
- **OpenAI Responses — parallel_tool_calls no longer hardcoded**: The
  Responses adapter previously hardcoded `parallel_tool_calls: Some(true)`
  in `build_responses_request`. Now driven by `ProviderConfig.parallel_tool_calls`
  (default `None` → omit field). OpenAI public endpoint server default
  remains `true`, so observable behavior on OpenAI is unchanged. Compat
  backends will now receive `None` instead of forced `true`.
- `CacheControl` enum reshaped from unit variant `Ephemeral` to struct variant `Ephemeral { ttl: Option<EphemeralTtl> }`. Wire output unchanged for `ttl: None` (still `{"type":"ephemeral"}`); `ttl: Some(OneHour)` adds `"ttl":"1h"` for Anthropic 1-hour prompt cache. No production behavior change in this commit — all existing construction sites continue passing `cache_control: None`.
- **OpenAI provider — `response_format` strict-schema normalization** — `JsonSchema` variants in `response_format` now run through the same `normalize_strict_schema` helper used for tool definitions, injecting `additionalProperties: false` recursively. This brings `response_format` strict-mode schema normalization to parity with tool-definition strict mode.
- **Anthropic protocol Cycle 4 — `service_tier` wired from config** — `MessagesRequest.service_tier` was hardcoded `None` in `build_request`. Now wired from `ProviderConfig.service_tier` and capability-gated: emitted on Official, stripped on Custom by `AnthropicPolicy::apply`.
- **Anthropic protocol Cycle 4 — `top_p` / `top_k` / `stop_sequences` wired** — the Anthropic adapter previously ignored these three `ProviderConfig` fields. They now flow into `MessagesRequest` (`stop_sequences` parsed from the comma-separated config string, trimmed, empties dropped — matching the OpenAI-side convention). Capability-gated; Custom keeps all three on, but the gate is in place for future endpoint variants.
- **Anthropic protocol Cycle 4 — `effective_cache_retention` refactor** — simplified to resolve only `None → Short`. The host-level gate moved to `policy.capabilities.supports_cache_control`, which now wraps the entire `cache_control` injection + `extended_cache_ttl` block in `build_request`. The system block is built with `SystemBlock::text` (not `cached_text`) so `cache_control` is added solely via the gated injection path. Wire-level behavior is identical to pre-Cycle 4 — Custom hosts never receive `cache_control`. The `cache_retention = long` warning on third-party hosts is preserved for auditability.
- **Anthropic protocol Cycle 4 — provider-aware `anthropic-beta` filtering** — `interleaved-thinking-2025-05-14` and `fine-grained-tool-streaming-2025-05-14` were previously sent unconditionally to every Anthropic-protocol endpoint. `AnthropicCapabilities` gains three beta-gating bits (`supports_fine_grained_tool_streaming`, `supports_interleaved_thinking`, `supports_context_1m`) resolved per endpoint family. MiniMax's Anthropic-compatible endpoints (`api.minimax.io/anthropic`, `api.minimaxi.com/anthropic`) now have `fine-grained-tool-streaming` stripped — MiniMax 400s on tool-use messages when that beta is present. `build_beta_headers` consults the capability bits instead of hardcoding the two betas.

### Fixed
- **OpenAI Chat — max_completion_tokens for reasoning models**: Chat adapter
  now sends `max_completion_tokens` instead of `max_tokens` for `o1-` / `o3-` /
  `o4-` / `gpt-5` model families. Previously, any Aleph user configuring these
  models on a Chat endpoint received HTTP 400 from OpenAI; this is now
  resolved automatically based on model name. Responses adapter unaffected
  (already correctly uses `max_output_tokens`).
- Desktop tool screenshot quality (D6) — default JPEG quality lifted from 0.75 to 0.9 and the LLM prompt example now suggests `max_width:1920` (was 1280). Step 3 e2e revealed the LLM was reading UI text from a 1280px-wide JPEG q=0.75 capture, which compressed small text past legibility. Tool description also now explicitly warns against downscaling below 1920 when text matters. PNG output (the default when no format is specified) is unaffected.
- Memory compound-ingest no longer fails when raw memory concatenations exceed the embedding provider's 8192-token input cap. `RemoteEmbeddingProvider::call_api` now UTF-8-safely truncates each input to `EmbeddingProviderConfig.max_input_chars` (default 24000 chars — well under 8192 tokens for English BPE, ~16000 Chinese chars safely) before the API call. Logs at `tracing::debug!` per truncated input instead of hourly `compound ingest failed` WARNs. Configurable per provider; existing configs auto-migrate via `#[serde(default)]`.
- Default-provider hot-reload follow-up — `SemanticLlmMatcher` (A2A semantic agent routing) and `GroupChatExecutor` (coordinator LLM) now hold `Arc<dyn DefaultProviderHandle>` instead of frozen `Arc<dyn AiProvider>` snapshots. UI "Set as default" reaches both paths on the next match/round without a restart. Env-only `GroupChatExecutor` boot retains a `StaticDefault` snapshot (matches `SingleProviderRegistry` immutability).
- Default-provider hot-reload (Step 5) — UI "Set as default" now takes effect on the very next orchestrator turn without a server restart. Previously `AgentHarnessRunner.default_provider` was an `Arc<dyn AiProvider>` snapshot frozen at boot, so even though `MultiProviderRegistry::set_default()` updated the live registry, the harness kept dispatching to the cached pointer. Replaced the snapshot with `Arc<dyn DefaultProviderHandle>`; `MultiProviderRegistry` impls the trait directly and reads through its existing `RwLock` on each `pick_llm` call. Boot path prefers the live registry when available and falls back to `StaticDefault` for env-only single-provider mode. No new event subscription, no caching, no harness logic added (R7/R10 compliant). 2 unit tests lock the contract: `StaticDefault` is identity, and `MultiProviderRegistry::set_default()` reflects in subsequent `current()`.
- `MeteringProvider` now emits a `tracing::info!` event under target `aleph::provider_usage` alongside the existing `LoopTraceEvent::ProviderUsage` sink emit. Fields: `agent_id`, `provider`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_creation_tokens`, `thinking_tokens`. Closes the Step 2 observability gap where Anthropic cache hits required a probe TraceSink to verify; now visible in default stderr/stdout logs.
- `gateway/session_store` (sqlite_backend + legacy migration): coerce NULL `input_tokens` / `output_tokens` columns to 0 on read. Historical sessions / messages predating the column ADD now load cleanly instead of panicking with `Invalid column type Null at index: 11, name: input_tokens` during boot. Read-side fix only — no schema migration or backfill required.
- Anthropic protocol: `DeltaCollector::finish` now returns `Value::Object({})` for malformed tool-call JSON instead of `Value::String(raw)`, preserving the dispatcher invariant that tool arguments are JSON objects. Also removes the unused `last_model` field from `AnthropicProtocol`, replaced by `stream_idle_timeout_secs: Arc<AtomicU64>`.
- **Anthropic protocol Cycle 4 — OAuth Bearer authentication** — Anthropic OAuth tokens (`sk-ant-oat*`, JWT `eyJ*`, Claude Code `cc-*`) were detected only to append the `token-restricted` beta header, but the request still authenticated via `x-api-key` — Anthropic rejects OAuth tokens on that header with HTTP 401. `build_request` now routes OAuth tokens through `Authorization: Bearer` plus the Claude Code identity headers (`User-Agent: claude-cli/<version>`, `x-app: cli`) that Anthropic's OAuth infrastructure requires, and emits the full OAuth beta stack (`claude-code-20250219`, `oauth-2025-04-20`, `token-restricted`). Console API keys (`sk-ant-api*`) and third-party keys keep `x-api-key`. New `is_oauth_token` helper mirrors hermes-agent's positive-prefix detection (`sk-ant-api` explicitly excluded).
- **Anthropic protocol Cycle 4 — tool schema union stripping + dedup** — tool `input_schema` is now sanitized before send: top-level `oneOf` / `allOf` / `anyOf` keywords are stripped (Anthropic's tool validator rejects them with HTTP 400 — schemars commonly emits these for Rust enums; a fallback `type: "object"` + empty `properties` keeps the schema valid), and duplicate tool names (including post-sanitization collisions) are dropped with a `tracing::warn!` instead of failing the whole request with a 400. Both defenses convert hard API failures into recoverable warnings.
- OpenAI Responses strict-mode normalizer now handles multi-type JSON schemas. `["null", X]` (Option<T>-shaped) is rewritten to `anyOf` with sibling-keyword preservation; other multi-type shapes (e.g., the 7-type "any-value" emitted by schemars for `serde_json::Value`) trigger a per-tool strict downgrade via `tracing::warn!` audit log. Fixes 400 `invalid_function_parameters` errors on tools containing `serde_json::Value` fields (e.g., `desktop` tool's `actions: Option<Vec<serde_json::Value>>`).
- OpenAI Responses tool schemas now go through a pre-flight sanitizer in `build_tools`. (1) Top-level `type: "object"` is injected when missing. (2) Top-level `oneOf`/`anyOf` from schemars' internally-tagged enum output is flattened: each branch's `properties` is merged into root properties (first-branch-wins on collision), `required` becomes the intersection across branches, and the union keyword is dropped. Top-level `allOf`/`enum`/`not` are stripped (rare keywords). Fixes 400 `schema must have type 'object' and not have 'oneOf'/'anyOf'/'allOf'/'enum'/'not' at the top level` on internally-tagged enum tools (`remember`, `self_config`, `note_schema`, `cron_manage`, `user_profile`). (3) In the non-strict path (Custom endpoints), multi-type fields are rewritten lenient-mode: `["null", X]` → `anyOf` (lossless), other multi-types → `type` keyword stripped (lossy but accepted). Resolves the remaining `invalid_function_parameters` 400s exposed during Step 3 e2e against T8Star.
- **OpenAI Provider Cycle 1 — canonical `TokenUsage` cache + reasoning fields** — `cache_read_tokens` and `thinking_tokens` are now extracted from OpenAI Chat and Responses usage payloads (`prompt_tokens_details.cached_tokens` + `completion_tokens_details.reasoning_tokens` on Chat; `input_tokens_details.cached_tokens` + `output_tokens_details.reasoning_tokens` on Responses). Previously hardcoded to `None`. `MeteringProvider` tracing logs now show real cache-hit and reasoning-token counts on OpenAI traffic, closing the Step 2 observability gap for OpenAI providers. `cache_creation_tokens` correctly remains `None` (OpenAI surfaces no cache-write metric — only Anthropic does).
- **OpenAI Provider Cycle 1 — Chat finish_reason mapping** — expanded to cover `function_call` (legacy tool-call shape) → `ToolUse`, `content_policy_violation` → `MaxTokens`, and `incomplete` → `MaxTokens` (aligns with Responses-side `is_incomplete` handling). Unknown finish_reason values now `tracing::warn!` and fall back to `EndTurn` instead of silent `None` (which could be interpreted as "stream not done" and hang the loop driver).
- **OpenAI Provider Cycle 1 — Responses `reasoning_summary_*` event coverage** — the four `StreamEvent::ReasoningSummary*` variants (`PartAdded` / `TextDelta` / `TextDone` / `PartDone`) are now explicitly matched. `TextDelta` continues to emit `ProviderDelta::ThinkingDelta` (unchanged behavior); the other three emit `tracing::debug!` markers under target `aleph::openai_responses_sse` instead of being silently dropped by the `_ => {}` catch-all. Per R10 YAGNI no new canonical Delta variant is introduced — there are no consumers for part boundaries today.

## [26.5.7]

### Added
- **Harness Stage 6a — Verification turn-level seam (#10, partial)** — new `TurnVerifier` trait + `VerifierChain` registry land in `src/verification/`. `StopHookVerifier` migrates the existing pre-stop shell-hook flow 1:1 (only fires when `stop_reason.is_some()`); `ToolLoopVerifier` closes the master roadmap § 1.4 P1 gap by detecting N consecutive identical tool calls with no thinking text (default threshold = 5, `args_hash` via `DefaultHasher` of canonical `serde_json` bytes). A single callsite in `AgentHarness::run_turn_internal` now covers both pre-stop and mid-turn checks, replacing the legacy `evaluate_stop_hooks` helper. The harness `run` loop holds an 8-slot `VecDeque<ToolCallSummary>` ring buffer so the verifier can detect repetition without re-scanning event history. `HarnessDeps.stop_hooks` retired in favour of `HarnessDeps.verifier_chain: Option<Arc<VerifierChain>>`; orchestrator boot wraps shell hooks into a `StopHookVerifier` and adds a default `ToolLoopVerifier`. `MAX_VERIFIER_VETOS = 10` cap and `[verifier veto]` injected message preserve the pre-6a safety net. 16 acceptance tests added (4 chain semantics + concurrency hammer, 3 stop-hook adapter, 6 tool-loop edges, 1 harness death-loop integration end-to-end). `agent.rs` net trim: 1500 → 1499 lines (under R10 cap). Stage 6b (`JudgeVerifier` + `ComputationalVerifier`) explicitly deferred pending an explicit redline waiver in `src/verification/mod.rs`. Master spec § Stage 6 / 6a plan: `docs/superpowers/specs/2026-05-06-harness-stage6a-turn-verifier-plan.md`.
- **Harness Stage 5b — Guardrails Pipeline (ToolCall + Fallback) — closes module #9** — wires the third guardrail callsite at `AgentHarness::act` so each `tools.execute` is gated by `GuardrailRegistry::evaluate_tool_call(name, &args)`. `Block` skips ONLY the offending call (`ToolError` event persisted, `on_safety_block` fired, `ToolCallCompleted` trace emitted with `retryable: false`); the rest of the batch dispatches normally. `Sanitize` re-parses the replacement JSON into `call.arguments` and proceeds. The new `HarnessDeps.fallback_llm: Option<Arc<dyn AiProvider>>` seam adds single-step fallback at the Think-phase LLM call: when the primary returns `ErrorClass::Transient`, the harness retries once via the fallback provider. On fallback success the previously dead `HarnessCallback::on_model_fallback(reason, fallback_name)` fires; if both fail the primary error surfaces. `parent_cancel` and `turn_timeout` race over both calls. The harness extracts two tight helpers (`apply_tool_call_guardrail`, `race_llm_call`) so `agent.rs` stays at exactly 1500 lines (R10 cap). 6 new acceptance tests — 3 ToolCall integration (Block-skip-batch, Sanitize-rewrites-args, Allow-passthrough), 2 fallback (Transient-engages, no-fallback-still-propagates), 1 concurrency hammer (`evaluate_tool_call` vs `disable_all`). Master roadmap module #9 now ✅ Shipped.
- **Harness Stage 5a — Guardrails Pipeline (Input + Output)** — new `src/guardrails/` module wires three trait surfaces (`InputGuardrail`, `OutputGuardrail`, `ToolCallGuardrail`) consulted by `AgentHarness` (master spec module #9). 5a ships the Input + Output callsites in `run_turn_internal`: input guardrail inspects the latest `UserMessage` in the tail before prompt assembly (Block ends the turn as `Done` via `on_safety_block`; Sanitize rewrites the in-memory event clone — original session log preserved for audit). Output guardrail inspects model text before `AssistantMessage` is persisted (Block returns `HarnessError::Llm`; Sanitize rewrites text in place). `GuardrailRegistry` aggregates all three surfaces with an `AtomicBool` `disable_all()` runtime kill-switch (high-risk rollback per master spec § Stage 5). `PiiSecretsGuardrail` ships as the first real consumer, wrapping the existing `PiiEngine` and `SecretLeakDetector` (secret-leak first, PII second). `HarnessDeps.guardrails: Option<Arc<GuardrailRegistry>>` is the harness-side seam (zero-cost noop when `None`); `SpawnerBase` + `AgentRuntime` propagate the registry so subagents inherit by default. 19 acceptance tests added (15 module-level + 4 harness-level integration covering input Block/Sanitize and output Block/Sanitize). Decisions reuse Stage 1 `ErrorClass` for unified retry vocabulary. Stage 5b will wire the `ToolCall` callsite + `on_model_fallback` ProviderRegistry fallback list. Master spec § Stage 5.
- **Harness Stage 4 — Subagent ChainContext Wiring** — `AgentHarness::chain_context()` accessor exposes the harness's position in the subagent chain (master spec module #11). The `Harness` trait gains an `Option<&ChainContext>` default method (`None`) so non-`AgentHarness` impls (test mocks, alternative drivers) stay ergonomic; `AgentHarness` overrides to `Some(...)`. `HarnessDeps` gains a `chain_context: ChainContext` field (defaults to a fresh root chain). `subagent_spawner::spawn` writes the descended `child_chain` into the inner harness's deps so each nested level reports the correct depth/chain_id without re-derivation. 6 acceptance tests added (root default, injected level-2, 3-layer id+depth invariants, trait default `None`, `&dyn Harness` dispatch, 16×1000 concurrent reader smoke). Master spec § Stage 4.
- **Harness Stage 3 — Prompt Assembly Seam** — `PromptBuilder` trait + `DefaultPromptBuilder` ship as the single seam through which `AgentHarness` produces the per-turn `Vec<UnifiedMessage>`. `DefaultPromptBuilder` is byte-equivalent to the previous private `build_prompt`; downstream stages (#11 Subagent, #10 Verification) can inject custom builders without patching `agent.rs`. `TurnContext` input struct carries the per-turn event slice + tail boundary into `PromptBuilder::assemble`. `HarnessDeps` gains a `prompt_builder: Arc<dyn PromptBuilder>` field; all 21 construction sites pass `Arc::new(DefaultPromptBuilder)`. 3 golden tests + a 64-case proptest verify behavior; `agent.rs` shrinks from 1375 → 1237 lines. Master spec § Stage 3.
- **Harness per-turn timeout** (`HarnessDeps.turn_timeout`) — wraps each Think (LLM `process()`) and Act (tool `execute()`) phase with `tokio::time::timeout` / `tokio::select!`. New `TurnPhase::{Think, Act{tool_name}}` enum identifies which phase hung in `HarnessError::StalledTurn { phase, elapsed }`. Parent `CancellationToken` always wins over the timeout via `tokio::select! { biased; cancel; sleep; llm_fut }`.
- **Harness TraceSink fire points** wired across the full turn lifecycle: `TurnStarted`, `TurnStateEntered { Think | Act }`, `TextEmitted`, `ToolCallStarted`, `ToolCallCompleted` (success / error / skipped), `TurnCompleted`, `SessionCompleted`. The `LoopTraceEvent` schema in `src/harness/trace.rs` is now live; existing `GatewayTraceSink` (mpsc-backed) consumes events without code change. The `emit()` helper short-circuits when `trace_sink` is `None` — closure is never invoked, zero allocation.
- **Harness consecutive-failure cap** (`HarnessDeps.consecutive_failure_cap`) — terminates the loop with `Done { hit_limit: true }` after N consecutive turns where every tool call failed, preventing infinite retry on permanently-broken tools.
- **Harness Stage 2 — Tools Surface Unification** — `ToolService::dispatcher_schema()` exposes the cached dispatcher-form tool list as `Arc<[ToolDefinition]>`. Per-turn LLM tool list is now an O(1) `Arc::clone` instead of an O(n) `Vec` allocation; cache invalidates on `ToolRegistry` snapshot pointer change for `CoreDispatch` and on MCP `poll_changes()` generation bump for `ScopedToolService`. Middleware decorators delegate without per-layer caching. New helper `to_dispatcher_form()` is the single source of truth for the loop→dispatcher conversion. 4 acceptance tests added (2 integration + 1 perf assertion + 1 property test with 64 cases). Master spec § Stage 2.

### Fixed
- **Harness tool-error abort** — tool failures inside `act()` no longer abort the entire session via `HarnessError::Tool`. Errors are now persisted as `SessionEvent::ToolError` and surfaced to the next Think as `tool_result.is_error=true` (matching Claude Code recoverable-error semantics). The model decides whether to retry, switch tactics, or stop — the harness no longer makes that decision.
- **Harness stall false-positives** — `StallTracker::record_activity` now fires after Think completes (post `AssistantMessage` emit) and after each tool execute, in addition to the existing top-of-loop call. Eliminates spurious `Stalled` errors during legitimate long Think phases.

### Removed
- **Harness private `build_prompt`** — function (was `agent.rs:846`) deleted; body lives in `DefaultPromptBuilder::assemble`. `resolve_tool_name` helper retired; `parse_tool_use_block` moved from `agent.rs` to `prompt.rs` (sole production caller is `DefaultPromptBuilder`).
- **Harness `src/harness/stall.rs` module file** — contents folded into `deps.rs` next to `HarnessDeps.stall_config`. R10 file count rebalanced for the new `prompt.rs` seam.
- **Harness per-turn schema conversion** — `agent.rs` no longer rebuilds `Vec<DispatcherToolDefinition>` every Think turn; replaced by cached `Arc<[T]>` from `ToolService::dispatcher_schema()`.

## [26.4.27]

### Added
- **Canvas Agent Selector** — `AgentSelectorBar` component in the Memory (Canvas) panel, letting users switch between agents. Graph API handlers (`graph.query`, `graph.neighbors`, `graph.node_detail`, `graph.search`) now accept an optional `agent_id` parameter to filter results per agent.
- **Elastic Node Drag** — Full press-move-release-cancel interaction on the knowledge graph with spring-physics animation (`Spring2D` critically-damped primitive), drag overlay rendering (stretched edge + glow), and idle/settle transitions.
- **Gateway Instanceless Channel Tests** — Contract-level tests for 17 channels without requiring live credentials: Slack, Email, Webhook, IRC, Mattermost, Matrix, Discord, LINE, CLI, Signal, XMPP, Nostr, MS Teams, QQ, Feishu, iMessage, WeChat, WhatsApp.

### Changed
- **Memory `agent_id` Unification** — Replaced local hardcoded `"default"` constants across six modules (`memory_explore`, `memory_browse`, `memory/events`, `memory/dreaming`, `memory/notes`, `memory/scheduler`) with `routing::DEFAULT_AGENT_ID`. Legacy default-agent notes are auto-migrated into the `main` agent directory on startup.
- **Canvas Detail Slider** — Re-fold behavior now uses `Effect`-refold + `NavController::retarget` tween for smoother animation. Deferred item loading fixed.
- **Canvas Navigation** — `NavController::retarget` added for slider re-fold tweens. Pre-fetch cache keys simplified to raw id-only for deduplication.
- **Canvas View Lifecycle** — View state fully resets when `agent_id` changes. Initial agent fetch is gated on WebSocket connection readiness.

### Fixed
- **Config Secrets Vault Routing** — `config.patch` now intercepts `channels.*` patches and routes secrets (`bot_token`, `api_key`, etc.) into the vault before persisting config, preventing plaintext leakage in `config.toml`.
- **Config Handler Registration** — Removed duplicate `config.get` registration in server builder that caused a boot-time handler collision.

## [26.4.23]

### Changed
- **Idiomatic Rust refactoring:** Systematic cleanup across core modules to eliminate technical debt and improve code purity.
  - `core/capability`: `sort_by_priority` now accepts `&mut [Capability]` to avoid unnecessary ownership transfer.
  - `utils/pii`: Chain regex replacements via `Cow<str>` without 7 intermediate `String` allocations.
  - `security/ssrf`: Extract `validate_url_common` to eliminate ~60 lines of duplicated validation logic between sync and async URL validators.
  - `session/streaming`: Remove dead `run_with_progress` code and unused `ToolProgress` import.
  - `providers/openai_chat`: Drop redundant `sanitize_tool_name` wrapper.
  - `thinker/soul`: Introduce `NonEmptyOr` trait to collapse repetitive `if-is-empty` branches in `merge_with`.
  - `builtin_tools/web_fetch`: Reuse `utils::text_format::truncate_text` instead of inline char-count truncation.
  - `extension/mod`: Remove unused `tool_registry()` getter.

### Fixed
- **Windows build:** Fix 20 compilation errors in `src/sandbox/platforms/windows/`:
  - `appcontainer.rs`: Replace corrupted `0026self` with `&self`; remove non-existent `windows-sys` 0.59 APIs (`CreateAppContainerProfile`, `DeleteAppContainerProfile`, `DeriveAppContainerSidFromAppContainerName`)
  - `driver.rs`: Use `profile.proxy_ports` instead of undefined `proxy_ports`
  - `token.rs`: Import `OpenProcessToken` from correct path, use `null_mut()` for HANDLE initialization
  - `wfp.rs`: Replace `windows_sys::Win32::Foundation::GUID` with `[u8; 16]`
  - `job.rs`: Use `is_null()` for HANDLE comparison, ignore `CloseHandle` result
  - `mod.rs`: Make `driver` module public
- **Loom concurrency tests:** Fixed compilation errors when running with `--features loom`. Resolved `MutexGuard` export, static initialization issues, and type mismatches between `std::sync` and `loom::sync` primitives.
- **Telegram config parsing:** Add `#[serde(default)]` to `groups` field in `TelegramAccountConfig`, fixing V2 config parsing when groups are omitted.
- **Memory ingest snapshot:** Update snapshot to match updated prompt text.

## [26.4.22]

### Added
- **Config migration for vector_db:** Auto-migrates `vector_db = "lancedb"` to `vector_db = "sqlite-vec"` on server boot. Migration is logged and persisted back to `config.toml`.
- **Phase 7 — SubagentTool → Harness migration:** Complete migration of subagent execution from legacy `AgentLoop` to `Harness`-based spawner. Includes `AllowlistToolService` decorator, `HarnessDeps` injection, and deletion of `src/agent_loop/` directory.
- **Phase 6 — Gateway Orchestrator flip + cleanup:** Routed Gateway chat through `Orchestrator::dispatch`, deleted `factory.rs` and `integration_probe.rs`, relocated `AgentRuntime` to `agents/runtime.rs`, rewrote architecture docs for Orchestrator-driven teams.
- **Phase 5 — Orchestrator & Flow Composition:** Full Orchestrator implementation with FlowSpec TOML parsing, FlowRegistry with ArcSwap hot-reload, 7-step dispatch pipeline, HarnessRunner bridge, and cross-module e2e tests.
- **Phase 4b — Harness Think→Act loop:** Implemented `Harness` trait with Think phase (tool planning), Act phase (tool execution + turn reconstruction), and `SessionDriver` trait for session runtime integration.
- **Kimi for Coding preset:** Added preset configuration with model optimizations for Kimi provider.

### Changed
- **Config loading:** Pre-process TOML migrations (`migrate_mcp_builtin_in_toml`, `migrate_vector_db_in_toml`) now run before parsing and save migrated config back to disk.
- **Architecture docs:** Rewrote `AGENT_SYSTEM.md`, `MULTI_AGENT_SYSTEM.md`, and `ARCHITECTURE.md` to reflect Orchestrator+Harness runtime topology.
- **ACP naming:** Renamed `AcpHarness` family to `AcpAdapter` to free "Harness" for managed-agents meaning.

### Fixed
- **Config persistence:** Migrated values are now written back to `config.toml` instead of only existing in memory.

## [26.4.18]

This release lands a 10-day stretch of foundational refactors: the memory layer is rebuilt around an 8-spec architecture (notes-as-source-of-truth, L0 raw memory, compound LLM ingestion, strategy-driven Dream Daemon), seven new chat channels ship (WeChat, QQ, Discord, Matrix, Signal, LINE, WhatsApp) plus a structured Telegram v2, the runtime security orchestrator goes live, browser automation migrates from MCP to Playwright CLI, the agent loop adopts Claude Code-style preflight + recovery cascades, and the provider/vault path is hardened end-to-end. 566 commits since v26.4.8.

### Memory Layer — 8-Spec Evolution
  `src/session/driver.rs` introduces the `SessionDriver` trait as the
  seam between session runtime and whatever drives its turns. `AgentHarness`
  (Phase 4b) implements it via blanket delegation to `Harness::run`.
  `ALEPH_HARNESS_V2=1` is read at startup and logged for discoverability;
  production driver swap lands in Phase 5 alongside the Orchestrator bridge.
  Integration-test coverage: `tests/harness_run_e2e.rs` (harness run loop)
  and `src/harness/tests/driver.rs` (SessionDriver delegation).
- **Sandbox subsystem (Phase 3):** `src/sandbox/` introduces the agent-level
  `Sandbox` trait and `WorkspaceSandbox` implementation — lazy per-session
  workspace under `~/.aleph/workspaces/{hash(session_id)}/`, strict
  capability baseline (`SandboxCapabilities`: fs_read / fs_write / network /
  spawn_subprocess), `ApprovalGate`-arbitrated escalation with per-session
  grant cache, `capability_ledger` tracing audit. Composed via
  `build_sandbox(cfg, driver, approval)` factory; disabled-sandbox path
  returns a fail-fast `NoopSandbox`. Spec:
  `docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md`.
- **`SESSION_ID` task-local:** `crate::sandbox::context::SESSION_ID` scoped by
  `invoke_with_session_trace` so exec-class tools discover the current
  session via `current_session()` without touching the `AlephTool` trait
  signature.
- **`LayeredPermissionResolver` + `AgentPermissionFilter`:** backfills the
  Phase 2 placeholder `SmartFilter` with a concrete policy-backed resolver
  over merged two-tier (global + per-agent, most-restrictive-wins)
  `ToolPermissionsConfig`. Live-reloadable via `ArcSwap`. Boot-time wiring
  attaches the global config + an empty per-agent default to the
  `PermissionLayer` via `build_tool_service_with_handles`; per-agent
  overrides plug in at Phase 4 session activation.
- **`SandboxConfig`:** `[sandbox]` TOML section with `workspace_root`,
  `enabled`, `default_timeout_seconds`, `max_output_bytes`. Defaults
  preserve existing behaviour; `enabled = false` switches the subsystem to
  `NoopSandbox` for tests / CI.
- **End-to-end integration test** (`tests/sandbox_capability_approval.rs`)
  covering the six-step pipeline via the public `build_sandbox` surface:
  strict-cap bypass, network-elevated approve, spawn-denied error,
  per-session approval cache. 4/4 passing.
- **Docs:** `docs/reference/SANDBOX.md` reference (architecture, pipeline,
  capabilities, task-local, tool consumption pattern, testing pattern);
  GLOSSARY entries for `WorkspaceSandbox`, `OsSandboxDriver`,
  `OsSandboxDriverTrait`, `SandboxCapabilities`,
  `LayeredPermissionResolver`, `AgentPermissionFilter`; ARCHITECTURE
  sandbox paragraph + module summary row.
- **Tool Service façade:** `src/tools/service.rs` exposes a single
  `execute(name, input) → Result<ToolOutput, ToolError>` across builtin, MCP,
  and extension sources. Five-layer decorator chain (audit / permission /
  context-rule / timeout / core) replaces scattered policy logic. `SmartFilter`
  and `ContextRule` trait surfaces established for future policy plug-in.
  `ArcSwap`-backed `ToolRegistry` supports MCP/extension hot-reload. Phase 2
  of the managed-agents refactor. Phase 3 adds
  `build_tool_service_with_handles` + `ToolServiceHandles` so boot wiring can
  attach a live `SmartFilter` / `Approver` to `PermissionLayer` without
  downcasting.
- **Session-event tracing helper:** `crate::session::invoke_with_session_trace`
  bundles `ToolService::execute` with automatic `ToolCallRequested` /
  `ToolResult` / `ToolError` / `ToolCallDenied` event emission into the session
  log. Ready for Phase 4 when Harness rewrites the main loop.

### Changed
- **`SandboxManager` → `OsSandboxDriver`:** renamed
  (`src/exec/sandbox/executor.rs`) to reflect OS-level role and free the
  name for the agent-level `Sandbox` trait. Now implements
  `OsSandboxDriverTrait` so `WorkspaceSandbox` can drive it.
- **`SmartFilter` is no longer a placeholder:** production
  `PermissionLayer` now wraps `LayeredPermissionResolver`; the Phase 2
  `ScriptedFilter` stub is test-only.
- **Exec-class tools route through `Arc<dyn Sandbox>`:** `code_exec` and
  `bash_exec` hold an optional `Arc<dyn Sandbox>` attached via
  `with_sandbox` at boot and call `sandbox.execute(SandboxCommand)` instead
  of `Command::new(...)`. Unconfigured tools fail-fast with a structured
  error rather than bypassing sandboxing.
- Extension tool registration now routes into `ToolRegistry` at boot via
  `ExtensionManager::set_tool_registry`; MCP wiring setter is in place (waits
  for central `McpClient` instantiation in a future phase).

### Fixed
- **H3 — `LayeredPermissionResolver` now actually wired.** Phase 3 shipped
  the resolver but never attached it to `PermissionLayer`, so every call
  defaulted to `Classification::Allow`. Boot wiring in `aleph-server` now
  calls `PermissionLayer::set_smart_filter` +
  `PermissionLayer::set_approver` so global `[policies.tool_permissions]`
  gates tool invocations. Approver is the same `ApprovalGate` shared with
  the Sandbox capability-escalation path.
- **H5 — `OsSandboxDriver::run` honors env, timeout, and max_output_bytes.**
  Previously `_env`, `_timeout`, and `_max_output_bytes` were bound with
  underscore (silently dropped). `CodeExecTool`'s carefully-built PATH
  injection never reached the sandbox child and the 60s
  `SandboxConfig::default_timeout_seconds` was masked by the legacy 300s
  pin. Now: `SandboxCommand.env` is threaded into the subprocess via
  `Command::envs`, timeout is rounded up to u64 seconds (min 1s, non-zero
  subseconds round up) and flows through the adapter's internal
  `tokio::time::timeout`, and stdout/stderr are clamped to
  `max_output_bytes` per stream on UTF-8 char boundaries with the
  `truncated` flag set. stdin remains unimplemented; `OsSandboxDriver::run`
  now logs a warn when `stdin: Some(_)` is passed rather than silently
  dropping bytes.

### Known Limitations
- **H1 — `ApprovalGate` still has no real `ApprovalRequester` transport.**
  The shared gate is constructed at boot and wired into both the
  Sandbox and `PermissionLayer`, but the existing `ChannelApprovalBridge`
  (Telegram / Discord inline-keyboard approvals) uses the legacy
  `ApprovalRequest { command, cwd, session_key, ... }` shape and is not
  trivially adaptable to the tool-level `ApprovalRequester { tool_name,
  reason }` trait. Until that adapter lands,
  `ApprovalGate::request_approval_for_tool` falls back to `Denied` —
  elevated-capability sandbox escalations and Ask-tier tool calls are
  rejected by policy rather than hanging. Baseline-capability commands
  and globally-Allow-tier tools are unaffected. A `tracing::warn!` at boot
  surfaces this state so it is not silent. Follow-up: Phase 4 introduces
  a `ToolApprovalRequester` adapter over `ChannelApprovalBridge`.
- **H4 (preview) — exec-class tools own the sandbox-side prompt.** When
  `bash` / `code_exec` are classified `Ask` in `[policies.tool_permissions]`,
  `PermissionLayer` and `WorkspaceSandbox` would each request approval
  independently, yielding two prompts. The current global-default `Allow`
  for exec tools avoids this in practice; Phase 4 will introduce a
  first-class exclude-list on `LayeredPermissionResolver` so exec-class
  tools skip the middleware prompt and defer to sandbox capabilities.

### Added
- **Session Service foundation:** `src/session/` introduces `SessionService` + `InProcessActorSessionService` — an append-only event log per session, one tokio actor per session, SQLite-backed via a new `session_events` table (`migrate_add_session_events`), crash-recoverable through `wake(session_id)`. Public surface: `attach` / `emit_event` / `get_events` / `subscribe` / `wake` / `detach`. Event schema lives in `src/session/events.rs` (`SessionEvent`, `#[non_exhaustive]`). Read-side helper `project_messages` in `src/session/projection.rs` provides a classic message-history view over raw events. Phase 1 of the managed-agents refactor ([roadmap](docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md)).
- **Dual-write shim:** `src/session/shim.rs` mirrors every `SessionManager` append into `SessionService` so `session_events` stays populated in parallel with the legacy `messages` table. The shim is removed in Phase 6 when Gateway `session.*` RPC migrates to `SessionService` directly.
- **Crash-recovery integration test** (`tests/session_wake_recovery.rs`) exercising the `wake(session_id)` replay path.
- **Docs:** `docs/reference/SESSION_SERVICE.md` reference documentation; ARCHITECTURE.md cross-link; GLOSSARY.md "Session" entry updated.

### Changed
- **ACP:** renamed `AcpHarness` trait and type family (`HarnessMode`, `HarnessConfig`, `AcpHarnessEntry`, `GenericAcpHarness`, `CustomHarness`) to the `AcpAdapter` family, freeing "Harness" for its Anthropic managed-agents meaning (the Think→Act loop) in upcoming phases. Module paths renamed: `src/acp/harness.rs` → `adapter.rs`, `src/acp/harnesses/` → `adapters/`. Legacy `[acp.harnesses]` TOML key remains accepted via `#[serde(alias = "harnesses")]` — no user config changes required. Phase 0 of the managed-agents refactor ([roadmap](docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md)).
- **SessionManager is now a compatibility layer** for Gateway `session.*` RPC (see module docstring in `src/gateway/session_manager/mod.rs`). No behavior change yet — Phase 6 removes it. `agent_loop` was already decoupled from `SessionManager` before this work began; no migration was required inside the loop. Phase 4 will port `agent_loop` onto `SessionService` directly.

### Added (pre-existing)
- **Docs:** canonical glossary at `docs/reference/GLOSSARY.md` aligning Harness / Sandbox / Session / Tools / Orchestrator / AcpAdapter with Anthropic's managed-agents paradigm.

## [26.4.18]

This release lands a 10-day stretch of foundational refactors: the memory layer is rebuilt around an 8-spec architecture (notes-as-source-of-truth, L0 raw memory, compound LLM ingestion, strategy-driven Dream Daemon), seven new chat channels ship (WeChat, QQ, Discord, Matrix, Signal, LINE, WhatsApp) plus a structured Telegram v2, the runtime security orchestrator goes live, browser automation migrates from MCP to Playwright CLI, the agent loop adopts Claude Code-style preflight + recovery cascades, and the provider/vault path is hardened end-to-end. 566 commits since v26.4.8.

### Memory Layer — 8-Spec Evolution

This is the headline refactor. The legacy `facts` table and the `MemoryStore` trait are gone. Notes are now the single source of truth, stored under `~/.aleph/memory/{raw,note}/{agent_id}/{category}/`. Six new dream stages, two new ingestion paths, and three new memory tools ship with it.

#### Added
- **Working Memory Assembler (Spec 1)** — HybridAssembler with concurrent candidate gathering, LLM rerank, deterministic skeleton fallback packer; MemoryEnvelope v1.0 with markdown/xml/json renderers; `assembly_logs` writer; UTF-8 safe truncation; integration tests across 6 paths.
- **L0 Raw Memory Capture Hooks (Spec 1)** — RawMemory + RawMemoryStore trait, `raw_memories` SQLite table; G1 pre-compress, G2 delegation, G3-A session-disconnect hooks; `on_capture` filter at every insert site; SessionCompactor and TranscriptIndexer dual-write to raw_memories.
- **MemoryReflector (Spec 2)** — LLM synthesis path with packet adapter, recall_signals per note, empty-packet short-circuit, builtin tool `memory_reflect`, MemoryProducerScheduler ticking the produce hook.
- **Context Fencing & Memory Modes (Spec 3)** — `MemoryInjectionMode` enum + `MemoryConfig.injection_mode`; MemoryContextProvider emits fenced UnifiedMessage; `memory_*` retrieval tools gated on injection mode.
- **Pluggable Memory Extensions (Spec 4)** — `MemoryExtension` trait + Registry with hybrid dispatch, `McpMemoryExtension` adapter, `[memory]` plugin manifest section, first-party `EnvelopeRelevanceFloorExtension` POC, on_retrieve invocation in MemoryContextProvider.
- **Wiki Orientation Layer (Spec 5)** — `NoteOrientation` handle, `parse_domain_topic` VFS path utility, palace topology generated columns (domain/topic) on facts, wiki compilation with manual-page protection, `wiki_sync` module for wikilink↔graph sync, fact↔node bidirectional index.
- **Compound Ingest (Spec 6)** — `CompoundIngestor` trait + `DefaultCompoundIngestor`, LLM-planned operations with source-aware suffix prompts, `CompoundApplyTx` transactional staging + rollback, `gather_related` Phase 1 retrieval with 1-hop expansion, `ingest_batch` with hash-conflict retry; CompressionService routed through CompoundIngestor.
- **User Profile (Spec 7)** — `ProfileSynthesizer` trait + `FsProfileSynthesizer` with merge algorithm and bootstrap prompts, `ProfileStore` USER.md read/write with hash-guard, profile injection into prompt layer, triggered on SessionEnd in CompressionService, builtin tool `user_profile` (read + history).
- **Query Filed-Back (Spec 8)** — `QueryFiler` trait + `DefaultQueryFiler` with two-tier gating (LLM gate + heuristic), fire-and-forget hook in `memory_reflect`, `query_filed` SQLite table, query category excluded from synthesis.
- **Dream Daemon Evolution Upgrade** — Strategy-driven pipeline replaces hardcoded daily/weekly. New components: `DreamStrategy` enum with stage mapping, Signal Collector (4-source signal extraction), Strategy Selector (personality-adaptive), MutationGate (cycle/oscillation/waste detection), 4-tier Validation Layer, immutable EventLog audit trail, SkillDistill stage; integration tests included.
- Six note-based dream stages: NoteSynthesis (weekly cross-note insight), NoteConsolidate (merge similar), NoteDecay (access-based archival), NoteDrift (contradiction/staleness), NoteLint (format + broken link repair, graph-based suggested links), DailyDigest, plus TunnelDiscoveryStage.
- Builtin memory tools: `memory_explore` (RippleTask wrapper), `memory_timeline`, `memory_browse` (notes filesystem browser), `session_complete`.
- Temporal validity — `valid_from`/`valid_to` on facts, schema columns, SearchFilter `as_of`/`include_historical`; DriftDetect closes validity windows on supersede instead of invalidating.
- Cross-encoder reranking integrated into retrieval; `RerankConfig` field on FactRetrieval; tunnel-aware traversal in Ripple; progressive search scope narrowing module; multi-agent NoteFactRetrieval queries; `top_synthesized` and `touch_fact_updated_at` for layered retrieval; LLM-based conflict arbitration for fact dedup.

#### Changed (refactor)
- **Notes are the single source of truth.** All retrieval, reranking, dream stages, memory tools, compression, panel reads migrated to NoteStore.
- Memory paths reorganized to `~/.aleph/memory/{raw,note}/{agent_id}/{category}/`; Canvas tab renamed to Memory in Panel.
- `FactType` renamed to `NoteType`; "facts" terminology removed from codebase.
- Compaction's `session_summary_source` uses RawMemoryStore; conflict detector uses NoteStore similarity with warn-log resolution.
- Tool index coordinator stores tools as notes in `tool/` category; retrieval uses NoteStore vector search.
- `recall_signals.fact_id` renamed to `note_path`.
- LLM-produced Contradict ops replace deterministic ConflictDetector.

#### Removed
- `MemoryStore` trait + `facts.rs` backend; `facts` DDL + `memory_facts`/`facts_fts`/`facts_vec` tables.
- `fact_retrieval`, `hybrid_retrieval`, `retrieval_trace` modules; legacy `MemoryContext` prompt-path; transitional migration scripts.
- Cortex module (POE never built); VFS module entirely (L1Generator gone with it).
- Cognitive modules — evolution, consolidation, reflection, promotion, decay; auxiliary modules — backup, cleanup, perf_monitor, lazy_decay, adaptive_retrieval, archival, noise_filter; `value_estimator`, `importance_weight` scoring stage.
- `MemoryScope`/`MemoryTier` enums; `MemoryEventStore` trait; `GraphNode`/`GraphEdge`/`memory_entities` (notes replaced them); `l1_overview` field (NoteSynthesis covers it).

### Agent Loop Evolution (Claude Code-inspired)

A 14-task overhaul aligning Aleph's loop with Claude Code's preflight + recovery model.

- `PreflightStage` trait + `PreflightPipeline` running microcompact → context_collapse → autocompact before each iteration.
- `ToolExecutionContext` with cascade policy and progress reporting.
- Multi-tier 413 recovery cascade and `TruncationRecovery` escalation loop.

### Channels — Multi-Platform Expansion

Seven new chat channels plus a structured Telegram v2.

#### Added
- **WeChat** — iLink Bot API integration with structured errors, dedup, and media support; markdown list/italic ordering bug fixed.
- **QQ** — Channel implementation with stabilized delivery and handlers.
- **Discord** — Full nested config hierarchy: `AccountResolver` (channel→account mapping), `ApprovalQueue` (exec command approval workflow), `DiscordAccountPool` (multi-bot-instance), `ChannelSettingsResolver` (per-channel override), security audit infrastructure, handlers module covering interactions, streaming, threads.
- **Matrix** — Refactor to matrix-sdk 0.16, enhanced message operations.
- **Signal** — `SignalMonitor` SSE-based message monitor with typed `SignalError`; polling code removed.
- **LINE** — Messaging API client, channel config, event types, webhook server with HMAC verification.
- **WhatsApp (native Rust)** — `WaRuntime` state machine + wa-rs client wrapper, Vault-backed auth storage, callback-based event loop with Pairing state, policy engine, outbound sender with media preprocessing; legacy native design docs marked superseded; whatsapp-rust Bot integration with WaRuntime.
- **Feishu** — Channel architecture alignment design + plan landed.

#### Telegram — Structured Upgrade
- Multi-account support: `BotInstance`, hierarchical config v2 types, `AccountResolver`, `start()` refactor for multi-bot startup.
- Reasoning lane, sticker pipeline, polls, error policy, status reactions; stream orchestrator with reasoning lane, status reactions, draft API support.
- WebhookHandler implementation for webhook mode.
- Config resolver with account-group-topic inheritance + unit tests.
- Legacy V1 (flat `bot_token`) configs auto-upgrade to V2 at registration so existing users don't have to migrate manually — resolves the "Failed to create channel 'AlephzBot'" startup error.
- `thread_id` wired into InboundMessage metadata; `AccessDecision` branches split.

### Security — Runtime Security Orchestrator (Phase 1-2)

- `RuntimeSecurityGuard` core types mounted in agent loop; inbound + outbound security pipelines.
- `ContextIdHasher` for session identifier hashing in InboundContext prompts.
- Platform-aware PII policies with `platform_name` propagation through PII filtering.
- Audit log integration with tracing spans; structured media fallback placeholders unify media handling.
- Phase 2 integration tests covering context hashing, placeholders, and platform PII; LLM context protection design docs.

### Browser & Runtime

- Migration: Playwright MCP → Playwright CLI for managed browser automation and PDF generation.
- Runtime bootstrap — one-click install of fnm, Node LTS, @playwright/cli, Chromium, plus skills (Panel → Settings → Browser → "Install All").
- Top-level Panel "Runtimes" view showing fnm, Node.js, uv, playwright-cli, cargo placeholder.
- Gateway RPCs: `runtimes.list`, `runtimes.install`, `runtimes.refresh`, plus `runtime_status`/`install_runtime`/`refresh_runtime` and `RuntimeInstallProgressEvent`.
- Cross-OS support — Windows via PowerShell (`winget install Schniz.fnm`, `irm astral.sh/uv/install.ps1 | iex`).
- `BrowserBackend` reshaped to text-first (SnapshotOutput / ScreenshotOutput / String) for token-efficient LLM responses.
- `chromiumoxide` and `@playwright/mcp` integrations removed; legacy `builtin_tools/browser/` deleted; `src/browser/bootstrap.rs` merged into unified `src/runtimes/` (~430 lines).
- Runtime install paths now defer to language-native tool defaults (`~/.local/share/fnm/`, `~/.local/bin/uv`, etc.) instead of bespoke `~/.aleph/runtimes/`.
- TOML `[playwright_mcp]` reads as `[playwright_cli]` via serde alias for backward compat.

### Canvas / Knowledge Graph

- Obsidian-style rendering: glow dots, title labels, continuous drift.
- Click-to-center animation, hover neighbor highlighting, focus dimming.
- Graph handlers query NoteStore directly (no separate GraphStore).
- Gateway RPCs: `graph.query`, `graph.neighbors`, `graph.node_detail`, `graph.search`.
- Canvas route renamed `/memory`; `PanelMode::Canvas` → `PanelMode::Memory`.

### Providers / Vault

- **Vault api_key runtime injection** — `handle_set_default` and startup `MultiProviderRegistry` now hydrate api_key from vault before `create_provider`. Resolves the "API key is required" error users hit on Telegram/Feishu chat paths after configuring providers via the panel.
- **Plaintext config → vault auto-migration is now wired into startup** — `migrate_all_secrets_to_vault` (previously dead code) runs before reverse-hydration; any plaintext key written into config.toml (manual edit, LLM patch) is relocated to the vault and stripped from disk.
- **Anthropic protocol hardening** — Filter empty/whitespace-only text blocks and replace empty-blocks fallback with single-space placeholder, so Anthropic-compatible backends (e.g. Kimi for Coding) don't reject historical empty-turn artifacts with HTTP 400 "must not be empty".
- New preset `kimi-for-coding` (alias `kimi-coding`) for the Kimi for Coding Anthropic-compatible IDE/agent endpoint at `https://api.kimi.com/coding/v1`. Standard `moonshot`/`kimi` presets restored to `https://api.moonshot.ai/v1` + `kimi-k2-0905-preview` + `protocol=openai`.
- `register_handler!` macro gains a 4-arg variant.

### Session Store

- `SessionStore` trait + file backend implementation with auto-migration from SQLite blob (Phases 1-3).
- Phase 4 cleanup — deprecated legacy SQLite message types, added migration verification.
- `metadata_json` string blob replaced with typed fields.
- Session manager refactor with richer metadata and events.
- `SessionKey` unified by removing legacy gateway/router enum.

### Event Sourcing

- Event sourcing wiring design + implementation.
- Handler writes to NoteStore instead of facts table.
- Server init wires event-sourced handler with migration; replay tolerates unknown event variants; PII-risky `event_json` snippet dropped from replay skip log.

### Other

- Gateway loads agent identity files into system prompt.
- Gateway: `CancellationToken` with broadcast cancellation; channel registry migrated to broadcast sender.
- Webchat: new-chat creates next epoch; epoch resolution corrected; refresh routes to same session.
- Teams: simplified to string-based dynamic roles (Explorer/Critic hardcoding gone); broadcast/peek/lock; lifecycle and plan modules.
- Browser: extended `browser_config` RPC with timeouts and persistent_sessions.
- `sources/{agent_id}/` structure for raw data storage.

### Fixed

- Memory pipeline — 4 wiring gaps closed for full L0 capture + L1 compression on the WS gateway path.
- Discord resolver and QQ delivery pre-existing test bugs corrected.
- Wiki compilation no longer overwrites manually curated pages.
- Notes — title sanitization prevents path traversal; canonical data dir used.
- NoteIndexer writes sync to SQLite immediately.
- `memory_search` session-local path restored via RawMemoryStore.
- Canvas sizing and graph data loading; sidebar hidden in Canvas mode for full-width layout; dark background `#0a0a0f` applied to Canvas container.
- Telegram cyclic import between config and config_v2 resolved; missing `max_retries` and coalescing fields added.
- WhatsApp bridge adapted to whatsmeow post-0.2 API changes.
- Compiler warnings resolved across the codebase.

### Migration Notes
- **Provider api_key**: any existing provider config with plaintext `api_key = "..."` in config.toml will be automatically migrated to the vault on first startup; the field is stripped from disk and re-injected at runtime. No action required.
- **Memory layout**: legacy `facts` table is dropped on startup; data lives under `~/.aleph/memory/{raw,note}/`. Existing notes are preserved.
- **Telegram**: V1 flat configs (top-level `bot_token`) auto-upgrade to V2 at channel registration. No action required.
- **Browser**: `[playwright_mcp]` TOML sections silently read as `[playwright_cli]`; old `command`/`args` fields are discarded.
- **Runtime status UI**: moved from Settings → Browser to top-level Panel "Runtimes" sidebar entry.
- **Kimi/Moonshot users**: `moonshot` preset now points to `https://api.moonshot.ai/v1` (OpenAI-compatible). For the Anthropic-compatible IDE endpoint use the new `kimi-for-coding` preset instead.

## [26.4.8]

### Added
- **记忆数据库迁移: LanceDB → sqlite-vec** — 新增 SqliteMemoryBackend 骨架与 sqlite-vec 向量搜索支持，移除 LanceDB 和 Arrow 全部依赖，删除 SessionStore trait 及所有调用方，记忆层全面轻量化
- 记忆图谱增强: 统一抽取管线支持 LLM graph triples（事实 + 实体 + 关系），LLM 驱动的冲突仲裁去重，C-layer 去重 prompt
- 记忆检索优化: RecallSignalStore 检索信号追踪，top_synthesized 查询，分层衰减率，promote 阈值调优
- Dream pipeline 增强: 报告持久化审计，session 过期清理（24h 保留），压缩后原始 chunk 失效
- Teams 多智能体协作优化: broadcast 广播机制、成员移除、peek 窥探、任务锁定，生命周期与计划管理模块
- Teams 角色系统重构: 移除硬编码 Explorer/Critic 角色，简化为字符串化动态角色
- Compaction 增强: SessionSummarySource 零成本上下文压缩，CacheMonitor prompt 缓存命中追踪，FileContentTracker 压缩后文件恢复，ToolResultStore 大结果磁盘持久化
- 工具: self_config 身份文件与配置管理工具，独立 FileReadTool
- file_ops 模块拆分为 edit.rs/write.rs/read.rs 子模块提升内聚性
- Agent 实例按需懒加载，降低启动开销

### Fixed
- Webchat 新建对话和刷新浏览器始终路由到同一 session 的问题 — AgentRouter 现正确解析 epoch，新建对话调用 sessions.new RPC
- Compaction double-write 关键 bug 修复，UTF-8 安全性，clippy 警告，边界检查
- Desktop OCR 模块缺失导入修复
- 移除 AgentInstance 中冗余的内存 session 缓存

## [26.4.7]

### Added
- Unified prompt system: PromptSnapshot, ToolUsageGrammarLayer, section caching, and hybrid memory injection replacing 26 legacy prompt_sections (~1000 lines removed)
- AgentRuntime with fresh-path execution, transcript capture, and SharedSnapshot support for prompt fork path
- SessionResumeLayer for cross-session context restoration with SnapshotWriter/SnapshotReader
- AgentCatalogLayer and McpToolIndexLayer in thinker pipeline for on-demand agent/MCP discovery
- MCP server instructions injection: cache instructions from initialize response, wire into prompt assembly
- McpInstructionsLayer for MCP server instruction injection into prompts
- mcp_tool_schema and agent_info builtin tools for on-demand discovery
- Skill trigger enhancement: when_to_use field in SkillManifest, `<when>` tag in XML output
- Agent descriptions and when_to_use metadata for builtin agents
- Feishu webhook mode with multi-account support
- Session snapshot written on root agent loop exit for cross-session resume
- Subagent transcript persistence with key_findings extraction
- MicroCompact upgrade to tiered ResultClearing with semantic compression
- Tool pipeline: structured tracing spans for all stages, execution timing
- Hook PermissionDecision enum (allow/ask/block/deny) integrated with SafetyGuard
- SafetyGuard: ToolSafetyPolicy keyword inference, check_permissions_only() for hook Allow bypass
- LoopCallback trait: on_confirmation_needed() for user confirmation routing
- NeedsConfirmation routed to confirmation flow instead of hard deny
- Matrix interface improvements with enhanced message operations
- MS Teams interface improvements with Graph API and polling support
- Channel secrets (bot_token, etc.) now visible in Panel via vault injection into config.get response

### Fixed
- Prompt builder: simplified builder API, wired cache, improved grammar layer
- Pre-existing broken test imports updated for new prompt system
- WhatsApp: stability fixes, escape_xml, Display impl, markdown handling
- Slack: removed unused is_empty method
- MS Teams streaming: removed unnecessary mut binding
- Gateway: clippy lints and code quality improvements

### Changed
- Deleted old PromptBuilder and 26 prompt_sections in favor of unified pipeline
- Extracted ToolInfo to standalone module for migration
- Deleted subagent_runner.rs, migrated types to AgentRuntime
- LoopCallback consumes needs_user_confirmation instead of silent denial

## [26.4.6]

### Added
- Session state machine: lifecycle states (Created → Active → Running → Idle → Stopped/Error) with enforced valid transitions in session manager
- Channel approval system: ChannelApprovalCapability trait, ChannelApprovalBridge for routing tool approval requests through Telegram and other channels
- Two-phase exec approval gate in agent loop with safety floor and retry logic
- Gateway plugin registry infrastructure for ChannelPlugin system with channel health monitoring
- OpenAPI 3.0 spec generation and schema introspection endpoints
- Tower-style handler middleware chain with typestate request pipeline for JSON-RPC tracking
- Unified typed channel event bus with backpressure-aware per-client buffers
- Swarm intelligence layer: LLM summarization in aggregator, task context injection, critical event interrupt mechanism
- TeammateManager for auto team creation and SubagentAction extensions (name/team_name, messaging)
- Compaction pipeline: CompactionOrchestrator with MicroCompactor (3D scoring), ToolAwareChunker, ConstraintInjector, and recall_context builtin tool
- DreamGate with 3-level cheap-to-expensive gate chain for memory dreaming
- Dreaming staged pipeline: CollectStage, ConsolidateStage (STM→LTM), DeepSynthesisStage, DriftDetect
- Desktop enhancements: multi-display support, ComputerUseLock, EscapeAbort, 8 new ScreenCapability methods (double_click, drag, hover, etc.)
- Transcription as first-class GenerationType with dedicated provider config and voices_url support
- Resilience: rate limit classification, circuit breaker, tool truncation, bootstrap budget
- Code-block-aware sanitization and high-risk tool permissions
- gateway_route builtin tool for channel plugin routing
- Telegram edit-based streaming delivery with message coalescing and offset persistence

### Fixed
- Channel approval bridge: delivery timeouts, error sanitization, expired approval cleanup
- Vault-based STT provider API key resolution at runtime (api_key is #[serde(skip)])
- Compaction pressure sync, panic safety for mem::take and cleanup chain
- 5 pre-existing test failures unrelated to compaction work
- Telegram: saturating arithmetic for offset, typing breaker counter, EditedMessageIsTooLong handling
- Swarm: wire AiProvider via OnceLock, close interrupt loop, pass agent_id to context injection
- Subagent: team_id routing, error handling, race condition fixes
- Desktop: safety comments, atomic ordering, drag cap, mutex warning
- Sanitizer: zero-alloc byte-slice scanning, TOCTOU fix, double-backtick support
- Gateway middleware P1/P2 fixes, lagged event count tracking
- Transcription provider lookup checks API key before matching default

### Changed
- License changed from AGPL-3.0 to MIT
- Removed dispatcher confirmation and integration modules
- Gateway handlers (health, echo, version) migrated to HandlerSchema trait
- Refactored desktop: split action.rs and perception.rs into sub-modules, removed legacy DesktopCapability trait
- Extracted subagent_runner.rs from agent_loop, dreaming pipeline from monolithic dreaming.rs
- Removed CompressionDaemon in favor of raw chunk storage for semantic recovery

## [26.4.2]

### Added
- ToolPipeline: 7-stage hook-integrated tool execution engine with deny semantics, input schema validation, structured tracing spans, and post-hook output modification
- Session-level hook callsites in AgentLoop (PreToolUse, PostToolUse, AfterToolCallFailure)
- Hook system: parse_command_output line-prefix protocol, updated_input/additional_contexts/prevent_continuation in HookResult
- Agent prompt pipeline with section registry and cache-aware verification
- Background agent status retrieval, model override, and restored default/plan agents
- Integration tests for full hook pipeline round trip

### Fixed
- session_search ACL: use actual caller identity instead of hardcoded "main" — subagents now use correct privilege level for permission checks
- session_search result quality: over-fetch before ACL filtering to prevent inaccessible hits from consuming the entire result window
- Auto-topic generation: remove unsupported max_output_tokens parameter for OpenAI Responses API
- /new command topic generation: add required system_prompt (instructions) field for OpenAI Responses API
- Telegram HTML delivery: add repair_html_tags() to fix mismatched tag nesting, preventing unnecessary fallback to plain text
- Hook runtime: fix interceptor double-execution and stale post-hook context
- Hook additional_contexts now properly injected into conversation as system-reminder messages
- ToolPipeline: deny precedence, alias-aware validation, effective tracing spans

### Changed
- **Project structure**: promoted `core/` to project root — `src/` is now top-level, workspace root is both `[workspace]` and `[package]` (standard Rust practice)
- **Project structure**: renamed `crates/` to `desktop/` with cleaner internal naming (`desktop/shared/`, `desktop/macos/`, `desktop/linux/`, `desktop/windows/`), moved `logging` to `shared/logging/`
- Removed TaskTool and SubAgent delegation framework (12 files, -5766 lines of dead code)
- Removed SubAgentHandler, EscalateTaskTool placeholder, and unused InterceptorResult
- Removed system_prompt from AgentDef, replaced with prompt_sections

## [26.3.29]

### Added
- MS Teams channel configuration UI in Panel — brand card, 7-field config form (App ID, App Password, Tenant ID, Webhook Path, Group/Team toggle, Typing Indicator, Allowed Users)
- MS Teams backend: Bot Framework REST API client, JWT validation, OAuth token cache, native streaming handler
- MS Teams Channel trait implementation with WebhookHandler and access control (DM + group policy)
- Teams Evolution: three-layer communication (messaging, collaborative sessions, task delegation) with role-based prompts
- Teams tools: team_digest, message_send, inbox_read, session_collaborate, session_turn, review_score, task_submit
- Collaborative sessions with SessionCoordinator and SQLite persistence
- MessageRouter with TTL, escalation suggestions, and threaded inbox
- TaskArtifact storage with auto-persist delegation results
- Event log system with configurable retention policy
- ACP (Agent Communication Protocol): file-based session persistence, unified acp_delegate tool with streaming + trust levels
- ACP structured error types, notification parsing, and session loading
- OpenAI Responses API passthrough handler (/v1/responses) and SSE formatter
- OpenAI /v1/embeddings endpoint with provider wiring
- Skill system unification: SkillManifest with primary_env/homepage/emoji, SkillsConfig TOML persistence, skill_status/install/manage LLM tools
- Bundled skills: include_dir extractor replaces git-based updater, manifest-driven source classification
- Panel skills view rewrite with status tabs, grouping, and detail dialog
- Feishu channel overhaul: extract api.rs, auth.rs, config.rs, websocket.rs, dedup.rs, user_cache.rs, streaming.rs
- Telegram channel overhaul: policy-based access control, HTML-safe chunking with tag balancing, extract handlers/polling/delivery modules
- Gateway StreamingController state machine and real-time streaming in ReplyEmitter
- Gateway semantic chunking in StreamingDeltaSink with accumulated text
- Discord: edit/react in Channel trait, resolve helpers
- Aleph E2E verification skill for production-level module testing via WebSocket
- Full-project code review infrastructure with module-by-module results (65 modules reviewed)

### Fixed
- MS Teams: access control bypass and JWKS key staleness
- MS Teams: 8 code review issues (dead fields, URL safety, eviction, retry-after)
- Teams: intermediate message delivery and agent reuse fixes
- Teams: WAL mode, TOCTOU race, N+1 queries, UTF-8 validation
- Feishu: UTF-8 truncation, WebSocket endpoint dedup, executor improvements
- Telegram: iterative chunking extracted to chunking.rs, dead code removal
- Agent loop: 4 code logic issues from review
- Router: use ResolvedRoute session_key, extract guild/team from raw message
- Thinker: UTF-8 indexing, buffer ops, dead code, lock safety fixes
- Gateway: ResponseChunk content→delta rename with compat alias
- Full-project review fixes across 65 modules (lock safety, UTF-8, SQL injection prevention)

## [0.3.1] - 2026-03-26

### Added
- ACP Coding Orchestrator: Aleph can now act as a "tech lead", autonomously directing Claude Code, Codex, and Gemini CLI through multi-step coding workflows (plan → code → review)
- All ACP harnesses support dual execution mode (oneshot + native_acp), switchable per call or via Panel config
- Session pool with canonicalized SessionKey and extract-use-reinsert locking for concurrent ACP sessions
- AcpChunkCallback for real-time streaming passthrough from native ACP harnesses
- Tool args extended with `mode` and `reuse_session` parameters for LLM-driven orchestration
- Coding orchestration strategy injected into system prompt (R10 compliance)
- "外部代码 Agent" tool category in Panel for ACP tool grouping
- ACP tools registered in BUILTIN_TOOL_DEFINITIONS for direct LLM visibility
- Streaming phase 2: thinking/tool deltas, execution engine enhancements
- Gateway i18n module for localized messages
- Markdown-to-platform formatter rewrite with full test suite
- Telegram polling resilience and message formatting improvements
- Desktop MediaCapability: audio recording, screen capture, speech-to-text
- macOS native APIs: OCR (Vision), window management, clipboard, sysinfo, global hotkey
- TCC permission management tool for macOS privacy permissions
- Team management tools (team_create, team_delegate, team_status, team_disband) with SQLite store
- Heartbeat monitoring system with probe execution, dedup engine, and wake queue
- Capability-driven plugin architecture with ManifestAdapter trait and multi-format support
- Agent engine resilience: resettable deadline, pair-aware truncation, three-layer timeout cascade
- Gateway evolution: IdempotencyGuard, ConnectionRole, CapabilityApi, execution config RPC
- Provider protocol refactor: stream_deltas(), DeltaCollector, ProviderDelta foundation
- OpenAI Responses API protocol support (/v1/responses)

### Fixed
- ACP preset mode defaults: Claude Code and Codex now correctly default to Oneshot (was NativeAcp)
- Anthropic API tool_use.input empty parameter bug: empty input now serializes as `{}` instead of `""`
- ACP tools now visible to LLM (were registered for execution but missing from tool list)
- Panel ACP mode display: correctly reads runtime mode from server, not stale config field
- ACP config persistence: preset harnesses auto-populate via serde deserializer, Panel changes persist
- Panel config field name mismatch (mode → default_mode) for ACP harnesses
- UTF-8 chunk boundary bug in all SSE stream parsers
- Intermediate message delivery and agent reuse in team workflows

## [0.3.0] - 2026-03-24

### Added
- feat: wire MultiProviderRegistry into server initialization
- feat: add provider fallback module with transient error retry
- feat: add MultiProviderRegistry with model-key routing
- feat: add tool_choice support in Codex protocol
- feat: add tool_choice support and hash-based IDs in Gemini protocol
- feat: add tool_choice support in Anthropic protocol
- feat: add tool_choice support and fix argument parsing in OpenAI protocol
- feat: add ToolChoice enum and protocol capabilities to adapter
- feat: add is_transient() to AlephError for provider fallback
- feat(panel): add streaming, render_mode, typing_indicator fields to Feishu settings
- feat(feishu): wire FeishuEventEmitter into execution flow
- feat(feishu): add markdown card rendering and updated capabilities
- feat(feishu): add FeishuEventEmitter with streaming cards and typing indicators
- feat(feishu): add Card Kit streaming, static card, and reaction API methods
- feat(feishu): add streaming, render_mode, typing config fields and API types
- feat(panel): add Feishu/Lark channel settings card
- feat(feishu): fix clippy warnings — unused import, visibility, closure
- feat(feishu): add FeishuChannel impl and wire into factory registry
- feat(feishu): add FeishuClient with token, HTTP API, and media support
- feat(feishu): add WebSocket event parsing and text extraction
- feat(feishu): add types, config, and API response structs
- feat: add Persistent Completion Protocol for agent task verification
- desktop-macos: implement PimCapability via SwiftBridge
- desktop-macos: implement SystemCapability (apps, notifications, clipboard, sysinfo)
- desktop-macos: implement AutomationCapability (osascript + Shortcuts CLI)
- desktop: wire NativeScreen into all platform crates
- desktop: add NativeScreen shared ScreenCapability implementation
- core: add SystemTool and AutomationTool builtin tools
- desktop: add per-platform crate skeletons (macos, linux, windows)
- desktop: add SwiftBridge utility for macOS native API calls
- desktop: update crate doc to reflect two-layer architecture
- desktop: add capability trait hierarchy and shared types
- core: add aleph-client dependency for server binary
- feat: enable native tool calling for ChatGPT/Codex Responses API
- core: add Strict Mode support (schema strictification + provider integration)
- core: add #[cfg(unix)] guards for Unix socket code on Windows
- desktop: fix Windows OCR compilation errors
- feat(browser): add profile config types and browser system configuration
- feat(browser): add SsrfPolicy for URL validation and private network blocking
- feat(config): add queue_mode session configuration with gateway wiring
- feat(anthropic): wire cache_control ephemeral breakpoint for system prompt caching
- feat(thinker): partition system prompt into stable/dynamic zones for cache optimization
- feat(compressor): add pre-compaction silent memory flush
- feat(agent-loop): add CollectQueue with time-window message merging
- feat(agent-loop): add SteerQueue with interrupt signaling
- feat(agent-loop): add SessionQueue trait and FollowupQueue implementation
- feat(agent-loop): wire interrupt channel into RunContext and loop execution
- feat(agent-loop): add InterruptChannel for steering support
- core: add missing tracing::warn import for non-macOS builds
- feat: unified slash command system
- feat: wire memory tools into agent execution + Two-Phase Smart Recall
- feat(server): add desktop feature gate for in-process desktop capabilities
- feat(desktop): integrate DesktopCapability into DesktopTool with dual-path execution
- feat(desktop): implement input actions with enigo
- feat(desktop): implement screenshot and OCR via xcap
- feat: add aleph-desktop crate skeleton with DesktopCapability trait
- desktop: fix Tauri build for macOS and add app/dmg bundle targets
- feat(wasm): register host functions via PluginBuilder with capability kernel
- feat(manifest): parse WASM capabilities from aleph.plugin.toml
- feat(wasm): add WasmCapabilityKernel — per-execution security enforcement
- feat(wasm): add CredentialInjector — plugins never see secrets
- feat(wasm): add AllowlistValidator with anti-bypass security
- feat(wasm): add WasmCapabilities types with default-deny model
- feat(exec): add LeakDetector with Aho-Corasick bidirectional scanning
- desktop: add all_day and calendar_id to PimCalendarUpdate
- desktop: add PIM variants to DesktopRequest and JSON-RPC mapping
- desktop: remove macOS target, add server embedding for Linux/Windows
- desktop: fix flaky tests that assumed bridge socket absence
- desktop-bridge: implement Windows OCR (WinRT) and UI Automation AX tree
- desktop-bridge: implement window management (list, focus, launch)
- desktop-bridge: implement Windows input simulation (click, type, key combo, scroll)
- desktop: wire snapshot and new actions in DesktopBridgeServer dispatch
- desktop: implement scroll, double-click, drag, hover, paste, and ref-aware targeting
- desktop: implement UI snapshot with ref generation in Perception.swift
- desktop: add RefStore for snapshot ref management (Swift)
- desktop: update tool args and build_request for snapshot, ref targeting, and new actions
- desktop: add core types for snapshot, ref system, and new action primitives
- desktop: update tool messaging for bridge architecture
- desktop: probe managed and standalone socket paths
- feat(runtimes): add ensure_capability orchestration (Probe -> Bootstrap -> Register)
- feat(runtimes): wire CapabilityLedger into prompt system
- feat(runtimes): add bootstrap module with shell-driven installation
- feat(runtimes): wire ledger into exec layer PATH
- feat(runtimes): add Probe module for system-first capability detection
- feat(runtimes): add legacy manifest.json migration to ledger.json
- feat(runtimes): add CapabilityLedger for lightweight runtime state tracking
- feat(desktop): implement desktop.screenshot in Tauri DesktopBridge
- feat(desktop): add DesktopBridge UDS server with ping support
- feat(protocol): add desktop_bridge types for cross-platform Bridge
- feat(halo): switch macOS HaloWindow from SwiftUI to WKWebView
- feat(halo): add /halo route with chat UI, message list, and input area
- feat(halo): add event handler to wire run.* streaming events to HaloState
- feat(halo): add HaloState reactive signals for chat state management
- feat(halo): add ChatApi module for chat.send/abort/history/clear
- feat(desktop): Task 11 complete — DesktopTool active in agent via builtin registry
- feat(desktop): implement WKWebView canvas overlay with A2UI patch support
- feat(desktop): implement mouse, keyboard, and window actions in Action.swift
- feat(desktop): add accessibility permission description and runtime check
- feat(desktop): implement screenshot, OCR, and AX tree in Perception.swift
- feat(desktop): point settings window to Leptos Control Plane server
- feat(macos): add Settings menu item opening Control Plane WebView
- feat(macos): add SettingsWebView WKWebView wrapper
- feat(desktop): add Swift UDS server skeleton with stub handlers
- feat(desktop): register DesktopTool in executor builtin registry
- feat(desktop): add DesktopTool builtin with graceful degradation
- feat(desktop): add UDS client with JSON-RPC 2.0 and unit tests
- feat(desktop): add types, error, and module scaffold
- feat(skill): integrate SkillSystem v2 into ExtensionManager and ExecutionEngine
- feat(skill): add SkillSystem facade with Arc<Inner> pattern
- feat(skill): add slash command resolution
- feat(skill): add InstallSpec to shell command converter
- feat(skill): add SkillStatusReport for eligibility dashboard
- feat(skill): add SkillSnapshot with version-invalidated cache
- feat(skill): add XML prompt builder for skill injection
- feat(skill): add EligibilityService with OS/binary/env checks
- feat(skill): add SKILL.md parser with YAML frontmatter support
- feat(skill): add SkillRegistry with priority-based dedup
- feat(skill): add SkillManifest AggregateRoot with Entity trait
- feat(skill): add EligibilitySpec, InstallSpec, InvocationPolicy, PromptScope ValueObjects
- feat(skill): add SkillId, PluginId, SkillSource domain types
- feat(thinker): add skill_instructions to PromptConfig for SkillSystem v2
- feat(extension): add SkillSystem v2 and wire skill XML into agent prompts
- feat(swarm): add event statistics and logging
- feat(agent_loop): integrate ContextProvider into MessageBuilder
- feat(swarm): implement SwarmContextProvider
- feat(agent_loop): define ContextProvider trait
- feat(agent_loop): implement event publishing (shadow mode)
- feat(agent_loop): define AgentLoopEvent enum
- feat(agent_loop): implement Builder build() method
- feat(agent_loop): add AgentLoopBuilder structure
- feat(perception): integrate PAL with SystemStateBus
- feat(perception): add Platform Abstraction Layer (PAL)
- feat(swarm): Phase 5 - End-to-End Integration
- feat(perception): implement Phase 5 - Documentation, Examples & Testing
- feat(perception): implement Phase 4 - Vision Connector architecture
- feat(ssb): implement Phase 3 - action dispatcher
- feat(ssb): implement Phase 2 - robustness & privacy
- feat(ssb): implement Phase 1 - core infrastructure
- feat(control-plane): implement WebSocket subscription for real-time alerts
- feat(shared_ui_logic): add alerts API module for system health and memory monitoring
- feat(skill-evolution): integrate SuccessManifest with tool execution
- feat(control-plane): pass mode and alert_key to SidebarItems
- feat(control-plane): integrate Tooltip and Badge into SidebarItem
- feat(control-plane): add StatusBadge component for alert indicators
- feat(control-plane): add Tooltip component for narrow mode labels
- feat(skill-evolution): implement CollaborativeSolidificationPipeline
- feat(control-plane): implement Sidebar narrow/wide mode switching
- feat(skill-evolution): implement ConstraintValidator
- feat(skill-evolution): implement SuccessManifest data structure
- feat(control-plane): add SettingsLayout for nested routing
- feat(control-plane): add alert bus and sidebar mode override to DashboardState
- feat(control-plane): add sidebar types (SidebarMode, AlertLevel, SystemAlert)
- feat(control-plane): compile Tailwind CSS locally for production
- feat(dashboard): add Plugins, Skills, and Policies settings pages
- feat(dashboard): add sidebar navigation to settings UI
- feat(dashboard): add Generation Providers navigation card to Settings page
- feat(dashboard): implement Generation Providers CRUD functionality
- feat(dashboard): add Generation Providers frontend UI
- feat(dashboard): add Generation Providers backend and API layer
- feat(dashboard): implement comprehensive configuration management UI
- feat(macos): implement WebSocket client for Gateway connection
- feat(macos): complete Phase 4 client simplification for ControlPlane integration
- feat(dashboard): complete Phase 3 SDK integration with RPC, events, and API layer
- feat(dashboard): complete Phase 2 SDK integration with error handling and reconnection
- feat(dashboard): add connection state awareness to Memory view
- feat(dashboard): integrate shared_ui_logic SDK into Dashboard
- feat(dashboard): full architectual refactor with Leptos 0.8.15 and rust-ui components
- feat(dashboard): complete Memory Explorer view and fix System Status
- feat(dashboard): initialize Aleph Dashboard with Leptos 0.6
- feat(shared-ui-logic): implement Plugins and Providers APIs
- feat(shared-ui-logic): implement WASM WebSocket connector
- feat(shared-ui-logic): implement API and Observability layers
- feat(shared_ui_logic): implement protocol layer
- feat(shared_ui_logic): implement native WebSocket connector
- feat(shared_ui_logic): initialize Aleph UI Logic SDK
- feat(cortex): implement LLM-based critic report generation
- feat(cortex): add AiProvider to CriticAgent
- feat(cortex): implement LLM-based root cause analysis
- feat(cortex): add AiProvider to ReactiveReflector
- feat(agent_loop): add meta-cognition integration for Phase 6
- feat(cortex): implement CortexIntegration orchestrator (Task #11)
- feat(cortex): implement experience clustering and deduplication
- feat(dispatcher): implement L1.5 ExperienceReplayLayer
- feat(cortex): implement Cortex Dreaming background service
- feat(cortex): implement LLM-based pattern extraction
- feat(cortex): implement DistillationService core structure
- feat(engine): add FeatureExtractor for advanced ML rule learning
- feat(cortex): implement multi-dimensional experience value estimator
- feat(cortex): add agent loop telemetry capture
- feat(cortex): implement Experience CRUD operations
- feat(cortex): define core data structures
- feat(engine): add ML-based L2 rule generation (RuleLearner)
- feat(cortex): add experience_replays database table
- feat(builtin_tools): add AtomicOpsTool for atomic operations
- feat(browser): implement JavaScript-based context freeze/resume
- feat(browser): implement Phase 2.4 CDP integration for context freeze/resume
- feat(engine): add comprehensive testing and performance validation
- feat(executor): add AtomicActionExecutor with L1/L2 routing
- feat(engine): implement atomic engine with L1/L2/L3 routing
- feat(dispatcher): implement Phase 2 Intelligent Scheduling for Liquid Hub
- feat(macos): add guest session activity log UI
- feat(macos): add activity log RPC types and methods
- feat(gateway): add RPC request activity logging for guest sessions
- feat(gateway): add guests.getActivityLogs RPC handler
- feat(gateway): integrate activity logging into GuestSessionManager
- feat: implement guests.revokeInvitation RPC method
- feat(macos): add Guest management UI in Settings
- feat(gateway): register config.get and config.patch RPC handlers
- feat(gateway): add SessionIdentityMeta for identity storage
- feat(protocol): add IdentityContext for stateless security
- feat(gateway): add config.patch RPC handler with events
- feat(memory): add idempotent namespace migration
- feat(gateway): add RPC handlers for guest management
- feat(memory): add namespace column for data isolation
- feat(protocol): add discovery types for mDNS
- feat(protocol): add ConfigChangedEvent for config sync
- feat(gateway): add InvitationManager for guest invitations
- feat(protocol): add invitation types for guest management
- feat(gateway): add PolicyEngine for permission checks
- feat(gateway): add IdentityMap for external identity resolution
- feat(protocol): add Role and GuestScope for Owner+Guest model
- feat(phase3): complete Tauri Desktop migration to thin client
- feat(phase3): migrate Tauri Desktop to SDK architecture (WIP)
- feat(phase2): refactor CLI to use SDK
- feat(phase2): implement GatewayClient with authentication
- feat(phase2): implement transport and RPC layers in SDK
- feat(phase2): create aleph-client-sdk skeleton
- feat(gateway): add Server-Client routing infrastructure to ConnectionState
- feat: add tool routing config and scope checking for Server-Client architecture
- feat(executor): integrate RoutedExecutor with Agent Loop
- feat(cli): create aleph-cli as protocol reference implementation
- feat(protocol): create aleph-protocol crate for shared types
- feat(executor): integrate ToolRouter with execution engine
- feat(dispatcher): add execution_policy field to UnifiedTool
- feat(executor): add ToolRouter for Server-Client routing decisions
- feat(gateway): add tool.call protocol messages
- feat(gateway): add ReverseRpcManager for Server-to-Client calls
- feat(gateway): store ClientManifest in ConnectionState
- feat(gateway): extend ConnectParams to accept ClientManifest
- feat(gateway): add ClientManifest for capability negotiation
- feat(dispatcher): add ExecutionPolicy enum for Server-Client routing
- feat(spec_driven): implement BDD dual-track testing system
- feat(domain): implement DDD foundation with marker traits
- feat(dispatcher): implement L2 async LLM enhancement for tool descriptions
- feat(memory): add performance monitoring for LLM calls
- feat(scheduler): implement recursion depth tracking
- feat(scheduler): implement anti-starvation logic
- feat(scheduler): implement LaneScheduler core
- feat: implement CompressionDaemon for background compression scheduling
- feat(scheduler): implement LaneState with queue and semaphore
- feat: enhance ContextComptroller with priority-based token management
- feat: implement ValueEstimator for memory importance scoring
- feat(scheduler): add lane scheduler infrastructure
- feat: add sliding window chunking to TranscriptIndexer
- feat: add TranscriptIndexer for near-realtime memory indexing
- feat(sub_agents): add active runs query and stats to SubAgentRegistry
- feat(sub_agents): add FactsDB persistence helpers for SubAgentRun
- feat(sub_agents): add state transition to SubAgentRegistry
- feat(sub_agents): add SubAgentRegistry with in-memory indexing
- feat(memory): add SubAgent fact types for Multi-Agent 2.0 persistence
- feat(sub_agents): add SubAgentRun data model for Multi-Agent 2.0
- feat(dispatcher): integrate HydrationPipeline into Agent Loop
- feat(core): export tool_index types from lib.rs
- feat(memory): add VectorDatabase::in_memory() for testing
- feat(dispatcher): add ToolRetrieval with dual-threshold hydration
- feat(dispatcher): add ToolIndexCoordinator for Memory synchronization
- feat(dispatcher): add SemanticPurposeInferrer for L0/L1 inference
- feat(dispatcher): add tool_index module with ToolRetrievalConfig
- feat(memory): add Tool variant to FactType for tool-as-resource
- feat(memory): add Multi-Agent Resilience database layer
- feat(gateway): add identity management RPC handlers
- feat(thinker): add thinking transparency guidance to PromptBuilder
- feat(agent_loop): integrate ThinkingParser into DecisionParser
- feat(gateway): add ReasoningBlock and UncertaintySignal stream events
- feat(agent_loop): add ThinkingParser for semantic reasoning extraction
- feat(agent_loop): add StructuredThinking types for CoT Transparency
- feat(thinker): integrate Soul into PromptBuilder
- feat(thinker): add markdown parser for soul.md files
- feat(thinker): add IdentityResolver for layered identity resolution
- feat(thinker): add SoulManifest types for Embodiment Engine
- feat(test): migrate logging, security, and e2e tests to BDD
- feat(test): migrate iMessage routing and subagent tests to BDD
- feat(gateway): add ChannelProvider trait for interaction manifests
- feat(agent_loop): add Silent and HeartbeatOk decision types
- feat(thinker): add environment contract and security sections to PromptBuilder
- feat(thinker): add ContextAggregator for environment reconciliation
- feat(test): migrate markdown skills tests to BDD
- feat(thinker): add SecurityContext for policy-driven permissions
- feat(thinker): add InteractionManifest for channel capability awareness
- feat(test): migrate models and protocol integration tests to BDD
- feat(test): migrate DAG and worldmodel dispatcher tests to BDD
- feat(test): migrate smart tool discovery and sessions tests to BDD
- feat(thinker): add provider-specific context caching strategies
- feat(dispatcher): add dual-layer profile-based tool filtering
- feat(test): migrate extension v2 and runtime tests to BDD
- feat(gateway): add WorkspaceManager for Anti-Gravity Architecture
- feat(test): migrate extension plugin registry tests to BDD
- feat(test): migrate tool server tests to BDD
- feat(test): migrate gateway inbound router tests to BDD
- feat(test): migrate dispatcher cortex tests to BDD
- feat(test): migrate memory integration tests to BDD
- feat(tests): migrate memory facts tests to BDD
- feat(tests): migrate message builder tests to BDD
- feat(tests): migrate thinker prompt builder tests to BDD
- feat(tests): migrate POE tests to BDD
- feat(tests): migrate agent loop tests to BDD
- feat(config): add ProfileConfig for Workspace Architecture
- feat(tests): migrate perception and watcher tests to BDD
- feat(tests): migrate daemon IPC and launchd tests to BDD
- feat(tests): migrate daemon core tests to BDD
- feat(tests): migrate config validation tests to BDD
- feat(tests): migrate config basic tests to BDD
- feat(tests): migrate scripting engine tests to BDD
- feat(tests): add cucumber BDD infrastructure
- feat: add example YAML policies and E2E tests
- feat(dispatcher): add YAML policy loader and PolicyEngine integration
- feat(dispatcher): implement YamlPolicy with Rhai evaluation
- feat(scripting): add BaselineApi with lazy TTL caching
- feat(scripting): implement HistoryApi.last() with WorldModel queries
- feat(scripting): implement EventApi and EventCollection filtering
- feat(scripting): add HistoryApi and EventCollection stubs
- feat(scripting): add duration parsing and helpers for Rhai
- feat(dispatcher): add YAML rule schema parsing
- feat(dispatcher): add Rhai sandbox engine with strict limits
- feat(worldmodel): add JSON state persistence
- feat(dispatcher): add core data structures
- feat(daemon): integrate perception layer with daemon CLI
- feat(daemon): implement FSEventWatcher
- feat(daemon): implement SystemStateWatcher
- feat(daemon): implement ProcessWatcher
- feat(daemon): implement TimeWatcher
- feat(daemon): add watcher trait and registry
- feat(daemon): add perception configuration system
- feat(daemon): add event system foundation
- feat(protocols): implement hot reload with notify file watching
- feat(protocols): implement ProtocolLoader file and directory loading
- feat(protocols): implement ConfigurableProtocol custom mode with template rendering
- feat(protocols): implement ConfigurableProtocol minimal mode (extends base + differences)
- feat(protocols): add JSONPath parser for response value extraction
- feat(protocols): add template engine wrapper for request/response transformation
- feat(protocols): add dependencies for configurable protocols (handlebars, jsonpath, notify)
- feat(providers): add ProtocolLoader stub for hot reload
- feat(providers): add ConfigurableProtocol stub
- feat(providers): implement ProtocolRegistry for dynamic protocol management
- feat(providers): add ProtocolDefinition types for YAML configs
- feat(tools): implement VirtualFs sandbox mode
- feat(tools): add Evolution auto-load integration
- feat(gateway): add Markdown Skills RPC handlers
- feat(tools): add replace_tool() API with explicit update semantics
- feat(tools): add hot reload support for Markdown Skills (Phase 4)
- feat(tools): add Evolution Loop integration for Markdown Skills (Phase 3)
- feat(tools): add examples() method to AetherTool trait (Phase 2)
- feat(tools): complete Markdown Tool Adapter integration
- feat(tools): implement Markdown Tool Adapter (Phase 1)
- feat(providers): add Tier 3 specialized OpenAI-compatible provider presets
- feat(providers): add Tier 2 OpenAI-compatible provider presets
- feat(providers): add Tier 1 OpenAI-compatible provider presets
- feat(providers): add Gemini presets and update factory
- feat(providers): implement GeminiProtocol adapter
- feat(providers): add Gemini API types module
- feat(providers): add Claude/Anthropic presets
- feat(providers): implement AnthropicProtocol adapter
- feat(providers): add Anthropic API types module
- feat(gateway): add approval RPC handlers
- feat(mcp): add ApprovalHandler for human-in-the-loop
- feat(mcp): add approval request types for human-in-the-loop
- feat(mcp): add streaming types for sampling responses
- feat(mcp): add TokenRefreshManager for automatic token refresh
- feat(mcp): add OAuth token refresh support
- feat(mcp): integrate context injection with SamplingHandler
- feat(mcp): add ContextInjector for cross-server context
- feat(mcp): add IncludeContext enum type for sampling requests
- feat(config): add protocol field to ProviderConfig
- feat(providers): add provider presets registry
- feat(providers): add HttpProvider container with ProtocolAdapter
- feat(providers): implement OpenAiProtocol adapter
- feat(providers): add ProtocolAdapter trait with streaming support
- feat(providers): add RequestPayload DTO for protocol adapters
- feat(mcp): add sampling callback integration to McpManager
- feat(mcp): add response mechanism for server-initiated requests
- feat(mcp): integrate SamplingHandler with McpClient
- feat(memory): complete Memory v3 Milestones 4-6
- feat(mcp): add SamplingHandler for server-initiated LLM calls
- feat(mcp): implement real SSE event listening with reqwest-eventsource
- feat(mcp): add SSE event types and reqwest-eventsource dependency
- feat(memory): implement CLI list and show commands
- feat(memory): implement AuditLogger for operation tracking
- feat(mcp): add Sampling RPC types for P2 server-initiated LLM calls
- feat(memory): add audit log schema and types
- feat(memory): add CLI module with file locking
- feat(memory): implement ArchivalService for scratchpad archiving
- feat(memory): implement HybridTrigger with token threshold safety net
- feat(memory): implement LazyDecayEngine for read-time decay evaluation
- feat(memory): add type-aware decay calculation with temporal scope
- feat(memory): add decay_invalidated_at field for recycle bin
- feat(memory): complete Milestone 1 - Scratchpad Foundation
- feat(memory): implement ScratchpadManager with CRUD operations
- feat(memory): implement SessionHistory for scratchpad archival
- feat(memory): add scratchpad module structure and template
- feat(mcp): implement real McpResourceManager and McpPromptManager
- feat(tools): add mcp_get_prompt builtin tool
- feat(tools): add mcp_read_resource builtin tool
- feat(mcp): implement real aggregation for resources and prompts
- feat(mcp): add resources and prompts methods to McpClient
- feat(mcp): add resources and prompts support to McpServerConnection
- feat(mcp): add Resources and Prompts RPC types
- feat(mcp): add health check logic for servers
- feat(gateway): wire MCP handlers to McpManagerHandle
- feat(mcp): implement McpManagerActor core loop
- feat(mcp): add config persistence for McpManager
- feat(mcp): add McpManagerHandle public API
- feat(mcp): add McpCommand and McpManagerEvent types
- feat(cortex): implement DecisionConfig with session override
- feat(cortex): implement security rules (tag injection, PII masking, instruction override)
- feat(cortex): add SanitizerRule trait and SecurityPipeline
- feat(cortex): add greedy JSON repair logic
- feat(cortex): implement JsonStreamDetector state machine
- feat(cortex): add module skeleton with unified error types
- feat(extension): add PluginHttpHandler for plugin REST routes
- feat(extension): add PluginProviderAdapter for plugin AI providers
- feat(extension): add ChannelManager skeleton for plugin channels
- feat(extension): add HTTP route types
- feat(extension): add provider plugin types
- feat(extension): add channel plugin types
- feat(gateway): add service lifecycle RPC handlers
- feat(extension): integrate ServiceManager with ExtensionManager
- feat(extension): add ServiceManager for background services
- feat(extension): add service lifecycle types
- feat(gateway): add plugins.executeCommand RPC handler
- feat(extension): add command execution to PluginLoader
- feat(extension): add DirectCommandResult type
- feat(extension): implement scope-aware skill injection
- feat(extension): implement V2 prompt loading with scope support
- feat(extension): add scope and bound_tool to ExtensionSkill
- feat(extension): add PromptScope enum for V2 skill injection
- feat(extension): add V2 hook conversion from TOML manifest
- feat(extension): implement typed hook execution (interceptor/observer/resolver)
- feat(extension): add kind and priority to HookConfig
- feat(extension): add HookKind and HookPriority enums
- feat(extension): integrate TOML parser with auto-detection (TOML > JSON)
- feat(extension): add V2 fields to PluginManifest
- feat(extension): add TOML manifest parser types
- feat(exec): check skill_allowlist in approval decision
- feat(exec): add skill_allowlist config option
- feat(exec): extend ExecContext with skill origin info
- feat(skills): implement CLI Wrapper validator
- feat(skills): add health checking methods to SkillsRegistry
- feat(skills): add install suggestion methods to SkillsInstaller
- feat(skills): implement HealthChecker for dependency validation
- feat(skills): extend SkillFrontmatter with requirements and metadata
- feat(skills): add types for requirements and health checking
- feat(poe): replace PlaceholderWorker with real AgentLoopWorker
- feat(gateway): wire POE contract signing to Gateway
- feat(poe): implement contract signing workflow for first principles closure
- feat(core): add snapshot capture tool and registry updates
- feat(config): add memory configuration types and validation
- feat(memory): enhance retrieval and add dreaming module
- feat(macos): add tool emoji formatting to HaloStreamingView
- feat(macos): update GatewayStreamAdapter with enhanced summary
- feat(macos): add HaloResultViewV2 with detail popover support
- feat(macos): add HaloResultDetailPopover for detailed results
- feat(macos): add EnhancedRunSummary and ToolSummaryItem models
- feat(gateway): add EnhancedRunSummary and per-runId sequences
- feat(gateway): add message deduplication with text normalization
- feat(gateway): add stream buffer for block-level text flushing
- feat(gateway): add tool display module with emoji and smart formatting
- feat(halo): integrate commandList state into HaloViewV2
- feat(halo): add HaloCommandListView for / command panel
- feat(halo): add CommandItem and CommandListContext types for / command
- feat(halo): add HaloInputCoordinator for lightweight input handling
- feat(gateway): add 150ms throttling for response chunks
- feat(halo): add HaloViewV2 main component integrating all state views
- feat(halo): add HaloHistoryListView for conversation history
- feat(halo): add HaloResultView for compact result display
- feat(halo): add HaloStreamingView for unified streaming display
- feat(halo): add HaloStateV2 with 6 simplified states
- feat(halo): add new streaming types for simplified state model
- feat(skill-evolution): implement Skill Compiler (Phase 10)
- feat(agent-loop): add on_user_question method to LoopCallback
- feat(agent-loop): add AskUserRich decision variant with QuestionKind
- feat(agent-loop): export question and answer modules
- feat(agent-loop): add UserAnswer type for structured responses
- feat(agent-loop): add QuestionKind types for structured user interaction
- feat(resilient): add cron integration with PodcastTask example
- feat(resilient): implement ResilientExecutor with retry and fallback
- feat(resilient): define ResilientTask trait
- feat(resilient): add core types for resilient task execution
- feat(skill_evolution): implement GitCommitter for auto-commit
- feat(skill_evolution): implement SkillGenerator for SKILL.md creation
- feat(skill_evolution): implement SolidificationDetector for pattern detection
- feat(skill_evolution): implement EvolutionTracker for execution logging
- feat(skill_evolution): add core types for skill evolution system
- feat(spec_driven): implement SpecDrivenWorkflow orchestrator
- feat(spec_driven): implement LlmJudge for evaluation
- feat(spec_driven): implement TestWriter for test generation
- feat(spec_driven): implement SpecWriter for requirement analysis
- feat(spec_driven): add core types for spec-driven workflow
- feat(gateway): add exec.callback.handle RPC for approval callbacks
- feat(telegram): add edit_message method for approval updates
- feat(gateway): add approval bridge handler utilities
- feat(exec): add ApprovalBridge for channel integration
- feat(telegram): add callback query handling
- feat(telegram): add inline keyboard support

### Fixed
- fix: serialize tool_calls on assistant messages in OpenAI protocol
- fix: unify provider verified pattern across embedding and reranking
- fix: pass provider name in generation test_connection for verified persistence
- fix: pass provider name in test_connection so verified=true persists
- fix: rewrite changelog generator (fix escape bug), clean up CHANGELOG.md
- fix: add tool_call_id to OpenAI tool result messages
- fix: unignore CHANGELOG.md, fix release recipe git add
- fix: remove unused imports across codebase (cargo fix)
- fix: resolve 42 test warnings — deprecated API, unused imports, dead code
- fix: slash command fast-path + CLI arg parser + E2E tests
- fix: enable slash command fast-path for WebChat chat.send
- fix: replace env!("HOME") with dirs::home_dir() for Windows compatibility
- fix: correct PluginKind::Mcp mapping and remove debug output
- fix: update discovery to find CC-format plugins in installed/ directory
- fix: channel binding not replacing old peer_id rows
- fix: channel status showing disconnected after page refresh
- fix: pass session_manager to BuiltinToolConfig for session tools
- fix: resolve agent from session_key instead of WorkspaceManager
- fix: separate agent identity files from workspace directory
- fix: use bold *name* for agent prefix instead of [name]
- fix: use Markdown (legacy) instead of MarkdownV2 for Telegram messages
- fix: remove backslash escaping from agent name prefix in replies
- fix: override relative working_dir with agent workspace
- fix: change default workspace root from agents/ to workspaces/
- fix: default bash/code_exec working directory to agent workspace
- fix: register JSON Schema for all builtin tools + Codex protocol alignment
- fix: prevent token regeneration on HMAC mismatch to protect vault secrets
- fix: Codex SSE function_call_arguments delta collection + logging
- fix: use vault_key() function instead of undefined VAULT_KEY constant
- fix: unify reranking vault key format with other modules
- fix: reranking Panel fetches per-provider API key from vault
- fix: clear api_key from reranking config signal after save
- fix: isolate rerank API keys per provider in vault
- fix: move rerank API key from config.toml to encrypted vault
- fix: correct default reranking model name in Panel and tests
- fix: ACP panel buttons hang due to spawn_local context loss
- fix: ACP test/save button hang and preset mode defaults
- fix: ACP panel gemini preset ID mismatch and test button hang
- fix: resolve all 75 compilation errors from provider routing refactor
- fix: vault-backed provider API keys and config handler improvements
- fix(acp): adapt harnesses to real CLI protocols after e2e probe testing
- fix: workspace schema migration, workspace.getActive response, and providers page freeze
- fix: remove redundant binding in ConfigPatcher
- fix: session history, agent.list RPC, and embedding dedup
- fix: count only running runs for concurrency limit, reduce cleanup delay
- fix: add multi-dimension vector columns to memories table schema
- fix: hot-swap runtime provider when switching default via Panel UI
- fix: resolve chat quality issues — bootstrap, escalation, and response format
- fix: resolve pre-existing test compilation errors
- fix: wire missing RPC handlers and correct TUI method names
- fix: update remaining port 18789 references to 18790
- fix: unify channel config persistence — Panel UI save/load/connect now works
- fix: resolve compilation errors from feature flag removal
- fix(desktop): address final review — version alignment, input validation, Unicode
- fix(desktop): address clippy needless-borrow warning in agent handler
- fix(desktop): address code quality review — validation, approval gates
- fix(desktop): wire NativeDesktop into registry + complete re-exports
- fix: logic review R2 architecture — 14 findings across 5 categories
- fix: logic review R2 — 29 files across 4 priority batches
- fix: address code review findings for self-configuration
- fix: RAII semaphore guard and env var expansion ordering (Known Issues)
- fix: replace std::sync::RwLock with crate::sync_primitives (P2-15)
- fix: sort HashMap-derived collections for deterministic ordering (P2-14)
- fix: replace SystemTime UNIX_EPOCH .unwrap() with .unwrap_or_default() (P2-12)
- fix: release locks before awaiting in 4 async patterns (P2-11)
- fix: normalize task_type and task_id in SessionKey::task() (P1-9)
- fix: use bounded cast for POE token count u32 conversion (P1-8)
- fix: resolve remaining UTF-8 byte slicing panics (P1-7)
- fix: ConfigPatcher use save_incremental and hard-error on conflict
- fix: logic review Phase 6 — 45 fixes across gateway, memory, poe, exec, providers, and 15 more modules
- fix: resolve 5 remaining Warning-level issues from logic review Phase 5
- fix: logic review Phase 4 — 18 fixes across daemon, engine, secrets, skills, components, cron
- fix: resolve 5 Known Issues from logic review
- fix: comprehensive logic review fixes across 53 files in 77 modules
- fix: use cfg(feature = "loom") instead of cfg(loom) to avoid poisoning dependencies
- fix(gateway): eliminate TOCTOU in execution_engine concurrent run limit check
- fix(gateway): use Mutex for channel_registry take-once inbound_rx pattern
- fix(resilience): simplify governor session_tokens from AtomicU64 to u64
- fix: update doctest to use poe::meta_cognition::BehavioralAnchor
- fix: add Clone derive to NoiseFilter and remove duplicate mod declarations
- fix: remove duplicate scoring_pipeline module declaration in memory/mod.rs
- fix(clippy): resolve print_literal warnings in secret providers command
- fix(tests): migrate secret_boundary_integration tests to async
- fix(runtimes): address critical and important code review findings
- fix: resolve all clippy warnings in aleph-tauri and alephcore
- fix(desktop): use ERR_NOT_IMPLEMENTED for stubbed methods, add debug logging
- fix(halo): address code review findings for view and events
- fix(halo): guard against empty run_id in event handler
- fix(halo): use monotonic counter for unique message IDs, remove redundant phase guard
- fix(desktop): restrict UDS socket to owner-only access
- fix(desktop): add 30s timeout to UDS request to prevent indefinite task hang
- fix(desktop): log evaluateJavaScript errors in Canvas, add runAsync main-thread assert
- fix(desktop): replace deprecated activate(options:) with activate() for macOS 15
- fix(desktop): avoid PNG round-trip in OCR path by sharing captureCurrentScreen
- fix: address code review findings
- fix(desktop): replace strcpy with strncpy to prevent buffer overflow
- fix(desktop): require x/y for click and window_id for focus_window
- fix(desktop): remove misleading serde tags from DesktopRequest, add From conversions
- fix(skill): address code review findings
- fix(skill): resolve clippy warnings in skill module
- fix(skill): use single colon separator for SkillId (matches OpenClaw convention)
- fix(start): add cfg guard for builder mod, tighten handler visibility to pub(in crate::commands::start)
- fix(start): move session banner print into register_session_handlers for consistency
- fix: resolve all compilation errors from server purification
- fix: clean up remaining Server-Client terminology in source comments
- fix: repair 2 broken doc-tests in skill_evolution module
- fix: resolve 8 pre-existing test failures
- fix(control-plane): document AlertsApi integration limitation
- fix(control-plane): complete mock data removal
- fix(control-plane): fix memory leaks and improve error handling in alert subscriptions
- fix(shared-ui-logic): improve error handling in alerts API
- fix(control-plane): use Tailwind CDN for CSS compilation
- fix(control-plane): add WASM initialization in lib.rs
- fix(control-plane): update startup log message to show correct URL
- fix(control-plane): fix root path access and static asset loading
- fix: resolve compilation errors and add missing imports
- fix(dashboard): add wasm_bindgen entry point to enable app initialization
- fix(gateway): extract guest_session_id when require_auth=false
- fix: resolve compilation errors in auth and guest handlers
- fix: use rowid instead of id for sqlite-vec virtual table updates
- fix(phase2): fix RPC tests and update progress report
- fix(cli): use correct method names for session commands
- fix(cli): resolve event streaming issue between gateway and CLI
- fix(cli): align command handlers with gateway API
- fix(memory): handle new SubAgent FactType variants in consolidation
- fix: resolve failing BDD tests for embodiment and CoT transparency
- fix: resolve failing unit tests
- fix: resolve module export and test compilation errors
- fix: resolve all 29 compiler warnings
- fix: add dylib.* pattern to gitignore
- fix: update .gitignore for Aleph rename and remove dylib from tracking
- fix(compressor): fix string concatenation in tests
- fix(protocols): error on nonexistent JSONPath instead of returning null
- fix(scratchpad): use EAFP pattern instead of sync exists() checks
- fix(scratchpad): remove async from exists() and export ScratchpadConfig
- fix(core): fix format strings in manifest.rs and doctest in pty.rs
- fix: clean up remaining MultiTurnCoordinator references
- fix(gateway): remove MultiTurnCoordinator dependency from adapter
- fix(halo): update DependencyContainer comment for HaloInputCoordinator
- fix(halo): update AppDelegate to use HaloInputCoordinator
- fix(halo): update HotkeyService to use HaloInputCoordinator
- fix: update tests for 5 builtin tools and skill evolution
- fix: compilation errors in skill evolution and perception modules
- fix: resolve test compilation errors

### Changed
- refactor: delete ~38K lines of dead dispatcher code
- refactor: rename chatgpt → codex protocol across codebase
- refactor: rename ToolGroup → ToolCategory to avoid confusion with Team
- phase4: clean all Tauri references from codebase
- phase4: remove Tauri, archive old apps, move Swift bridge to crates/desktop-macos/bridge
- refactor: move CLI/TUI/WebChat to interfaces/, client to shared/
- cleanup: remove bootstrap auto-clone and legacy plugin index code
- cleanup: remove AgentLifecycleEvent::Switched and AgentRouter from inbound router
- cleanup: remove agent switching (tool, intent detector, /switch command)
- cleanup: remove unregistered self-management tool source files
- cleanup: remove old subagent tools (spawn/steer/kill + delegate)
- cleanup: move e2e tests into tests/, remove unused shared_ui_logic crate, add secret scanning exclusion
- cleanup: remove temporary debug logging for chatgpt protocol
- refactor: rename workspace to agent across memory/config/paths, enhance agent loop and ChatGPT protocol
- cleanup: remove zombie code, update default config and shared_ui_logic
- cleanup: remove stale ALEPH_MASTER_KEY references from docs and error messages
- refactor: flatten agent_loop/ — remove minimal/ subdirectory
- cleanup: remove deprecated APIs (register_agent_tools, with_working_dir, ToolCategory::Native, PolicyEngine stubs, AuditStore, InvalidateOld)
- refactor: rename Minimal* types to standard names — this IS the loop
- cleanup: fix clippy warning in legacy_adapter detect_entry_point
- cleanup: eliminate all clippy warnings (58→0)
- cleanup: fix clippy warnings (derive Default, redundant closures, simplified conditionals)
- cleanup: remove stale app_bundle_id references from comments and BDD tests
- cleanup: remove TypeScript webchat (replaced by Panel /chat route)
- cleanup: remove dead SubagentAuthority and tools/sessions domain layer
- refactor: simplify memory types, use floor_char_boundary, add mtime cache to daily memory
- refactor(pdf): split pdf_generate.rs into module directory
- refactor: strip #[cfg(feature)] from gateway, server, extension, and misc modules
- refactor: strip #[cfg(feature)] from all 12 channel implementations
- refactor: strip 20+ Cargo feature flags from core crate
- refactor: Occam's Razor pass — eliminate clippy warnings and dead code
- cleanup: remove fastembed and local embedding model remnants
- cleanup: fix unused import in host_functions.rs
- refactor(wasm): simplify PermissionChecker to facade over WasmCapabilities
- cleanup: broad DRY refactoring and clippy compliance across codebase
- cleanup: remove stale fastembed references, fix integration tests
- cleanup: remove macOS-specific CI workflow and build scripts (C8-C12)
- cleanup: remove deprecated macOS Swift app (C7)
- cleanup: remove UniFFI Swift bindings (C1-C2)
- refactor(core): introduce register_handler! macro, eliminate handler boilerplate (Wave 4)
- refactor(core): replace &Vec<T> with &[T] in arrow_convert and shadow_replay (Wave 3B)
- refactor(core): convert InternalEventHandler String params to &str (Wave 3A)
- refactor(core): manual Clippy fixes — expect_fun_call, useless_vec, ptr_arg, type_complexity, module_inception, needless_borrows, and more (Wave 2B)
- refactor(core): replace Default::default() field reassignment with struct literals (Wave 2A)
- refactor(core): auto-fix Clippy warnings and remove unused imports (Wave 1)
- refactor(runtimes): delete old runtime managers, replace with Ledger/Probe system
- refactor(video): replace RuntimeRegistry with CapabilityLedger in caption.rs
- refactor(init): replace forced runtime installation with zero-install ledger
- refactor(desktop): delete RPC proxy commands and clean up dead code (~1600 lines)
- refactor(halo): delete React frontend source from Tauri app
- refactor(halo): point Tauri halo window to Leptos server URL
- refactor(halo): delete legacy Swift Halo views and fix references (~4500 lines removed)
- refactor(start): split initialize_auth, extract load_app_config, restore register calls to orchestrator
- refactor(start): move register_* handler functions to commands/builder/handlers.rs
- refactor(extension): thin mod.rs facade, delegate load_all to ComponentLoader
- refactor(start): extract subsystem initializers from start_server
- refactor: remove distributed execution infrastructure (ExecutionPolicy, ClientManifest, ReverseRpc, ToolRouter, RoutedExecutor)
- refactor: clean up auth handler by removing ClientManifest references
- refactor: simplify gateway server by removing client routing infrastructure
- refactor: simplify ExecutionEngine by removing client routing
- refactor: rename gateway/channels/ to gateway/interfaces/
- refactor: rename clients/ to apps/
- cleanup: remove unused imports from exec_security_gate (post-rebase)
- cleanup: fix Arc misuse, large variants, and private interfaces (Pass 3 final)
- cleanup: extract type aliases and parameter structs (Pass 3)
- cleanup: suppress module_inception for intentional nested module pattern
- cleanup: fix 22 miscellaneous clippy warnings
- cleanup: Pass 2 local refactoring (clone, strip_prefix, dead code, redundant closures)
- cleanup: fix boolean simplifications, identity ops, and &PathBuf signatures
- cleanup: remove unused imports and replace derivable impls
- cleanup: apply cargo clippy --fix auto-corrections
- refactor(control-plane): split Sidebar into sidebar/ directory
- refactor(control-plane): use nested routes for Settings with SettingsLayout
- refactor(control-plane): remove /cp prefix from routing
- refactor(core): rename aleph-gateway to aleph-server
- refactor(macos): completely remove settings UI from macOS client
- refactor(desktop): completely remove settings UI from Tauri client
- refactor(desktop): migrate Plugins, Skills, and Policies settings to Dashboard
- refactor(clients): complete Phase 4 - remove Generation Providers UI
- refactor(clients): migrate Providers, Memory, and MCP config to Dashboard
- refactor(agent_loop): introduce RunContext pattern for cleaner API
- refactor(agent-loop): add RunContext structure (WIP)
- refactor(domain): implement Newtype pattern for Answer and Ruleset
- refactor(domain): implement Newtype pattern for 5 ID types
- refactor(api): implement FromStr trait for remaining types
- refactor(api): implement FromStr trait for extension and resilience types
- refactor(api): implement FromStr trait for memory context types
- refactor(perf): replace trim_start_matches with strip_prefix for fixed prefixes
- refactor(perf): optimize &PathBuf → &Path in 6 files
- refactor(core): add #[allow(dead_code)] to 12 reserved fields
- refactor(deps): remove 5 unused dependencies
- refactor(core): remove 2 confirmed dead code items
- refactor(core): remove 160+ unused imports across 50 files
- refactor(tools): extract builtin tool registration and types (Phase 6)
- refactor(gateway): modularize plugins handlers (Phase 5.1)
- refactor(poe): extract services to dedicated modules (Phase 4.2 - P1)
- refactor(poe): extract handler types to dedicated modules (Phase 4.1 - P0)
- refactor(browser): extract types and scripts modules (Phase 3 - Part 1)
- refactor(engine): complete atomic executor composition refactoring (Phase 2)
- refactor(engine): add atomic module base architecture (Phase 2 WIP)
- refactor(extension): split types.rs into modular structure
- refactor(security): transform PolicyEngine to stateless
- refactor(protocol): add equality derives and helper methods to auth types
- refactor(phase1): reorganize client directory structure
- refactor: complete final Aether to Aleph cleanup
- refactor: complete Aether to Aleph rename - scripts, workflows, and remaining code
- refactor: complete Aether to Aleph rename across entire codebase
- refactor(providers): use ProtocolRegistry in create_provider factory
- refactor(providers): remove technical alias presets
- refactor(config): remove provider_type field from ProviderConfig
- refactor: fix P3 clippy warnings - batch 2
- refactor: fix P3 clippy warnings - batch 1
- refactor: fix P1/P2 clippy warnings and improve code quality
- refactor(providers): delete legacy OpenAiProvider
- refactor(providers): delete legacy GeminiProvider
- refactor(providers): delete legacy ClaudeProvider
- refactor(providers): use HttpProvider for Anthropic protocol
- refactor(providers): remove redundant vendor wrappers (~850 lines)
- refactor(providers): use HttpProvider for OpenAI protocol in factory
- refactor(macos): cleanup and improve hotkey/halo components
- refactor(halo): replace HaloState with simplified 6-state version
- refactor(halo): switch HaloWindow to V2 components
- refactor(halo): remove MultiTurn references from EventHandler
- refactor(halo): remove MultiTurn directory (~3000 lines)
- refactor: split large modules into smaller files
- cleanup: remove unused modules and merge thinking into thinker
- cleanup: eliminate all compilation warnings
- cleanup(lib): slim down exports from 590 to 272 lines
- cleanup: remove FFI-related comments
- cleanup: rename FFI types to standard names
- cleanup(dispatcher): rename ffi.rs to tool_info.rs
- cleanup(intent): remove Type A FFI residuals

### Build
- docs: add official repositories to CLAUDE.md, fix security headers
- docs: add generation provider isolation implementation plan
- docs: add generation provider isolation and URL normalization spec
- docs: add media attachment infrastructure implementation plan
- docs: fix second review issues in media attachment spec
- docs: update media attachment spec — fix review issues and unify temp files
- docs: add media attachment infrastructure design spec
- docs: add model routing optimization implementation plan
- docs: add full-chain security hardening implementation plan
- docs: fix security spec based on review feedback
- docs: update model routing spec with review feedback
- docs: add full-chain security hardening design spec
- docs: add model routing optimization design spec
- docs: add generation tools unification spec and plan
- docs: add implementation plan for self-management and telegram resilience
- docs: spec v3 — full skill-ization + skills repo separation
- docs: fix spec review issues — backoff off-by-one, reuse ChannelStatus::Connecting
- docs: add self-management system and telegram resilience design spec
- ci: remove install instructions from release body, keep changelog only
- ci: include changelog in GitHub Release page body
- release: v0.2.11
- build: fix install scripts — proper upgrade flow and service management
- release: v0.2.10
- docs: add skill scope filtering implementation plan
- docs: fix skill scope filtering spec per review
- docs: add skill scope filtering design spec
- release: v0.2.9
- docs: add voice conversation implementation plan
- docs: fix PromptBuilder voice state access path in voice spec
- docs: update voice conversation spec with review fixes
- docs: add voice conversation system design spec
- docs: add release workflow and version management to CLAUDE.md
- release: v0.2.8
- build: unify version source — VERSION file drives all version strings
- release: v0.2.8
- docs: add multimodal probe tests implementation plan
- docs: add multimodal probe tests design spec
- docs: add core multimodal enhancement implementation plan
- docs: fix spec review issues in core multimodal design
- docs: add core multimodal enhancement design spec
- docs: add Telegram channel enhancement implementation plan
- docs: fix spec review issues in Telegram enhancement design
- docs: add Telegram channel enhancement design spec
- docs: add Feishu enhanced features implementation plan
- docs: address spec review — FeishuEventEmitter, typing lifecycle, capabilities
- docs: add Feishu enhanced features design spec
- docs: add Feishu channel implementation plan
- docs: address spec review feedback for Feishu channel
- docs: add Feishu/Lark channel design spec
- release: v0.2.7 — multi-agent system, UI updates, bug fixes
- docs: fix spec issues from review — stale final_text, test plan, consecutive_errors
- docs: add Persistent Completion Protocol design spec
- docs: fix multi-agent modes spec per review findings
- docs: add multi-agent modes taxonomy design spec
- docs: add task coordination implementation plan (12 tasks)
- docs: fix event type conventions in task coordination spec
- docs: address spec review findings for task coordination
- docs: add task coordination system design spec
- build: update WASM panel dist
- ci: upgrade GitHub Actions to Node.js 24 compatible versions
- ci: scope fmt check to maintained crates (skip legacy formatting issues)
- build: consolidate to single release workflow, fix CI protoc dependency
- build: remove archive from git (large binaries exceed GitHub limit)
- release: bump version to 0.2.6
- build: update install scripts for aleph-server binary name
- build: rename workflows, fix --bin aleph→aleph-server, add platform release workflows
- build: update justfile and CI workflows for post-Tauri architecture
- build: add swift-bridge recipe to justfile for macOS native APIs
- docs: add Phase 3 implementation plan for macOS PIM & system capabilities
- docs: add Phase 2 implementation plan for screen control native migration
- docs: address spec review feedback for hierarchical commands
- docs: add hierarchical slash commands design spec
- docs: add Phase 1 implementation plan for desktop native capabilities
- docs: add desktop native capabilities design spec
- docs: update design spec with new directory structure
- docs: add implementation plan for intermediate message delivery
- docs: add PLUGIN_SYSTEM.md — CC-compatible plugin architecture reference
- docs: address spec review feedback for CLI/TUI separation
- docs: add CLI/TUI separation design spec
- docs: add P4 runtime migration implementation plan
- docs: add prompt guidance as in-scope changes to intermediate message spec
- docs: add edge cases to intermediate message delivery spec
- docs: add intermediate message delivery design spec
- docs: add P3 scope management implementation plan
- docs: add P2 marketplace system implementation plan
- docs: add P0+P1 implementation plan for plugin CC compat
- docs: fix remaining spec review items (round 2)
- docs: address spec review findings for plugin compat design
- docs: add plugin system Claude Code compatibility redesign spec
- docs: update spec and plan — keep peer_id signatures unchanged
- docs: update agent-bot 1:1 binding spec with review fixes
- docs: add agent-bot 1:1 binding simplification design spec
- docs: add chat sidebar redesign spec and implementation plan
- docs: add panel agent routing fix design spec
- docs: add workspace output migration implementation plan
- docs: revise workspace output migration spec after review
- docs: add workspace output migration design spec
- docs: add generation providers wiring implementation plan
- docs: fix generation providers spec after review
- docs: add generation providers wiring design spec
- docs: add ClawHub integration implementation plan
- docs: address spec review feedback for ClawHub integration
- docs: add ClawHub integration design spec
- ci: upgrade GitHub Actions to Node.js 24, fix Windows dead-code warnings
- docs: fix plan review issues (3 blockers + 6 warnings)
- docs: address spec review feedback for Chrome DevTools MCP Mode
- docs: add Chrome DevTools MCP Mode design spec
- docs: add process management rules to CLAUDE.md
- docs: add tool permission system implementation plan
- docs: update tool permission spec after review
- docs: add tool permission system design spec
- docs: add ACP probe tests design document
- docs: add ACP harness management implementation plan
- docs: add ACP harness management design document
- docs: add provider routing refactor implementation plan
- docs: fix remaining spec review issues
- docs: fix spec issues from review
- docs: add provider routing refactor design spec
- docs: add provider config testing implementation plan
- docs: update provider config testing spec after review
- docs: add provider config testing design spec
- docs: add simplify-model-config implementation plan
- docs: update simplify-model-config spec after review
- docs: add simplify-model-config design spec
- ci: read release version from VERSION file instead of manual input
- docs: add cron probe tests implementation plan
- docs: add cron probe tests design spec
- docs: add cron module redesign implementation plan
- docs: add cron module redesign spec
- build: rebuild panel WASM and update docs after worktree merges
- docs: add provider zero-config implementation plan
- docs: add message pipeline implementation plan
- docs: add provider zero-config UX design spec
- docs: add message pipeline design for gateway pre-processing
- docs: add model discovery probe tests implementation plan
- docs: add model discovery probe tests design spec
- docs: add model discovery implementation plan
- docs: fix model discovery spec issues from review
- docs: add model discovery design spec
- docs: add cognitive evolution beta implementation plan
- docs: add cognitive evolution beta design (immune-complete loop)
- docs: add POE Phase 2+3 implementation plan
- docs: add POE Phase 1 implementation plan (BlastRadius + Taboo)
- docs: add POE Architecture Evolution Whitepaper 2026
- ci: fix Linux/Windows compilation errors for missing imports
- docs: update extension system architecture documentation
- docs: add unified plugin system implementation plan
- docs: add unified plugin system design
- docs: add one-line install commands as primary installation method
- docs: remove refactoring backstory from intent section
- docs: update intent detection section to reflect unified LLM pipeline
- docs: add detailed Aleph vs OpenClaw comparison
- docs: add P4.3 core plugins implementation plan
- docs: add plugin development guide
- docs: add P4 plugin ecosystem implementation plan
- ci: add Windows x86_64 build target and PowerShell installer
- docs: add P3 media pipeline implementation plan
- ci: fix Linux warn import, remove darwin-x86_64 target
- ci: add libxdo-dev for Linux, fix darwin x86_64 AVX-512 link error
- ci: fix Linux pipewire compat (ubuntu-24.04) and macOS x86_64 openssl
- ci: add libegl and X11 extension deps for Linux build
- ci: use macos-latest for x86_64 cross-compile (macos-13 EOL)
- ci: add dbus, drm, gbm deps for Linux build
- ci: add pipewire and clang deps for Linux xcap build
- ci: add libwayland-dev to Linux build dependencies
- docs: add author note to README
- docs: rename panel screenshots with consistent numbering
- docs: restore dashboard screenshot, keep all 3 panel images
- docs: update README screenshots with Panel chat and settings views
- build: remove webchat recipes from justfile
- docs: add webchat Rust rewrite implementation plan
- docs: add webchat Rust rewrite design
- docs: remove acknowledgments section from README
- ci: enable all platform build targets for server release
- ci: add manual server release workflow and improve install script
- docs: overhaul README.md, CLAUDE.md and add LICENSE
- docs: add inline directives and legacy cleanup implementation plan
- docs: add inline directives and legacy cleanup design
- docs: add language-agnostic intent detection implementation plan
- docs: add language-agnostic intent detection design
- docs: update cleanup plan with execution results
- docs: clarify cleanup strategy — scoped responsibility, not fallback
- docs: add multi-agent code redundancy cleanup plan
- docs: add A2A protocol implementation plan
- docs: add A2A protocol design document
- docs: add per-agent tool configuration implementation plan
- docs: add per-agent tool configuration design
- docs: add multi-bot Panel UI implementation plan
- docs: add multi-bot Panel UI design
- docs: add multi-bot channel implementation plan
- docs: add multi-bot channel support design
- docs: add memory alignment design for dual-directory architecture
- docs: add agent-workspace separation implementation plan
- docs: add agent-workspace separation design
- docs: add agent management panel implementation plan
- docs: add agent management panel design
- docs: add webchat restructure implementation plan
- docs: add webchat restructure design
- docs: add agent switching enhancement implementation plan
- docs: add agent switching enhancement design
- docs: add unified command registry implementation plan
- docs: add unified command registry design
- docs: add dynamic agent switching implementation plan
- docs: add dynamic agent switching design
- docs: add system prompt optimization implementation plan
- docs: add system prompt architecture optimization design
- docs: add Agent/Workspace/Session unification implementation plan
- docs: add Agent/Workspace/Session relationship design
- docs: add task routing decision layer implementation plan
- docs: add task routing decision layer design
- docs: add architecture activation diagnostic report
- docs: add architecture activation diagnostic implementation plan
- docs: add architecture activation diagnostic design
- docs: add native tool_use implementation plan (9 tasks)
- docs: add native tool_use migration design
- docs: add PDF dual-engine implementation plan
- docs: add PDF dual-engine rendering design
- docs: add cron and group chat backend implementation plan
- docs: add cron and group chat backend implementation design
- docs: add scheduled tasks panel implementation plan
- docs: add scheduled tasks panel design
- docs: add CLI full RPC coverage implementation plan
- docs: add CLI full RPC coverage design
- docs: add CLI bugfix and JSON unification design
- docs: add CLI full commands implementation plan
- docs: add CLI full commands design
- docs: add CLI infrastructure enhancement implementation plan
- docs: add CLI infrastructure enhancement design
- docs: add lifecycle observability logging implementation plan
- docs: add lifecycle observability logging design
- docs: add system prompt enhancement implementation plan
- docs: add system prompt enhancement design
- docs: add agent system Phase 2 full coverage implementation plan
- docs: add agent system full coverage design (Phase 2)
- docs: add Codex panel UI design and implementation plan
- docs: add Codex Responses API implementation plan
- docs: add Codex Responses API protocol adapter design
- docs: add gateway enhancement implementation plan (20 tasks)
- docs: add gateway enhancement design (OpenClaw-inspired)
- docs: add implementation plan for agent/workspace/binding
- docs: add agent definition + workspace + binding design
- docs: add OpenAI subscription provider implementation plan
- docs: add OpenAI subscription provider design
- docs: add Lazy POE Activation design
- build: rename just server → just build, add just all
- docs: update binary name and port references across all documentation
- build: enable axum ws feature for port unification
- docs: add port unification implementation plan
- docs: add port unification and binary rename design
- docs: add channel infrastructure fix implementation plan
- docs: add channel infrastructure fix design
- docs: update CLAUDE.md for feature flag removal
- build: simplify justfile — remove all --features flags
- docs: add runtime channel control implementation plan
- docs: add runtime channel control design — eliminate feature flag fragmentation
- docs: add chat persistence & memory pipeline implementation plan
- docs: add chat persistence & memory pipeline fix design
- docs: add full chain + smart recall implementation plan
- docs: add full chain + smart recall design
- docs: add workspace enhancements implementation plan (9 tasks)
- docs: add workspace enhancements design (4 features)
- docs: add workspace wiring implementation plan (11 tasks)
- docs: add workspace wiring design for multi-role persona system
- docs: add config externalization implementation plan
- docs: add config externalization design for ~/.aleph workspace
- ci: keep only macOS ARM64 build, document other platform blockers
- ci: fix remaining build issues across platforms
- ci: fix cross-platform build issues
- ci: pin wasm-bindgen-cli to 0.2.108 matching Cargo.lock
- ci: allow test job to fail without blocking builds
- ci: add X11/xscrnsaver dev libraries for Linux builds
- ci: install protoc for lance-encoding build dependency
- ci: improve release workflow with WASM build, test job, and cross-platform desktop
- build: rewrite justfile for desktop-as-muscle architecture
- docs: add crates/desktop to project structure and build commands
- docs: add Desktop-as-Muscle implementation plan
- docs: add Desktop-as-Muscle architecture design
- docs: add self-configuration implementation plan
- docs: add self-configuration design document
- ci: add loom concurrency test job and increase proptest coverage
- build: add test-proptest, test-loom, test-logic just recipes
- docs: add logic review system implementation plan (15 tasks, 49 properties)
- docs: add logic review system design (three-layer defense architecture)
- docs: move obsolete embedding/sqlite-vec plans to legacy
- docs: update memory system docs to reflect remote embedding migration
- build: replace trunk with manual WASM pipeline in justfile
- docs: fix macOS Resources path in build pipeline design
- build: add justfile for unified build pipeline
- docs: add unified build pipeline design
- docs: add channel config panel implementation plan
- docs: add channel config panel design document
- docs: add POE full evolution implementation plan (19 tasks, 4 phases)
- docs: add POE full evolution design (event-driven closed loop)
- docs: add WASM capability kernel implementation plan
- docs: add WASM capability kernel design
- docs: add macOS PIM native API implementation plan
- docs: add macOS PIM native API integration design
- docs: add POE cognitive hub implementation plan
- docs: add POE cognitive hub upgrade design
- docs: add social bot channels expansion implementation plan
- docs: add social bot channels expansion design
- docs: add surgical DRY refactoring implementation plan
- docs: add surgical DRY refactoring design for embedding provider files
- docs: add embedding provider LLM migration implementation plan
- docs: add embedding provider LLM migration design
- docs: add large file refactoring implementation plan — 6 tasks, 5 files
- docs: add large file refactoring design — 5 files, pure module splitting
- ci: add server, macOS app, and Tauri release workflows
- docs: add distribution implementation plan (24 tasks, 9 phases)
- docs: add distribution architecture design
- docs: add PromptPipeline implementation plan — 10 tasks, TDD, strangler fig
- docs: add PromptPipeline design — Trait-per-Layer evolution from Plan A
- docs: add automation skills implementation plan
- docs: add automation skills (#21-30) design
- docs: add memory event sourcing implementation plan
- docs: add memory event sourcing design (CQRS Light)
- docs: add prompt system enhancement implementation plan
- docs: add prompt system enhancement design
- docs: add skills system, update runtimes refs, add macOS components
- docs: update acceptance results after bridge fixes (27/30 pass)
- docs: add implementation plan for fixing bridge known issues
- docs: add design for fixing bridge known issues
- docs: remove remaining Swift references from CLAUDE.md
- docs: update CLAUDE.md and create migration completion record (C13-C16)
- docs: add macOS Swift app removal implementation plan
- docs: add macOS Swift app removal design with acceptance criteria
- docs: add desktop capabilities evolution implementation plan
- docs: add desktop capabilities evolution design
- docs: add semantic targeting implementation plan
- docs: add semantic targeting and action primitives design
- docs: update CLAUDE.md for Server-Centric Build Architecture
- docs: add Phase 3 and Phase 4 implementation plans
- docs: replace Ghost aesthetic with concrete product constraints R5-R7
- docs: add Phase 2.5 bridge integration completion plan
- docs: add design for removing Ghost aesthetic concept
- docs: add Phase 1 bridge skeleton implementation plan
- docs: add server-centric build architecture design
- docs: update worktree guidelines with EnterWorktree CWD lock caveat
- docs: add cron system redesign plan — surpassing openclaw
- docs: add memory optimization implementation plan
- docs: add memory module optimization design
- docs: address code review findings (JIT-approval TODO, RwLock rationale)
- docs: bring in Late-Binding Secure Execution design and plan from main
- docs: add Late-Binding Secure Execution implementation plan (14 tasks, 4 waves)
- docs: add Late-Binding Secure Execution Architecture design
- docs: add git worktree safety guide; fix missing ScreenRegion import
- docs: add Rust refactoring implementation plan (7 tasks, 4 waves)
- docs: add Rust core refactoring design (4-wave strategy)
- docs: add runtime on-demand implementation plan (13 tasks, 4 phases)
- docs: add runtime on-demand implementation plan (13 tasks, 4 phases)
- docs: add runtime on-demand native bootstrapping architecture design
- docs: add verification test results to Tauri shell design doc
- docs: add Tauri cross-platform shell implementation plan
- docs: add Tauri cross-platform shell & DesktopBridge design
- build(halo): rebuild WASM with /halo route
- docs: split CLAUDE.md and reorganize docs/ into docs/reference/
- docs: add 1-2-3-4 architecture constitution design document
- docs: add Halo UI Unification implementation plan (10 tasks)
- docs: establish 1-2-3-4 architecture model as constitutional principles in CLAUDE.md
- build(macos): add WebKit framework dependency for Settings WebView
- docs: add Phase 1 implementation plan — Settings WebView integration
- docs: add UI unification design — Leptos as single UI codebase
- docs: add Desktop Bridge implementation plan (11 tasks, 4 phases)
- docs: add Desktop Bridge design for UDS-based Swift-Rust IPC
- docs: add Skill System v2 implementation plan (15 TDD tasks)
- docs: add Skill System v2 design (complete DDD rebuild)
- docs: update all documentation for server-centric architecture
- docs: update CLAUDE.md for server-centric architecture
- docs: add server purification implementation plan
- docs: add server purification design - remove desktop control, embrace MCP plugins
- docs: add Skill System implementation plan with 14 TDD tasks
- docs: add server-centric architecture implementation plan
- docs: add server-centric architecture reframing design
- docs: add Skill System domain-driven design document
- docs: add P0 refactoring implementation plan for start.rs and extension/mod.rs
- docs: add CODE_ORGANIZATION guide with refactoring backlog
- docs: add social connectivity evolution design and implementation plan
- build: add missing imports in control-plane cfg block
- docs: add IronClaw Phase 2/3 detailed implementation plan
- docs: add IronClaw Phase 2/3 design (host-boundary + EVM signing)
- docs: add code cleanup implementation plan (16 tasks, 3 passes)
- docs: add code cleanup design plan (Occam's Razor Pass)
- docs: add ACMA implementation plan with 7 TDD tasks
- docs: add ACMA (Aleph Cognitive Memory Architecture) design document
- docs: add exec security integration design
- docs: add blog post on PII filtering gateway implementation
- docs: add agent secret management implementation plan
- docs: add agent secret management design (Phase 1)
- docs: add Discord Control Plane implementation plan
- docs: add Discord Control Plane panel design
- docs: add memory workspace implementation plan
- docs: add memory workspace isolation design
- docs: update architecture docs to reflect LanceDB migration
- docs: add WhatsApp Bridge implementation plan (10 tasks)
- docs: add WhatsApp Bridge design (Thin Sidecar + Rich Adapter)
- docs: update MEMORY_SYSTEM.md and CLAUDE.md for LanceDB migration
- docs: embedding evolution implementation plan (13 tasks)
- docs: embedding evolution design (abstract provider + lazy migration)
- docs: add Memory VFS Evolution implementation plan
- docs: add Memory VFS Evolution design document
- docs: add Swarm Agent Loop integration implementation plan
- docs: add Swarm Intelligence Architecture Agent Loop integration design
- docs(ssb): add Phase 6 cross-platform implementation plan
- docs(ssb): add cross-platform architecture design
- docs: clarify server-side execution model in CLAUDE.md
- docs(ssb): add Phase 6 enhancement plan and complete roadmap
- docs: add Swarm Intelligence Architecture design
- build(control-plane): update compiled UI assets for Phase 3
- docs: add System State Bus (SSB) architecture design
- docs(skill-evolution): add comprehensive documentation and examples
- docs: add Collaborative Skill Evolution architecture design
- docs: add detailed implementation plan for Control Plane three-column layout
- docs: add Control Plane three-column layout architecture design
- docs: update Control Plane UI build workflow with Tailwind CSS compilation
- docs(claude.md): add WASM initialization mechanism explanation
- docs(claude.md): add comprehensive Server development and deployment guide
- docs: add UI comparison analysis for ControlPlane and Tauri settings
- docs: add WebSocket client implementation summary and migration plan
- docs: add ControlPlane integration implementation summary
- docs: add Phase 3 implementation plan
- docs: add Phase 3 design for skill sandboxing
- docs: add comprehensive skill sandboxing documentation
- docs: add Phase 2 skill sandboxing implementation plan
- docs: add Phase 2 skill sandboxing design document
- docs(shared-ui-logic): mark API Layer as complete
- docs(shared-ui-logic): mark WASM connector as complete
- docs(shared-ui-logic): update README with API and Observability progress
- docs(shared_ui_logic): update README with protocol layer status
- docs(shared_ui_logic): update README with native connector status
- docs(shared_ui_logic): add comprehensive README
- docs: add shared_ui_logic design document
- docs: complete Phase 3 architecture documentation
- docs: add Phase 1 implementation plan for skill sandboxing
- docs: add skill sandboxing architecture design
- docs(architecture): add comprehensive cleanup design document
- docs: reorganize root directory and establish documentation structure
- docs(architecture): add Phase 3 browser refactoring design
- docs(architecture): add Phase 6 tools server refactoring design
- docs(architecture): add Phase 5 plugins handlers refactoring design
- docs(architecture): add Phase 4 POE handlers refactoring design
- docs: add Phase 2 continuation guide for next session
- docs(architecture): add Phase 2 atomic executor refactoring design
- docs(architecture): add Phase 1 types refactoring design
- docs(cortex): add Month 3 implementation plan
- docs(cortex): add Month 3 Meta-Cognition Layer design
- docs: add Atomic Engine final implementation report
- docs: add comprehensive Atomic Engine documentation
- docs: add Atomic Engine progress report (90% complete)
- docs: add Atomic Engine short-term task completion status
- docs: add Cortex evolution system design
- docs: add Atomic Engine evolution roadmap (3-12+ months)
- docs: add atomic engine implementation status report
- docs: add language preference to CLAUDE.md
- docs: add Phase 2 Intelligent Scheduling design
- docs: add guest session activity logging implementation plan
- docs: add Liquid Hub cross-platform architecture design
- docs: complete Identity Context security documentation
- docs: add Identity Context & Security Enforcement design
- docs: add ConfigManager and Memory Namespace implementation plan
- docs: add ConfigManager and Memory Namespace design
- docs: add Personal AI Hub implementation plan
- docs: add Personal AI Hub architecture design
- docs: add client architecture documentation and testing guide
- docs: add Phase 2 progress report
- docs: add client architecture refactoring plan
- docs: document Server-Client architecture in CLAUDE.md
- docs: add Server-Client implementation plan
- docs: add Server-Client architecture design
- docs: add DDD terminology and domain modeling guide
- docs: add DDD+BDD dual-wheel architecture design
- docs: add comprehensive Tool-as-Resource usage guide and update Phase 4 status
- docs: update Phase 3 progress - L2 and observability completed
- docs: update Phase 2 checkboxes to completed
- docs: update MEMORY_SYSTEM.md with Memory Evolution features
- docs(bdd): add comprehensive BDD testing guide and update plans
- docs: add Phase 3 implementation plan
- docs: mark Phase 2 as complete with all tasks done
- docs: document Phase 2 memory system components in TOOL_SYSTEM.md
- docs: update Phase 2 plan with completion status
- docs: update implementation plan with completion summary
- docs: add Phase 1 MVP implementation plan
- docs: add Multi-Agent 2.0 Phase 1 implementation plan
- docs: add memory system evolution design
- docs: add Multi-Agent Resilience documentation
- docs: update Phase 1 checkboxes to completed
- docs: update Tool-as-Resource design status to In Progress
- docs: add Tool-as-Resource implementation plan
- docs: add Multi-Agent Resilience & Governance architecture design
- docs: add Tool-as-Resource architecture design
- docs: add Embodiment Engine and CoT Transparency documentation
- docs: add Multi-Agent 2.0 architecture design
- docs(plans): add Embodiment Engine & CoT Transparency design
- docs(agent-system): add Channel Capability Awareness documentation
- docs: add channel capability awareness implementation plan
- docs: add channel capability awareness architecture design
- docs: add workspace architecture design
- docs: add Phase 5 implementation plan
- docs: add Phase 5 Custom Rules Engine architecture design
- docs: add WorldModel + Dispatcher architecture design
- docs(daemon): add perception layer documentation
- docs: add Protocol Adapter Phase 4 implementation summary
- docs(architecture): document configurable protocol adapter system
- docs(protocols): add comprehensive protocol adapter user guide
- docs: add Phase 2 Perception Layer implementation plan
- docs(protocols): add example YAML protocol configurations
- docs: add Phase 2 Perception Layer design
- docs: add daemon module documentation
- docs: add Phase 1 daemon implementation plan
- docs: add proactive AI architecture design
- build: remove deprecated cabi feature and fix Discord API
- docs: add comprehensive Markdown Tool Adapter implementation summary
- docs: add Protocol Adapter Phase 4 design
- docs: add Markdown Tool Adapter design specification
- docs: add Protocol Adapter Phase 3 implementation summary
- docs: add Protocol Adapter Phase 2 implementation summary
- docs: add Protocol Adapter Phase 2 implementation plan
- docs: add Protocol Adapter Phase 2 design for Claude/Gemini migration
- docs(providers): update module documentation for Protocol Adapter architecture
- docs: add Protocol Adapter implementation plan
- docs: add Protocol Adapter architecture design
- docs(plans): add P2.5 MCP Advanced Features implementation plan
- docs(mcp): add P2 advanced features implementation plan
- docs: add Memory v3 implementation plan with bite-sized TDD tasks
- docs(mcp): add P1 capabilities implementation plan
- docs: add Memory System v3 "Glass Box" architecture design
- docs(mcp): add MCP Orchestration Layer implementation plan
- docs(mcp): add MCP Orchestration Layer design
- docs(cortex): add detailed implementation plan with TDD steps
- docs(extension): add P0.5-P2 feature documentation
- docs(extension): add P0.5-P2 implementation plan
- docs(extension): add SDK V2 documentation
- docs(dispatcher): add Cortex 2.0 architecture design
- docs(extension): add SDK V2 P0 implementation plan
- docs(extension): add Aether Extension SDK V2 design specification
- docs(skills): add detailed implementation plan for requirements feature
- docs(skills): add requirements & CLI wrapper architecture design
- docs(poe): add contract signing design for first principles closure
- docs: update memory system docs and add halo command system plan
- docs: add message flow optimization design and implementation plan
- docs: add Halo-Only message flow design and implementation plan
- docs: add comprehensive architecture documentation
- docs: add detailed POE implementation plan
- docs: add POE (Principle-Operation-Evaluation) architecture design
- docs: add Agent-Action interaction implementation plan
- docs: add Agent-Action interaction system design
- docs: mark Milestone 6 (ResilientTask) as complete
- docs: add Rust layer code cleanup design plan
- docs: add Milestone 6 resilient task implementation plan
- docs: mark Milestone 5 (skill evolution) as complete
- docs: add Milestone 5 skill evolution implementation plan
- docs: mark Milestone 4 (spec-driven dev) as complete
- docs: add Milestone 4 spec-driven development implementation plan
- docs: mark Milestone 3 (Telegram approval) as complete


## [0.2.11] - 2026-03-23

### Added
- webchat: add i18n infrastructure with leptos_i18n v0.6
- panel: i18n all pages — dashboard, chat, settings, agents, cron, memory, logs
- panel: i18n settings pages — plugins, skills, clawhub, policies, acp, providers
- panel: wire language switching in general settings

### Changed
- core: dead code cleanup — remove unused modules (question, spec_driven, suggestion)
- core: plugin discovery and manifest parsing improvements
- core: prompt builder and skill instruction updates

### Fixed
- build: fix install scripts — proper upgrade flow and service management
- panel: fix i18n reactivity, remove dead code

## [0.2.10] - 2026-03-23

### Added
- core: gemini provider improvements
- core: generation builder enhancements
- core: telegram interface updates

### Changed
- core: rerank providers (jina, pinecone, siliconflow, vllm, voyage) improvements
- core: memory embedding provider updates
- webchat: settings UI refinements

## [0.2.9] - 2026-03-22

### Added
- core: codex provider improvements
- core: agent loop updates

### Changed
- webchat: panel UI refresh

## [0.2.8] - 2026-03-22

### Added
- build: unified version source — VERSION file drives all version strings
- build: `just release x.x.x` automated release recipe with changelog generation
- desktop-macos: implement AutomationCapability (osascript + Shortcuts CLI)
- desktop-macos: implement SystemCapability (apps, notifications, clipboard, sysinfo)
- desktop-macos: implement PimCapability via SwiftBridge
- apps: implement real macOS API calls in Swift CLI bridge (Notes, Calendar, Reminders, Contacts)
- desktop: add NativeScreen shared ScreenCapability implementation

### Changed
- core: rewire DesktopTool to dispatch via DesktopPlatform.screen()
- core: rewire PimTool to dispatch via DesktopPlatform.pim()
- core: remove legacy NativeDesktop, use DesktopPlatform for screen control
- phase4: remove Tauri, archive old apps, move Swift bridge to crates/desktop-macos/bridge
- phase4: clean all Tauri references from codebase
- build: rename workflows, fix --bin aleph→aleph-server, add platform release workflows
- build: update justfile and CI workflows for post-Tauri architecture

### Fixed
- fix: replace env!("HOME") with dirs::home_dir() for Windows compatibility
- build: update install scripts for aleph-server binary name

## [0.2.7] - 2026-03-22

### Added
- core: multi-agent system improvements
- webchat: UI updates

## [0.2.6] - 2026-03-21

### Added
- desktop: add capability trait hierarchy (Screen, PIM, System, Automation)
- desktop: add per-platform crate skeletons (macOS, Linux, Windows)
- desktop: add SwiftBridge utility for macOS native API calls
- core: add SystemTool and AutomationTool builtin tools
- apps: add Swift CLI bridge skeleton for macOS native APIs

### Changed
- core: wire up DesktopPlatform and register system/automation tools
- desktop: Phase 1 architecture scaffold complete
