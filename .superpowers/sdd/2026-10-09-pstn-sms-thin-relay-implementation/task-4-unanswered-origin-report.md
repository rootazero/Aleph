# Task 4 — Unanswered-seed origin carrier (`SessionEvent::SeedProvenance`)

## Problem
`handle_unanswered` could not tell a *current* ordinary/Public unanswered seed from an
old seed with no provenance record, so it either over-trusted or had to guess.

## Change
- `SessionEvent::SeedProvenance { turn_id, origin: PersistedTelecomMarker }` (`src/session/events.rs`):
  server-authored, durable, serde-roundtrips; old logs simply lack it.
- Writer: `seed_session` (`src/orchestrator/harness_bridge/session_seed.rs`) takes `origin` and emits the carrier
  **immediately before the final real seeded `UserMessage`**, bound to that message's exact `turn_id`
  (Prompt / Messages(last only) / History / Multimodal). `FlowInput::Resume` and empty seeds write none.
- `runner_impl.rs` passes `PersistedTelecomMarker::capture(telecom_public)` — typed `telecom_public` only;
  no metadata / `source_channel` inference.
- Reader: `unanswered_seed_provenance(bearing, user_seq)` (`src/session/reduction.rs`) accepts a carrier only if it
  precedes the message and its `turn_id` matches exactly; missing / mismatched / Unknown / Corrupt → `Legacy`
  (fail-closed). `SeedProvenance` is disposition-bearing for reduction.
- `resume_coordinator::check_unanswered` passes `.restore()` of that marker to `handle_unanswered`:
  Ordinary → ordinary resume, Public → Public context restored, others refused untouched (no write).
- Non-carriers: `extract_turn_id` → None, `event_type_tag` "seed_provenance", subagent fork
  `is_prompt_bearing` false, so synthetic/mid-loop/subagent paths never become unanswered carriers.
- Census test `capture_has_exactly_five_production_callers` kept intact (no new `capture(` callers);
  tests build the Public marker from v1 wire JSON.

## Tests added
event roundtrip + old-missing; reducer exact-turn match and refusal of missing/unknown/corrupt;
seed carrier order + Ordinary/Public + no-carrier cases (harness_bridge); resume ordinary/public/missing
(no-write) and integration boot + manual for ordinary/public, unproven refusal untouched, not inherited.
Integration helper `unanswered_coordinator` creates the session row so the boot scan sees the seed.

## Verification (fresh, `OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2`)
- `cargo check -p alephcore --lib --tests`: OK (3 pre-existing unrelated warnings)
- lib: resume_coordinator 46, session::events 29, session::reduction 61, session::store 63, session_split 14,
  fast_path 12, harness_bridge 126, persisted_marker 10, subagent_spawner 59 — all pass
- `cargo test -p alephcore --test resume_coordinator_integration --features test-helpers -- --test-threads=1`: 45 passed, 0 failed
- rustfmt (`--edition 2021`) on changed Rust files only.

## Not done / residual
- No full `cargo test --workspace`, clippy, or cross-platform build.
- Pre-existing logs without a carrier stay fail-closed (`Legacy`) by design; no backfill/migration.
- The three pre-existing dirty docs files were left untouched and uncommitted.
