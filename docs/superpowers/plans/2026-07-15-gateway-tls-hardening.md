# Gateway remote-connection TLS hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every *remote* (non-loopback) gateway connection TLS-encrypted — via a reverse proxy (default) or optional native in-process TLS (incl. self-signed) — and close the trusted-proxy XFF gap so per-IP protections and the audit log see the real client behind a proxy.

**Architecture:** Five off-by-default changes, four in `src/gateway/` plus one in the Panel. A single-point client-IP resolution at the WS-upgrade seam feeds all IP-keyed consumers *and* the connect-auth loopback test; native TLS swaps the accept layer only; a boot gate + per-connect gate make plaintext-to-a-remote fail-closed. Loopback stays plaintext (desktop/CLI/internal-proxy-hop redline).

**Tech Stack:** Rust, axum 0.8, `axum-server` (tls-rustls), `rustls` 0.23 (ring provider), `rcgen` 0.13 (self-signed), Leptos/WASM (Panel).

## Global Constraints

- **Loopback stays plaintext `ws://`; every non-loopback connection MUST be TLS.** Loopback (`127.0.0.1`/`::1`) is always exempt — the zero-config desktop / CLI-IPC / Caddy→aleph same-machine hop must behave identically.
- **All new config keys are off-by-default.** The default loopback install must be byte-for-byte unchanged. The config root has no `deny_unknown_fields` (old TOML keeps loading).
- **One intentional breaking change:** a config that explicitly sets a non-loopback `host` with no TLS and no reverse proxy now *refuses to boot* (secure-by-default). Recovery: add TLS/proxy, or set `allow_insecure_remote = true`.
- **R3 (核心轻量化):** add only `axum-server`, `rustls` (ring), `rcgen`. No ACME crate — auto-issuance is delegated to Caddy/certbot. No second async runtime, no platform crate, no non-serde serialization.
- **`src/gateway/` is the trust boundary:** auth/authz/origin changes MUST ship tests in the same task (`src/gateway/CLAUDE.md`).
- **Simple proxy config:** the Caddy recipe stays a one-liner; all robustness lives in Aleph.
- **Test command:** `cargo test -p alephcore --lib <test_name>` (targeted — never the full suite; the user is cargo-frugal).

---

### Task 1: Config types + dependencies

