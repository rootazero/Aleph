# Task #56 — Exact QA credential reaper / 精确 QA 凭据清理工具

**Status / 状态: UNCOMPILED AND UNRUN. No real keyring access, no deletion performed. / 未编译、未运行；未触碰真实钥匙串，未删除任何凭据。**

## 1. What was added / 新增内容

- `shared/client/src/credential_store.rs`
  - `DeviceCredentialStore::qa_service_for_nonce(nonce)` (`#[cfg(feature = "qa-service")]`): derives the QA keyring service **only** from a nonce (`aleph-qa.device-credential.<32..128 lowercase hex>`), reusing `validate_qa_service`.
  - Unit test `qa_service_for_nonce_derives_only_from_valid_hex`.
  - Production behavior unchanged (default features compile none of it).
- `shared/client/examples/qa_credential_reaper.rs` (new, feature-gated example). Without `qa-service`, `main` prints `{"ok":false,"code":"E_FEATURE"}` and exits 2.

## 2. CLI / 调用

```
qa_credential_reaper --owned-root <abs path> --nonce <hex> --gateway <ws://loopback[:port]> --mode check-absent|delete-exact
```

All four flags required exactly once, space-separated form only; there is no `--service` flag.

## 3. Validation chain (all before any keyring call) / 校验链

1. nonce: 32..=128 lowercase hex.
2. root: absolute, lexical direct child of canonical OS temp dir, basename prefix `aleph-shell-qa-`, no symlink/junction/reparse on the path or any ancestor, canonical == lexical.
3. `<root>/.qa-owned` is a small regular file whose content equals the nonce (trailing CR/LF tolerated).
4. `<root>/.aleph/.desktop-shell-panel-target` is a small plain file holding an origin (`http://host:port` -> `ws://`, `https://` -> `wss://`; no userinfo/path/query/fragment).
5. `--gateway` must be `ws://` loopback (127.0.0.1 / localhost / [::1]), no userinfo, and its keyring account must equal the account derived from the marker origin. Any port (incl. 18790) is allowed; the service is the random nonce-derived one, so a real Gateway account with a random service never touches production entries.
6. Store built with `for_gateway_in_service(gateway, service_from_nonce)`; existing transport validation still applies.

## 4. Modes / 模式

- `check-absent`: `load_for_connect() == None` else `E_PRESENT` (or `E_KEYRING`). Stdout `{"mode":"check-absent","absent":true}`.
- `delete-exact`: `delete_for_connect()`, then `load_for_connect()` must be `None` (else `E_NOT_DELETED`), then a second `delete_for_connect()` must return `Ok(false)`. Stdout `{"mode":"delete-exact","absent":true}`. Never writes/overwrites any secret.

Errors go to stderr as `{"ok":false,"code":"E_..."}`; exit 1 for `E_KEYRING/E_PRESENT/E_NOT_DELETED`, else 2. Codes: E_USAGE, E_NONCE, E_TEMP_UNAVAILABLE, E_ROOT_SHAPE, E_ROOT_MISSING, E_ROOT_NOT_DIR, E_ROOT_LINK, E_ROOT_NOT_TEMP_CHILD, E_ROOT_PREFIX, E_OWNER_MISSING, E_OWNER_INVALID, E_OWNER_MISMATCH, E_TARGET_MARKER_INVALID, E_GATEWAY_INVALID, E_GATEWAY_NOT_LOOPBACK, E_GATEWAY_MISMATCH, E_KEYRING, E_PRESENT, E_NOT_DELETED. No provider errors, service names, or credential material are printed.

## 5. Tests / 测试

Pure `#[cfg(test)]` tests inside the example's `imp` module (args, nonce, root lexical shape, marker->ws conversion, service derivation, gateway/marker matching, code set) plus the credential_store test. No OS I/O. To run later (main, after audit):

```
cargo test -p aleph-client --features qa-service --lib qa_service_for_nonce
cargo test -p aleph-client --features qa-service --example qa_credential_reaper
cargo run  -p aleph-client --features qa-service --example qa_credential_reaper -- --owned-root <root> --nonce <hex> --gateway ws://127.0.0.1:<port> --mode check-absent
```
(Package name `aleph-client` is assumed; verify against `shared/client/Cargo.toml`. Use CARGO_BUILD_JOBS=1 per machine notes.)

## 6. NOT done / 未完成

- Not compiled, not tested, not clippy'd; cfg gating, unused-code warnings and `&str`/`String` inference are unverified.
- No real keyring precheck/delete ever executed.
- No Cargo.toml / lib.rs / Cargo.lock edits; hence no `[[example]] required-features` — the example's own cfg handles it, and its tests only exist with `--features qa-service`.
- Shell/harness (`qa/native_ipc/run.mjs`, desktop shell) NOT integrated: the mandatory precheck before shell OS runtime and recovery-by-root-marker still need wiring.
- No forbidden-roots (HOME/APPDATA) list (redundant with direct-child-of-temp) and no namespace-digest output.
- Windows junction detection relies on reparse-point attributes / symlink_metadata; not exercised on a real junction.
- No window launched.
