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
    let path = store_path().ok_or("home dir not found")?;
    // Persist-then-clear: the disk I/O happens *before* the pending record
    // is dropped, so a failed persist leaves the record in place and the
    // user can retry without re-triggering a TLS challenge. Without this
    // order, a transient disk-full / EACCES / I/O error would silently
    // swallow the user's approval and the next connection would prompt
    // for the same cert again.
    persist_pending_if_match(state.inner(), &host, &fingerprint, &path)?;
    set_trust_pending(false);
    // Reload the remote target now that the cert is pinned.
    crate::reroute_for_target(&app, crate::connection::load_target());
    Ok(())
}

/// Persist the pending record that matches `host`+`fingerprint`, and clear
/// the in-memory pending state ONLY after a successful disk write. The
/// pending record is cloned for the persist; if the persist fails the
/// original stays put and the user can retry the approval click. The whole
/// sequence runs under one lock so a concurrent TLS challenge cannot
/// re-create a pending record and have us watch it cleared.
fn persist_pending_if_match(
    state: &PendingCert,
    host: &str,
    fingerprint: &str,
    path: &std::path::Path,
) -> Result<(), String> {
    let mut guard = state
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Match-only read — *no* take. This is the new invariant of W-13: the
    // pending record lives until the disk write returns Ok.
    let record = match guard.as_ref() {
        Some(r) if r.host == host && r.fp == fingerprint => r.clone(),
        Some(_) => {
            return Err(
                "pending cert host or fingerprint mismatch — the displayed certificate changed; reload the trust page to review the new one".into(),
            );
        }
        None => return Err("no pending cert".into()),
    };

    let mut store = TrustStore::load(path);
    if let Err(e) = store.insert_and_save(&record.host, &record.fp, path) {
        // Persist failed; pending record is still in `guard`. Return Err
        // with the underlying I/O kind so the UI can surface it, and let
        // the user retry the click — their approval is not lost.
        return Err(format!("persist trust: {e}"));
    }

    // Persist succeeded, NOW take. `guard.as_ref()` was Some(record); the
    // post-write load can only differ if a concurrent TLS challenge raced
    // between our read and our clear — but we hold the lock the whole time,
    // so no one else could have replaced the record.
    *guard = None;
    Ok(())
}

/// Take the pending record iff `host` *and* `fingerprint` both match the
/// stored one. On a fingerprint mismatch (the TOCTOU race C-2), the stored
/// record is *preserved* — `guard` is re-filled with the new record so the
/// trust page can re-render and the user can re-review instead of having
/// their click silently approve a cert they never saw. Extracted as a pure
/// helper so the contract is unit-testable without a `tauri::AppHandle`.
#[cfg(test)]
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
        assert!(
            take_matching_record(&state, "gw.example", "AAA").is_err(),
            "drained"
        );
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

    // W-13 regression tests for the persist-then-clear ordering.

    /// Use a fresh tmpdir as HOME so the trust store path lands on disk
    /// under it. Each test scopes to its own dir via the `tag` so parallel
    /// test runs (or repeated runs after a flaky failure) don't trample
    /// each other. `drop` test scope is enforced by `serial_test::serial`.
    /// Returns a guard path + the resolved store file path. The dir is
    /// rm'd on `Guard::drop`.
    struct ScopedHome {
        path: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
    }
    impl Drop for ScopedHome {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    fn scoped_tempdir(tag: &str) -> (ScopedHome, std::path::PathBuf) {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "aleph-cert-trust-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("mkdir HOME temp");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &p);
        let store = store_path_in(&p);
        (ScopedHome { path: p, prev }, store)
    }
    fn store_path_in(home: &std::path::Path) -> std::path::PathBuf {
        let mut p = home.to_path_buf();
        p.push(".aleph");
        p.push("desktop-shell-trusted-certs");
        p
    }

    /// W-13 happy path: pending matches, persist succeeds, state cleared.
    #[test]
    #[serial_test::serial]
    fn persist_pending_happy_path_clears_state_and_writes_store() {
        let (_tmp, path) = scoped_tempdir("w13-happy");
        let state = PendingCert::default();
        put(&state, fixture("gw.example", "AAA"));

        persist_pending_if_match(&state, "gw.example", "AAA", &path).expect("persist succeeds");

        // State cleared, store has the pin.
        assert!(
            state.0.lock().unwrap().is_none(),
            "pending cleared after persist"
        );
        let store = TrustStore::load(&path);
        assert_eq!(store.lookup("gw.example"), Some("AAA"));
    }

    /// W-13 contract — a transient disk error must NOT lose the user's
    /// approval. Chmod 0500 on the store's parent dir makes
    /// `insert_and_save` fail (create_dir_all/write_all are denied), and
    /// the pending record must survive for the user to retry.
    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn persist_pending_preserves_record_when_persist_fails() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, path) = scoped_tempdir("w13-persist-err");
        let state = PendingCert::default();
        put(&state, fixture("gw.example", "AAA"));

        // Build the parent dir but take away write so insert_and_save
        // (which writes a tmp + rename) returns PermissionDenied.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o500),
        )
        .expect("chmod 0500");

        let err = persist_pending_if_match(&state, "gw.example", "AAA", &path)
            .expect_err("persist must fail under 0500 parent");
        assert!(err.contains("persist trust"), "got: {err}");

        // The pending record is still there — the user can retry.
        assert!(
            state.0.lock().unwrap().is_some(),
            "pending record preserved across persist failure"
        );

        // Restore so the tempdir can be cleaned up by ScopedHome::drop.
        let _ = std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        );
    }

    /// W-13 contract — a fingerprint mismatch (C-2 TOCTOU race) still
    /// rejects and still preserves the new record. The persist-then-clear
    /// helper must compose with the C-2 guard, not silently bypass it.
    #[test]
    #[serial_test::serial]
    fn persist_pending_rejects_wrong_fingerprint_and_preserves_record() {
        let (_tmp, path) = scoped_tempdir("w13-fp-mismatch");
        let state = PendingCert::default();
        put(&state, fixture("gw.example", "AAA"));

        let err = persist_pending_if_match(&state, "gw.example", "BBB", &path).unwrap_err();
        assert!(err.contains("mismatch"), "got: {err}");

        // Pending still present (the page renders B for re-review).
        let r = take_matching_record(&state, "gw.example", "AAA").expect("still present");
        assert_eq!(r.fp, "AAA");

        // Nothing was written to the store on the failed attempt.
        let store = TrustStore::load(&path);
        assert!(store.lookup("gw.example").is_none(), "no pin written");
    }

    /// W-13 contract — no pending cert at all returns Err without
    /// touching the store.
    #[test]
    #[serial_test::serial]
    fn persist_pending_no_pending_record_returns_err() {
        let (_tmp, path) = scoped_tempdir("w13-no-pending");
        let state = PendingCert::default();

        let err = persist_pending_if_match(&state, "gw.example", "AAA", &path).unwrap_err();
        assert!(err.contains("no pending"), "got: {err}");
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
