//! Per-agent Ed25519 signing keys.
//!
//! **Wiring, not new crypto.** The keypair primitives
//! ([`generate_keypair`](crate::gateway::security::crypto::generate_keypair),
//! [`sign_message`](crate::gateway::security::crypto::sign_message),
//! [`verify_signature`](crate::gateway::security::crypto::verify_signature),
//! [`DeviceFingerprint`]) have shipped in `gateway::security::crypto` since the
//! device-auth work but had **no production consumer** — the `devices.public_key`
//! column every writer fills with empty bytes is the visible half of that. This
//! module is their first real caller; nothing cryptographic is added here.
//!
//! Private keys are stored as hex in the existing encrypted vault
//! (`SharedTokenManager` → AES-256-GCM + per-entry HKDF salt), keyed by
//! **fingerprint** rather than by agent id so a rotation keeps the retired key
//! decryptable — records it signed must stay verifiable. Vault rotation
//! (`gateway.token.rotate`) re-encrypts them along with every other secret, so
//! agent keys inherit that lifecycle for free.
//!
//! ## What a signature here does and does not prove
//!
//! It proves a record was produced by a process holding that agent's decrypted
//! private key, and it makes any later edit to a stored row detectable without
//! trusting the row itself. It does **not** defend against an adversary who
//! owns `~/.aleph` — vault, master key and database are all on the same disk.
//! That bound is inherent to a local-first daemon; the value is that the public
//! key can be exported and pinned off-box, after which an exported chain
//! segment is verifiable by someone who does not trust the machine it came
//! from. See `docs/reference/AGENT_IDENTITY.md`.
//!
//! This module signs; it deliberately does **not** verify. Verification lives
//! in [`verify`](super::verify), where a signature is checked against a key
//! *and* that key is checked to be the chain owner's. A convenience
//! `verify(fingerprint, …)` here would be a second, weaker path — valid
//! signature, wrong agent — and a second copy of a security check is how the
//! divergence gets shipped.

use std::collections::HashMap;

use zeroize::Zeroizing;

use crate::gateway::security::crypto::{generate_keypair, sign_message, DeviceFingerprint};
use crate::gateway::security::shared_token::SharedTokenManager;
use crate::gateway::security::store::SecurityStore;
use crate::sync_primitives::{Arc, Mutex};

/// A public key an agent has held. Retired keys are kept, never deleted.
#[derive(Debug, Clone)]
pub struct AgentKeyRow {
    pub fingerprint: String,
    pub agent_id: String,
    pub public_key: Vec<u8>,
    pub created_at: i64,
    pub retired_at: Option<i64>,
}

/// An agent's identity: its active key plus its chain anchor.
#[derive(Debug, Clone)]
pub struct AgentIdentityRow {
    pub agent_id: String,
    pub active_fingerprint: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
    /// Highest sequence this chain is known to have reached. Compared against
    /// the stored rows by [`verify`](super::verify) to catch tail truncation.
    pub head_seq: i64,
    pub head_hash: Option<Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("identity store error: {0}")]
    Store(#[from] rusqlite::Error),
    #[error("vault error: {0}")]
    Vault(String),
    #[error("no signing key for fingerprint {0}")]
    MissingKey(String),
    #[error("stored key material for {0} is malformed")]
    MalformedKey(String),
    #[error("agent {0} has no identity")]
    UnknownAgent(String),
    #[error("agent {0} is revoked")]
    Revoked(String),
}

/// Vault entry name for a signing key. Keyed by fingerprint — see module doc.
fn secret_name(fingerprint: &str) -> String {
    format!("agent-key:{fingerprint}")
}

/// Mints, stores and uses per-agent signing keys.
pub struct AgentKeystore {
    store: Arc<SecurityStore>,
    vault: Arc<SharedTokenManager>,
    /// Decrypted seeds, keyed by fingerprint. Bounded by the number of agents
    /// that have ever signed in this process (single digits in practice), so
    /// no eviction policy is warranted — adding one would be speculative.
    cache: Mutex<HashMap<String, Zeroizing<[u8; 32]>>>,
}

