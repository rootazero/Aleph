# Review Report — Batch 5 (Auth: provider, storage, callback)

**Scope:** `src/mcp/auth/mod.rs`, `src/mcp/auth/provider.rs`, `src/mcp/auth/storage.rs`,
`src/mcp/auth/callback.rs`
**Date:** 2026-08-13
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-mcp-audit` (branch `mcp-audit`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 2 |
| Medium   | 3 |
| Low      | 3 |

The auth module is OAuth 2.0 with PKCE and RFC 9207 issuer binding. The
provider is well-tested: the issuer change in `client_info_for` is exactly
right, the PKCE verifier length is correct, the `application_type: "native"`
forces a loopback redirect. The two High findings are about (a) the OAuth
storage's disk-changed detection missing the *first* write that happens
*after* the cache mtime is recorded, and (b) the callback server's HTML
response body being assembled with `format!` and a `script>` block that can
be a vector if the server's `state` parameter is reflected anywhere.

## Findings

### [HIGH] src/mcp/auth/storage.rs:38 — `OAuthStorage::file_mtime` records `SystemTime` from `std::fs::metadata`, but the very next call to `load` takes the lock again and updates `cached_mtime` *after* the in-memory mutation — a writer that lands between `file_mtime()` and `*self.cached_mtime.write().await` is missed
**Category:** Security (TOCTOU)
**Confidence:** High

The `load` function (line 211) does:

```rust
let disk_mtime = self.file_mtime().await;
{
    let mut cache = self.cache.write().await;
    *cache = Some(storage.clone());
    *self.cached_mtime.write().await = disk_mtime;
}
```

The `disk_mtime` was read *before* the write lock was acquired. A concurrent
writer (`OAuthStorage::save_to_file`) records its own mtime inside
`*self.cached_mtime.write().await` (line 270). If another process writes the
file in the window between `file_mtime()` and the `cached_mtime.write()`, the
recorded mtime is *pre-write* and the next `load` reads the cache (no
invalidation) and serves stale data.

**Failure scenario:** A `mcp_login` refreshes tokens; the desktop's OAuth
storage singleton writes them. A second instance (e.g. the CLI session) reads
the cache, sees the pre-write mtime matches the cache, and serves the old
tokens. The desktop's instance then makes a request with the new tokens;
the CLI makes a request with the old tokens; the server rejects the CLI's
one with `AuthExpired`, and the user sees a confusing "you logged out" error.

**Suggested fix:** take the `cache` write lock *first*, then re-stat the file
under the lock, then write the cache. The current order is fine for the
in-memory case (the writer's `cached_mtime` is set after the file write) but
not for the between-process case.

### [HIGH] src/mcp/auth/callback.rs:140 — `url_decode` decodes percent-encoded bytes one at a time, but a *truncated* `%` at end of input is silently emitted as a literal, allowing a callback URL with a malformed `state` to slip past `secret_equal_bytes`
**Category:** Security (CSRF bypass)
**Confidence:** High

`url_decode` (line 257) handles `%XY` where XY is a hex pair. At the end of the
input, `%` followed by zero or one hex digits is emitted as a literal `%` (or
`%X`). The function never returns `Err`; any URL that gets as far as the
browser is passed through. The `state` parameter is then compared via
`crate::security::secret_equal_bytes` (line 240 of provider.rs):

```rust
if !crate::security::secret_equal_bytes(stored_state.as_bytes(), received_state.as_bytes())
```

If `stored_state` is `abc123` and the callback URL is
`/callback?code=…&state=abc12%`, the decoded `received_state` is `abc12%`,
the comparison fails, and the error is "State mismatch - possible CSRF
attack". Good. But if the **attacker's** callback URL is
`/callback?code=…&state=abc%20123` (URL-encoded space), the decoded
`state` is `abc 123`, the comparison fails, the error is the same. The
correct path is: any malformed `state` should be rejected as a *parse* error,
not a CSRF mismatch — the two errors suggest different mitigations.

**Suggested fix:** validate the raw `state` query parameter against a strict
character class (URL-safe base64 + `-` + `_`) *before* calling `secret_equal_bytes`.
The legacy escape hand-waving is a code smell.

### [MEDIUM] src/mcp/auth/provider.rs:55 — `OAuthServerMetadata` does not validate `issuer` is a URL, nor that `authorization_endpoint` / `token_endpoint` / `registration_endpoint` are URLs on the same origin as `issuer`
**Category:** Security (server substitution)
**Confidence:** High

`discover_metadata` (line 90) fetches `https://{server_url}/.well-known/oauth-authorization-server`
and accepts whatever JSON the server returns. RFC 8414 §3.3 requires the
client to verify the `issuer` matches the request URL. The provider does
neither:

```rust
response.json::<OAuthServerMetadata>().await
    .map_err(|e| AlephError::IoError(format!("Failed to parse OAuth metadata: {e}")))?;
```

The `issuer` is recorded as-is. The `authorization_endpoint` can be a
different origin (e.g. a server that hands out an `issuer: ""` and an
`authorization_endpoint: "https://evil.example/auth"`).

**Failure scenario:** A malicious MCP server's `/.well-known/oauth-authorization-server`
returns `issuer: "https://api.example.com"`, `authorization_endpoint: "https://evil.example/auth"`,
`token_endpoint: "https://api.example.com/token"`. The client registers
against the legitimate `https://api.example.com` issuer, then sends the user
to evil.example for authorization. The user authenticates against
evil.example, which returns a *legit* `code` for `api.example.com` (because
the client_id is the api.example.com one). The client redeems the code at
the **api.example.com** token endpoint, which returns a real token; the
attacker has now obtained a token bound to **the attacker's** auth at
evil.example, but credited to the user's client_id.

