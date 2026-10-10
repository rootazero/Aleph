# SDD ledger — plan: docs/superpowers/plans/2026-10-09-pstn-sms-thin-relay-implementation.md

## Setup

- User chose `Subagent-driven` execution in message m00763.
- Design authority: `docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md`.
- Current base before implementation: `6281248f9dd4e8d3b0feff5ccc1d6b5a72f33aea`.
- Pre-existing untracked screenshot `CleanShot 2026-10-09 at 8.46.21 AM@2x.png` must remain untouched.
- No provider provisioning, real SMS/calls, external credentials, or public endpoint actions are authorized in this worktree.

## Pre-flight conflict scan

| Scope | Producer / consumer | Finding | Ruling |
|---|---|---|---|
| Task 1 ↔ Task 2 | Shared wire/state contract → SMS adapter | Task 2 consumes outcome, generation, dedup and binding fields from Task 1. No incompatible field names found; Task 1 must land first. | Follow plan order; shared types are authoritative. |
| Task 1 ↔ Task 3 | Shared contract → SMS transport | Task 3 depends on `unknown`, expiry, operation and generation semantics. Compatible if transport never retries unknown implicitly. | Enforce explicit terminal expiry and new operation for retry. |
| Task 1 ↔ Task 5 | Shared contract → voice protocol | Task 5 uses call/turn/epoch/generation fields. Plan and spec agree that `phone_call_id` is distinct from tool `call_id`. | Keep phone lifecycle identifiers distinct. |
| Task 2 ↔ Task 3 | SMS channel → SMS transport | Task 2 must expose a truthful pending/outcome seam; Task 3 supplies the authenticated transport. No provider API is assumed. | Skeleton may be implemented without a live provider; unavailable stays explicit. |
| Task 2 ↔ Task 4 | SMS binding/permission → channel | Task 4 changes assurance and approval around the channel. It must not elevate default Chat/guest or alter ordinary SMS capability claims. | Permission changes remain after channel contract is stable. |
| Task 3 ↔ Task 4 | SMS operation/binding → approval | Task 3 requires binding, consent, approval and budget revalidation; Task 4 owns identity/approval semantics. | Core owns approval meaning; transport only enforces bound deterministic checks. |
| Task 4 ↔ Task 5 | Approval cancellation → call lifecycle | Task 5 hangup/interrupt/generation events must cancel pending approvals from Task 4. | Task 5 consumes Task 4 cancellation API; no auto-resume. |
| Task 4 ↔ Task 6 | Tools/approval → release validation | Task 6 cannot claim release until Task 4 fail-closed approval and off-switches are tested. | Gate real-line validation on all prior tests and G1–G5. |
| Task 5 ↔ Task 6 | Voice transport → lifecycle tools/effect tests | Task 6 consumes call status, hangup, cancel and outcome semantics from Task 5. | Test effect delivery, not HTTP/function success. |
| Task 1 self-consistency | Files, tests, commands | Plan initially used `src/gateway/interfaces/telecom`; corrected to `src/gateway/telecom` plus `src/gateway/mod.rs`; corrected protocol test command. | The corrected plan and this ledger are authoritative. |
| Task 2 self-consistency | Factory/Panel/protocol registration | Plan names the four reconciliation surfaces and existing equality tests. | Register all advertised surfaces together. |
| Task 3 self-consistency | Transport and budget tests | Mock transport tests precede provider effects; real line is a later evidence step. | No live provider dependency in unit/fixture tests. |
| Task 4 self-consistency | Identity/approval/tools | Plan leaves exact vault/session seam to execution-time inspection rather than inventing signatures. | Implementer must inspect current seam and report any plan correction. |
| Task 5 self-consistency | Call state and transport | Pure protocol state precedes external transport; plugin stub is reference only. | Do not revive plugin as production manager. |
| Task 6 self-consistency | Release and G1–G5 | Plan blocks release until both-region evidence; no silent overseas substitute. | Keep experiment flag/evidence if gates fail. |

## Rulings

