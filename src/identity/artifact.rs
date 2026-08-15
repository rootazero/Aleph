//! Binding an artifact — a patch, a report, a release candidate — to the agent
//! that produced it.
//!
//! Agents can open repos and produce diffs, but a file on disk says nothing
//! about who made it: any process could have written those bytes. This module
//! is the "发补丁、审代码" primitive that closes that: the agent's active key
//! signs the artifact's SHA-256, and the envelope lands next to the artifact
//! as `<path>.aleph-sig.json`.
//!
//! ## What a valid envelope proves — and what it does not
//!
//! A [`ArtifactVerdict::Valid`] verdict proves the artifact's current bytes are
//! exactly the bytes a process holding the named agent's private key signed,
//! and that the key belongs to the agent the envelope claims. The same bound as
//! the rest of this subsystem applies: it is **tamper-evidence, not defence
//! against someone who owns the machine** — vault and database share a disk, so
//! a local adversary can sign anything as any agent. The value is off-box: hand
//! the artifact, the envelope and the agent's public fingerprint to someone who
//! does not trust this machine, and the signature checks without it.
//!
//! `at_ms` is carried for the reader's convenience and is deliberately **not**
//! inside the signature: a timestamp supplied by the signer attests to nothing
//! (a process holding the key can claim any instant), and signing it would
//! dress an informational field up as evidence.
//!
//! Rotation and revocation do not invalidate envelopes: keys are keyed by
//! fingerprint and never deleted, so a retired or revoked agent's signatures
//! still verify — that is the point of keeping the keys. Only a *deleted* key
//! row degrades a verdict to [`ArtifactVerdict::UnknownSigner`].
//!
//! ## Preimage
//!
//! `b"aleph-agent-artifact-v1" || len32be(agent_id) || agent_id || sha256_bytes`
//!
//! The agent id leads, length-prefixed in the style of [`super::hash`], so an
//! envelope cannot be re-homed: claiming a different signer changes the
//! preimage and the signature no longer covers it. The digest enters raw — it
//! is fixed-width, so no framing is needed. The file bytes themselves are
//! never signed directly; SHA-256 is what lets the signing path read the
//! artifact once, under a hard size cap, instead of trusting an unbounded
//! buffer to Ed25519.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::keystore::{AgentKeystore, KeyError};
use crate::gateway::security::crypto::verify_signature;
use crate::gateway::security::store::SecurityStore;

/// Domain separator, leading the signing preimage — see [`super::hash`] for
/// why every subsystem gets its own. Bump only alongside a format migration:
/// changing it invalidates every envelope ever written.
const DOMAIN: &[u8] = b"aleph-agent-artifact-v1";

/// The only envelope version this binary reads. Carried in the document so a
/// future format change is refused loudly rather than guessed at — the same
/// posture as [`EXPORT_FORMAT`](super::export::EXPORT_FORMAT).
const ENVELOPE_VERSION: u32 = 1;

/// Suffix appended to the artifact's name to derive the envelope's path.
/// Appended rather than extension-replaced, so `fix.patch` signs to
/// `fix.patch.aleph-sig.json` and the artifact's own name survives intact.
pub const ENVELOPE_SUFFIX: &str = ".aleph-sig.json";

/// Largest artifact this module will read and hash. The cap exists because
/// signing reads the whole file into memory; past it, hash the artifact
/// yourself and have the agent sign the digest instead. 256 MiB is arbitrary
/// but comfortably above any patch or report an agent produces in practice.
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(
        "refusing to sign or hash {path}: {size} bytes exceeds the {MAX_ARTIFACT_BYTES}-byte cap \
         — hash it yourself and sign the digest instead"
    )]
    TooLarge { path: PathBuf, size: u64 },
    #[error("{path} is not an Aleph artifact signature: {detail}")]
    Malformed { path: PathBuf, detail: String },
    #[error("{0}")]
    Key(#[from] KeyError),
}

