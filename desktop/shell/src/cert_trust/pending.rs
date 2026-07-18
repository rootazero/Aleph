//! Pending self-signed cert awaiting a user decision — the bridge between a
//! native TLS-error hook (per-engine callback, later task) and the
//! `cert-trust.html` approval page (later task). One pending slot: the shell
//! hosts a single webview against a single Gateway target at a time, so only
//! one connection attempt is ever blocked on a trust decision.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;

use super::store::TrustStore;
use super::{CertInfo, Decision};

/// A cert decision awaiting the user's approve/reject choice.
#[derive(Debug, Clone, Serialize)]
pub struct PendingCertInfo {
    pub host: String,
    pub fp: String,
    pub old_fp: Option<String>,
    pub info: CertInfo,
}

impl PendingCertInfo {
    /// Build from a non-`Allow` decision; `Allow` has nothing to prompt for.
    fn from_decision(host: &str, decision: &Decision) -> Option<Self> {
        match decision {
            Decision::Allow => None,
            Decision::PromptUnknown { fp, info } => Some(Self {
                host: host.to_string(),
                fp: fp.clone(),
                old_fp: None,
                info: info.clone(),
            }),
            Decision::WarnChanged {
                old_fp,
                new_fp,
                info,
            } => Some(Self {
                host: host.to_string(),
                fp: new_fp.clone(),
                old_fp: Some(old_fp.clone()),
                info: info.clone(),
            }),
        }
    }
}

/// Shared pending-cert slot, managed by Tauri so the TLS-error hook (writer)
/// and the approval page's commands (readers) agree on the one outstanding
/// decision.
#[derive(Default)]
pub struct PendingCert {
    slot: Mutex<Option<PendingCertInfo>>,
}

impl PendingCert {
    /// Stash a decision as pending; a no-op for `Decision::Allow`. Called by
    /// the (later-task) TLS-error hook once it has run [`super::decide`].
    pub fn set(&self, host: &str, decision: &Decision) {
        if let Some(pending) = PendingCertInfo::from_decision(host, decision) {
            *self
                .slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pending);
        }
    }
}

/// Where pinned-fingerprint decisions persist: mirrors the other
/// `.desktop-shell[-panel]-*` markers, namespaced per shell variant so the
/// full app and lite shell never share trust state.
fn store_path() -> Option<PathBuf> {
    crate::connection::marker_path("trusted-certs")
}

// ---------------------------------------------------------------------------
// Tauri commands — consumed by `cert-trust.html` (later task) to display the
// pending cert and record the user's choice.
// ---------------------------------------------------------------------------

/// Return the cert currently awaiting approval, if any.
#[tauri::command]
pub fn get_pending_cert(state: tauri::State<'_, PendingCert>) -> Option<PendingCertInfo> {
    state
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Pin the pending cert's fingerprint and re-route to the connection target.
#[tauri::command]
pub fn approve_cert(
    app: tauri::AppHandle,
    state: tauri::State<'_, PendingCert>,
) -> Result<(), String> {
    let pending = state
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let Some(pending) = pending else {
        return Err("no certificate is pending approval".to_string());
    };
    let path = store_path().ok_or_else(|| "home directory not found".to_string())?;
    let mut store = TrustStore::load(&path);
    store
        .insert_and_save(&pending.host, &pending.fp, &path)
        .map_err(|e| e.to_string())?;
    crate::reroute_for_target(&app, crate::connection::load_target());
    Ok(())
}

/// Discard the pending cert without pinning it; the connection stays blocked.
#[tauri::command]
pub fn reject_cert(state: tauri::State<'_, PendingCert>) {
    *state
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}