**Files:**
- Modify: `Cargo.toml` (workspace deps)
- Modify: `src/gateway/config.rs:92-204` (struct `GatewayServerConfig` + its `Default`)
- Test: `src/gateway/config.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `GatewayTlsConfig { enabled: bool, cert_path: String, key_path: String }`; `TrustedProxyConfig { enabled: bool, trusted_ips: Vec<String> }`; new `GatewayServerConfig` fields `tls: GatewayTlsConfig`, `trusted_proxy: TrustedProxyConfig`, `allow_insecure_remote: bool`.

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

Under `[dependencies]` (add alongside the existing `axum = { version = "0.8", features = ["ws"] }`):

```toml
axum-server = { version = "0.7", features = ["tls-rustls"] }
rustls = { version = "0.23", default-features = false, features = ["ring"] }
rcgen = "0.13"
```

- [ ] **Step 2: Write the failing test** (append to the `mod tests` block in `src/gateway/config.rs`)

```rust
#[test]
fn tls_and_trusted_proxy_default_off_and_parse() {
    // Defaults: everything off, loopback trusted.
    let d = GatewayServerConfig::default();
    assert!(!d.tls.enabled);
    assert!(!d.trusted_proxy.enabled);
    assert!(!d.allow_insecure_remote);
    assert_eq!(d.trusted_proxy.trusted_ips, vec!["127.0.0.1", "::1"]);

    // Round-trips from TOML.
    let toml = r#"
host = "0.0.0.0"
allow_insecure_remote = false
[tls]
enabled = true
cert_path = "/x/cert.pem"
[trusted_proxy]
enabled = true
"#;
    let c: GatewayServerConfig = toml::from_str(toml).unwrap();
    assert!(c.tls.enabled);
    assert_eq!(c.tls.cert_path, "/x/cert.pem");
    assert!(c.trusted_proxy.enabled);
    assert_eq!(c.trusted_proxy.trusted_ips, vec!["127.0.0.1", "::1"]); // still defaulted
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tls_and_trusted_proxy_default_off_and_parse`
Expected: FAIL — compile error, `GatewayTlsConfig` / field `tls` not found.

- [ ] **Step 4: Add the config types** — insert immediately before `/// Gateway server configuration` (above line 91 in `src/gateway/config.rs`):

```rust
/// Native in-process TLS for the gateway listener. Default off → plaintext,
/// unchanged. When `enabled` with empty paths, a self-signed cert is
/// auto-generated and persisted (see [`crate::gateway::tls`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayTlsConfig {
    /// Terminate TLS in-process. Default false.
    pub enabled: bool,
    /// PEM certificate chain path. Empty + `enabled` ⇒ auto self-signed.
    pub cert_path: String,
    /// PEM private-key path. Empty + `enabled` ⇒ auto self-signed.
    pub key_path: String,
}

impl Default for GatewayTlsConfig {
    fn default() -> Self {
        Self { enabled: false, cert_path: String::new(), key_path: String::new() }
    }
}

/// Trusted reverse-proxy forwarding. When `enabled`, `X-Forwarded-For` /
/// `X-Forwarded-Proto` from an immediate peer in `trusted_ips` are believed,
/// restoring the real client IP and TLS status behind a proxy. Default off.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrustedProxyConfig {
    /// Honor forwarding headers from trusted peers. Default false.
    pub enabled: bool,
    /// Immediate-peer IPs whose `X-Forwarded-*` are trusted. Default loopback.
    pub trusted_ips: Vec<String>,
}

impl Default for TrustedProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            trusted_ips: vec!["127.0.0.1".to_string(), "::1".to_string()],
        }
    }
}
```

- [ ] **Step 5: Add the three fields to `GatewayServerConfig`** — insert after the `allow_any_origin` field (after line 118 in `src/gateway/config.rs`):

```rust
    /// Native in-process TLS. See [`GatewayTlsConfig`].
    #[serde(default)]
    pub tls: GatewayTlsConfig,
    /// Trusted reverse-proxy forwarding. See [`TrustedProxyConfig`].
    #[serde(default)]
    pub trusted_proxy: TrustedProxyConfig,
    /// Allow plaintext to a remote (non-loopback) client. Default `false` ⇒
    /// remote connections MUST be TLS (native or trusted-proxy https); an
    /// insecure remote is refused and the server refuses to bind a plaintext
    /// non-loopback listener. Set `true` only to knowingly restore
    /// LAN-plaintext trust.
    #[serde(default)]
    pub allow_insecure_remote: bool,
```

- [ ] **Step 6: Add the fields to the `Default` impl** — inside `impl Default for GatewayServerConfig` (after `allow_any_origin: false,`, line 192):

```rust
            tls: GatewayTlsConfig::default(),
            trusted_proxy: TrustedProxyConfig::default(),
            allow_insecure_remote: false,
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test -p alephcore --lib tls_and_trusted_proxy_default_off_and_parse`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock src/gateway/config.rs
git commit -m "gateway: add tls / trusted_proxy / allow_insecure_remote config (off by default)"
```

---

### Task 2: `trusted_proxy::resolve_client` pure resolver

**Files:**
- Create: `src/gateway/trusted_proxy.rs`
- Modify: `src/gateway/mod.rs` (add `pub mod trusted_proxy;`)
- Test: in `src/gateway/trusted_proxy.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `TrustedProxyConfig` (Task 1) — but takes `enabled: bool` + `trusted_ips: &[IpAddr]` directly to stay pure.
- Produces: `pub struct ResolvedClient { pub ip: IpAddr, pub secure: bool }`; `pub fn resolve_client(peer: IpAddr, headers: &HeaderMap, enabled: bool, trusted_ips: &[IpAddr]) -> ResolvedClient`.

- [ ] **Step 1: Register the module** — add to `src/gateway/mod.rs` alongside the other `pub mod` lines:

```rust
pub mod trusted_proxy;
```

- [ ] **Step 2: Write the failing test** — create `src/gateway/trusted_proxy.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr { s.parse().unwrap() }
    fn hdrs(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn trusted_peer_uses_last_xff_and_proto() {
        let r = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "https")]),
            true,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("203.0.113.7"));
        assert!(r.secure);
    }

    #[test]
    fn untrusted_peer_ignores_xff_no_spoof() {
        // Peer is NOT in trusted_ips → XFF is ignored, raw peer wins, not secure.
        let r = resolve_client(
            ip("198.51.100.9"),
            &hdrs(&[("x-forwarded-for", "127.0.0.1"), ("x-forwarded-proto", "https")]),
            true,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("198.51.100.9"));
        assert!(!r.secure);
    }

    #[test]
    fn disabled_always_raw_peer() {
        let r = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[("x-forwarded-for", "203.0.113.7"), ("x-forwarded-proto", "https")]),
            false,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("127.0.0.1"));
        assert!(!r.secure);
    }

    #[test]
    fn malformed_xff_falls_back_to_peer() {
        let r = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[("x-forwarded-for", "not-an-ip")]),
            true,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("127.0.0.1"));
        assert!(!r.secure); // no proto header
    }

    #[test]
    fn last_entry_of_multi_hop_xff() {
        // v1 single-hop: the trusted proxy appended the rightmost entry.
        let r = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[("x-forwarded-for", "10.0.0.5, 203.0.113.7")]),
            true,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("203.0.113.7"));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --lib trusted_proxy::tests`
Expected: FAIL — `resolve_client` / `ResolvedClient` not found.

- [ ] **Step 4: Write the implementation** — prepend to `src/gateway/trusted_proxy.rs` (above the test module):

```rust
//! Trusted reverse-proxy client resolution.
//!
//! Behind a reverse proxy the transport peer is the proxy, so IP-keyed
//! protections (per-IP cap, rate-limit, audit) and the connect-auth loopback
//! test would all collapse onto the proxy address. When the immediate peer is a
//! configured trusted proxy, this restores the real client from
//! `X-Forwarded-For` and the client-leg TLS status from `X-Forwarded-Proto`.
//! An untrusted peer's forwarding headers are ignored entirely, so they can
//! never be spoofed. v1 trusts a single proxy hop (browser → proxy → aleph).

use std::net::IpAddr;

use axum::http::HeaderMap;

/// The effective client identity after honoring a trusted proxy's headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedClient {
    /// Effective client IP: the forwarded client behind a trusted proxy, else
    /// the raw transport peer.
    pub ip: IpAddr,
    /// Whether the client-facing leg was TLS, per a trusted proxy's
    /// `X-Forwarded-Proto: https`. Native in-process TLS is folded in by the
    /// caller (via `tls_enabled`), not here.
    pub secure: bool,
}

/// Resolve the effective client for an inbound WS upgrade. See module docs.
#[must_use]
pub fn resolve_client(
    peer: IpAddr,
    headers: &HeaderMap,
    enabled: bool,
    trusted_ips: &[IpAddr],
) -> ResolvedClient {
    if !enabled || !trusted_ips.contains(&peer) {
        return ResolvedClient { ip: peer, secure: false };
    }
    let ip = last_forwarded_for(headers).unwrap_or(peer);
    let secure = forwarded_proto_https(headers);
    ResolvedClient { ip, secure }
}

/// The last (rightmost) valid IP in `X-Forwarded-For` — the address the trusted
/// proxy itself appended. `None` on absent/garbage input (caller falls back).
fn last_forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    let raw = headers.get("x-forwarded-for")?.to_str().ok()?;
    raw.rsplit(',')
        .map(str::trim)
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse::<IpAddr>().ok())
}

fn forwarded_proto_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("https"))
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib trusted_proxy::tests`
Expected: PASS (5 tests)

- [ ] **Step 6: Commit**

```bash
git add src/gateway/trusted_proxy.rs src/gateway/mod.rs
git commit -m "gateway: add trusted-proxy client resolver (pure, spoof-safe)"
```

---

### Task 3: Wire resolver into the WS-upgrade seam (closes F5)

**Files:**
- Modify: `src/gateway/server/mod.rs` (`GatewaySharedState` struct ~line 300-316; its construction in `build_router`)
- Modify: `src/gateway/server/handler.rs:106-172` (`ws_upgrade_handler`)
- Test: `src/gateway/server/handler.rs` (`#[cfg(test)] mod tests`) — parse helper

**Interfaces:**
- Consumes: `trusted_proxy::resolve_client` (Task 2); config fields (Task 1).
- Produces: `GatewaySharedState` gains `trusted_proxy_enabled: bool`, `trusted_proxy_ips: Vec<IpAddr>`, `allow_insecure_remote: bool`, `tls_enabled: bool`. `handler.rs` sets `client_ip` + a local `secure: bool` from the resolver. (The C3 gate that *uses* `secure` is Task 5.)

- [ ] **Step 1: Write the failing test** — append to `src/gateway/server/handler.rs` `mod tests`:

```rust
#[test]
fn parses_trusted_ips_dropping_garbage() {
    let parsed = super::parse_trusted_ips(&[
        "127.0.0.1".to_string(),
        "::1".to_string(),
        "not-an-ip".to_string(),
    ]);
    assert_eq!(parsed.len(), 2);
    assert!(parsed.contains(&"127.0.0.1".parse().unwrap()));
    assert!(parsed.contains(&"::1".parse().unwrap()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib parses_trusted_ips_dropping_garbage`
Expected: FAIL — `parse_trusted_ips` not found.

- [ ] **Step 3: Add the parse helper** — near the top of `src/gateway/server/handler.rs` (after the `use` block):

```rust
/// Parse configured trusted-proxy IP strings into `IpAddr`, silently dropping
/// unparseable entries (fail-safe: a garbage entry just isn't trusted).
pub(super) fn parse_trusted_ips(raw: &[String]) -> Vec<IpAddr> {
    raw.iter().filter_map(|s| s.parse::<IpAddr>().ok()).collect()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib parses_trusted_ips_dropping_garbage`
Expected: PASS

- [ ] **Step 5: Add fields to `GatewaySharedState`** — inside the struct (after `audit_log` at line 315 in `src/gateway/server/mod.rs`):

```rust
    /// Trusted-proxy toggle (mirror of `[gateway.trusted_proxy] enabled`).
    trusted_proxy_enabled: bool,
    /// Parsed trusted-proxy peer IPs whose `X-Forwarded-*` are honored.
    trusted_proxy_ips: Vec<std::net::IpAddr>,
    /// Mirror of `[gateway] allow_insecure_remote`. `false` ⇒ a non-loopback
    /// insecure connection is refused at upgrade (Task 5).
    allow_insecure_remote: bool,
    /// True when the gateway terminates TLS in-process (native tiers). Every
    /// connection is then secure regardless of forwarding headers.
    tls_enabled: bool,
```

- [ ] **Step 6: Populate them where `GatewaySharedState` is constructed in `build_router`** — add these to the struct literal (use `self.config.gateway`):

```rust
            trusted_proxy_enabled: self.config.gateway.trusted_proxy.enabled,
            trusted_proxy_ips: super::handler::parse_trusted_ips(
                &self.config.gateway.trusted_proxy.trusted_ips,
            ),
            allow_insecure_remote: self.config.gateway.allow_insecure_remote,
            tls_enabled: self.config.gateway.tls.enabled,
```

- [ ] **Step 7: Replace the client-IP derivation** in `ws_upgrade_handler` — swap `src/gateway/server/handler.rs:106-109`:

```rust
    // IP-keyed abuse protections (per-IP cap, rate limiting), the security
    // audit log, AND the connect-auth loopback test all read `client_ip`.
    // Behind a trusted proxy the transport peer is the proxy, so resolve the
    // real client from forwarding headers first (spoof-safe: untrusted peers'
    // headers are ignored). `secure` = native TLS OR the proxy's XFF-Proto.
    let resolved = crate::gateway::trusted_proxy::resolve_client(
        peer_addr.ip(),
        &headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    let client_ip = resolved.ip;
    let _secure = state.tls_enabled || resolved.secure; // used by Task 5
```

- [ ] **Step 8: Fix `channel_class` to use the resolved client** — change `src/gateway/server/handler.rs:172` from `if peer_addr.ip().is_loopback()` to:

```rust
    let channel_class = if client_ip.is_loopback() {
```

- [ ] **Step 9: Run the gateway handler + connect tests to verify no regression**

Run: `cargo test -p alephcore --lib gateway::server::handler`
Run: `cargo test -p alephcore --lib gateway::handlers::connect`
Expected: PASS (existing tests still green; loopback path unchanged since defaults keep `trusted_proxy_enabled=false` ⇒ `client_ip == peer_addr.ip()`).

- [ ] **Step 10: Commit**

```bash
git add src/gateway/server/mod.rs src/gateway/server/handler.rs
git commit -m "gateway: resolve real client IP behind trusted proxy (closes F5 for cap/rate-limit/audit/connect-auth)"
```

---

### Task 4: Native in-process TLS (`tls.rs` + bind_rustls)

**Files:**
- Create: `src/gateway/tls.rs`
- Modify: `src/gateway/mod.rs` (add `pub mod tls;`)
- Modify: `src/gateway/server/mod.rs` (`run` ~661, `run_until_shutdown` ~686 — the two `axum::serve` sites)
- Test: `src/gateway/tls.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `GatewayTlsConfig` (Task 1).
- Produces: `pub enum TlsMode { Disabled, Provided { cert_path, key_path }, SelfSigned }`; `pub fn resolve_mode(cfg: &GatewayTlsConfig) -> TlsMode`; `pub async fn load_or_generate(cfg: &GatewayTlsConfig, dir: &Path) -> anyhow::Result<(Vec<u8>, Vec<u8>, String)>` returning `(cert_pem, key_pem, sha256_fingerprint_hex)`.

- [ ] **Step 1: Register the module** — add to `src/gateway/mod.rs`:

```rust
pub mod tls;
```

- [ ] **Step 2: Write the failing test** — create `src/gateway/tls.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::config::GatewayTlsConfig;

    #[test]
    fn mode_resolution() {
        assert!(matches!(resolve_mode(&GatewayTlsConfig::default()), TlsMode::Disabled));

        let mut c = GatewayTlsConfig { enabled: true, ..Default::default() };
        assert!(matches!(resolve_mode(&c), TlsMode::SelfSigned));

        c.cert_path = "/a".into();
        c.key_path = "/b".into();
        assert!(matches!(resolve_mode(&c), TlsMode::Provided { .. }));
    }

    #[tokio::test]
    async fn self_signed_generates_persists_and_reuses() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = GatewayTlsConfig { enabled: true, ..Default::default() };

        let (cert1, key1, fp1) = load_or_generate(&cfg, dir.path()).await.unwrap();
        assert!(cert1.starts_with(b"-----BEGIN CERTIFICATE-----"));
        assert!(!key1.is_empty());
        assert_eq!(fp1.len(), 64); // hex SHA-256

        // Second call reuses the persisted cert → identical fingerprint.
        let (_c2, _k2, fp2) = load_or_generate(&cfg, dir.path()).await.unwrap();
        assert_eq!(fp1, fp2);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::tls::tests`
Expected: FAIL — `resolve_mode` / `load_or_generate` not found.

- [ ] **Step 4: Write the implementation** — prepend to `src/gateway/tls.rs`:

```rust
//! Native in-process TLS material for the gateway listener.
//!
//! Three modes off `[gateway.tls]`: disabled (plaintext), operator-provided
//! cert/key files, or auto self-signed (generated once via `rcgen`, persisted
//! to `~/.aleph/tls/`, fingerprint printed for client pinning). No ACME here —
//! auto-issuance is Caddy's / certbot's job (R3).

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::gateway::config::GatewayTlsConfig;

/// Which TLS material the listener should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsMode {
    Disabled,
    Provided { cert_path: String, key_path: String },
    SelfSigned,
}

