# Panel Self-Signed Cert Trust (TOFU) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a Panel app authorize a remote core's self-signed TLS cert in-app (TOFU), across macOS / Linux / Windows desktop shells and iOS, so a bare-IP self-signed deployment loads the Panel without any OS-keychain fiddling.

**Architecture:** Approach A — per-webview in-app cert pinning. A pure shared decision core (trust store + `decide` + fingerprint) is fed the cert by each engine's reactive TLS-error hook; on an unknown cert the shell shows a TOFU approval prompt and pins the SHA-256 fingerprint; only pinned fingerprints are ever allowed (fail-closed). Never touches the OS trust store.

**Tech Stack:** Rust (desktop shell, Tauri 2 / wry 0.55), `sha2` / `hex` / `x509-parser` / `serde` / `serde_json`; webkit2gtk 2.0.2 (Linux), webview2-com 0.38.2 (Windows), objc/WKWebView (macOS); Swift + WebKit (iOS).

## Global Constraints

- **Trigger only on TLS validation failure.** A CA-valid cert (domain deployment) passes system validation, the engine hook never fires, the Panel loads silently. Bare-IP self-signed is the only case that engages this flow. (Spec §"Trigger Logic")
- **Fail-closed.** Never a blanket accept-invalid mode. Only an exact pinned-fingerprint match is allowed. Default path / user cancel = engine default validation = load fails.
- **No OS trust-store modification.** Trust is pinned in-app only. All allow mechanisms are per-challenge temporary grants.
- **TOFU semantics.** Unknown host → prompt + pin. Known + matching fp → silent allow. Known + changed fp → prominent MITM warning + explicit re-approval.
- **R1 brain/limb:** the shared decision core is pure logic (no platform API). All platform-API contact is isolated in per-platform adapters.
- **Fingerprint display form:** SHA-256, uppercase hex, colon-grouped per byte (e.g. `49:3D:51:...`), matching `openssl x509 -fingerprint -sha256`.
- **No new async runtime, no new heavy deps.** Reuse existing engine crates + `sha2`/`hex`/`x509-parser`/`serde`.
- **Store best-effort:** corrupt/unreadable store → treated as empty (fail toward "prompt", never brick, never auto-allow).

## File Structure

**Desktop (Rust) — `desktop/shell/src/cert_trust/`:**
- `mod.rs` — module root; `Decision` enum, `CertInfo` struct, `decide()`, re-exports.
- `store.rs` — `TrustStore` (JSON file at `connection::marker_path("trusted-certs")`; load/lookup/insert-and-save).
- `fingerprint.rs` — `fingerprint_sha256(der)`, `parse_cert_info(der, reason)`.
- `pending.rs` — `PendingCert` shared state + Tauri commands `get_pending_cert` / `approve_cert` / `reject_cert`.
- `adapter_linux.rs` / `adapter_windows.rs` / `adapter_macos.rs` — cfg-gated per-engine hook install.
- `install.rs` — `install_cert_trust(window)` dispatching to the platform adapter (mirrors `webview_perms::grant_microphone`).

**Desktop wiring:**
- `desktop/shell/Cargo.toml` — add `sha2`, `hex`, `x509-parser`, `serde` (workspace deps); macOS objc deps under the existing `[target.'cfg(target_os = "macos")'.dependencies]`.
- `desktop/shell/src/main.rs` — call `cert_trust::install::install_cert_trust(&window)` where `grant_microphone` is called; register the 3 Tauri commands; add the `trust_pending` latch to `supervise_remote_lite`.
- `desktop/shell/splash/cert-trust.html` — the approval page.

**iOS (Swift) — `mobile/ios/AlephPaneliOS/`:**
- `Services/CertTrustStore.swift` — Keychain-backed pinned store + `decide` mirror.
- `Views/CertTrustSheet.swift` — SwiftUI approval sheet.
- `Views/PanelWebView.swift` — add the ServerTrust challenge handler to `Coordinator`.
- `State/AppState.swift` — hold pending-cert state + drive the sheet.

## Core Interfaces (defined in Phase 1, consumed by all later phases)

```rust
// desktop/shell/src/cert_trust/mod.rs
pub struct CertInfo {
    pub sans: Vec<String>,
    pub subject: String,
    pub reason: String, // human failure reason for display, e.g. "self-signed (untrusted issuer)"
}

pub enum Decision {
    Allow,
    PromptUnknown { fp: String, info: CertInfo },
    WarnChanged { old_fp: String, new_fp: String, info: CertInfo },
}

/// Pure decision. `presented_fp` is the colon-grouped SHA-256 of the leaf DER.
pub fn decide(host: &str, presented_fp: &str, info: CertInfo, store: &store::TrustStore) -> Decision;

// desktop/shell/src/cert_trust/fingerprint.rs
pub fn fingerprint_sha256(leaf_der: &[u8]) -> String;           // "49:3D:..." uppercase
pub fn parse_cert_info(leaf_der: &[u8], reason: &str) -> CertInfo;

// desktop/shell/src/cert_trust/store.rs
pub struct TrustStore { /* host:port -> fp */ }
impl TrustStore {
    pub fn load(path: &std::path::Path) -> Self;                 // best-effort, empty on error
    pub fn lookup(&self, host: &str) -> Option<&str>;
    pub fn insert_and_save(&mut self, host: &str, fp: &str, path: &std::path::Path) -> std::io::Result<()>;
}
```

---

## Phase 1 — Shared Rust core (pure logic, fully unit-tested)

### Task 1: Add crate dependencies + module skeleton

**Files:**
- Modify: `desktop/shell/Cargo.toml` (`[dependencies]`)
- Create: `desktop/shell/src/cert_trust/mod.rs`
- Modify: `desktop/shell/src/main.rs` (add `mod cert_trust;`)