impl AgentKeystore {
    #[must_use]
    pub fn new(store: Arc<SecurityStore>, vault: Arc<SharedTokenManager>) -> Self {
        Self {
            store,
            vault,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The agent's identity, minting one on first use. **Refuses a revoked
    /// agent** — this is the operator-facing provisioning call (`keygen`), and
    /// silently reinstating a key someone deliberately revoked would make the
    /// revocation meaningless. `rotate` is the explicit way back.
    pub fn ensure(&self, agent_id: &str) -> Result<AgentIdentityRow, KeyError> {
        let row = self.signing_identity(agent_id)?;
        if row.revoked_at.is_some() {
            return Err(KeyError::Revoked(agent_id.to_string()));
        }
        Ok(row)
    }

    /// The identity the ledger signs with — get-or-create, **and it tolerates a
    /// revoked agent**.
    ///
    /// Get-or-create rather than an explicit provisioning step: an agent that
    /// acts before anyone remembered to run `keygen` must still produce
    /// attributable records, and a ledger that silently skips unprovisioned
    /// agents is exactly the "operator runs the report and sees zeros" failure
    /// this subsystem exists to avoid.
    ///
    /// Revoked agents keep signing with their (retired) key rather than having
    /// their records dropped. Nothing in this subsystem gates execution —
    /// revocation is a mark on the identity, not an enforcement hook — so
    /// refusing to record would not stop the action, it would only stop the
    /// evidence. "This agent acted 40 minutes after it was revoked" is precisely
    /// what an accountability record exists to be able to show, and a record
    /// that was never written is the one thing a hash chain can never reveal.
    pub fn signing_identity(&self, agent_id: &str) -> Result<AgentIdentityRow, KeyError> {
        if let Some(row) = self.store.get_agent_identity(agent_id)? {
            return Ok(row);
        }
        self.mint(agent_id)?;
        self.store
            .get_agent_identity(agent_id)?
            .ok_or_else(|| KeyError::UnknownAgent(agent_id.to_string()))
    }

    /// Replace the active key. The old key is retired (kept, so its records
    /// stay verifiable) and the chain anchor is deliberately untouched — see
    /// [`SecurityStore::upsert_agent_identity`].
    pub fn rotate(&self, agent_id: &str) -> Result<AgentIdentityRow, KeyError> {
        if let Some(prev) = self.store.get_agent_identity(agent_id)? {
            self.store.retire_agent_key(&prev.active_fingerprint)?;
        }
        self.mint(agent_id)?;
        self.store
            .get_agent_identity(agent_id)?
            .ok_or_else(|| KeyError::UnknownAgent(agent_id.to_string()))
    }

    /// Mark an identity revoked and retire its keys. Its chain stays readable
    /// and verifiable. Returns `false` when the agent had no live identity.
    ///
    /// Deliberately **not** an execution gate: nothing in this subsystem stops
    /// a revoked agent from running, so if one does, the action is still
    /// recorded (see [`Self::signing_identity`]) — "acted after revocation" is
    /// a fact the ledger should be able to prove, not one it should erase.
    pub fn revoke(&self, agent_id: &str) -> Result<bool, KeyError> {
        self.store
            .revoke_agent_identity(agent_id)
            .map_err(Into::into)
    }

    /// Sign `message` with the key named by `fingerprint`.
    pub fn sign(&self, fingerprint: &str, message: &[u8]) -> Result<[u8; 64], KeyError> {
        let seed = self.seed(fingerprint)?;
        Ok(sign_message(&seed, message))
    }

    pub fn identity(&self, agent_id: &str) -> Result<Option<AgentIdentityRow>, KeyError> {
        Ok(self.store.get_agent_identity(agent_id)?)
    }

    pub fn list(&self) -> Result<Vec<AgentIdentityRow>, KeyError> {
        Ok(self.store.list_agent_identities()?)
    }

    pub fn keys_of(&self, agent_id: &str) -> Result<Vec<AgentKeyRow>, KeyError> {
        Ok(self.store.list_agent_keys(agent_id)?)
    }

    #[must_use]
    pub fn store(&self) -> &Arc<SecurityStore> {
        &self.store
    }

    /// Generate a keypair, persist the public half in the DB and the private
    /// half in the vault, and bind it as the agent's active key.
    ///
    /// Order matters: the private key is vaulted **before** the identity is
    /// bound, so a crash between the two leaves an orphan vault entry (inert)
    /// rather than an identity whose key cannot be loaded (a signer that fails
    /// on every append).
    fn mint(&self, agent_id: &str) -> Result<String, KeyError> {
        let (signing, verifying) = generate_keypair();
        let seed = Zeroizing::new(signing);
        let fingerprint = DeviceFingerprint::from_public_key(&verifying).0;

        self.vault
            .store_secret(&secret_name(&fingerprint), &hex::encode(seed.as_ref()))
            .map_err(|e| KeyError::Vault(e.to_string()))?;
        self.store
            .insert_agent_key(&fingerprint, agent_id, &verifying)?;
        self.store.upsert_agent_identity(agent_id, &fingerprint)?;

        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fingerprint.clone(), seed);

        tracing::info!(
            agent_id,
            fingerprint = %fingerprint,
            "minted agent signing key"
        );
        Ok(fingerprint)
    }

    /// Load (and memoize) a private seed.
    fn seed(&self, fingerprint: &str) -> Result<Zeroizing<[u8; 32]>, KeyError> {
        if let Some(seed) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(fingerprint)
        {
            return Ok(seed.clone());
        }

        let secret = self
            .vault
            .get_secret(&secret_name(fingerprint))
            .map_err(|e| KeyError::Vault(e.to_string()))?
            .ok_or_else(|| KeyError::MissingKey(fingerprint.to_string()))?;

        let bytes = Zeroizing::new(
            hex::decode(secret.expose())
                .map_err(|_| KeyError::MalformedKey(fingerprint.to_string()))?,
        );
        let seed: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| KeyError::MalformedKey(fingerprint.to_string()))?;
        let seed = Zeroizing::new(seed);

        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fingerprint.to_string(), seed.clone());
        Ok(seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// What `verify::Keyring` does for one row, minus the ownership check:
    /// resolve the stored public key by fingerprint and check the signature.
    fn verifies(ks: &AgentKeystore, fingerprint: &str, message: &[u8], signature: &[u8]) -> bool {
        ks.store()
            .get_agent_key(fingerprint)
            .unwrap()
            .is_some_and(|k| {
                crate::gateway::security::crypto::verify_signature(
                    &k.public_key,
                    message,
                    signature,
                )
                .is_ok()
            })
    }

    fn keystore() -> (AgentKeystore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store.clone(),
            dir.path().join("test.vault"),
        ));
        // Seed the vault master key so store_secret/get_secret work.
        let _ = vault.generate_token();
        (AgentKeystore::new(store, vault), dir)
    }

    #[test]
    fn ensure_is_get_or_create() {
        let (ks, _d) = keystore();
        let first = ks.ensure("main").unwrap();
        let again = ks.ensure("main").unwrap();
        assert_eq!(first.active_fingerprint, again.active_fingerprint);
        assert_eq!(first.head_seq, 0);
    }

    #[test]
    fn distinct_agents_get_distinct_keys() {
        let (ks, _d) = keystore();
        let a = ks.ensure("main").unwrap();
        let b = ks.ensure("trader").unwrap();
        assert_ne!(a.active_fingerprint, b.active_fingerprint);
    }

    #[test]
    fn sign_verifies_against_the_stored_public_key() {
        let (ks, _d) = keystore();
        let id = ks.ensure("main").unwrap();
        let sig = ks.sign(&id.active_fingerprint, b"payload").unwrap();
        assert!(verifies(&ks, &id.active_fingerprint, b"payload", &sig));
        assert!(!verifies(&ks, &id.active_fingerprint, b"tampered", &sig));
    }

    #[test]
    fn rotation_retires_the_old_key_but_keeps_it_verifiable() {
        let (ks, _d) = keystore();
        let old = ks.ensure("main").unwrap();
        let sig = ks
            .sign(&old.active_fingerprint, b"before rotation")
            .unwrap();

        let new = ks.rotate("main").unwrap();
        assert_ne!(old.active_fingerprint, new.active_fingerprint);

        // The retired key still verifies what it signed — this is why keys are
        // keyed by fingerprint and never deleted.
        assert!(verifies(
            &ks,
            &old.active_fingerprint,
            b"before rotation",
            &sig
        ));

        let keys = ks.keys_of("main").unwrap();
        assert_eq!(keys.len(), 2);
        let retired = keys
            .iter()
            .find(|k| k.fingerprint == old.active_fingerprint)
            .unwrap();
        assert!(retired.retired_at.is_some());
    }

    #[test]
    fn rotation_does_not_reset_the_chain_anchor() {
        // Rotating must never look like a way to erase history.
        let (ks, _d) = keystore();
        let id = ks.ensure("main").unwrap();
        ks.store()
            .ledger_insert(&crate::identity::record::LedgerRecord {
                agent_id: "main".into(),
                seq: 1,
                prev_hash: None,
                hash: vec![7u8; 32],
                signature: vec![0u8; 64],
                signer_fp: id.active_fingerprint.clone(),
                action: crate::identity::record::LedgerAction::ToolCall,
                target: "bash".into(),
                outcome: crate::identity::record::LedgerOutcome::Ok,
                args_fp: None,
                detail: "x".into(),
                at_ms: 1,
            })
            .unwrap();

        ks.rotate("main").unwrap();
        let after = ks.identity("main").unwrap().unwrap();
        assert_eq!(after.head_seq, 1);
        assert_eq!(after.head_hash.as_deref(), Some(&[7u8; 32][..]));
    }

    #[test]
    fn revoked_agent_cannot_be_ensured_back_into_service() {
        let (ks, _d) = keystore();
        ks.ensure("main").unwrap();
        assert!(ks.revoke("main").unwrap());
        assert!(matches!(ks.ensure("main"), Err(KeyError::Revoked(_))));
        // Revoking twice is not an error, it just changes nothing.
        assert!(!ks.revoke("main").unwrap());
        // The identity remains listable — a revoked agent's chain is evidence.
        assert_eq!(ks.list().unwrap().len(), 1);
    }

    #[test]
    fn a_revoked_agent_can_still_be_recorded_against() {
        // Revocation must not become a way to make an agent's actions
        // unrecordable: nothing here gates execution, so refusing to sign
        // would suppress the evidence without stopping the act.
        let (ks, _d) = keystore();
        let before = ks.ensure("main").unwrap();
        ks.revoke("main").unwrap();

        let signing = ks.signing_identity("main").unwrap();
        assert_eq!(signing.active_fingerprint, before.active_fingerprint);
        assert!(signing.revoked_at.is_some(), "the mark survives");

        // And the retired key still signs and verifies.
        let sig = ks
            .sign(&signing.active_fingerprint, b"after revocation")
            .unwrap();
        assert!(verifies(
            &ks,
            &signing.active_fingerprint,
            b"after revocation",
            &sig
        ));
    }

    #[test]
    fn rotate_reinstates_a_revoked_agent() {
        // The operator-visible escape hatch: re-keying is a deliberate act.
        let (ks, _d) = keystore();
        ks.ensure("main").unwrap();
        ks.revoke("main").unwrap();
        let back = ks.rotate("main").unwrap();
        assert!(back.revoked_at.is_none());
    }
}
