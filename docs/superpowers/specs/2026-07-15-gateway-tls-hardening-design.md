# Gateway remote-connection TLS hardening — design

**Date:** 2026-07-15
**Scope decision (user-approved):** Public direct-connect from any device/browser · has a
domain · **combined**: Caddy reverse proxy as the default path + optional native in-process
TLS (incl. **self-signed, no-domain**). Aleph-side code closes the deferred **F5
trusted-proxy** gap and adds an openclaw-style refuse-insecure-remote guardrail.
**Hard principle (user):** keep the reverse-proxy config trivially simple — push all
robustness/complexity into Aleph's code, keep the user's proxy config a one-liner.

Continues [2026-07-15-gateway-remote-auth-hardening-design.md](2026-07-15-gateway-remote-auth-hardening-design.md),
which deferred F5 (trusted-proxy XFF). This round lands it.

---

## Context — the actual state

Aleph's gateway serves **plaintext only**. `GatewayServer::run` binds a bare
`tokio::net::TcpListener` + `axum::serve` (`src/gateway/server/mod.rs:668,692`); there is
**no TLS acceptor anywhere in `src/gateway/`** and **no TLS config key** — `GatewayServerConfig`
has `host / port / allowed_origins / allow_any_origin` and nothing else
(`src/gateway/config.rs:94`). The startup banner prints `ws://` / `http://`. By deliberate
design (SECURITY.md §auth-ux): *"The Gateway token is the only transport auth Aleph itself
provides"* — transport encryption is expected to be terminated upstream.

The remote-auth path itself is **already strong** (4-level `resolve_connect_auth`:
loopback → device_token → bootstrap_ticket → legacy shared token → wall; SHA-256-hashed
device tokens, constant-time shared-token verify, atomic single-use tickets,
DNS-rebinding-aware `origin_policy.rs`, per-IP `Auth`-scope rate-limit 10/60s + 5-min lockout,
per-connection flood guard, per-IP connection cap, token-rotation force-close). **Do not
re-harden auth.** The single missing property is **wire confidentiality**, plus one structural
gap it exposes.

### The user's current Debian box (the thing being fixed)

A public ColoCrossing VPS running `aleph-server` reachable over the public internet by IP +
token. To allow that remote connect, it must currently bind `0.0.0.0` and speak **plaintext
`ws://`** — the token and all conversation data cross the public internet in the clear,
sniffable/MITM-able by anyone on-path. This is the exact posture to eliminate.

### Reference audit (openclaw / hermes-agent, under /Volumes/TBU4/Github)

- **openclaw** (TS) offers three transport-security tiers: (1) native `gateway.tls.enabled`
  in-process TLS; (2) **Tailscale Serve/Funnel** managed TLS (preferred for public/mobile);
  (3) external reverse proxy via `gateway.remote.url = wss://`. Crucially it **enforces** a
  secure gateway URL (`wss://`) for public/mobile pairing — it *refuses* insecure remote
  pairing. We port that refuse-insecure-remote idea (C3) and the native-TLS option (C1); we
  do **not** port Tailscale integration (user requires browser direct-connect, not a mesh).
- **hermes-agent** (Py) terminates TLS at an external reverse proxy and layers OAuth/OIDC
  (issuer / public_url) on top. Confirms the "terminate upstream" model; no in-process TLS.

Verdict: the reverse-proxy path is what Aleph's architecture already prescribes; native
self-signed TLS is the pragmatic no-domain add openclaw already validates.

---

## Threat model & the three-tier topology

**Governing invariant: loopback (`127.0.0.1`) stays plaintext `ws://`; every *remote*
(non-loopback) connection MUST be TLS — plaintext to a remote is absolutely forbidden.**
Loopback traffic never leaves the machine, so it cannot be sniffed by a network attacker;
keeping it plaintext preserves the zero-config desktop / CLI-IPC / Caddy→aleph-internal-hop
paths untouched (the desktop-operator redline). The `0.0.0.0`-plaintext-to-the-network posture
is the one shape being eradicated. One Aleph codebase supports three operator-selectable
remote tiers, ordered convenience ↔ strength:

