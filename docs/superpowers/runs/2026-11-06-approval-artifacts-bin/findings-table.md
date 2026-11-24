| # | ID | Batch | P | Finding | Verdict |
|---|----|-------|---|---------|---------|
| 1 | `B1-01` | B1 | P1 | The approval cascade resolves sibling cards whose originator never consented — the group-chat approval gate protects only the clicked id | fixed |
| 2 | `B1-02` | B1 | P2 | The originator gate fails open for any approval record that carries no originator | fixed |
| 3 | `B1-03` | B1 | P2 | `ActionType::inherited_from` is honoured by the defaults map but NOT by the allowlist/blocklist rule lookup | fixed |
| 4 | `B1-04` | B1 | P3 | `audit_identity` collapses a subagent's agent id to the parent agent's, so two different subagents produce identical audit identities | fixed |
| 5 | `B1-05` | B1 | P3 | `ApprovalPolicy::check`'s doc says the decision is not audited, but the only implementation audits the `Ask` inside `check` | deferred (Jev triage: cosmetic / needs redesign) |
| 6 | `B10-001` | B10 | P1 | Per-instance channel policies are registered under an ID the runtime does not use | fixed |
| 7 | `B10-002` | B10 | P2 | Config watcher has no shutdown path and survives server teardown | fixed (wave 3c: config watcher task is captured and torn down) |
| 8 | `B10-003` | B10 | P2 | Separate WebChat server is detached from gateway shutdown | fixed |
| 9 | `B11-01` | B11 | P2 | One fleet-wide `StreamRegistry` + mint-time TTL turns a per-caller voice stream into a cross-user liveness kill | fixed |
| 10 | `B11-02` | B11 | P3 | Dead duplicate memory-context factories; the live path already diverges, and config docs point at the dead one | deferred (Jev triage: cosmetic / needs redesign) |
| 11 | `B11-03` | B11 | P3 | Absent optional subsystems answer `-32601` instead of the codebase's typed `SERVICE_UNAVAILABLE` contract | fixed |
| 12 | `B11-04` | B11 | P3 | Five parallel hot/cold `providers.*` arms make provider semantics depend on which arm a verb was added to | deferred (Jev triage: cosmetic / needs redesign) |
| 13 | `B12-P1-01` | B12 | P1 | Memory extension registry panic on duplicate registration | fixed |
| 14 | `B12-P1-02` | B12 | P1 | Tool health probe registration without existence check | not-applicable |
| 15 | `B12-P2-01` | B12 | P2 | Unvalidated API key hydration from vault | fixed |
| 16 | `B12-P2-02` | B12 | P2 | Race between extension load and tool registration | not-applicable |
| 17 | `B12-P2-03` | B12 | P2 | Leaked spawned tasks on early return | partially-fixed |
| 18 | `B12-P2-04` | B12 | P2 | Blocking SQLite operations in async context | fixed |
| 19 | `B12-P3-01` | B12 | P3 | Duplicate default-provider selection logic | fixed |
| 20 | `B12-P3-02` | B12 | P3 | Oversized function register_agent_handlers (1638 lines, complexity 90) | deferred (Jev triage: cosmetic / needs redesign) |
| 21 | `B12-P3-03` | B12 | P3 | Unused assignment to naked_loop_planner_provider and team_planner_provider | deferred (Jev triage: cosmetic / needs redesign) |
| 22 | `B13-01` | B13 | P1 | Re-indexing an existing database retains sessions absent from the source | fixed |
| 23 | `B13-02` | B13 | P1 | A mid-index error commits a partially updated target | fixed |
| 24 | `B13-03` | B13 | P1 | Removing a transcript leaves its old FTS content searchable | fixed |
| 25 | `B13-04` | B13 | P1 | Attachment metadata lookup errors are exported as “no attachments” | fixed |
| 26 | `B13-05` | B13 | P1 | Failed JSON serialization can destroy the previous export and leave a truncated artifact | fixed |
| 27 | `B13-06` | B13 | P1 | Overwrite failures corrupt previously transformed sessions | fixed |
| 28 | `B13-07` | B13 | P1 | A failed attachment write is later trusted as a complete deduplicated file | fixed |
| 29 | `B13-08` | B13 | P1 | A valid relative output directory with no parent component is rejected | fixed |
| 30 | `B13-09` | B13 | P2 | Out-of-range timestamps are silently replaced with the current time | fixed |
| 31 | `B13-10` | B13 | P1 | Filesystem-only sessions can still produce a successful verification | fixed |
| 32 | `B13-11` | B13 | P1 | FTS verification does not check transcript coverage or stale rows | fixed |
| 33 | `B13-12` | B13 | P0 | Orphan cleanup deletes attachments owned by sessions whose directories are missing | fixed |
| 34 | `B13-13` | B13 | P1 | Identical bytes with different MIME types cannot be represented by the attachment index | partially-fixed |
| 35 | `B13-14` | B13 | P1 | “Metadata consistency” accepts values that disagree with the database | fixed |
| 36 | `B13-15` | B13 | P1 | Metadata auto-fix splits legal tags containing commas | fixed |
| 37 | `B13-16` | B13 | P1 | Export succeeds after dropping source rows with null timestamps | partially-fixed |
| 38 | `B13-17` | B13 | P2 | Missing attachment counts silently disable attachment indexing | partially-fixed |
| 39 | `F-01` | B14 | P1 | `revoke_exact` aborts the whole helper on any resolve error other than `device_not_found` | fixed |
| 40 | `F-02` | B14 | P1 | `main()` discards the error, so every failure looks identical | fixed |
| 41 | `F-03` | B14 | P2 | `shutdown` waits for the HTTP server without a timeout → a parked asset request hangs the helper forever | fixed |
| 42 | `F-04` | B14 | P2 | `hold_response` swaps the `Notify` instance → a parked waiter is orphaned (lost wakeup) | fixed |
| 43 | `F-05` | B14 | P2 | The `fixture` mutex is held across the stdout write → the HTTP middleware stalls behind it | fixed |
| 44 | `F-06` | B14 | P2 | A `release_response` that lands before the request parks silently disables the gate — the case the gate exists to prove is never exercised, yet the ru | fixed |
| 45 | `F-07` | B14 | P2 | No bound on the stdin line length (the protocol declares 16 KiB) → unbounded allocation | fixed |
| 46 | `F-08` | B14 | P2 | The binary is excluded from every CI build → 1028 LOC that nothing compiles or tests | fixed |
| 47 | `F-09` | B14 | P2 | The `stopped` message reports `fixtureClosed: true` while the fixture is open and the helper keeps serving commands | partially-fixed |
| 48 | `F-10` | B14 | P3 | An unknown command is reported as `bad_state` instead of `bad_command` | fixed |
| 49 | `F-11` | B14 | P3 | `control_loop` is a 392-line if-chain with nine copy-pasted reply shapes | deferred (Jev triage: cosmetic / needs redesign) |
| 50 | `B2-001` | B2 | P0 | Renamed actions bypass blocklist and allowlist rules | fixed |
| 51 | `B2-002` | B2 | P1 | Home-directory failure loads policy from an untrusted working directory | fixed |
| 52 | `B2-003` | B2 | P2 | Policy patterns leak secrets into debug and error logs | fixed |
| 53 | `B2-004` | B2 | P2 | Audit logging copies an unbounded raw target into the log pipeline | fixed |
| 54 | `B2-005` | B2 | P2 | Policy loading reads an unbounded file into memory | fixed |
| 55 | `B2-006` | B2 | P0 | URL case variation bypasses navigation blocklists | fixed |
| 56 | `B2-007` | B2 | P1 | Long-target redaction emits the secret-bearing prefix verbatim | not-applicable |
| 57 | `B3-001` | B3 | P2 | Half-open guardian breaker admits a provider stampede | fixed |
| 58 | `B3-002` | B3 | P1 | Node approval treats an undeliverable prompt as a live request | fixed (wave 2a: node approval is denied when no operator surface received it) |
| 59 | `B3-003` | B3 | P2 | Aborting an attended approval defers zombie cleanup until a later registration | fixed |
| 60 | `B3-004` | B3 | P2 | Fallback routing loses the operator fallback across channel teardown | fixed |
| 61 | `B4-01` | B4 | P0 | Concurrent eviction can delete another successful `put` | fixed |
| 62 | `B4-02` | B4 | P1 | Acknowledged writes are not crash-durable | fixed |
| 63 | `B4-03` | B4 | P2 | Failed writes and eviction leave untracked blobs that bypass the cap | fixed |
| 64 | `B4-04` | B4 | P2 | Reads allocate the whole on-disk file before enforcing any bound | fixed |
| 65 | `B4-05` | B4 | P2 | Session encoding leaves Windows device names unchanged | fixed |
| 66 | `B5-001` | B5 | P1 | Stop can SIGTERM/SIGKILL a recycled unrelated PID | fixed |
| 67 | `B6-001` | B6 | P2 | Bootstrap token hides database open failures as “no token” | fixed |
| 68 | `B6-002` | B6 | P2 | Identity commands misreport vault/database failures as a missing token | fixed |
| 69 | `B6-003` | B6 | P2 | Export size cap is defeated by a file replacement race | fixed |
| 70 | `B6-004` | B6 | P2 | Hook review can hang or exhaust memory on untrusted command output | fixed |
| 71 | `F-01` | B7 | P1 | Synchronous blocking file I/O inside async function (node.rs) | fixed |
| 72 | `F-02` | B7 | P1 | Privilege escalation in plugins.rs: unvalidated symlink in uninstall path | not-applicable |
| 73 | `F-03` | B7 | P1 | Process::exit called on error paths in plugins.rs without cleanup | fixed |
| 74 | `F-04` | B7 | P2 | Implicit panic on path construction in resume.rs | not-applicable |
| 75 | `F-05` | B7 | P2 | Race condition: plugin discovery reads disk after enable/disable writes | not-applicable |
| 76 | `F-06` | B7 | P3 | Dead code: #[allow(dead_code)] suppresses cross-platform warnings | deferred (Jev triage: cosmetic / needs redesign) |
| 77 | `B8-01` | B8 | P2 | Channel-pairing credential store falls back to a predictable, world-readable `/tmp` path | fixed |
| 78 | `B8-02` | B8 | P2 | Pairing store silently degrades to in-memory: durable credentials are lost while the RPCs keep answering | fixed |
| 79 | `B8-03` | B8 | P3 | A2A tools stay registered but permanently dead when the execution adapter is missing; the error names the wrong cause and daemon mode logs nothing | fixed |
| 80 | `B8-04` | B8 | P2 | The spend ledger's durability guarantee is false when the security store fell back to memory | fixed |
| 81 | `B8-05` | B8 | P2 | Provider build errors are swallowed: the provider's models silently vanish from the OpenAI-compatible routing table | fixed |
| 82 | `B8-06` | B8 | P2 | Duplicate model names across providers bind to a random provider (HashMap iteration order) | fixed |
| 83 | `B8-07` | B8 | P3 | Boot prints "Task reaper: started" even when the reaper was disabled and never spawned | skipped-too-risky |
| 84 | `B9-01` | B9 | P1 | Invalid default config is replaced by defaults instead of failing startup | fixed |
| 85 | `B9-02` | B9 | P1 | Unknown session-store backend silently selects SQLite | fixed |
| 86 | `B9-03` | B9 | P2 | Runtime warmup performs unbounded blocking subprocess and file I/O on a Tokio worker | fixed |
| 87 | `B10-S2-01` | second/B10 | P2 | Config hot-reload consumer exits permanently on a single `broadcast::RecvError::Lagged` | fixed |
| 88 | `B10-S2-02` | second/B10 | P2 | `config.reload` / `config.validate` / `config.path` are unregistered whenever the watcher cannot be | fixed |
| 89 | `B10-S2-03` | second/B10 | P3 | Dead note-maintenance lifecycle wrappers, kept alive only by tests that prove nothing about production | deferred (Jev triage: cosmetic / needs redesign) |
| 90 | `B10-S2-04` | second/B10 | P2 | `enabled = false` is honoured at boot only for `feishu` | fixed |
| 91 | `B10-S2-05` | second/B10 | P3 | `StartupResources::cleanup` runs twice on the success path and the second failure is swallowed | not-applicable |
| 92 | `B10-S2-06` | second/B10 | P2 | Security artifacts fall back to predictable `/tmp` paths | fixed |
| 93 | `B10-S2-07` | second/B10 | P3 | Gateway teardown owns none of the background tasks it spawns | not-applicable |
| 94 | `B3-S2-01` | second/B3 | P1 | Node requester omits Ok(0) check, silently parks approval for full timeout | fixed |
| 95 | `B5-S2-01` | second/B5 | P1 | Daemon parent reports success before daemon setup succeeds | fixed |
| 96 | `B5-S2-02` | second/B5 | P2 | Requested WebChat listener failure is detached and startup still succeeds | fixed |
| 97 | `B5-S2-03` | second/B5 | P2 | Failed installer launch leaks downloaded update files | fixed |
| 98 | `R1-approval/DC-01` | diffcheck | P2 | Originator callback authorization is not atomic across approval ID reuse | fixed |
| 99 | `R1-approval/DC-02` | diffcheck | P2 | URL case folding changes case-sensitive path and non-URL text | fixed |
| 100 | `R2-artifacts-migrate/DC-01` | diffcheck | P1 | Path-keyed locks do not cover aliased store roots | fixed |
| 101 | `R2-artifacts-migrate/DC-02` | diffcheck | P1 | Directory-sync failures are still acknowledged as durable writes | fixed |
| 102 | `R2-artifacts-migrate/DC-03` | diffcheck | P2 | Reserved-name encoding strands existing non-Windows artifacts | fixed |
| 103 | `R2-artifacts-migrate/DC-04` | diffcheck | P2 | Deduplicated attachment verification reads an unbounded file | fixed |
| 104 | `R3-servercmd/DC-01` | diffcheck | P1 | Dynamic `handlers_mut().register(...)` loop breaks the gateway method-census test | fixed |
| 105 | `R3-servercmd/DC-02` | diffcheck | P1 | `stopped` message now reports `fixtureClosed: false`, breaking the documented contract and the QA harness | fixed |
| 106 | `R3-servercmd/DC-03` | diffcheck | P2 | Shutdown ignores the server join result and always reports `listenerClosed: true` | fixed |
| 107 | `R3-servercmd/DC-04` | diffcheck | P2 | 16 KiB line bound is checked after the whole line has been buffered | fixed |
| 108 | `R3-servercmd/DC-05` | diffcheck | P3 | `release_response` ack uses a status outside the declared ack vocabulary | fixed |
| 109 | `R3-servercmd/DC-06` | diffcheck | P3 | `runtime_channel_id` still leaves same-type instances on an unread key | fixed |
| 110 | `R3-servercmd/DC-07` | diffcheck | P3 | `revoke_exact` masks every non-ambiguity error as `already_revoked` | fixed |
| 111 | `R3-servercmd/DC-08` | diffcheck | P3 | New CI step re-checks the whole crate on all three OS runners while clippy still skips the gated bin | fixed |