/// Resolve the mode from config (pure).
#[must_use]
pub fn resolve_mode(cfg: &GatewayTlsConfig) -> TlsMode {
    if !cfg.enabled {
        return TlsMode::Disabled;
    }
    if !cfg.cert_path.is_empty() && !cfg.key_path.is_empty() {
        return TlsMode::Provided {
            cert_path: cfg.cert_path.clone(),
            key_path: cfg.key_path.clone(),
        };
    }
    TlsMode::SelfSigned
}

/// Return `(cert_pem, key_pem, sha256_fingerprint_hex)` for the resolved mode.
/// For `SelfSigned`, generate-and-persist under `dir`, reusing an existing pair.
pub async fn load_or_generate(
    cfg: &GatewayTlsConfig,
    dir: &Path,
) -> anyhow::Result<(Vec<u8>, Vec<u8>, String)> {
    match resolve_mode(cfg) {
        TlsMode::Disabled => anyhow::bail!("TLS disabled"),
        TlsMode::Provided { cert_path, key_path } => {
            let cert = tokio::fs::read(&cert_path).await?;
            let key = tokio::fs::read(&key_path).await?;
            let fp = fingerprint(&cert);
            Ok((cert, key, fp))
        }
        TlsMode::SelfSigned => {
            let cert_file = dir.join("cert.pem");
            let key_file = dir.join("key.pem");
            if cert_file.exists() && key_file.exists() {
                let cert = tokio::fs::read(&cert_file).await?;
                let key = tokio::fs::read(&key_file).await?;
                let fp = fingerprint(&cert);
                return Ok((cert, key, fp));
            }
            let (cert, key, fp) = generate_self_signed()?;
            tokio::fs::create_dir_all(dir).await?;
            tokio::fs::write(&cert_file, &cert).await?;
            tokio::fs::write(&key_file, &key).await?;
            Ok((cert, key, fp))
        }
    }
}