| Tier | Topology | Passive sniff | Active MITM | Browser UX | Cert ops |
|------|----------|:---:|:---:|:---:|:---:|
| **① Caddy auto-HTTPS** *(user's primary; needs domain)* | browser ──https/wss (LE real cert)──> Caddy:443 ──ws **loopback** (same box)──> aleph `127.0.0.1:18790` | ✅ | ✅ | 🔒 green | none (auto-renew) |
| **② Native self-signed** *(no domain, "save the hassle")* | device ──wss (self-signed)──> aleph `0.0.0.0:18790` (in-proc rustls) | ✅ | ⚠️ (needs client cert-pin) | ❗ warning | auto-generated |
| **③ Native + real cert** *(single-binary purist)* | browser ──wss (real cert)──> aleph `0.0.0.0:18790` (in-proc rustls) | ✅ | ✅ | 🔒 green | operator (certbot) |

The Caddy→aleph hop in tier ① is plaintext `ws://` but is a **co-located same-machine loopback
hop** that never touches any network — identical trust to the desktop App talking to its own
loopback gateway. This is what keeps the Caddy config a literal one-liner.

**Honest bound on tier ②:** self-signed TLS **encrypts but does not authenticate the server
identity**. It defeats a *passive* eavesdropper (the token is no longer on the wire in the
clear) but not an *active* MITM who presents their own self-signed cert, because browsers
train users to click "proceed anyway". Pinning the printed SHA-256 fingerprint on a native
client closes that; browsers can't pin. Even so, **self-signed ≫ plaintext** and is a valid
"encrypt without a domain" rung.

---

## Design principle — simple proxy config, complex Aleph

> Push every knob into Aleph; keep the operator's reverse-proxy config minimal and default.

Concretely this constrains the design:

- **Caddy is a literal one-liner** — `your.domain { reverse_proxy 127.0.0.1:18790 }`. Caddy v2
  already: auto-provisions & renews the LE cert, auto-redirects HTTP→HTTPS, auto-forwards
  `X-Forwarded-For` / `X-Forwarded-Proto` / `X-Forwarded-Host`, and auto-detects the WebSocket
  `Upgrade` (no special directive). Nothing else is required or recommended.
- **nginx is a minimal required block** (only because nginx does *not* forward those headers
  or handle WS upgrade by default). Kept to the essential `proxy_set_header` + `Upgrade`
  lines; documented as second choice with a "prefer Caddy for the one-liner" note.
- **All robustness lives in Aleph, not the proxy:** trusted_proxy defaults to trusting
  loopback (operator only flips `enabled = true`); XFF parsing is fail-safe on malformed
  input (falls back to the raw peer IP, never trusts a spoofable value); WebSocket keepalive,
  read timeouts, and large-payload limits are handled server-side; no operator is ever asked
  to tune proxy buffer sizes, timeouts, or header rewrites.

---

## B. Aleph-side code changes (the implementable spec)

All in `src/gateway/` (the network trust boundary — **auth/authz/origin changes MUST update
tests in the same change**, per `src/gateway/CLAUDE.md`). R3-lean: adds `tokio-rustls` /
`axum-server` and the pure-Rust `rcgen` (self-signed generation) — **no ACME crate in core**
(auto-issuance is delegated to Caddy or the operator's certbot).

### C1 — `[gateway.tls]` native in-process TLS

New config struct `GatewayTlsConfig`:

```toml
[gateway.tls]
enabled   = false        # default off → current plaintext behavior, unchanged
cert_path = ""           # optional PEM chain; empty + enabled ⇒ auto self-signed
key_path  = ""           # optional PEM key
```

Three sub-modes, resolved at server start:

1. **Provided cert/key** (`cert_path` + `key_path` set) — load them (real CA cert from
   certbot, or an externally-made self-signed). Tier ③.
2. **Auto self-signed** (`enabled = true`, paths empty) — generate a self-signed cert via
   `rcgen`, persist to `~/.aleph/tls/{cert.pem,key.pem}` (reuse on next boot), and **print the
   cert's SHA-256 fingerprint** to the startup log so it can be verified/pinned client-side.
   Tier ②.
3. **Disabled** (`enabled = false`) — today's plaintext `axum::serve`. Unchanged.

Implementation: when TLS is enabled, replace `axum::serve(TcpListener, …)` with
`axum_server::bind_rustls(addr, RustlsConfig::from_pem_file(...))` (or an in-memory
`RustlsConfig` for the generated cert). The router, connect-info, and all handlers are
identical — only the accept layer changes. Cert renewal for tier ③ = restart (no in-process
SIGHUP reload in v1; note as a known limitation). Startup banner prints `wss://` when TLS is on.

### C2 — `[gateway.trusted_proxy]` XFF/Proto resolution (closes deferred **F5**)

```toml
[gateway.trusted_proxy]
enabled     = false                    # default off → behavior identical to today
trusted_ips = ["127.0.0.1", "::1"]     # immediate-peer IPs whose X-Forwarded-* are believed
```

Behavior — a single resolution point maps the *transport* peer to an **effective client
identity** `{ ip, secure }`:

- If the **immediate TCP peer** ∈ `trusted_ips`: take the client IP as the **last (rightmost)
  entry** of `X-Forwarded-For` — the address the trusted proxy itself appended — and
  `secure = (X-Forwarded-Proto == "https")`. **v1 supports a single trusted proxy hop** (the
  recipe's `browser → Caddy → aleph`); a multi-proxy chain with a per-hop trust depth is an
  explicit non-goal for v1 (documented, not silently mis-parsed).
- Otherwise (untrusted peer, or `enabled = false`): **ignore XFF entirely**, `ip = raw peer
  IP`, `secure = connection is native-TLS`. This is what makes XFF unspoofable — a direct
  attacker who is not the configured proxy can never inject a forged client IP.
- Malformed / absent XFF when peer is trusted ⇒ fail-safe to the raw peer IP (never panic,
  never trust a garbage value).

The resolved **effective client IP** replaces the raw `SocketAddr` fed to the two consumers
that currently collapse behind a proxy:

- the per-IP `Auth`-scope **rate-limiter** + **per-IP connection cap** (real client throttled,
  not the shared proxy IP — one abuser no longer locks out everyone, and the limiter actually
  bites again);
- the **security audit log** `AuthFailure` / `RateLimited` entries (records the real attacker
  IP, making a fail2ban jail meaningful).

The per-connection `UnauthorizedFloodGuard` is already per-connection and needs no change.

### C3 — `[gateway] allow_insecure_remote` — plaintext-remote is forbidden by default

**User hard requirement:** a remote (IP/network) connection MUST be TLS; plaintext to a
remote is *absolutely forbidden*. This is expressed as **one** knob, secure-by-default:

```toml
[gateway]
allow_insecure_remote = false   # default → no plaintext EVER reaches a non-loopback client
```

Semantics of the default (`false`) — enforced at **two** layers (defense in depth):

- **Per-connect runtime gate** (in `connect`): a connection whose **effective client IP is
  non-loopback** and whose transport is **not secure** (`secure == false`: neither native-TLS
  nor a trusted-proxy `X-Forwarded-Proto: https`) is **refused** with a clear diagnostic.
  Loopback is always exempt (same-machine, never leaves the box → the zero-config desktop
  redline is preserved).
- **Boot gate** (see C4): the server refuses to even bind a plaintext listener on a
  non-loopback interface.

Set `allow_insecure_remote = true` only to knowingly restore the legacy "I trust my LAN"
plaintext model (SECURITY.md §auth-ux). Loopback behavior is identical regardless of this flag.

This generalizes openclaw's "secure URL required for remote pairing" to *every* remote connect,
and makes "no token ever crosses plaintext to a remote" a **guarantee**, not a convention.

### C4 — boot gate: refuse to expose plaintext (fail-closed, replaces the old warning)

`warn_if_network_exposed` (`src/gateway/server/mod.rs:649`) is upgraded from a *warning* to a
**hard refusal**. At startup, if `host` is **non-loopback** AND TLS is **disabled** AND
`trusted_proxy` is **not** enabled (no upstream that could be terminating TLS) AND
`allow_insecure_remote == false` → the server **exits with a diagnostic** naming the fix
(enable `[gateway.tls]`, or a reverse proxy + `[gateway.trusted_proxy]`, or — knowingly —
`allow_insecure_remote = true`). It never silently serves plaintext to the network. Loopback
bind and all TLS/proxy tiers start normally.

### C5 — Panel client forces `wss://` for remote (browser rejects http)

Scope note: this touches the **Panel (Leptos/WASM)** crate, not `src/gateway/`. The Panel
derives its gateway WebSocket URL from `window.location`:

- **loopback host** (`127.0.0.1` / `::1` / `localhost`) → `ws://` allowed (zero-config desktop).
- **non-loopback host** → **`wss://` only**, hard-coded. The Panel never constructs a `ws://`
  URL to a remote host. If the Panel page itself was loaded over **`http:` from a non-loopback
  host**, it refuses to connect and shows an "insecure transport — use https" error instead of
  silently opening a plaintext socket.

Server-side belt-and-suspenders for "browser rejects http": tier ① Caddy auto-redirects
http→https; native-TLS tiers serve only on the TLS port; and an **HSTS** response header
(added by Caddy in tier ①, and by the gateway's `security/headers.rs` in native-TLS tiers)
makes browsers auto-upgrade subsequent visits.

### Config-combination map

| Tier | `tls.enabled` | `trusted_proxy.enabled` | `allow_insecure_remote` | `host` |
|------|:---:|:---:|:---:|:---:|
| ① Caddy | false | **true** | `false` (default) | `127.0.0.1` |
| ② self-signed | **true** (auto) | false | `false` (default) | `0.0.0.0` |
| ③ native cert | **true** (paths) | false | `false` (default) | `0.0.0.0` |

`allow_insecure_remote` stays at its secure default (`false`) in every tier — no tier ever
sets it. The two secure paths are mutually exclusive and internally consistent: the proxy path
proves security via `X-Forwarded-Proto`, the native path by terminating TLS itself.

---

## C. Debian deployment recipe — tier ① (operational, zero code)

The migration for the user's current box (from `0.0.0.0`-plaintext):

1. **Aleph config** (`~/.aleph/config.toml`):
   ```toml
   [gateway]
   host = "127.0.0.1"                     # withdraw from 0.0.0.0 back to loopback
   allowed_origins = ["https://your.domain"]
   # allow_insecure_remote defaults to false → plaintext-remote already forbidden; nothing to set

   [gateway.trusted_proxy]
   enabled = true                         # trusts loopback by default; nothing else to set
   ```
2. **Caddy** (`/etc/caddy/Caddyfile`) — the entire file:
   ```
   your.domain {
       reverse_proxy 127.0.0.1:18790
   }
   ```
   (auto HTTPS + auto renew + HTTP→HTTPS + XFF + WS upgrade, all implicit.)
3. **UFW**: allow `443`, `80` (ACME challenge + redirect), `22` (ideally source-restricted);
   deny the rest. `18790` is already unreachable publicly (loopback bind); UFW is belt-and-braces.
4. **systemd hardening** for `aleph-server` (`NoNewPrivileges`, `ProtectSystem=strict`,
   `ProtectHome`, `PrivateTmp`) — sample unit in the plan.

Tier ② (no domain): skip Caddy/UFW-443; set `[gateway.tls] enabled = true`, `host = "0.0.0.0"`
(`allow_insecure_remote` stays `false`); open `18790`; verify the printed fingerprint on first
connect.

---

## D. Defense-in-depth checklist (focused, not sprawling)

- `gateway.token.rotate` once after setup; treat the token as a root-equivalent secret (an
  authorized remote = full operator incl. shell/PTY) — transmit only over a trusted channel.
- Caddy: add an HSTS header (one line) — the sole recommended extra beyond the one-liner.
- SSH: key-only, no root login, optional non-standard port.
- Optional fail2ban jail on the audit log's `AuthFailure` (meaningful now that C2 records the
  real client IP).

## E. Explicitly unchanged (already strong — do not touch)

4-level `resolve_connect_auth`, device-token / bootstrap-ticket lifecycle, `origin_policy.rs`
+ DNS-rebinding defense, SSRF engine, exec tiers, per-connection flood guard. All orthogonal
to wire confidentiality.

---

## Testing (mandatory — this is the trust boundary)

Per `src/gateway/CLAUDE.md`, auth/authz/origin changes must ship tests in the same change:

- **C1**: config parse (all three sub-modes); self-signed generation is well-formed + persisted
  + reused on second call; fingerprint is stable & correctly computed; enabled-no-paths vs
  enabled-with-paths vs disabled selection.
- **C2**: XFF resolution table — trusted peer + valid XFF ⇒ last-entry client IP; untrusted
  peer + XFF ⇒ **raw peer IP** (spoof rejected); malformed/empty XFF ⇒ fail-safe to peer IP;
  `X-Forwarded-Proto` → `secure` mapping; `enabled=false` ⇒ raw peer IP always.
- **C3**: `allow_insecure_remote` truth table — {loopback, remote} × {secure, insecure} ×
  {false=default, true} ⇒ allow/refuse; loopback always allowed; remote-insecure refused when
  `false`; refusal audited via existing `should_audit_connect_failure`.
- **C4**: boot gate refuses to start iff `host` non-loopback ∧ TLS disabled ∧ `trusted_proxy`
  off ∧ `allow_insecure_remote == false`; starts normally in every other combination
  (loopback bind, any TLS tier, proxy tier, or explicit opt-out).
- **C5** (Panel, WASM test): loopback host ⇒ `ws://`; non-loopback host over https page ⇒
  `wss://`; non-loopback host reached over an `http:` page ⇒ refuse + error, no socket opened.
- Regression: the **default loopback install** (no new keys set, `host` at its `127.0.0.1`
  default) behaves **identically** — the zero-config desktop path is untouched. See migration
  note for the one intentionally-broken case.

## R3 / redline compliance

- **R3 核心轻量化**: adds `tokio-rustls` + `axum-server` (standard axum TLS companions) and
  `rcgen` (pure-Rust self-signed, no OpenSSL, no ACME). ACME auto-issuance is deliberately
  **out of core** — delegated to Caddy or certbot. No second async runtime, no platform crate,
  no non-serde serialization.
- **R4 (I/O-only interfaces)**: all changes are transport/handshake plumbing in `src/gateway/`;
  no business logic added.
- **New config keys are additive, off-by-default**; the config root has no
  `deny_unknown_fields`, so older configs load unchanged. The **default loopback install is
  untouched** (host `127.0.0.1` → C4 gate never fires).

### One intentional breaking change (secure-by-default)

`allow_insecure_remote` defaults to `false`, and C4 is fail-closed. Therefore the **only**
configs that change behavior are those that **explicitly set `host = "0.0.0.0"` (or another
non-loopback interface) with no TLS and no reverse proxy** — i.e. today's plaintext-to-the-LAN
opt-in. After upgrade those refuse to boot with a diagnostic pointing at the fix. This is a
deliberate, security-positive break of the legacy plaintext-LAN posture (SECURITY.md §auth-ux),
not an accident. Recovery is one line: add TLS/proxy, **or** set `allow_insecure_remote = true`
to keep the old behavior knowingly. Called out in release notes.

## Rollout

1. Land C1–C4 (`src/gateway/`) behind off-by-default config + C5 (Panel forces wss for remote).
   No behavior change to the default loopback install until opted in.
2. Deploy new binary to the Debian box.
3. Flip to tier ① (loopback bind + Caddy one-liner + trusted_proxy; `allow_insecure_remote`
   stays at its secure default).
4. `gateway.token.rotate`, verify green-lock wss:// connect from a remote browser, verify a
   direct `:18790` public hit now fails (loopback-bound), verify audit log shows real client IP.

## Known limitations (honest)

- Native-TLS cert renewal (tier ③) needs a restart in v1 (no SIGHUP reload).
- Self-signed (tier ②) protects against passive sniffing, not active MITM, on browsers that
  can't pin — documented at point of use.
- `tools.invoke` argument-level tier parity and the wait-mode-child unattended-inheritance
  items from the prior round remain out of scope here.
