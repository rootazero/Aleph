# Task 4 unanswered-origin review I1 fix

Review finding I1 (`runner_impl.rs` had two `PersistedTelecomMarker::capture(` sites; census counted files only).

- `src/orchestrator/harness_bridge/runner_impl.rs`: capture once into local `telecom_provenance` at the runner boundary; `seed_session` gets `.clone()`, `RunStarted` moves it. Same typed capture, no behavior change, no source_channel/metadata inference.
- `src/gateway/telecom/persisted_marker.rs`: census test renamed `capture_has_exactly_five_production_call_sites`; it now counts exact production call occurrences (comment lines excluded), one hit per occurrence, so a second call in an allowed file fails the exact-list assertion.

Verification (OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2): rustfmt --check on both files clean; `cargo check -p alephcore --lib --tests` OK; `cargo test -p alephcore --lib -- persisted_marker harness_bridge session_seed` 136 passed / 0 failed; `git diff --check` clean.

Not done: M1-M3 minors; census still strips `#[cfg(test)]`-onward text and `//` lines only (block comments / strings not parsed).