/// rcgen 0.13 self-signed for localhost + loopback. Returns PEM cert, PEM key,
/// and the SHA-256 fingerprint hex of the PEM cert bytes.
fn generate_self_signed() -> anyhow::Result<(Vec<u8>, Vec<u8>, String)> {
    let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])?;
    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();
    let fp = fingerprint(&cert_pem);
    Ok((cert_pem, key_pem, fp))
}

fn fingerprint(cert_pem: &[u8]) -> String {
    let digest = Sha256::digest(cert_pem);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::tls::tests`
Expected: PASS. (If a `tempfile` dev-dep is missing, add `tempfile` under `[dev-dependencies]` in `Cargo.toml` — it is already used elsewhere in the tree.)

- [ ] **Step 6: Swap the accept layer in `run_until_shutdown`** — in `src/gateway/server/mod.rs`, replace the `axum::serve(...).with_graceful_shutdown(...)` block (lines ~702-709) with a branch on TLS. Insert before the existing `axum::serve` call:

```rust
        // Native TLS tiers terminate in-process; plaintext stays on axum::serve.
        if self.config.gateway.tls.enabled {
            install_ring_provider();
            let tls_dir = crate::paths::aleph_home().join("tls");
            let (cert_pem, key_pem, fp) =
                crate::gateway::tls::load_or_generate(&self.config.gateway.tls, &tls_dir)
                    .await
                    .map_err(|e| GatewayError::ConnectionError(format!("TLS material: {e}")))?;
            info!("Aleph listening on https://{}", self.addr);
            info!("  WebSocket: wss://{}/ws", self.addr);
            info!("  TLS cert SHA-256 fingerprint: {fp}");
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert_pem, key_pem)
                .await
                .map_err(|e| GatewayError::ConnectionError(format!("rustls config: {e}")))?;
            let handle = axum_server::Handle::new();
            let shutdown_handle = handle.clone();
            tokio::spawn(async move {
                let _ = shutdown.await;
                shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(3)));
            });
            axum_server::bind_rustls(self.addr, tls)
                .handle(handle)
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;
            return Ok(());
        }