**Interfaces:**
- Produces: the `cert_trust` module path; `CertInfo`, `Decision` types.

- [ ] **Step 1: Add deps.** In `desktop/shell/Cargo.toml` under `[dependencies]`, add:
```toml
sha2 = { workspace = true }
hex = { workspace = true }
x509-parser = { workspace = true }
serde = { workspace = true }
```
(All four are already declared in the root `Cargo.toml` `[workspace.dependencies]`.)

- [ ] **Step 2: Create the module root** `desktop/shell/src/cert_trust/mod.rs`:
```rust
//! In-app TOFU trust for self-signed TLS certs (Approach A). Pure decision core
//! (this file + `store`/`fingerprint`); platform adapters feed it the cert from
//! each engine's TLS-error hook. Never touches the OS trust store; only an exact
//! pinned-fingerprint match is ever allowed (fail-closed).

pub mod fingerprint;
pub mod store;

use serde::Serialize;

/// Cert facts shown to the user. Display-only — the decision pins the
/// fingerprint regardless of the specific failure reason.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CertInfo {
    pub sans: Vec<String>,
    pub subject: String,
    pub reason: String,
}

/// TOFU verdict for a presented leaf cert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    PromptUnknown { fp: String, info: CertInfo },
    WarnChanged { old_fp: String, new_fp: String, info: CertInfo },
}

/// Pure decision: compare the presented fingerprint against the pinned store.
#[must_use]
pub fn decide(host: &str, presented_fp: &str, info: CertInfo, store: &store::TrustStore) -> Decision {
    match store.lookup(host) {
        None => Decision::PromptUnknown { fp: presented_fp.to_string(), info },
        Some(pinned) if pinned == presented_fp => Decision::Allow,
        Some(pinned) => Decision::WarnChanged {
            old_fp: pinned.to_string(),
            new_fp: presented_fp.to_string(),
            info,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::TrustStore;

    fn info() -> CertInfo {
        CertInfo { sans: vec!["172.245.43.211".into()], subject: "CN=Aleph".into(), reason: "self-signed".into() }
    }

    #[test]
    fn unknown_host_prompts() {
        let store = TrustStore::empty();
        assert!(matches!(decide("h:1", "AA:BB", info(), &store), Decision::PromptUnknown { .. }));
    }

    #[test]
    fn matching_fp_allows() {
        let mut store = TrustStore::empty();
        store.insert_mem("h:1", "AA:BB");
        assert_eq!(decide("h:1", "AA:BB", info(), &store), Decision::Allow);
    }

    #[test]
    fn changed_fp_warns() {
        let mut store = TrustStore::empty();
        store.insert_mem("h:1", "AA:BB");
        assert!(matches!(decide("h:1", "CC:DD", info(), &store), Decision::WarnChanged { .. }));
    }
}
```

- [ ] **Step 3: Add `mod cert_trust;`** to `desktop/shell/src/main.rs` near the other `mod` declarations (e.g. beside `mod webview_perms;`).

- [ ] **Step 4: Verify it compiles** (tests will fail to build until Task 2 adds `TrustStore`):
Run: `cargo check -p aleph-desktop-shell`
Expected: error — `store::TrustStore` / `TrustStore::empty` not found (Task 2 supplies them). This confirms the module is wired; proceed to Task 2 before committing.

- [ ] **Step 5:** (No commit yet — Task 1+2 land together since the `mod.rs` tests depend on `store`.)

### Task 2: `TrustStore` (JSON persistence, best-effort)

