# CLI Backend Gaps

> Companion to [CLI parity R2 work](../../interfaces/cli/src/commands). Tracks
> CLI surfaces that exist *as a thin-client subcommand* but whose backing RPC
> isn't fully implemented yet, plus the design intent for filling each gap.

The CLI cycle 2 wired the thin-client to every JSON-RPC method that already
exists on the gateway (pairing / devices / auth / hooks / secrets / OAuth).
This document collects the **remaining gaps** — surfaces where either the
backend doesn't exist at all, or only partially (no management RPC for a
fully implemented receiver, etc.).

Each section answers four questions:

1. **What exists today** (in-tree code, even if not wired)
2. **What's missing** (the gap)
3. **Proposed RPC + storage shape**
4. **Recommended sequence** for filling it

---

## 1. `al webhook` — inbound webhook subscriptions

### What exists today

- The former src/gateway/webhooks/ module — a complete receiver: HMAC-SHA256
  verification, GitHub / Stripe / Generic signature formats, session-key
  templating, `WebhookEndpointConfig` schema with `id / path / secret / agent /
  session_key_template / allowed_events / extract_headers / max_body_size` —
  was **cut on 2026-08-17** as dead code. The only webhook server the gateway
  mounts today is `src/gateway/webhook_receiver.rs`, a different shape
  (channel-level ingestion for WhatsApp / Generic, not external-service
  subscriptions).
- `src/tasks/cron/webhook_target.rs` — outbound webhook (delivery target
  for cron jobs).
- The removed src/gateway/webhooks/mod.rs was **never mounted** in any
  production startup path, and `WebhooksConfig` went with it — the global
  `Config` has no `[webhooks]` block any more.

### Gap

- No RPC to list / create / remove webhook endpoints at runtime.
- The receiver is gone, so even adding RPCs is a no-op until a receiver is
  re-landed and its router hooked into `GatewayServer::build_router`.

### Proposed RPC

```
webhooks.list        →  { endpoints: [WebhookEndpointConfig] }
webhooks.get         →  { endpoint: WebhookEndpointConfig }    params: { id }
webhooks.add         →  { endpoint, public_url }               params: WebhookEndpointConfig
webhooks.update      →  { endpoint }                           params: { id, patch }
webhooks.remove      →  { removed: true }                      params: { id }
webhooks.test        →  { fired: true, response_code }         params: { id, payload }
webhooks.deliveries  →  { items: [{ id, ts, status, body_excerpt }] }
                                                              params: { id, limit }
```

Errors: `-32004` (id not found), `-32030` (duplicate path),
`-32602` (validation failure).

### Storage

- Proposed storage: an inbound webhook endpoint table in SQLite under `~/.aleph/data/`.
  with columns mirroring `WebhookEndpointConfig` plus
  `created_at_unix / updated_at_unix`.
- Delivery log: ring buffer of last N deliveries per endpoint
  (default 50), capped at 1 MB on disk per endpoint. Pruned by a
  daemon similar to `src/tasks/shared/reaper.rs`.
- Secrets are written through `SharedTokenManager` (vault) under
  the key `webhook:{id}:secret`, never inlined in the SQLite row.

### Sequence to fill the gap

1. Mount an inbound subscription router in Gateway's `build_router` (the
   `webhooks.enabled` config key was removed with the module and has to come
   back with it). Backfill loads from SQLite into the in-memory
   `WebhookHandlerState` at boot.
2. Add an administrative handler module for the proposed subscription RPCs under `src/gateway/handlers/` with
   the 7 RPCs above.
3. Wire the existing `interfaces/cli/src/commands/webhook_cmd.rs`
   stubs to call those RPCs.
4. Panel page lists endpoints / payload preview / "fire test" button.

### Why deferred from R2

Pulling in the storage + router mount is at least a day of careful
work (router collision checks, restart-resume of in-flight
deliveries, secret rotation flow). R2 stays focused on parity for
already-shipped infrastructure.

---

## 2. `al proxy` — outbound network proxy configuration

### What exists today