```

(The existing plaintext `axum::serve(...).with_graceful_shutdown(...)` below stays as the `else` path — leave it unchanged; the early `return Ok(())` above skips it when TLS is on.)

- [ ] **Step 7: Mirror the branch in `run`** — in `src/gateway/server/mod.rs` `run` (lines ~674-681), add the same TLS branch before its `axum::serve`, but without the shutdown oneshot:

```rust
        if self.config.gateway.tls.enabled {
            install_ring_provider();
            let tls_dir = crate::paths::aleph_home().join("tls");
            let (cert_pem, key_pem, fp) =
                crate::gateway::tls::load_or_generate(&self.config.gateway.tls, &tls_dir)
                    .await
                    .map_err(|e| GatewayError::ConnectionError(format!("TLS material: {e}")))?;
            info!("Aleph listening on https://{} (wss:// on /ws), cert fp {fp}", self.addr);
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert_pem, key_pem)
                .await
                .map_err(|e| GatewayError::ConnectionError(format!("rustls config: {e}")))?;
            axum_server::bind_rustls(self.addr, tls)
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;
            return Ok(());
        }
```

- [ ] **Step 8: Add the ring-provider installer** — add near the top of the `impl GatewayServer` block or as a free fn in `src/gateway/server/mod.rs`:

```rust
/// Install the process-wide rustls crypto provider once (idempotent).
fn install_ring_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
```

> **Executor note:** confirm the helper `crate::paths::aleph_home()` exists (it is the `~/.aleph` resolver used across the tree). If the exact path differs, use the same resolver the vault (`~/.aleph/data/`) uses. Confirm `axum-server` 0.7's `RustlsConfig::from_pem`, `Handle`, and `bind_rustls(...).handle(...).serve(...)` signatures compile against axum 0.8's `into_make_service_with_connect_info::<SocketAddr>()`; these are the documented axum-server APIs.

- [ ] **Step 9: Run the tls tests + a gateway compile check**

Run: `cargo test -p alephcore --lib gateway::tls::tests`
Run: `cargo check -p alephcore --lib`
Expected: tests PASS; check compiles.

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock src/gateway/tls.rs src/gateway/mod.rs src/gateway/server/mod.rs
git commit -m "gateway: native in-process TLS (provided cert or auto self-signed) via axum-server"
```

