# Task 4 — resume provenance gate fix (C1/C2/I1/I2)

Base HEAD be6ef9df5. Logs: /tmp/gate-check.log, /tmp/gate-integ.log, /tmp/gate-*.log.

## C1 — integration fixtures
All 8 `RunStarted` fixtures in `tests/resume_coordinator_integration.rs` set `telecom_provenance: PersistedTelecomMarker::Ordinary`
(none has telecom semantics). RED: at HEAD the real target (`required-features = ["test-helpers"]`) did not compile (review C1).
GREEN: compiles and passes with `--features test-helpers`.

## C2 — gate before side effects
`gate_provenance(Option<&MarkerRestore>)` (pure; generic refusal text, no marker contents) is invoked:
- `handle_interrupted`: right after `plan_resume`, before `authority_preflight`/`attempt_gate`/repair/stamp/retrigger
  (too-old abandon stays first: that is how legacy records drain). Refusal => `report.refused`, nothing written, no attempt spent.
- `handle_unanswered`: before `authority_preflight` and the stamp.
- `admit_resume`: moved before `resume_authority` (defense in depth; test resolver panics if consulted).
Valid Ordinary/Public behave as before.

## I1 ruling — unanswered tail: FAIL CLOSED
`UserMessage` has no typed telecom origin; the only telecom stamp is the in-memory `RunRequest`, set after the seed. A tail with no
`RunStarted` therefore cannot be told from a telecom inbound that crashed pre-`RunStarted`; missing is not proof of ordinary.
`unanswered_provenance()` returns `None` (seam for a future carrier), so every unanswered resume is refused under
`telecom_provenance_legacy`, writing nothing. **Ordinary unanswered auto-resume compatibility is NOT preserved** (HEAD already refused it,
now without writes; such tails still drain via recency abandon). A server-authored carrier would touch ~122 `UserMessage` constructors: out of scope.
Three unanswered integration tests + the manual-verb one were rewritten to assert refusal/no-stamp/no-execute.

## I2 — tests
Integration (real target): `boot_refuses_unproven_provenance_before_any_write` and `manual_resume_refuses_unproven_provenance_before_any_write`
(Legacy/Unknown/Corrupt; no ToolError/closer/stamp/execute, log length unchanged), `provenance_refusal_precedes_the_attempt_gate`
(Legacy + 3 stamps => refused, not abandoned), `resume_restores_public_into_the_dispatched_request` (Public region/binding/generation reach `RunRequest`; Ordinary => None),
plus the unanswered tests above. In-crate: `gate_provenance_admits_only_ordinary_and_public`, `an_unanswered_tail_has_no_provenance_and_is_refused`,
`admit_resume_refuses_unproven_provenance_before_consulting_authority`, `admit_resume_restores_public_into_the_request`.
`session_split::the_child_opener_preserves_the_parents_telecom_provenance` (Ordinary/Public/Legacy), `fast_path::ordinary_fast_path_writes_the_explicit_ordinary_marker`.
Scratch `task-4-resume-marker-tests.rs` left untouched (not evidence).

## Extra
`an_abandoned_interrupted_run_is_closed_under_its_own_run_id` was red once the target compiled (crash-loop-cap abandon passed `closes=None`, writing
`abandoned-<uuid>`); fixed by passing the open run's id, as `abandon`'s doc and the test require. Pre-existing, unrelated to provenance.

## Results (OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2)
- `cargo check -p alephcore --lib --tests`: OK (3 pre-existing dead-code warnings)
- `--lib resume_coordinator`: 46 passed / 0 failed
- `--lib fast_path`: 12/0; `--lib persisted_marker`: 10/0; `--lib session::events::tests`: 27/0; `--lib session_split`: 14/0
- `--test resume_coordinator_integration --features test-helpers`: 41 passed / 0 failed (first run after compile: 39/2 — the two fixed/rewritten above)

## Not done / residuals
No full suite, clippy, or baseline-red comparison (unverified). Unknown/Corrupt refusal text carries the stored reason string from `restore()`; only checked for no leak of marker contents in the in-crate gate test.
Ordinary unanswered auto-resume is lost until a typed carrier exists.