The attack is a *confused-deputy* against the user, not the client. The
defense is to validate that the endpoints are on the same origin as `issuer`.

**Suggested fix:** in `discover_metadata`, after parsing, reject the metadata
if any of the endpoints are not on the same origin as `issuer`. If `issuer` is
absent, refuse to proceed.

### [MEDIUM] src/mcp/auth/storage.rs:281 — `save_to_file` records `cached_mtime` *after* the file write, but the in-memory `cache` itself is not refreshed atomically — the reader's `load` can see a cache entry that the writer has not yet flushed
**Category:** Logic (read skew)
**Confidence:** Medium

`save_tokens` (line 287) takes the `cache` write lock, calls
`load_for_write` (which reads the cache or the file), then mutates the new
storage, then calls `save_to_file`. The `save_to_file` writes the file, then
sets `cached_mtime`. Inside `save_to_file`, the order is:

```rust
fs::write(&self.file_path, content).await?;
*self.cached_mtime.write().await = self.file_mtime().await;
```

The `cache` is `Some(new_storage)` *after* the file write completes. A
reader that takes the `cache` read lock between the file write and the
`cache` write sees the *old* cache, with the *new* `cached_mtime`. The
reader's `load` (line 211) compares `disk_mtime == cached_mtime` and decides
"no change". The cache says old, the file says new. The reader serves stale.

**Suggested fix:** set `cache` then `cached_mtime` in `save_to_file`, under
the already-held `cache` write lock. The current order is `file → cached_mtime`
in `save_to_file`, and `cache → file → cache` in the public methods. The
ordering is wrong.

### [MEDIUM] src/mcp/auth/storage.rs:200 — `load_for_write` takes `cached: Option<&StorageFile>` while the writer holds the `cache` write lock, but it does *not* take the `cached_mtime` lock — `load_for_write` reads `cached_mtime` while the *reader* of `load` is also reading it
**Category:** Logic (lock)
**Confidence:** Medium

Two locks: `cache` (write) and `cached_mtime` (read). The doc comment says
"`cached_mtime` is a separate lock, so this does not re-enter it." — true.
But the public `load` reads `cached_mtime` while the writer is mid-write.
The reader's `load` returns the cached storage without statting the file
(this is the fast path). The writer's `load_for_write` does the same.
The two paths are independent; the lock is incidental.

**Suggested fix:** collapse the two locks into one (struct with both fields).
Out of scope for this audit but worth noting.

### [LOW] src/mcp/auth/provider.rs:73 — `OAuthProvider::new` builds a `reqwest::Client` with `timeout(Duration::from_secs(30))`, but the `.unwrap_or_else(|_| Client::new())` swallows the build error and returns a client with no timeout
**Category:** Quality
**Confidence:** High

`Client::new()` has no timeout. If the `builder()` chain fails (e.g. TLS
config weirdness), the fallback is a client that can hang forever. The
auth flow's `discover_metadata` is on the startup path; a hung requirement
servers the caller indefinitely.

**Suggested fix:** surface the build error as `Err(AlephError::IoError(...))`
from `new`. The caller is `stored_bearer_token` (line 39 of mod.rs), which
sends the error to `tracing::warn!` and returns `None` — the connection
proceeds unauthenticated, which is the wrong default but acceptable.

### [LOW] src/mcp/auth/callback.rs:178 — `send_success_response` includes a `setTimeout(window.close, 3000)` script, but the script is unescaped — a server that controls the `error_description` could inject a `<script>` tag that runs in the user's browser
**Category:** Security (XSS)
**Confidence:** Medium

The error path (line 159) interpolates `error_description` into the *body* via
`html_escape` (line 192), so the error path is safe. The success path
(line 178) does the same. Good.

The bug is the static `setTimeout` block:

```rust
"<script>setTimeout(function() {{ window.close(); }}, 3000);</script>"
```

The `{{ }}` is Rust's escape for `{`/`}` in `format!` — the literal is
`{ window.close(); }`. No user data is interpolated here. Marker.

**Suggested fix:** none — keep the pattern.

### [LOW] src/mcp/auth/storage.rs:355 — `concurrent_instance_write_does_not_clobber_other_entry` test asserts the *production* invariant the previous finding breaks; the test passes because the writes are carefully sequenced
**Category:** Quality
**Confidence:** High

The test (line 514) sets up two `OAuthStorage` instances on the same file, both
writing concurrently. The test sleeps 20 ms between writes to ensure the
mtime advances. The real-world race is much tighter (a few μs). The test
proves the **design** is correct under serialized writes, but the finding
above (the order of `cache` and `cached_mtime` updates) breaks the design
under interleaved writes.

**Suggested fix:** covered by the previous finding.

## Architecture compliance (Batch 5)

| Redline | Status |
|---------|--------|
| R1 | clean — no platform APIs. |
| R3 | clean — uses `reqwest`, `sha2`, `rand`, `base64`. |
| R4 | clean — auth is a leaf module. |
| R7 | clean — no LLM. |
| R10 | clean — regex is only used for URL-decoding (machine format). |

## Cross-file note

The OAuth storage's disk-changed detection is the same pattern as
`McpPersistentConfig::save` (Batch 4). Both should `fsync` before rename, and
both should set `cached_mtime` *before* releasing the cache lock. The two
implementations have drifted: the OAuth storage sets `cached_mtime` in
`save_to_file`, the MCP config does not maintain a cache at all. Consistency
would help.