---

### Task 5: `allow_insecure_remote` per-connect gate

**Files:**
- Modify: `src/gateway/server/handler.rs` (`ws_upgrade_handler` — after Task 3's resolution block)
- Test: `src/gateway/server/handler.rs` (`mod tests`)

**Interfaces:**
- Consumes: `client_ip`, `_secure` (Task 3), `state.allow_insecure_remote` (Task 3 field).
- Produces: `pub(super) fn refuse_insecure_remote(client_ip: IpAddr, secure: bool, allow_insecure_remote: bool) -> bool`.

- [ ] **Step 1: Write the failing test** — append to `src/gateway/server/handler.rs` `mod tests`:

```rust
#[test]
fn insecure_remote_gate_truth_table() {
    use std::net::IpAddr;
    let lo: IpAddr = "127.0.0.1".parse().unwrap();
    let remote: IpAddr = "203.0.113.9".parse().unwrap();

    // Loopback is always allowed, secure or not, regardless of the flag.
    assert!(!super::refuse_insecure_remote(lo, false, false));
    assert!(!super::refuse_insecure_remote(lo, false, true));

    // Remote + insecure + not allowed ⇒ refuse.
    assert!(super::refuse_insecure_remote(remote, false, false));
    // Remote + secure ⇒ allow.
    assert!(!super::refuse_insecure_remote(remote, true, false));
    // Remote + insecure + explicitly allowed ⇒ allow.
    assert!(!super::refuse_insecure_remote(remote, false, true));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib insecure_remote_gate_truth_table`
Expected: FAIL — `refuse_insecure_remote` not found.

- [ ] **Step 3: Add the pure gate** — near `parse_trusted_ips` in `src/gateway/server/handler.rs`:

```rust
/// Whether to refuse this upgrade for insecure transport. A non-loopback client
/// on an unencrypted leg is refused unless the operator set
/// `allow_insecure_remote`. Loopback is always allowed.
pub(super) fn refuse_insecure_remote(
    client_ip: IpAddr,
    secure: bool,
    allow_insecure_remote: bool,
) -> bool {
    !client_ip.is_loopback() && !secure && !allow_insecure_remote
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib insecure_remote_gate_truth_table`
Expected: PASS

- [ ] **Step 5: Wire the gate into `ws_upgrade_handler`** — rename `_secure` to `secure` in the Task 3 block, then insert immediately after it (before the origin check at line ~117):

```rust
    let secure = state.tls_enabled || resolved.secure;
    if refuse_insecure_remote(client_ip, secure, state.allow_insecure_remote) {
        warn!(
            peer = %peer_addr, client = %client_ip,
            "rejected WebSocket upgrade: insecure transport to a remote client — \
             enable [gateway.tls], or a TLS reverse proxy + [gateway.trusted_proxy], \
             or set allow_insecure_remote=true"
        );
        return (
            axum::http::StatusCode::UPGRADE_REQUIRED,
            "TLS required for remote connections",
        )
            .into_response();
    }
```

- [ ] **Step 6: Run the handler tests + compile**

Run: `cargo test -p alephcore --lib gateway::server::handler`
Run: `cargo check -p alephcore --lib`
Expected: PASS / compiles. Loopback default path unaffected (loopback always allowed).

- [ ] **Step 7: Commit**

```bash
git add src/gateway/server/handler.rs
git commit -m "gateway: refuse insecure transport to a remote client (allow_insecure_remote=false default)"
```

---

### Task 6: Boot gate — refuse to expose plaintext

**Files:**
- Modify: `src/gateway/server/mod.rs` (`warn_if_network_exposed` ~649; call sites in `run` ~675 and `run_until_shutdown` ~701)
- Test: `src/gateway/server/mod.rs` (`mod tests`)

**Interfaces:**
- Produces: `fn insecure_exposure_refused(host_is_loopback: bool, tls_enabled: bool, trusted_proxy_enabled: bool, allow_insecure_remote: bool) -> Option<String>` (Some(diagnostic) ⇒ refuse boot).

- [ ] **Step 1: Write the failing test** — append to the `mod tests` in `src/gateway/server/mod.rs`:

```rust
#[test]
fn boot_gate_refuses_only_plaintext_non_loopback() {
    // Default loopback install: allowed.
    assert!(super::insecure_exposure_refused(true, false, false, false).is_none());
    // Non-loopback plaintext, no proxy, not allowed ⇒ refuse.
    assert!(super::insecure_exposure_refused(false, false, false, false).is_some());
    // Non-loopback but native TLS ⇒ allowed.
    assert!(super::insecure_exposure_refused(false, true, false, false).is_none());
    // Non-loopback behind trusted proxy ⇒ allowed.
    assert!(super::insecure_exposure_refused(false, false, true, false).is_none());
    // Non-loopback plaintext but explicitly allowed ⇒ allowed.
    assert!(super::insecure_exposure_refused(false, false, false, true).is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib boot_gate_refuses_only_plaintext_non_loopback`
Expected: FAIL — `insecure_exposure_refused` not found.

- [ ] **Step 3: Add the pure gate + replace the warning body** — in `src/gateway/server/mod.rs`, add the free function and rewrite `warn_if_network_exposed` to return a `Result`:

```rust
/// Boot-time verdict: `Some(diagnostic)` when the server would expose plaintext
/// to the network and must refuse to start. Loopback bind, any native-TLS tier,
/// a trusted-proxy (TLS-terminating) upstream, or an explicit
/// `allow_insecure_remote` all pass.
fn insecure_exposure_refused(
    host_is_loopback: bool,
    tls_enabled: bool,
    trusted_proxy_enabled: bool,
    allow_insecure_remote: bool,
) -> Option<String> {
    if host_is_loopback || tls_enabled || trusted_proxy_enabled || allow_insecure_remote {
        return None;
    }
    Some(
        "gateway would serve PLAINTEXT on a non-loopback interface. Refusing to start. \
         Fix: enable [gateway.tls], OR front it with a TLS reverse proxy and set \
         [gateway.trusted_proxy] enabled = true, OR knowingly set \
         [gateway] allow_insecure_remote = true."
            .to_string(),
    )
}
```

Rewrite `warn_if_network_exposed` (line 649) to a checking function:

```rust
    /// Refuse to boot if the gateway would serve plaintext to the network.
    /// Loopback / TLS / trusted-proxy / explicit-opt-out all pass.
    fn check_network_exposure(&self) -> Result<(), GatewayError> {
        if let Some(msg) = insecure_exposure_refused(
            self.addr.ip().is_loopback(),
            self.config.gateway.tls.enabled,
            self.config.gateway.trusted_proxy.enabled,
            self.config.gateway.allow_insecure_remote,
        ) {
            return Err(GatewayError::ConnectionError(msg));
        }
        Ok(())
    }
```

- [ ] **Step 4: Update both call sites** — replace `self.warn_if_network_exposed();` at lines ~675 and ~701 with:

```rust
        self.check_network_exposure()?;
```

(Both `run` and `run_until_shutdown` already return `Result<(), GatewayError>`, so `?` fits. Place the call **before** binding the listener so a misconfig fails fast.)

- [ ] **Step 5: Run test + compile**

Run: `cargo test -p alephcore --lib boot_gate_refuses_only_plaintext_non_loopback`
Run: `cargo check -p alephcore --lib`
Expected: PASS / compiles.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/server/mod.rs
git commit -m "gateway: fail-closed boot gate — refuse plaintext on a non-loopback bind"
```

---

### Task 7: Panel forces `wss://` for remote (C5)

**Files:**
- Modify: `interfaces/webchat/src/context.rs:343-361` (`derive_gateway_url`)
- Test: `interfaces/webchat/src/context.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `fn ws_url_for(protocol: &str, host: &str) -> Result<String, ()>` (Err ⇒ insecure remote, caller shows an error and opens no socket).

- [ ] **Step 1: Write the failing test** — add a `#[cfg(test)] mod tests` to `interfaces/webchat/src/context.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::ws_url_for;

    #[test]
    fn https_page_yields_wss() {
        assert_eq!(ws_url_for("https:", "app.example.com").unwrap(), "wss://app.example.com/ws");
    }

    #[test]
    fn loopback_http_yields_ws() {
        assert_eq!(ws_url_for("http:", "127.0.0.1:18790").unwrap(), "ws://127.0.0.1:18790/ws");
        assert_eq!(ws_url_for("http:", "localhost:18790").unwrap(), "ws://localhost:18790/ws");
    }

    #[test]
    fn remote_http_is_refused() {
        assert!(ws_url_for("http:", "app.example.com").is_err());
        assert!(ws_url_for("http:", "203.0.113.9:18790").is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-webchat --lib ws_url_for` *(use the webchat crate's package name from `interfaces/webchat/Cargo.toml`; substitute if different)*
Expected: FAIL — `ws_url_for` not found.

- [ ] **Step 3: Extract the pure helper + refuse remote http** — replace `derive_gateway_url` in `interfaces/webchat/src/context.rs`:

```rust
/// Build the gateway WS URL from a page protocol + host. `https:` ⇒ `wss://`.
/// Plain `http:` is only allowed for a loopback host (zero-config desktop);
/// a remote `http:` page is refused (`Err`) so the Panel never opens a
/// plaintext socket to a remote gateway.
fn ws_url_for(protocol: &str, host: &str) -> Result<String, ()> {
    if protocol == "https:" {
        return Ok(format!("wss://{host}/ws"));
    }
    let host_only = host.split(':').next().unwrap_or(host);
    let is_loopback = host_only == "127.0.0.1" || host_only == "::1" || host_only == "localhost";
    if is_loopback {
        Ok(format!("ws://{host}/ws"))
    } else {
        Err(())
    }
}

/// Derive the Gateway WebSocket URL from the current page location.
/// Same-origin (Panel UI and Gateway share a port). Remote-over-http is
/// refused — callers must surface an "insecure transport, use https" error.
fn derive_gateway_url() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let location = window.location();
            if let (Ok(protocol), Ok(host)) = (location.protocol(), location.host()) {
                match ws_url_for(&protocol, &host) {
                    Ok(url) => return url,
                    Err(()) => {
                        web_sys::console::error_1(
                            &"Aleph Panel: refusing insecure transport — open this Panel over https"
                                .into(),
                        );
                        // Non-connectable sentinel; the connection layer surfaces
                        // ConnectionFailure to the UI instead of opening ws://.
                        return String::new();
                    }
                }
            }
        }
        "ws://127.0.0.1:18790/ws".to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "ws://127.0.0.1:18790/ws".to_string()
    }
}
```

> **Executor note:** confirm the caller of `derive_gateway_url()` treats an empty string as "do not connect / show error" (it feeds `connection_failure`). If the caller unconditionally opens a socket, add a guard: empty URL ⇒ set `connection_failure = Some(ConnectionFailure::InsecureTransport)` and skip the connect. Add that enum variant if needed.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-webchat --lib ws_url_for`
Expected: PASS (3 tests). The `ws_url_for` fn is not `#[cfg(target_arch=…)]`-gated, so it tests on host.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/context.rs
git commit -m "panel: force wss:// for remote hosts, refuse plaintext to a remote gateway"
```

---

## HSTS (no code — verify only)

`src/security/headers.rs` already defines `SecurityHeadersLayer` emitting
`strict-transport-security: max-age=31536000; includeSubDomains` (line ~111) and it is
applied to the router. Confirm it is layered onto the main `/ws`+UI router in `build_router`
(grep `SecurityHeadersLayer` in `src/gateway/server/mod.rs`). If already applied, **no change**;
if only on the control-plane router, add `.layer(SecurityHeadersLayer::new())` to the main
router. This is the C5 server-side belt; the Caddy tier also sets HSTS at the edge.

---

## Deployment (operational — not code; ship in docs, no task)

Fold the tier-① recipe from the spec (`docs/superpowers/specs/2026-07-15-gateway-tls-hardening-design.md` §C) into `docs/reference/SECURITY.md` and/or the Debian deploy notes: loopback bind + `trusted_proxy.enabled=true` + `allowed_origins` + the one-line Caddyfile + UFW (443/80/22) + systemd hardening + `gateway.token.rotate`. Verify: green-lock `wss://` from a remote browser; a direct `:18790` public hit fails; audit log shows the real client IP.

---

## Self-Review

**Spec coverage:** C1→Task 4; C2→Tasks 2+3; C3→Task 5; C4→Task 6; C5→Task 7; HSTS→verify section; deployment recipe→docs section; defense-in-depth→docs. All spec sections mapped.

**Placeholder scan:** none — every code step carries complete code. Two "Executor note" blocks flag *external-API confirmations* (`crate::paths::aleph_home`, axum-server signatures, the webchat package name, the `derive_gateway_url` caller) rather than leaving code unwritten.

**Type consistency:** `ResolvedClient { ip, secure }` produced in Task 2, consumed in Task 3 (`resolved.ip`, `resolved.secure`); `client_ip`/`secure` from Task 3 consumed by `refuse_insecure_remote` in Task 5; `GatewaySharedState` fields added in Task 3 read in Tasks 3/5; `GatewayTlsConfig` from Task 1 read in Task 4's `resolve_mode`; `insecure_exposure_refused` bool arity matches its Task 6 test. Config field names (`tls`, `trusted_proxy`, `allow_insecure_remote`, `tls.enabled`, `trusted_proxy.enabled`, `trusted_proxy.trusted_ips`) identical across Tasks 1/3/4/6.

## Execution Handoff

Two execution options — see below.
