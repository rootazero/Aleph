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

#[derive(Clone, Debug)]
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
    let record = take_matching_record(state.inner(), &host, &fingerprint)?;
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

/// Take the pending record iff `host` *and* `fingerprint` both match the
/// stored one. On a fingerprint mismatch (the TOCTOU race C-2), the stored
/// record is *preserved* — `guard` is re-filled with the new record so the
/// trust page can re-render and the user can re-review instead of having
/// their click silently approve a cert they never saw. Extracted as a pure
/// helper so the contract is unit-testable without a `tauri::AppHandle`.
fn take_matching_record(
    state: &PendingCert,
    host: &str,
    fingerprint: &str,
) -> Result<PendingRecord, String> {
    let mut guard = state
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match guard.take() {
        Some(r) if r.host == host && r.fp == fingerprint => Ok(r),
        Some(other) => {
            *guard = Some(other);
            Err("pending cert host or fingerprint mismatch — the displayed certificate changed; reload the trust page to review the new one".into())
        }
        None => Err("no pending cert".into()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert_trust::CertInfo;

    fn fixture(host: &str, fp: &str) -> PendingRecord {
        PendingRecord {
            host: host.into(),
            fp: fp.into(),
            info: CertInfo {
                sans: vec!["127.0.0.1".into()],
                subject: "CN=test".into(),
                reason: format!("self-signed for {host}"),
            },
            changed_from: None,
        }
    }

    fn put(state: &PendingCert, r: PendingRecord) {
        *state.0.lock().unwrap() = Some(r);
    }

    #[test]
    fn take_matching_returns_record_on_exact_match() {
        let state = PendingCert::default();
        put(&state, fixture("gw.example", "AAA"));
        let r = take_matching_record(&state, "gw.example", "AAA").expect("match");
        assert_eq!(r.host, "gw.example");
        assert_eq!(r.fp, "AAA");
        assert!(take_matching_record(&state, "gw.example", "AAA").is_err(), "drained");
    }

    #[test]
    fn take_matching_rejects_wrong_fingerprint_and_preserves_record() {
        let state = PendingCert::default();
        put(&state, fixture("gw.example", "AAA"));
        let err = take_matching_record(&state, "gw.example", "BBB").unwrap_err();
        assert!(err.contains("mismatch"), "got: {err}");
        // The record must still be there — the UI re-renders and the user
        // re-reviews; silently dropping it would let a stale page approve
        // a different cert on retry.
        let r = take_matching_record(&state, "gw.example", "AAA").expect("still present");
        assert_eq!(r.fp, "AAA");
    }

    #[test]
    fn take_matching_rejects_wrong_host_and_preserves_record() {
        let state = PendingCert::default();
        put(&state, fixture("gw.example", "AAA"));
        let err = take_matching_record(&state, "other.example", "AAA").unwrap_err();
        assert!(err.contains("mismatch"), "got: {err}");
        let r = take_matching_record(&state, "gw.example", "AAA").expect("still present");
        assert_eq!(r.host, "gw.example");
    }

    /// T-1: regression test for C-2. Simulates a second TLS challenge that
    /// overwrites the pending record between page-load and user click. The
    /// first click must (a) reject and (b) preserve the *new* record so the
    /// page can re-render. Without the e6b57be4a fingerprint guard, this
    /// would be a silent auth bypass.
    #[test]
    fn fingerprint_change_during_review_is_rejected_not_bypassed() {
        let state = PendingCert::default();
        // Page loads with cert A.
        put(&state, fixture("gw.example", "AAA"));

        // A second TLS challenge overwrites the pending record with cert B.
        // The page is still showing cert A's SHA-256.
        std::thread::sleep(std::time::Duration::from_millis(5));
        put(&state, fixture("gw.example", "BBB"));

        // User clicks Trust — the click carries the *page-captured*
        // fingerprint A. Without e6b57be4a's guard this would silently pin B.
        let err = take_matching_record(&state, "gw.example", "AAA").unwrap_err();
        assert!(err.contains("mismatch"), "got: {err}");

        // The new record B is preserved — not lost — so the page can
        // re-render the cert and the user can re-review.
        let r = take_matching_record(&state, "gw.example", "BBB").expect("B survived");
        assert_eq!(r.fp, "BBB");
    }
}
