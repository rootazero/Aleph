use aleph_desktop_host::updater::{verify_artifact, VerifyError};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

// Public, already-signed test vectors from tauri-plugin-updater 2.14.0
// (src/updater.rs::offline_tests), not Aleph's production trust material.
const KEY: &str = "untrusted comment: minisign public key: A49EF4746FA6E6D7\nRWTX5qZvdPSepDGWg2bz8n6ArQnBMwgQK1geVosorcbLzcunD+KJ+cYe";
const SIGNATURE: &str = "untrusted comment: signature from tauri secret key\nRUTX5qZvdPSepPF2zsjvP/L/TQtBLRnWRvaq+4omK1UEugtQp/USrGiI1v762sDy3id3aor7bRC9ETyeot6lMHHrWoZ5hjUM1Qc=\ntrusted comment: timestamp:1791649051\tfile:payload\tversion:2.0.0\nDDq2CLOEl5cQmC2eJRHlnZPVViOZsu1CjpPKQoei4A/XBfkDqGO0B+QMrcrHgEQ5qV3mne75tdFWhJpqUXh9AA==";
const OLD_KEY: &str = "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const UNVERSIONED: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";
// The un-hashed legacy-algorithm vector from minisign-verify 0.3.0's tests.
// Algorithm compatibility is separate from missing-version compatibility.
const LEGACY_ALGORITHM: &str = "untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";

#[test]
fn accepts_actual_signed_artifact_and_equivalent_version_spellings() {
    for required in [false, true] {
        for version in ["2.0.0", "v2.0.0"] {
            assert_eq!(
                verify_artifact(
                    b"test",
                    &STANDARD.encode(SIGNATURE),
                    &STANDARD.encode(KEY),
                    version,
                    required,
                ),
                Ok(())
            );
        }
    }
}

#[test]
fn rejects_relabelled_version_even_when_signed_version_is_not_required() {
    for required in [false, true] {
        assert_eq!(
            verify_artifact(
                b"test",
                &STANDARD.encode(SIGNATURE),
                &STANDARD.encode(KEY),
                "3.0.0",
                required,
            ),
            Err(VerifyError::SignedVersionMismatch)
        );
    }
}

#[test]
fn unversioned_artifacts_follow_the_explicit_existing_compatibility_policy() {
    assert_eq!(
        verify_artifact(
            b"test",
            &STANDARD.encode(UNVERSIONED),
            &STANDARD.encode(OLD_KEY),
            "9.9.9",
            false,
        ),
        Ok(())
    );
    assert_eq!(
        verify_artifact(
            b"test",
            &STANDARD.encode(UNVERSIONED),
            &STANDARD.encode(OLD_KEY),
            "9.9.9",
            true,
        ),
        Err(VerifyError::MissingSignedVersion)
    );
}

#[test]
fn preserves_legacy_algorithm_support_without_inventing_a_signed_version() {
    for (required, expected) in [
        (false, Ok(())),
        (true, Err(VerifyError::MissingSignedVersion)),
    ] {
        assert_eq!(
            verify_artifact(
                b"test",
                &STANDARD.encode(LEGACY_ALGORITHM),
                &STANDARD.encode(OLD_KEY),
                "9.9.9",
                required,
            ),
            expected
        );
    }
}

#[test]
fn rejects_tampered_bytes_wrong_key_and_unverified_trusted_comment() {
    for (data, key, signature, version) in [
        (b"Test".as_slice(), KEY, SIGNATURE.to_string(), "2.0.0"),
        (b"test".as_slice(), OLD_KEY, SIGNATURE.to_string(), "2.0.0"),
        (
            b"test".as_slice(),
            KEY,
            SIGNATURE.replace("version:2.0.0", "version:3.0.0"),
            "3.0.0",
        ),
    ] {
        assert_eq!(
            verify_artifact(
                data,
                &STANDARD.encode(signature),
                &STANDARD.encode(key),
                version,
                true,
            ),
            Err(VerifyError::SignatureRejected)
        );
    }
}

#[test]
fn untrusted_comment_cannot_supply_a_signed_version() {
    let signature = UNVERSIONED.replace(
        "untrusted comment: signature from minisign secret key",
        "untrusted comment: version:9.9.9",
    );
    assert_eq!(
        verify_artifact(
            b"test",
            &STANDARD.encode(signature),
            &STANDARD.encode(OLD_KEY),
            "9.9.9",
            true,
        ),
        Err(VerifyError::MissingSignedVersion)
    );
}

#[test]
fn missing_version_compatibility_never_bypasses_authentication() {
    for (data, key, signature) in [
        (b"Test".as_slice(), OLD_KEY, UNVERSIONED.to_string()),
        (b"test".as_slice(), KEY, UNVERSIONED.to_string()),
        (
            b"test".as_slice(),
            OLD_KEY,
            UNVERSIONED.replace("file:test", "file:forged"),
        ),
    ] {
        assert_eq!(
            verify_artifact(
                data,
                &STANDARD.encode(signature),
                &STANDARD.encode(key),
                "9.9.9",
                false,
            ),
            Err(VerifyError::SignatureRejected)
        );
    }
}

#[test]
fn rejects_malformed_envelopes_without_returning_the_input() {
    for malformed in [
        "not-base64:DO_NOT_LOG_THIS".to_string(),
        STANDARD.encode([255]),
        STANDARD.encode("not a minisign envelope"),
    ] {
        assert_eq!(
            verify_artifact(
                b"test",
                &STANDARD.encode(SIGNATURE),
                &malformed,
                "2.0.0",
                true,
            ),
            Err(VerifyError::InvalidPublicKey)
        );
        assert_eq!(
            verify_artifact(b"test", &malformed, &STANDARD.encode(KEY), "2.0.0", true,),
            Err(VerifyError::InvalidSignatureEncoding)
        );
    }
}
