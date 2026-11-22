//! Pure artifact verification matching the existing desktop updater policy.
//! The caller supplies the existing key and policy; this module performs no I/O.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use minisign_verify::{PublicKey, Signature};
use semver::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("invalid updater public key")]
    InvalidPublicKey,
    #[error("invalid updater signature encoding")]
    InvalidSignatureEncoding,
    #[error("updater signature rejected")]
    SignatureRejected,
    #[error("updater signed version missing")]
    MissingSignedVersion,
    #[error("updater signed version mismatch")]
    SignedVersionMismatch,
}

pub fn verify_artifact(
    data: &[u8],
    release_signature: &str,
    public_key: &str,
    announced_version: &str,
    require_signed_version: bool,
) -> Result<(), VerifyError> {
    let public_key = STANDARD
        .decode(public_key)
        .map_err(|_| VerifyError::InvalidPublicKey)?;
    let public_key = std::str::from_utf8(&public_key).map_err(|_| VerifyError::InvalidPublicKey)?;
    let public_key = PublicKey::decode(public_key).map_err(|_| VerifyError::InvalidPublicKey)?;
    let signature = STANDARD
        .decode(release_signature)
        .map_err(|_| VerifyError::InvalidSignatureEncoding)?;
    let signature =
        std::str::from_utf8(&signature).map_err(|_| VerifyError::InvalidSignatureEncoding)?;
    let signature =
        Signature::decode(signature).map_err(|_| VerifyError::InvalidSignatureEncoding)?;
    public_key
        .verify(data, &signature, true)
        .map_err(|_| VerifyError::SignatureRejected)?;

    // Only the verified global signature makes this comment trustworthy.
    verify_signed_version(
        signature.trusted_comment(),
        announced_version,
        require_signed_version,
    )
}

fn verify_signed_version(
    trusted_comment: &str,
    announced_version: &str,
    require_signed_version: bool,
) -> Result<(), VerifyError> {
    let Some(signed_version) = trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
    else {
        if require_signed_version {
            return Err(VerifyError::MissingSignedVersion);
        }
        return Ok(());
    };
    let matches = match (
        Version::parse(signed_version.trim_start_matches('v')),
        Version::parse(announced_version.trim_start_matches('v')),
    ) {
        (Ok(signed), Ok(announced)) => signed == announced,
        _ => signed_version == announced_version,
    };
    if !matches {
        return Err(VerifyError::SignedVersionMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{verify_signed_version, VerifyError};

    // Policy-only checks do not claim that these literal comments are signed.
    // Cryptographic admission is exercised through the public API separately.
    #[test]
    fn literal_fallback_preserves_non_semver_policy() {
        let comment = "timestamp:0\tfile:test\tversion:2024-01-01";
        assert_eq!(verify_signed_version(comment, "2024-01-01", true), Ok(()));
        assert_eq!(
            verify_signed_version(comment, "2024-01-02", true),
            Err(VerifyError::SignedVersionMismatch)
        );
    }

    #[test]
    fn only_the_first_tab_separated_version_field_is_consumed() {
        let comment = "timestamp:0\tversion:2.0.0\tversion:3.0.0";
        assert_eq!(verify_signed_version(comment, "2.0.0", true), Ok(()));
        assert_eq!(
            verify_signed_version(comment, "3.0.0", false),
            Err(VerifyError::SignedVersionMismatch)
        );
    }
}
