# Task P7.2 Report — auth stage actually runs on this worktree

Plan: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P6P7-mcp-face.md` §"Task P7.1" (auth row of the §6 contract); progress.md dispatched row.
Worktree HEAD: bd027cac9 → new commit below. Scope: qa/ only; zero `src/*.rs` changes.

## Assumption (P7.2 is not formally defined)

The plan file `P6P7-mcp-face.md` ends at P7.1 — no `### Task P7.2` heading exists. progress.md:1203 records only "P7.2 dispatched MiniMax-M3 via Jev confidence .97; pending". The user prompt names "P7.1 auth SKIP (若相关)" as the in-scope carry. Read: P7.2 = the auth stage actually runs end-to-end on this worktree (the plan's Definition of Done: "Real-machine stages PASS on this worktree's binary: `qa/mcp_face/run.sh {handshake, tools, auth, list_changed, deny}`"). If a different P7.2 was intended, this commit should be re-shaped — flag for the lead.

## Diagnosis

P7.1 added a `/health` pre-flight in `stage_auth` to handle macOS-firewall-induced TimeoutError. The pre-flight passes on this host (LAN-reachable), and the **first** assertion `remote without a bearer is 401 — status=401` PASSES. The **second** assertion crashes:

```
AttributeError: 'NoneType' object has no attribute 'get'
  at drive.py:244 in stage_auth
  L.check("…with WWW-Authenticate: Bearer", hd.get("www-authenticate", "").startswith("Bearer"), ...)
```

Root cause: `Mcp.initialize()` returns `(st, sid, body)` (the session id captured from the `Mcp-Session-Id` header). `stage_auth` unpacks the call as `(st, hd, _)` and uses `hd` as the response-headers dict. On a 401 there is no session id, so `hd` is `None`, and `hd.get(...)` raises.

The P7.1 report's claim "On hosts with working self-connectivity the pre-flight returns 200 and the remote assertions run as written" was not actually exercised end-to-end here: pre-flight 200 → first 401 PASS → second assertion crash. P7.1's `/health` workaround prevented the TimeoutError; it did not address the header-unpacking bug.

## Files

- `qa/mcp_face/drive.py` — three changes, all in the client side of the fixture:
  1. `Mcp.__init__` now seeds `self.last_headers = {}`.
  2. `Mcp.request()` populates `self.last_headers` in both the success and `HTTPError` arms (and reuses the dict for the returned tuple).
  3. `stage_auth` reads the headers from `c_no_bearer.last_headers` / `c_bad.last_headers` instead of unpacking from `initialize()`.

No `src/*.rs` touched. The crate-side `HeaderValue::from_static("Bearer realm=\"aleph\"")` in `src/gateway/mcp_face/http.rs:172` is unchanged.

## Test results (SKIP_BUILD=1 after one rebuild)

| stage | result |
|---|---|
| auth | 7/7 PASS (pre-flight + 7 assertions; no SKIP) |
| handshake | PASS, exit 0 |
| tools | PASS, exit 0 |
| list_changed | PASS, exit 0 |
| deny | PASS, exit 0 |

`cargo test -p alephcore --lib mcp_face` → 62 passed, 0 failed (same count as P7.1; pre-existing red `capability::census::tests::every_installed_global_is_a_capability_slot` not affected).

## Mutation (fixture can go red)

Edited `src/gateway/mcp_face/http.rs:172` `Bearer realm=\"aleph\"` → `Mutation realm=\"aleph\"`, rebuilt, ran `qa/mcp_face/run.sh auth`:

```
[PASS] remote without a bearer is 401 — status=401
[FAIL] …with WWW-Authenticate: Bearer — Mutation realm="aleph"
[PASS] remote with a wrong bearer is 401 — status=401
[PASS] a shared gateway token exists
[PASS] remote with the shared token is admitted with a session
[PASS] …and can list tools — status=200 n=7
[PASS] loopback still needs nothing — status=200
```

Only the header assertion goes red; the other six stay PASS. Reverted before commit; rebuild + re-run auth → all 7 PASS again with `Bearer realm="aleph"`.

## Deviations

- The plan file has no `### Task P7.2`; the brief that dispatched this task to MiniMax-M3 is the progress.md row plus the user's "P7.1 auth SKIP (若相关)" hint. Logged above; if the intended P7.2 is something else, the commit should be re-shaped rather than this report.
- P7.1's `/health` pre-flight is kept verbatim (the LAN-reachability pre-flight is still load-bearing on macOS hosts where the firewall blocks self-connections).
- Did NOT add a Python unit test for `Mcp.last_headers` — the fixture's own assertion (`stage_auth`) is the wire-level test, and the mutation step proves it can go red on the actual response header.

## Boundaries / negatives

- `git diff bd027cac9 -- src/harness/` is empty; R10 maintained.
- `git diff --check` clean.
- `src/*.rs` unchanged — rustfmt did not need to run (no Rust files touched). The mutation temporarily edited `src/gateway/mcp_face/http.rs`; that edit is reverted and the binary was rebuilt before the green re-run.
- `.cargo/config.toml` is gitignored, not staged.
- Pre-existing red (untouched): `capability::census::tests::every_installed_global_is_a_capability_slot` (`src/capability/census.rs:823`).
- All five `qa/mcp_face/run.sh` stages now exit 0 on this worktree; the §6 contract's row "auth SKIPs cleanly where the host cannot reach its own LAN address" remains — it kicks in when `LAN_IP` is empty (host has no non-loopback address), distinct from the header-unpack crash this commit fixes.
- Did not modify `qa/mcp_face/run.sh`, `qa/mcp_face/patch_mcp.py`, or `qa/README.md` — only `drive.py`.