impl ArtifactError {
    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Self + '_ {
        move |source| Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    fn malformed(path: &Path, detail: impl std::fmt::Display) -> Self {
        Self::Malformed {
            path: path.to_path_buf(),
            detail: detail.to_string(),
        }
    }
}

/// The envelope written next to a signed artifact as `<path>.aleph-sig.json`.
///
/// `Deserialize` is safe here — unlike [`NewRecord`](super::record::NewRecord),
/// this document is *input to a verifier*, never to a writer: nothing that
/// parses an envelope can turn it into a ledger row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSignature {
    /// Envelope format version — see [`ENVELOPE_VERSION`].
    pub v: u32,
    /// The agent the envelope claims signed. Inside the preimage, so swapping
    /// it for another agent's name invalidates the signature.
    pub agent: String,
    /// Fingerprint of the signing key. The verifier resolves the public half
    /// from `agent_keys`, retired keys included.
    pub signer_fp: String,
    /// SHA-256 of the artifact's bytes, hex.
    pub sha256: String,
    /// Ed25519 signature over the preimage, hex.
    pub sig: String,
    /// When the envelope was produced. Informational — NOT signed; see the
    /// module doc.
    pub at_ms: i64,
}

/// What checking an artifact against its envelope found.
///
/// A verdict rather than an error for every "no" answer: a bad signature is
/// the expected output of the adversarial case, not a malfunction of the
/// check, and the caller (tool, CLI) needs to report *which* way it failed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum ArtifactVerdict {
    /// The artifact's bytes hash to the signed digest, the signature verifies
    /// under the named key, and the key belongs to the claimed agent.
    Valid,
    /// The file's current bytes no longer hash to what was signed — it was
    /// edited (or replaced) after the envelope was written.
    HashMismatch { signed: String, actual: String },
    /// The signature does not verify under the key the envelope names.
    BadSignature,
    /// The envelope names a fingerprint `agent_keys` has never held. Retired
    /// and revoked keys still resolve; only a deleted row — or a key minted
    /// elsewhere — lands here.
    UnknownSigner { fingerprint: String },
    /// The key is real but belongs to a **different agent** than the envelope
    /// claims. An envelope re-homed from another agent's signing lands here
    /// when the tamper is caught before the signature check.
    AgentMismatch { claimed: String, key_owner: String },
    /// The caller asserted a specific signer and the envelope names another.
    WrongAgent { expected: String, claimed: String },
}