**Files:**
- Create: `desktop/shell/src/cert_trust/store.rs`
- Test: inline `#[cfg(test)]` in `store.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `TrustStore { empty(), load(path), lookup(host) -> Option<&str>, insert_mem(host, fp), insert_and_save(host, fp, path) }`.

- [ ] **Step 1: Write failing tests** (`store.rs`, `#[cfg(test)] mod tests`):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_insert_lookup_save_load() {
        let dir = std::env::temp_dir().join(format!("aleph-ct-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trusted-certs");
        let mut s = TrustStore::empty();
        s.insert_and_save("172.245.43.211:18790", "49:3D", &path).unwrap();
        let reloaded = TrustStore::load(&path);
        assert_eq!(reloaded.lookup("172.245.43.211:18790"), Some("49:3D"));
        assert_eq!(reloaded.lookup("other:1"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_loads_empty_not_panic() {
        let dir = std::env::temp_dir().join(format!("aleph-ct-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trusted-certs");
        std::fs::write(&path, b"\x00not json{{").unwrap();
        let s = TrustStore::load(&path); // must not panic
        assert_eq!(s.lookup("anything"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overwrite_replaces_fingerprint() {
        let mut s = TrustStore::empty();
        s.insert_mem("h:1", "OLD");
        s.insert_mem("h:1", "NEW");
        assert_eq!(s.lookup("h:1"), Some("NEW"));
    }
}
```

- [ ] **Step 2: Run — expect FAIL** (`TrustStore` undefined):
Run: `cargo test -p aleph-desktop-shell cert_trust::store`
Expected: FAIL (does not compile — `TrustStore` not found).

- [ ] **Step 3: Implement** `store.rs`:
```rust
//! Pinned TOFU trust store: `host:port -> SHA-256 fingerprint`. JSON file,
//! best-effort load (corrupt/missing = empty; never brick, never auto-allow).

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pinned: BTreeMap<String, String>,
}

impl TrustStore {
    #[must_use]
    pub fn empty() -> Self {
        Self { pinned: BTreeMap::new() }
    }

    /// Best-effort load: any error (missing, unreadable, malformed) yields an
    /// empty store so every host re-prompts rather than the app bricking.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<TrustStore>(&s).ok())
            .unwrap_or_else(Self::empty)
    }

    #[must_use]
    pub fn lookup(&self, host: &str) -> Option<&str> {
        self.pinned.get(host).map(String::as_str)
    }

    /// In-memory insert (tests / pre-save staging).
    pub fn insert_mem(&mut self, host: &str, fp: &str) {
        self.pinned.insert(host.to_string(), fp.to_string());
    }

    /// Insert and persist atomically (write temp + rename).
    pub fn insert_and_save(&mut self, host: &str, fp: &str, path: &Path) -> std::io::Result<()> {
        self.insert_mem(host, fp);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}
```

- [ ] **Step 4: Run — expect PASS** (both this and the `mod.rs` `decide` tests):
Run: `cargo test -p aleph-desktop-shell cert_trust`
Expected: PASS (store tests + decide tests).

- [ ] **Step 5: Commit:**
```bash
git add desktop/shell/Cargo.toml desktop/shell/src/cert_trust/mod.rs desktop/shell/src/cert_trust/store.rs desktop/shell/src/main.rs
git commit -m "cert-trust: shared decision core + pinned TOFU trust store"
```

### Task 3: Fingerprint + cert parsing

**Files:**
- Create: `desktop/shell/src/cert_trust/fingerprint.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `CertInfo` from `mod.rs`.
- Produces: `fingerprint_sha256(der) -> String`, `parse_cert_info(der, reason) -> CertInfo`.

- [ ] **Step 1: Write failing test.** Use a self-signed DER fixture generated once with `rcgen` in the test (avoids checking in a binary):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Build a self-signed cert DER at test time (rcgen is already a workspace dep
    // used by gateway/tls.rs). SAN includes an IP so parse_cert_info sees it.
    fn sample_der() -> Vec<u8> {
        let cert = rcgen::generate_simple_self_signed(vec!["172.245.43.211".to_string()]).unwrap();
        cert.serialize_der().unwrap()
    }

    #[test]
    fn fingerprint_is_colon_grouped_uppercase_hex_32_bytes() {
        let fp = fingerprint_sha256(&sample_der());
        let groups: Vec<&str> = fp.split(':').collect();
        assert_eq!(groups.len(), 32, "sha256 = 32 bytes");
        assert!(groups.iter().all(|g| g.len() == 2 && g.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())));
    }

    #[test]
    fn parse_cert_info_extracts_ip_san() {
        let info = parse_cert_info(&sample_der(), "self-signed");
        assert!(info.sans.iter().any(|s| s.contains("172.245.43.211")));
        assert_eq!(info.reason, "self-signed");
    }
}
```
> Note: if the installed `rcgen` version returns a `CertifiedKey`/uses `.cert.der()`, mirror whatever `src/gateway/tls.rs` currently calls — copy that exact API shape into the test.

- [ ] **Step 2: Run — expect FAIL:**
Run: `cargo test -p aleph-desktop-shell cert_trust::fingerprint`
Expected: FAIL (functions undefined).

- [ ] **Step 3: Implement** `fingerprint.rs`:
```rust
//! SHA-256 leaf fingerprint + display-only cert parsing (SAN / subject).

use crate::cert_trust::CertInfo;
use sha2::{Digest, Sha256};

/// Colon-grouped uppercase hex SHA-256 of the leaf DER — matches
/// `openssl x509 -fingerprint -sha256`.
#[must_use]
pub fn fingerprint_sha256(leaf_der: &[u8]) -> String {
    let digest = Sha256::digest(leaf_der);
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Parse SAN + subject for display. Never fails hard — on a parse error the
/// returned info has empty SAN/subject but still carries the reason, so the
/// prompt can still show the fingerprint.
#[must_use]
pub fn parse_cert_info(leaf_der: &[u8], reason: &str) -> CertInfo {
    use x509_parser::prelude::*;
    let (subject, sans) = match X509Certificate::from_der(leaf_der) {
        Ok((_, cert)) => {
            let subject = cert.subject().to_string();
            let sans = cert
                .subject_alternative_name()
                .ok()
                .flatten()
                .map(|ext| {
                    ext.value
                        .general_names
                        .iter()
                        .map(|gn| gn.to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (subject, sans)
        }
        Err(_) => (String::new(), Vec::new()),
    };
    CertInfo { sans, subject, reason: reason.to_string() }
}
```
> If `x509-parser 0.17`'s `subject_alternative_name()` signature differs, adjust to its actual return (it returns `Result<Option<BasicExtension<SubjectAlternativeName>>, _>`); keep the "never panic, empty on error" behavior.

- [ ] **Step 4: Run — expect PASS:**
Run: `cargo test -p aleph-desktop-shell cert_trust::fingerprint`
Expected: PASS.

- [ ] **Step 5: Commit:**
```bash
git add desktop/shell/src/cert_trust/fingerprint.rs
git commit -m "cert-trust: SHA-256 fingerprint + SAN/subject parsing"
```

---

## Phase 2 — macOS adapter + approval UI + wiring (reference platform)

> This phase proves the whole flow end-to-end on macOS (the primary test env).
> It includes the pending-state + Tauri commands + `cert-trust.html` + the
> supervisor latch (all platform-agnostic wiring), then the macOS-specific
> WKWebView challenge hook — the **spike**.

### Task 4: Pending-cert state + Tauri approval commands

**Files:**
- Create: `desktop/shell/src/cert_trust/pending.rs`
- Modify: `desktop/shell/src/cert_trust/mod.rs` (add `pub mod pending;`)
- Modify: `desktop/shell/src/main.rs` (register commands in both `generate_handler!` arms, ~lines 233 & 240; manage `PendingCert` state)

**Interfaces:**
- Consumes: `CertInfo`, `store::TrustStore`, `connection::marker_path`.
- Produces: Tauri commands `get_pending_cert() -> Option<PendingCertView>`, `approve_cert(host) -> Result<(), String>`, `reject_cert()`; `PendingCert` managed state; helper `cert_trust::store_path()`.

- [ ] **Step 1:** Write `pending.rs` with a `Mutex`-guarded pending record + the store path helper + the three commands. Full code:
```rust
//! Shared pending-cert state bridging a platform adapter's TLS-error hook and
//! the `cert-trust.html` approval UI. The adapter stashes the pending cert here
//! and navigates the webview to the trust page; the page reads it via
//! `get_pending_cert` and resolves it via `approve_cert` / `reject_cert`.

use std::sync::Mutex;

use serde::Serialize;

use crate::cert_trust::{store::TrustStore, CertInfo};

/// Where the pinned store persists (namespaced like the other shell markers).
#[must_use]
pub fn store_path() -> Option<std::path::PathBuf> {
    crate::connection::marker_path("trusted-certs")
}

#[derive(Clone)]
pub struct PendingRecord {
    pub host: String,
    pub fp: String,
    pub info: CertInfo,
    pub changed_from: Option<String>, // Some(old_fp) => WarnChanged
}

#[derive(Default)]
pub struct PendingCert(pub Mutex<Option<PendingRecord>>);

#[derive(Serialize)]
pub struct PendingCertView {
    host: String,
    fingerprint: String,
    sans: Vec<String>,
    subject: String,
    reason: String,
    changed_from: Option<String>,
}

#[tauri::command]
pub fn get_pending_cert(state: tauri::State<'_, PendingCert>) -> Option<PendingCertView> {
    let guard = state.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref().map(|r| PendingCertView {
        host: r.host.clone(),
        fingerprint: r.fp.clone(),
        sans: r.info.sans.clone(),
        subject: r.info.subject.clone(),
        reason: r.info.reason.clone(),
        changed_from: r.changed_from.clone(),
    })
}

/// Pin the pending cert for `host` and reload the remote target. `host` must
/// match the pending record (guards against a stale page approving a different
/// cert than the one being shown).
#[tauri::command]
pub fn approve_cert(
    app: tauri::AppHandle,
    state: tauri::State<'_, PendingCert>,
    host: String,
) -> Result<(), String> {
    let record = {
        let mut guard = state.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.take() {
            Some(r) if r.host == host => r,
            Some(other) => {
                *guard = Some(other);
                return Err("pending cert host mismatch".into());
            }
            None => return Err("no pending cert".into()),
        }
    };
    let path = store_path().ok_or("home dir not found")?;
    let mut store = TrustStore::load(&path);
    store
        .insert_and_save(&record.host, &record.fp, &path)
        .map_err(|e| format!("persist trust: {e}"))?;
    // Reload the remote target now that the cert is pinned.
    crate::reroute_for_target(&app, crate::connection::load_target());
    Ok(())
}

#[tauri::command]
pub fn reject_cert(state: tauri::State<'_, PendingCert>) {
    let mut guard = state.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = None;
}
```

- [ ] **Step 2:** In `mod.rs` add `pub mod pending;`.

- [ ] **Step 3:** In `main.rs`: `.manage(cert_trust::pending::PendingCert::default())` on the builder, and add the three commands to **both** `generate_handler!` arms (lite ~233, full ~240):
```rust
cert_trust::pending::get_pending_cert,
cert_trust::pending::approve_cert,
cert_trust::pending::reject_cert,
```

- [ ] **Step 4: Verify build:**
Run: `cargo check -p aleph-desktop-shell`
Expected: PASS.

- [ ] **Step 5: Commit:**
```bash
git add desktop/shell/src/cert_trust/pending.rs desktop/shell/src/cert_trust/mod.rs desktop/shell/src/main.rs
git commit -m "cert-trust: pending-cert state + approve/reject Tauri commands"
```

### Task 5: `cert-trust.html` approval page

**Files:**
- Create: `desktop/shell/splash/cert-trust.html`

**Interfaces:**
- Consumes: Tauri commands `get_pending_cert`, `approve_cert`, `reject_cert` (Task 4). Reuses the `window.__TAURI__.core.invoke` pattern from `connect.html`.

- [ ] **Step 1:** Create `cert-trust.html` modeled on `connect.html` (same `<style>`, same `const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);`). It: calls `get_pending_cert()` on load; renders host, fingerprint (monospace), SAN list, reason; shows a prominent red banner when `changed_from` is non-null ("证书已变化，可能是服务器轮换或中间人攻击"); "信任" → `approve_cert({ host })`; "取消" → `reject_cert()` then navigate to `connect.html`. Full body:
```html
<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8" />
<title>Trust Certificate</title>
<style>
  body { font-family: -apple-system, system-ui, sans-serif; background:#1a1a1e; color:#e8e8ea; margin:0; padding:2rem; }
  .card { max-width: 32rem; margin: 3rem auto; background:#242429; border:1px solid #3a3a40; border-radius:16px; padding:2rem; }
  h2 { margin:0 0 .5rem; }
  .warn { background:#3a1a1a; border:1px solid #a33; color:#f7b0b0; padding:.75rem 1rem; border-radius:8px; margin-bottom:1rem; font-weight:600; }
  .fp { font-family: ui-monospace, monospace; font-size:.8rem; word-break:break-all; background:#1a1a1e; padding:.5rem .75rem; border-radius:8px; }
  .label { color:#9a9aa2; font-size:.75rem; margin-top:1rem; }
  .san { font-size:.8rem; }
  button { border:0; border-radius:10px; padding:.7rem 1rem; font-size:.9rem; font-weight:600; cursor:pointer; }
  .row { display:flex; gap:.75rem; margin-top:1.5rem; }
  .trust { background:#7c5cff; color:#fff; flex:1; }
  .cancel { background:#3a3a40; color:#e8e8ea; }
</style>
</head>
<body>
<div class="card" id="card"><p>Loading…</p></div>
<script>
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
  function esc(s){ return String(s).replace(/[&<>]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;'}[c])); }
  async function render() {
    const p = await invoke('get_pending_cert');
    const card = document.getElementById('card');
    if (!p) { card.innerHTML = '<p>没有待确认的证书。</p>'; return; }
    const warn = p.changed_from ? '<div class="warn">⚠ 证书已变化，可能是服务器轮换或中间人攻击。请确认指纹后再信任。</div>' : '';
    card.innerHTML = warn +
      '<h2>信任此服务器证书？</h2>' +
      '<div class="label">服务器</div><div>' + esc(p.host) + '</div>' +
      '<div class="label">原因</div><div>' + esc(p.reason) + '</div>' +
      '<div class="label">SHA-256 指纹</div><div class="fp">' + esc(p.fingerprint) + '</div>' +
      '<div class="label">证书 SAN</div><div class="san">' + (p.sans.length ? p.sans.map(esc).join('<br>') : '—') + '</div>' +
      '<div class="row"><button class="trust" id="trust">信任并连接</button><button class="cancel" id="cancel">取消</button></div>';
    document.getElementById('trust').onclick = async () => {
      try { await invoke('approve_cert', { host: p.host }); } catch (e) { alert('信任失败: ' + e); }
    };
    document.getElementById('cancel').onclick = async () => {
      await invoke('reject_cert');
      window.location.href = 'connect.html';
    };
  }
  render();
</script>
</body>
</html>
```

- [ ] **Step 2:** Confirm the file is bundled — `tauri.conf.json` `build.frontendDist` is `./splash`, so any file in `splash/` ships. No config change needed.

- [ ] **Step 3: Commit:**
```bash
git add desktop/shell/splash/cert-trust.html
git commit -m "cert-trust: approval splash page (fingerprint + SAN + TOFU/change warning)"
```

### Task 6: Supervisor `trust_pending` latch

**Files:**
- Modify: `desktop/shell/src/main.rs` (`supervise_remote_lite` ~1027; add a shared `AtomicBool`)
- Modify: `desktop/shell/src/cert_trust/pending.rs` (set the latch when stashing / clearing a pending cert)

**Interfaces:**
- Consumes: nothing new.
- Produces: a process-global `TRUST_PENDING: AtomicBool` (in `pending.rs`) that `supervise_remote_lite` reads to skip relocation while a prompt is up.

- [ ] **Step 1:** In `pending.rs` add:
```rust
use std::sync::atomic::{AtomicBool, Ordering};
pub static TRUST_PENDING: AtomicBool = AtomicBool::new(false);
pub fn set_trust_pending(v: bool) { TRUST_PENDING.store(v, Ordering::SeqCst); }
```
Call `set_trust_pending(true)` from the adapter when a prompt is shown (Task 7/8/9), and `set_trust_pending(false)` inside both `approve_cert` and `reject_cert` (after resolving).

- [ ] **Step 2:** In `supervise_remote_lite`'s loop (main.rs ~1042), before the `ShowConnectionError` relocation fires, add:
```rust
if crate::cert_trust::pending::TRUST_PENDING.load(std::sync::atomic::Ordering::SeqCst) {
    // A trust prompt owns the screen — do not relocate or count failure ticks.
    continue;
}
```
(Insert immediately after `let ready = ...` / before `match supervisor.tick(ready)`.)

- [ ] **Step 3: Verify build:**
Run: `cargo check -p aleph-desktop-shell`
Expected: PASS.

- [ ] **Step 4: Commit:**
```bash
git add desktop/shell/src/main.rs desktop/shell/src/cert_trust/pending.rs
git commit -m "cert-trust: suppress lite supervisor relocation while a trust prompt is pending"
```

### Task 7: macOS WKWebView challenge adapter (SPIKE)

**Files:**
- Create: `desktop/shell/src/cert_trust/adapter_macos.rs`
- Create: `desktop/shell/src/cert_trust/install.rs`
- Modify: `desktop/shell/src/cert_trust/mod.rs` (`pub mod install;` + cfg adapter mods)
- Modify: `desktop/shell/Cargo.toml` (macOS objc deps)
- Modify: `desktop/shell/src/main.rs` (call `install_cert_trust(&window)` next to `grant_microphone`)

**Interfaces:**
- Consumes: `decide`, `fingerprint_sha256`, `parse_cert_info`, `pending::{PendingCert, PendingRecord, set_trust_pending}`, `store::TrustStore`.
- Produces: `install::install_cert_trust(window: &tauri::WebviewWindow)`.

> **This task is a spike.** wry owns the `WKNavigationDelegate`, so intercepting
> `webView:didReceiveAuthenticationChallenge:completionHandler:` requires either
> subclass-and-forward or method swizzling of wry's delegate. The step below is
> the investigation contract; if no robust hook exists in wry 0.55, STOP and
> report — the contingency is a macOS-only loopback proxy (separate mini-plan),
> not shipping a broken macOS path.

- [ ] **Step 1: Investigate wry's macOS delegate.** In `~/.cargo/registry/src/.../wry-0.55.1/src/wkwebview/`, find where wry sets `navigationDelegate` / whether it exposes `on_navigation`/auth hooks. Determine the delegate class name and whether `didReceiveAuthenticationChallenge` is implemented (if wry doesn't implement it, WKWebView falls back to default handling — and we can add it via a category/swizzle). Document findings in the task report.

- [ ] **Step 2: Implement `install.rs`** (platform dispatch, mirrors `webview_perms::grant_microphone`):
```rust
//! Install the per-platform cert-trust hook on a Panel webview.
use tauri::WebviewWindow;

pub fn install_cert_trust(window: &WebviewWindow) {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        if let Err(e) = window.with_webview({
            let win = window.clone();
            move |pview| {
                #[cfg(target_os = "linux")]
                super::adapter_linux::install(&pview, &win);
                #[cfg(target_os = "windows")]
                super::adapter_windows::install(&pview, &win);
                #[cfg(target_os = "macos")]
                super::adapter_macos::install(&pview, &win);
            }
        }) {
            tracing::warn!("could not reach platform webview for cert-trust install: {e}");
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    let _ = window;
}
```

- [ ] **Step 3: Implement `adapter_macos.rs`** with the shared decision helper + the WKWebView challenge hook found in Step 1. The decision helper (platform-agnostic, put it in `install.rs` or a shared fn so all adapters reuse it):
```rust
// Shared across adapters — resolve a presented leaf DER for `host`.
pub(crate) enum HookAction { Allow, Reject }
pub(crate) fn resolve(window: &tauri::WebviewWindow, host: &str, leaf_der: &[u8], reason: &str) -> HookAction {
    use crate::cert_trust::{decide, Decision, fingerprint::{fingerprint_sha256, parse_cert_info}, store::TrustStore, pending};
    let fp = fingerprint_sha256(leaf_der);
    let info = parse_cert_info(leaf_der, reason);
    let path = match pending::store_path() { Some(p) => p, None => return HookAction::Reject };
    let store = TrustStore::load(&path);
    match decide(host, &fp, info.clone(), &store) {
        Decision::Allow => HookAction::Allow,
        Decision::PromptUnknown { fp, info } => { prompt(window, host, fp, info, None); HookAction::Reject }
        Decision::WarnChanged { old_fp, new_fp, info } => { prompt(window, host, new_fp, info, Some(old_fp)); HookAction::Reject }
    }
}
fn prompt(window: &tauri::WebviewWindow, host: &str, fp: String, info: crate::cert_trust::CertInfo, changed_from: Option<String>) {
    use crate::cert_trust::pending::{self, PendingCert, PendingRecord, set_trust_pending};
    if let Some(state) = window.try_state::<PendingCert>() {
        *state.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(PendingRecord { host: host.to_string(), fp, info, changed_from });
    }
    set_trust_pending(true);
    let _ = window.eval("window.location.href='cert-trust.html'");
}
```
The macOS-specific part installs (via the Step-1 mechanism) a `didReceiveAuthenticationChallenge` handler that, for `NSURLAuthenticationMethodServerTrust`, extracts the leaf cert DER from `SecTrustGetCertificateAtIndex(serverTrust, 0)` → `SecCertificateCopyData`, calls `resolve(...)`, and on `Allow` calls `completionHandler(.useCredential, URLCredential(trust: serverTrust))`, else `.performDefaultHandling` (which fails the load → the prompt is already showing). Reason string: `"self-signed / untrusted issuer"`.

- [ ] **Step 4:** Add macOS objc deps to `Cargo.toml` under `[target.'cfg(target_os = "macos")'.dependencies]` (use whatever wry 0.55 uses — likely `objc2` + `objc2-foundation` + `objc2-web-kit`; match versions to wry's to avoid a second objc stack). Wire `cert_trust::install::install_cert_trust(&window)` in `main.rs` right after the existing `webview_perms::grant_microphone(&window)` call.

- [ ] **Step 5: Manual e2e (macOS):** `just shell-dev-lite`, pre-seed `~/.aleph/.desktop-shell-panel-target` = `https://172.245.43.211:18790`, launch. Expect: `cert-trust.html` appears showing fingerprint `49:3D:...`; click 信任 → Panel loads → TokenWall. Relaunch → no prompt (pinned). Verify: `cat ~/.aleph/.desktop-shell-panel-trusted-certs` contains the host→fp entry.

- [ ] **Step 6: Commit:**
```bash
git add desktop/shell/src/cert_trust/adapter_macos.rs desktop/shell/src/cert_trust/install.rs desktop/shell/src/cert_trust/mod.rs desktop/shell/Cargo.toml desktop/shell/src/main.rs
git commit -m "cert-trust: macOS WKWebView challenge adapter + install dispatch (reference platform)"
```

---

## Phase 3 — Linux WebKitGTK adapter

### Task 8: Linux `load_failed_with_tls_errors` adapter

**Files:**
- Create: `desktop/shell/src/cert_trust/adapter_linux.rs`
- Modify: `desktop/shell/src/cert_trust/mod.rs` (`#[cfg(target_os="linux")] pub mod adapter_linux;`)

**Interfaces:**
- Consumes: `resolve` / `HookAction` (from `install.rs`, Task 7), `webkit2gtk`.
- Produces: `adapter_linux::install(pview, window)`.

- [ ] **Step 1: Implement** modeled on `webview_perms::grant_linux`:
```rust
//! Linux WebKitGTK cert-trust hook: on a TLS error, extract the cert, consult
//! the shared decision core, and either whitelist+reload (pinned) or show the
//! approval prompt.
use webkit2gtk::glib::object::Cast;
use webkit2gtk::{TLSErrorsPolicy, WebViewExt, WebContextExt};

pub fn install(pview: &tauri::webview::PlatformWebview, window: &tauri::WebviewWindow) {
    let webview = pview.inner();
    // Default policy must stay Fail so valid certs are validated normally and
    // self-signed ones surface load-failed-with-tls-errors (never Ignore — that
    // would blanket-accept, violating fail-closed).
    if let Some(ctx) = WebViewExt::context(&webview) {
        ctx.set_tls_errors_policy(TLSErrorsPolicy::Fail);
    }
    let win = window.clone();
    webview.connect_load_failed_with_tls_errors(move |wv, uri, cert, _errors| {
        let host = url::Url::parse(uri).ok()
            .and_then(|u| u.host_str().map(|h| {
                let port = u.port_or_known_default().unwrap_or(18790);
                format!("{h}:{port}")
            }));
        let Some(host) = host else { return false };
        // gio::TlsCertificate -> DER
        let der = cert.certificate().map(|bytes| bytes.to_vec());
        let Some(der) = der else { return false };
        match super::install::resolve(&win, &host, &der, "self-signed / untrusted issuer") {
            super::install::HookAction::Allow => {
                if let Some(ctx) = WebViewExt::context(wv) {
                    if let Some(h) = url::Url::parse(uri).ok().and_then(|u| u.host_str().map(str::to_string)) {
                        ctx.allow_tls_certificate_for_host(cert, &h);
                    }
                }
                wv.load_uri(uri);
                true
            }
            super::install::HookAction::Reject => true, // prompt shown; stop the failed load
        }
    });
}
```
> Verify the exact `webkit2gtk 2.0.2` names: `connect_load_failed_with_tls_errors`, `WebContextExt::allow_tls_certificate_for_host(&self, certificate: &TlsCertificate, host: &str)`, `TlsCertificate::certificate() -> Option<glib::Bytes>` (DER). Adjust imports to the crate's actual `*Ext` traits.

- [ ] **Step 2: Manual e2e (Linux, on UbuntuDev or a Linux box with a display):** build the lite shell, point at the self-signed remote, confirm prompt → trust → load; relaunch → silent.

- [ ] **Step 3: Commit:**
```bash
git add desktop/shell/src/cert_trust/adapter_linux.rs desktop/shell/src/cert_trust/mod.rs
git commit -m "cert-trust: Linux WebKitGTK TLS-error adapter"
```

---

## Phase 4 — Windows WebView2 adapter

### Task 9: Windows `ServerCertificateErrorDetected` adapter

**Files:**
- Create: `desktop/shell/src/cert_trust/adapter_windows.rs`
- Modify: `desktop/shell/src/cert_trust/mod.rs` (`#[cfg(target_os="windows")] pub mod adapter_windows;`)

**Interfaces:**
- Consumes: `resolve` / `HookAction` (Task 7), `webview2_com`.
- Produces: `adapter_windows::install(pview, window)`.

- [ ] **Step 1: Implement** modeled on `webview_perms::grant_windows`, using `ICoreWebView2_14::add_ServerCertificateErrorDetected`. On the event: get `args.ErrorStatus()` (only proceed for cert errors), `args.Certificate()` → `ICoreWebView2Certificate` → DER via `ToPemEncoding`/`get_DerEncodedSerialNumber`… (use the certificate's DER accessor; `CoreWebView2Certificate` exposes the raw cert), call `resolve`; on `Allow` set `args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW)`, else `..._DEFAULT`. Use `args.GetDeferral()` only if `resolve` must be async (it isn't — the prompt is fire-and-forget; the reload path re-triggers the event and gets `Allow`). Cast `core` to `ICoreWebView2_14` via `.cast()`; if the runtime is older and lacks `_14`, log and no-op (fail-closed: cert stays rejected).

- [ ] **Step 2: Manual e2e (Windows):** per `WINDOWS_RUNTIME.md`, build the lite shell, point at the self-signed remote, confirm prompt → trust → load; relaunch → silent.

- [ ] **Step 3: Commit:**
```bash
git add desktop/shell/src/cert_trust/adapter_windows.rs desktop/shell/src/cert_trust/mod.rs
git commit -m "cert-trust: Windows WebView2 ServerCertificateErrorDetected adapter"
```

---

## Phase 5 — iOS (Swift)

### Task 10: iOS trust store + decision mirror (unit-tested)

**Files:**
- Create: `mobile/ios/AlephPaneliOS/Services/CertTrustStore.swift`
- Create: `mobile/ios/AlephPaneliOSTests/CertTrustStoreTests.swift`

**Interfaces:**
- Produces: `CertTrustStore` (Keychain-backed, mirrors the Rust store) + `certDecision(host:presentedFP:store:) -> CertDecision`.

- [ ] **Step 1: Write failing Swift Testing tests** (`import Testing`), mirroring the Rust truth table: unknown→prompt, match→allow, changed→warn; store round-trip via an in-memory conformer (reuse the `InMemoryConnectionStore` pattern already in `AlephPaneliOSTests`).
```swift
import Testing
@testable import AlephPaneliOS

@Test func unknownHostPrompts() {
    let store = InMemoryCertStore()
    if case .promptUnknown = certDecision(host: "h:1", presentedFP: "AA:BB", store: store) {} else { Issue.record("expected prompt") }
}
@Test func matchingFPAllows() {
    let store = InMemoryCertStore(); store.pin("h:1", "AA:BB")
    #expect(certDecision(host: "h:1", presentedFP: "AA:BB", store: store) == .allow)
}
@Test func changedFPWarns() {
    let store = InMemoryCertStore(); store.pin("h:1", "AA:BB")
    if case .warnChanged = certDecision(host: "h:1", presentedFP: "CC:DD", store: store) {} else { Issue.record("expected warn") }
}
```

- [ ] **Step 2: Run — expect FAIL** (types undefined). `xcodebuild test` per the iOS test workflow (see memory `feedback-ios-panel-test-via-full-macos-app` — build via the app scheme).

- [ ] **Step 3: Implement** `CertTrustStore.swift`: a `CertStore` protocol (`lookup`/`pin`), a Keychain-backed conformer (mirror `ConnectionStore`'s Keychain usage), an `InMemoryCertStore` for tests, `enum CertDecision { case allow; case promptUnknown(fp:String, info:CertInfo); case warnChanged(old:String, new:String, info:CertInfo) }`, and `func certDecision(host:presentedFP:store:)`. Fingerprint helper `sha256Fingerprint(_ der: Data) -> String` using CryptoKit `SHA256`, colon-grouped uppercase.

- [ ] **Step 4: Run — expect PASS.**

- [ ] **Step 5: Commit:**
```bash
git add mobile/ios/AlephPaneliOS/Services/CertTrustStore.swift mobile/ios/AlephPaneliOSTests/CertTrustStoreTests.swift
git commit -m "cert-trust(ios): Keychain trust store + decision mirror (unit-tested)"
```

### Task 11: iOS WKWebView challenge handler + approval sheet

**Files:**
- Modify: `mobile/ios/AlephPaneliOS/Views/PanelWebView.swift` (add `didReceive challenge` to `Coordinator`)
- Create: `mobile/ios/AlephPaneliOS/Views/CertTrustSheet.swift`
- Modify: `mobile/ios/AlephPaneliOS/State/AppState.swift` + the view hosting `PanelWebView` (present the sheet)

**Interfaces:**
- Consumes: `CertTrustStore`, `certDecision`, `sha256Fingerprint` (Task 10).

- [ ] **Step 1: Add the challenge handler** to `PanelWebView.Coordinator`:
```swift
func webView(_ webView: WKWebView,
             didReceive challenge: URLAuthenticationChallenge,
             completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
    guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
          let trust = challenge.protectionSpace.serverTrust else {
        completionHandler(.performDefaultHandling, nil); return
    }
    // Default validation first: a CA-valid cert (domain) passes here → no prompt.
    var error: CFError?
    if SecTrustEvaluateWithError(trust, &error) {
        completionHandler(.performDefaultHandling, nil); return
    }
    guard let der = leafDER(from: trust) else { completionHandler(.cancelAuthenticationChallenge, nil); return }
    let host = "\(challenge.protectionSpace.host):\(challenge.protectionSpace.port)"
    switch certDecision(host: host, presentedFP: sha256Fingerprint(der), store: store) {
    case .allow:
        completionHandler(.useCredential, URLCredential(trust: trust))
    case .promptUnknown(let fp, let info), .warnChanged(_, let fp, let info):
        onCertPrompt(host, fp, info) { approved in
            if approved { store.pin(host, fp); completionHandler(.useCredential, URLCredential(trust: trust)) }
            else { completionHandler(.cancelAuthenticationChallenge, nil) }
        }
    }
}
```
(Thread `store` and an `onCertPrompt` closure through `Coordinator`/`PanelWebView`, set from the hosting view; `leafDER` extracts index-0 cert via `SecTrustCopyCertificateChain`/`SecCertificateCopyData`.)

- [ ] **Step 2: Create `CertTrustSheet.swift`** — a SwiftUI sheet showing host, fingerprint (monospaced), SAN/subject, reason, a red banner for the changed case, Trust/Cancel buttons calling back the completion.

- [ ] **Step 3: Wire the sheet** in the view hosting `PanelWebView` (driven by `AppState` pending-cert `@Published` state), so `onCertPrompt` presents it and the button resolves the completion.

- [ ] **Step 4: Manual e2e (iOS sim/device):** per the iOS test workflow, point the app at `https://172.245.43.211:18790`, confirm the sheet shows the fingerprint → Trust → Panel loads → TokenWall; relaunch → no sheet.

- [ ] **Step 5: Commit:**
```bash
git add mobile/ios/AlephPaneliOS/Views/PanelWebView.swift mobile/ios/AlephPaneliOS/Views/CertTrustSheet.swift mobile/ios/AlephPaneliOS/State/AppState.swift
git commit -m "cert-trust(ios): WKWebView ServerTrust challenge + SwiftUI approval sheet"
```

---

## Self-Review (against the spec)

- **Requirement 1 (auto-fetch + prompt + load):** Tasks 7/8/9/11 (adapters) + 4/5 (pending+UI) + 10/11 (iOS). ✓
- **Requirement 2 (trigger only on validation failure):** Linux `TLSErrorsPolicy::Fail` + `load_failed_with_tls_errors` (Task 8); Windows `ServerCertificateErrorDetected` (Task 9); WKWebView `SecTrustEvaluateWithError` default-first (Task 11) / default-handling fallback (Task 7). Valid certs never prompt. ✓
- **Requirement 3 (TOFU + change warning):** `decide` (Task 1) + `WarnChanged` UI (Tasks 5, 11). ✓
- **Requirement 4 (no OS trust store):** all allow paths are per-challenge grants (useCredential / allow_tls_certificate_for_host / AllowAlways). ✓
- **Requirement 5 (4 platforms):** Phases 2–5. ✓
- **Fail-closed:** default/cancel → engine default validation → fail (every adapter). ✓
- **Store best-effort:** Task 2 `load` + test `corrupt_file_loads_empty_not_panic`. ✓
- **Supervisor coordination:** Task 6. ✓

**Known spike:** Task 7 Step 1 (wry macOS delegate hook). If infeasible, contingency = macOS-only loopback proxy — escalate before proceeding.

**Type consistency:** `decide(host, presented_fp, info, store)`, `fingerprint_sha256(der)`, `parse_cert_info(der, reason)`, `TrustStore::{empty,load,lookup,insert_mem,insert_and_save}`, `pending::{store_path, PendingCert, PendingRecord, set_trust_pending, TRUST_PENDING}`, `install::{install_cert_trust, resolve, HookAction}` — used consistently across tasks.
