//! Shared pending-cert state bridging a platform adapter's TLS-error hook and
//! the `cert-trust.html` approval UI. The adapter stashes the pending cert here
//! and navigates the webview to the trust page; the page reads it via
//! `get_pending_cert` and resolves it via `approve_cert` / `reject_cert`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;

use crate::cert_trust::{store::TrustStore, CertInfo};

/// True while a cert-trust prompt owns the webview. The lite supervisor reads
/// this to skip its relocation tick so the user isn't pulled off the prompt.
pub static TRUST_PENDING: AtomicBool = AtomicBool::new(false);

pub fn set_trust_pending(v: bool) {
    TRUST_PENDING.store(v, Ordering::SeqCst);
}

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
    let guard = state
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref().map(|r| PendingCertView {
        host: r.host.clone(),
        fingerprint: r.fp.clone(),
        sans: r.info.sans.clone(),
        subject: r.info.subject.clone(),
        reason: r.info.reason.clone(),
        changed_from: r.changed_from.clone(),
    })
}

/// Pin the pending cert for `host` and reload the remote target. Both `host`
/// and `fingerprint` must match the pending record — the UI shows the user
/// exactly the fingerprint it captured at page-load, so the approval must
/// confirm THAT fingerprint was the one reviewed. Without the fingerprint
/// check, a second TLS challenge for the same host could overwrite the
/// pending record before the user clicks Approve, letting them pin a
/// certificate they were never shown (auth bypass).
#[tauri::command]
pub fn approve_cert(
    app: tauri::AppHandle,
    state: tauri::State<'_, PendingCert>,
    host: String,
    fingerprint: String,
) -> Result<(), String> {
    let record = {
        let mut guard = state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.take() {
            Some(r) if r.host == host && r.fp == fingerprint => r,
            Some(other) => {
                *guard = Some(other);
                return Err("pending cert host or fingerprint mismatch — the displayed certificate changed; reload the trust page to review the new one".into());
            }
            None => return Err("no pending cert".into()),
        }
    };
    let path = store_path().ok_or("home dir not found")?;
    let mut store = TrustStore::load(&path);
    store
        .insert_and_save(&record.host, &record.fp, &path)
        .map_err(|e| format!("persist trust: {e}"))?;
    set_trust_pending(false);
    // Reload the remote target now that the cert is pinned.
    crate::reroute_for_target(&app, crate::connection::load_target());
    Ok(())
}

#[tauri::command]
pub fn reject_cert(state: tauri::State<'_, PendingCert>) {
    let mut guard = state
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = None;
    set_trust_pending(false);
}