- `src/sandbox/proxy/` — *internal* managed proxy that the sandbox
  driver spawns on `127.0.0.1` and routes sandboxed tool processes
  through. This is per-sandbox-spawn, transient, and not user
  configurable in the conventional sense.
- `reqwest` clients across the codebase honour the standard
  `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` env vars set when the
  server is launched.

### Gap

- No single source of truth for "what proxy should Aleph route LLM
  provider traffic through?". Today the answer is "whatever the env
  said when `aleph-server start` ran".
- No way to set / clear that without restarting the server.
- No per-provider override (e.g. route OpenAI through proxy A,
  Anthropic direct).

### Proposed RPC

```
proxy.show     →  { effective: { https, http, no_proxy }, source: "env" | "config" | "runtime" }
proxy.set      →  { applied: ProxyConfig }       params: ProxyConfig
proxy.clear    →  { cleared: true }
proxy.providers →  { overrides: { provider: ProxyConfig } }  // per-provider future
```

Proposed proxy settings:

```rust
struct ProxyConfig {
    https: Option<String>,
    http: Option<String>,
    no_proxy: Vec<String>,
    auth: Option<{ username, secret_ref }>,  // secret_ref → vault key
}
```

### Storage

- One row in a dedicated table for persisted proxy settings (SQLite). Single-tenant —
  there is one effective config at a time.
- Auth credentials in the existing vault under
  `proxy:{key}:password`; never inlined in the SQLite row.
- An in-memory override layer above the persisted config so
  `proxy.set` takes effect immediately without disk I/O.

### Sequence

1. Define the proxy settings schema and SQLite migration.
2. Wire one proxy-policy resolver into each long-lived `reqwest::Client`
   constructor (`src/providers/*`, `gateway/clawhub`, MCP transports).
   Resolver returns `(url, optional auth)` for a given host.
3. Add 4 RPCs above + wire `interfaces/cli/src/commands/proxy_cmd.rs`
   stubs to them.
4. Hot-reload signal so providers swap their reqwest client without
   restart when `proxy.set` fires.

### Why deferred from R2

Touching every reqwest construction site (~12 call sites) is a
cross-cutting refactor that demands its own cycle.

---

## 3. `aleph auth login` — provider OAuth

### Status: **already wired in R2** ✅

This is *not* a backend gap — the gap was in the CLI catalogue.

### What exists today

- `providers.oauthLogin` / `providers.oauthLogout` /
  `providers.oauthStatus` — JSON-RPC handlers in
  `src/gateway/handlers/oauth.rs`.
- Supports `codex` / `chatgpt` provider aliases (both map to the
  ChatGPT OAuth flow via `CodexAuth::authorize_via_browser`).
- Tokens persist via `SharedTokenManager` (vault) with expiry +
  refresh-token tracking.

### What R2 added

- `aleph auth login <provider>`     →  `providers.oauthLogin`
- `aleph auth logout <provider>`    →  `providers.oauthLogout`
- `aleph auth oauth-status <provider>` → `providers.oauthStatus`

### Future work

- More providers: Anthropic, Google, OpenAI proper.
- Per-account multi-tenancy (the current store is single-token).
- Token-rotation hooks for daemon-only setups.

---

## Cross-cutting: how new CLI surfaces should land

When filling these gaps:

1. **Wire the RPC first.** A `clap` subcommand without a backend is a
   help-text-only placeholder — it lives in `*_cmd.rs` returning
   "not yet implemented" until the RPC lands.
2. **Reuse stores.** Vault for secrets, SQLite for structured state,
   the existing reaper pattern for cleanup. Don't introduce new
   storage primitives.
3. **Validate at boundary.** Same shape as
   `src/gateway/handlers/secrets.rs` — `INVALID_PARAMS` for shape
   failures, dedicated codes (`NOT_FOUND` = `-32004`) for semantic
   failures, generic `INTERNAL_ERROR` for everything else.
4. **Add a parse-guard test in `main.rs`** for backward-compat on
   every flag set you commit to.