impl ArtifactVerdict {
    /// `true` only for [`Self::Valid`]. Named `holds` rather than `ok` to match
    /// [`HeadPin::holds`](super::export::HeadPin::holds) — a verdict is a claim
    /// that either stands or does not.
    #[must_use]
    pub const fn holds(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

/// Where the envelope for `artifact` lives: its own path plus
/// [`ENVELOPE_SUFFIX`].
#[must_use]
pub fn envelope_path(artifact: &Path) -> PathBuf {
    let mut name = artifact.as_os_str().to_owned();
    name.push(ENVELOPE_SUFFIX);
    PathBuf::from(name)
}

/// The bytes a signature covers: domain tag, length-prefixed agent id, then
/// the raw 32-byte digest. Fixed layout — changing any part of it invalidates
/// every envelope ever written.
fn preimage(agent: &str, sha256: &[u8; 32]) -> Vec<u8> {
    let agent = agent.as_bytes();
    let mut p = Vec::with_capacity(DOMAIN.len() + 4 + agent.len() + 32);
    p.extend_from_slice(DOMAIN);
    p.extend_from_slice(&u32::try_from(agent.len()).unwrap_or(u32::MAX).to_be_bytes());
    p.extend_from_slice(agent);
    p.extend_from_slice(sha256);
    p
}

/// Read `path` under the size cap and hash it. Shared by signing and
/// verification so the two can never disagree about what was hashed.
fn hash_file(path: &Path) -> Result<[u8; 32], ArtifactError> {
    let size = std::fs::metadata(path)
        .map_err(ArtifactError::io(path))?
        .len();
    if size > MAX_ARTIFACT_BYTES {
        return Err(ArtifactError::TooLarge {
            path: path.to_path_buf(),
            size,
        });
    }
    let bytes = std::fs::read(path).map_err(ArtifactError::io(path))?;
    // The file may have grown between the metadata call and the read; the cap
    // is about what lands in memory, so it is enforced on the bytes, not only
    // on what metadata promised.
    let size = bytes.len() as u64;
    if size > MAX_ARTIFACT_BYTES {
        return Err(ArtifactError::TooLarge {
            path: path.to_path_buf(),
            size,
        });
    }
    Ok(Sha256::digest(&bytes).into())
}

/// Sign `path` as `agent` with its active key and return the envelope.
///
/// Get-or-creates the identity ([`AgentKeystore::signing_identity`]), the same
/// posture the ledger takes: an agent that signs before anyone ran `keygen`
/// must still produce an attributable signature, and the signing is recorded
/// on its chain immediately after by the caller. A **revoked** agent signs
/// with its retired key rather than being refused — nothing in this subsystem
/// gates execution, so refusing would suppress the evidence without stopping
/// the act.
///
/// Does not write the envelope to disk: that is the caller's I/O decision
/// (the tool writes [`envelope_path`]; a test may not write at all).
pub fn sign_artifact(
    keys: &AgentKeystore,
    agent: &str,
    path: &Path,
) -> Result<ArtifactSignature, ArtifactError> {
    let digest = hash_file(path)?;
    let identity = keys.signing_identity(agent)?;
    let sig = keys.sign(&identity.active_fingerprint, &preimage(agent, &digest))?;
    Ok(ArtifactSignature {
        v: ENVELOPE_VERSION,
        agent: agent.to_string(),
        signer_fp: identity.active_fingerprint,
        sha256: hex::encode(digest),
        sig: hex::encode(sig),
        at_ms: crate::session::events::now_ms(),
    })
}

/// Parse an envelope file, refusing unknown versions rather than guessing.
pub fn read_envelope(path: &Path) -> Result<ArtifactSignature, ArtifactError> {
    let body = std::fs::read_to_string(path).map_err(ArtifactError::io(path))?;
    let envelope: ArtifactSignature = serde_json::from_str(&body)
        .map_err(|e| ArtifactError::malformed(path, format!("not an envelope: {e}")))?;
    if envelope.v != ENVELOPE_VERSION {
        return Err(ArtifactError::malformed(
            path,
            format!(
                "envelope version {} (this binary reads {ENVELOPE_VERSION})",
                envelope.v
            ),
        ));
    }
    Ok(envelope)
}

/// Check `path` against `envelope`, resolving the public key from `store`.
///
/// Takes the store rather than the keystore on purpose: verification needs
/// public keys only, and taking the minimum is what lets the offline CLI run
/// this with no vault at all. `expected_agent`, when given, is the caller
/// asserting *who* must have signed — a mismatch is a verdict, not an error.
///
/// The checks run cheapest-answer-first, and each short-circuits: an edited
/// file reports [`ArtifactVerdict::HashMismatch`] even if the envelope is also
/// lying about the signer, because "the bytes changed" is the fact the reader
/// most needs first.
pub fn verify_artifact(
    store: &SecurityStore,
    path: &Path,
    envelope: &ArtifactSignature,
    expected_agent: Option<&str>,
) -> Result<ArtifactVerdict, ArtifactError> {
    let digest = hash_file(path)?;
    let actual = hex::encode(digest);
    if !actual.eq_ignore_ascii_case(&envelope.sha256) {
        return Ok(ArtifactVerdict::HashMismatch {
            signed: envelope.sha256.clone(),
            actual,
        });
    }
    if let Some(expected) = expected_agent {
        if expected != envelope.agent {
            return Ok(ArtifactVerdict::WrongAgent {
                expected: expected.to_string(),
                claimed: envelope.agent.clone(),
            });
        }
    }
    let Some(key) = store
        .get_agent_key(&envelope.signer_fp)
        .map_err(KeyError::from)?
    else {
        return Ok(ArtifactVerdict::UnknownSigner {
            fingerprint: envelope.signer_fp.clone(),
        });
    };
    // Ownership, not just arithmetic: a valid signature from another agent's
    // key attests to nothing about THIS agent — the chain verifier's
    // `ForeignSigner` argument, one surface over.
    if key.agent_id != envelope.agent {
        return Ok(ArtifactVerdict::AgentMismatch {
            claimed: envelope.agent.clone(),
            key_owner: key.agent_id,
        });
    }
    let sig = hex::decode(&envelope.sig)
        .map_err(|e| ArtifactError::malformed(path, format!("envelope sig: {e}")))?;
    Ok(
        if verify_signature(&key.public_key, &preimage(&envelope.agent, &digest), &sig).is_ok() {
            ArtifactVerdict::Valid
        } else {
            ArtifactVerdict::BadSignature
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::crypto::{generate_keypair, sign_message, DeviceFingerprint};
    use crate::gateway::security::shared_token::SharedTokenManager;
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    struct Fixture {
        keys: AgentKeystore,
        dir: TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let store = Arc::new(SecurityStore::in_memory().unwrap());
            let vault = Arc::new(SharedTokenManager::new(
                store.clone(),
                dir.path().join("t.vault"),
            ));
            let _ = vault.generate_token();
            Self {
                keys: AgentKeystore::new(store, vault),
                dir,
            }
        }

        fn artifact(&self, name: &str, body: &[u8]) -> PathBuf {
            let path = self.dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            path
        }

        fn verify(&self, path: &Path, envelope: &ArtifactSignature) -> ArtifactVerdict {
            verify_artifact(self.keys.store(), path, envelope, None).unwrap()
        }
    }

    #[test]
    fn sign_then_verify_round_trips() {
        let f = Fixture::new();
        let path = f.artifact("fix.patch", b"diff --git a/x b/x\n");
        let envelope = sign_artifact(&f.keys, "main", &path).unwrap();
        assert_eq!(envelope.v, ENVELOPE_VERSION);
        assert_eq!(envelope.agent, "main");

        // The envelope is what travels, so verify the form that survives the
        // disk, not just the struct in memory.
        let json = serde_json::to_string_pretty(&envelope).unwrap();
        let env_path = envelope_path(&path);
        std::fs::write(&env_path, &json).unwrap();
        let parsed = read_envelope(&env_path).unwrap();
        assert_eq!(f.verify(&path, &parsed), ArtifactVerdict::Valid);
    }

    #[test]
    fn editing_the_file_after_signing_is_a_hash_mismatch() {
        let f = Fixture::new();
        let path = f.artifact("fix.patch", b"harmless\n");
        let envelope = sign_artifact(&f.keys, "main", &path).unwrap();
        std::fs::write(&path, b"harmless, mostly\n").unwrap();

        let verdict = f.verify(&path, &envelope);
        assert!(
            matches!(verdict, ArtifactVerdict::HashMismatch { .. }),
            "{verdict:?}"
        );
        assert!(!verdict.holds());
    }

    #[test]
    fn claiming_another_agent_fails() {
        // Both layers of the defence are exercised: the owner check catches
        // the re-homed envelope before the signature check would, and if it
        // somehow did not, the agent id inside the preimage moves the digest.
        let f = Fixture::new();
        let path = f.artifact("report.md", b"quarterly\n");
        let mut envelope = sign_artifact(&f.keys, "main", &path).unwrap();
        envelope.agent = "trader".to_string();

        let verdict = f.verify(&path, &envelope);
        assert!(
            matches!(verdict, ArtifactVerdict::AgentMismatch { .. }),
            "{verdict:?}"
        );
        assert!(!verdict.holds());
    }

    #[test]
    fn a_tampered_signature_fails() {
        let f = Fixture::new();
        let path = f.artifact("fix.patch", b"diff\n");
        let mut envelope = sign_artifact(&f.keys, "main", &path).unwrap();
        // Flip one hex character of the signature.
        let first = envelope.sig.remove(0);
        envelope.sig.insert(0, if first == '0' { '1' } else { '0' });
        assert_eq!(f.verify(&path, &envelope), ArtifactVerdict::BadSignature);
    }

    #[test]
    fn an_unknown_signer_is_reported_not_refused() {
        // A perfectly well-formed envelope signed by a key this installation
        // never minted — minted offline, or its row deleted. The verdict names
        // the fact; it must not read as a generic bad signature.
        let f = Fixture::new();
        let path = f.artifact("fix.patch", b"diff\n");
        let (seed, public) = generate_keypair();
        let fingerprint = DeviceFingerprint::from_public_key(&public).0;
        let digest: [u8; 32] = Sha256::digest(b"diff\n").into();
        let sig = sign_message(&seed, &preimage("ghost", &digest));
        let envelope = ArtifactSignature {
            v: ENVELOPE_VERSION,
            agent: "ghost".to_string(),
            signer_fp: fingerprint.clone(),
            sha256: hex::encode(digest),
            sig: hex::encode(sig),
            at_ms: 0,
        };
        assert_eq!(
            f.verify(&path, &envelope),
            ArtifactVerdict::UnknownSigner { fingerprint }
        );
    }

    #[test]
    fn a_revoked_agents_signature_still_verifies() {
        // The whole reason keys are never deleted: "this artifact was signed by
        // an agent we have since revoked" is a fact a reviewer must be able to
        // establish, not a verification error.
        let f = Fixture::new();
        let path = f.artifact("fix.patch", b"diff\n");
        let envelope = sign_artifact(&f.keys, "main", &path).unwrap();
        assert!(f.keys.revoke("main").unwrap());
        assert_eq!(f.verify(&path, &envelope), ArtifactVerdict::Valid);
    }

    #[test]
    fn an_asserted_signer_is_checked() {
        let f = Fixture::new();
        let path = f.artifact("fix.patch", b"diff\n");
        let envelope = sign_artifact(&f.keys, "main", &path).unwrap();
        let store = f.keys.store();
        assert_eq!(
            verify_artifact(store, &path, &envelope, Some("main")).unwrap(),
            ArtifactVerdict::Valid
        );
        assert!(matches!(
            verify_artifact(store, &path, &envelope, Some("trader")).unwrap(),
            ArtifactVerdict::WrongAgent { .. }
        ));
    }

    #[test]
    fn oversized_artifacts_are_refused_before_they_are_read() {
        let f = Fixture::new();
        let path = f.dir.path().join("huge.bin");
        // Sparse: the length is set without writing the bytes, because the
        // metadata check is what must fire.
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_ARTIFACT_BYTES + 1)
            .unwrap();
        assert!(matches!(
            sign_artifact(&f.keys, "main", &path),
            Err(ArtifactError::TooLarge { .. })
        ));
        let envelope = ArtifactSignature {
            v: ENVELOPE_VERSION,
            agent: "main".to_string(),
            signer_fp: "whatever".to_string(),
            sha256: "00".repeat(32),
            sig: "00".repeat(64),
            at_ms: 0,
        };
        assert!(matches!(
            verify_artifact(f.keys.store(), &path, &envelope, None),
            Err(ArtifactError::TooLarge { .. })
        ));
    }

    #[test]
    fn an_unknown_envelope_version_is_refused_not_guessed() {
        let f = Fixture::new();
        let env_path = f.artifact(
            "future.aleph-sig.json",
            br#"{"v":2,"agent":"main","signer_fp":"x","sha256":"y","sig":"z","at_ms":0}"#,
        );
        assert!(matches!(
            read_envelope(&env_path),
            Err(ArtifactError::Malformed { .. })
        ));
    }
}