- Ruling: Begin with pure contract/state-machine work even though provider gates are not yet closed — it is reversible, has no external side effect, and is explicitly allowed by the design.
- Ruling: Do not perform Gate 0 commercial/provider outreach in this coding session — it would be an external side effect and requires separate authorization; record the blocker rather than pretending the route is production-ready.
- Ruling: The plan's proposed file names are targets, not permission to invent missing architecture. Implementers must match existing Aleph seams and stop/report if a required integration would violate R1/R3/R4/R6/R7/R8/R9/R10.
- Task 1 review findings: I1 `Unknown` can regress to non-terminal states and reset expiry; I2 `SmsCommand.expires_at` is ignored; I3 `created_at` retry semantics are unspecified and can bypass stale retention. Resume implementer to fix all three and add regression tests. Minor M1–M8 are deferred to later adapter tasks or final review unless they become load-bearing.

## Additional setup and scope rulings

- Ruling: Move remaining implementation to a persistent isolated worktree from 48d75f9b6 before Task 2 — Task 1 was executed on a feature branch but not isolated as the skill requires. Main working copy and its screenshot must be preserved; no merge or push is authorized. Cost if wrong: extra branch/worktree cleanup only.
- Ruling: Task 2 may implement an offline, unavailable-by-default channel skeleton and mock contract tests without provider eligibility — the spec permits isolated technical probes, not production enablement. No registered SMS card may claim a region/route is operational or request carrier configuration from the user. Cost if wrong: rework of the experimental adapter; no external telecom effects.
- Ruling: Task 3 must not activate production transport or perform live Step 6 before G1–G5 and Task 4 authorization are validated. Coding/mock tests can be staged, but any live gate stays explicitly blocked. Cost if wrong: delay to the live integration, not a security bypass.
- Ruling: The user chose Subagent-driven task execution, not an unrestricted multi-agent workflow; use individual implementer and reviewer calls only.

## Task status

- Task 1: complete (commits c7ff661cc..48d75f9b6, scoped review clean).
- Task 1: fix round 1/5 (I1–I3 addressed, 0 Important open; commits 0d1fc72b0..48d75f9b6). Scoped reviewer 60206108-01f6-47c independently ran integration tests: 26/26 passed.
- Task 1: minor (deferred): M1 inbound direction check; M2 Debug-based dedup enum strings; M3 receipt dedup includes outcome; M4 operation-domain outcome constraints; M5 dedup admission and durable queue must be atomic; M6 Acceptance and supplied envelope can mismatch; M7 missing call/playback/terminal serde tests; M8 positive bounded retention validation. Final review must triage all eight; consuming tasks must handle load-bearing ones.
- Task 1: residual contract: provider/Relay retries must preserve original created_at/expires_at; provider contract violations after metadata pruning cannot be detected by the pure model. Task 3/Relay qualification must enforce this.
- Task 1: minor (deferred): consumers must not reset OutcomeRecord.entered_at on Unchanged; add consumer regression coverage when wiring outcomes.
- Task 2: first implementer `8df53792-df9b-41e` hit turn limit and BLOCKED without commit/report completion. Partial uncompiled `src/gateway/interfaces/sms/config.rs` and `message_ops.rs` remain in isolated worktree; fresh implementer must inspect/reuse or correct them, then finish Channel, registrations, fixtures, tests and checks.
- Ruling: reuse the partial Task 2 files only after compiling and reviewing their behavior; do not discard blindly because no commit exists. Cost if wrong: local rework only; no external side effects.
- Task 2: implementation `a55f001db` awaiting fixes. Reviewer `404c8645-44c4-4f3` static PASS/PASS but four Important issues: unenforced Panel approval promise; stale Connected status after disconnect; unbounded and per-ingest cloned dedup ledger; operational metadata only in debugging raw. Fix before Task 3.
- Ruling: reword Panel approval text as upstream scope checks, not a locally enforced per-send approval promise — Task 4 owns scoped reply/proactive authorization; metadata approval references cannot establish trust. Cost if wrong: UI/documentation rework; production activation remains blocked.
- Task 2: deferred minors: segment units in MessageTooLarge; shape-only region validation; max_message_length reply chunker billing; missing start-invalid/stop/send-success tests; docs lists/E.164 docs_url; malformed-event acknowledgment; provider-account message ID scoping. Fix tests and ID scoping in this fix wave where practical; revisit chunking before any live transport.
- Task 3: pending
- Task 4: pending
- Task 5: pending
- Task 6: pending
