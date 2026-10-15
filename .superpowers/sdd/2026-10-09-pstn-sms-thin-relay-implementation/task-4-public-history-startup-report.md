# Task 4 — public history startup wiring: NOT WIRED (blocked on upstream key/config contract)

Base: 94259ee34. No product code changed. `public_history::install_global` intentionally remains uncalled.

## Finding
- `install_global` has zero production callers; `executor.rs:846` passes `public_history::installed()` (None in production) => public runs are one-turn only (fail-closed, previous behaviour). Pre-existing, unchanged.
- No telecom config struct exists (`src/config` has only `policies/spend.rs` mentioning telecom); `src/bin/aleph-server` has no telecom startup/initializer to host the seam. `SmsInbox` (same dedicated-key design) is likewise never opened in production.
- Existing secret material in startup: only `SharedTokenManager` vault (`shared_token.rs:244` `SecretsCrypto::new(login token)`), data dir `get_data_dir()` (`start/mod.rs:364`), `ALEPH_VAULT_KEY` (secret-env filter only; not a provisioning input).

## Why not fake it
1. Using shared token / vault master (or owner session key) violates the settled ruling (Task 3c; public_history.rs docs): the dedicated key must not derive from login/shared-token credentials.
2. Generating a random key on first start and storing it via `vault.store_secret` would (a) invent a credential and provisioning policy no spec/config defines, and (b) leave it recoverable by anyone holding the login token (vault is keyed by it), defeating key dedication; also silently enabling persistence of public conversation text without an explicit operator opt-in.
3. A path-only initializer without a key source cannot be tested meaningfully and would just be dead code.

## Missing upstream contract (needed to complete)
- A telecom/public-history config section (explicit enable flag, optional lowered limits via `PublicHistoryConfig::with_limits`, db file name under `get_data_dir()`, e.g. `telecom_public_history.db`).
- A dedicated key source decided by the user/operator that is independent of the login token and owner key (e.g. an operator-provisioned secret file with 0600 perms or a separate KMS/keystore entry), with documented rotation/loss behaviour (history is disposable <=24h, so loss = history reset).
- Then a small startup initializer: disabled/missing config or key => do not install (log warn, one-turn behaviour); `PublicHistoryStore::open(path, SecretsCrypto::new(key), cfg)`; `install_global` called once; tests for missing key/path/disabled using local stores only (never touching the OnceLock global).

## Not done / residuals
Task #14 stays open: cross-turn public history is not active in production. No tests run (no code changed). Three pre-existing dirty docs and untracked public_approval.rs untouched.
