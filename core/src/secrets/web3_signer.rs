//! EVM-compatible signing module.
//!
//! Signs messages and transactions using secp256k1 private keys
//! stored in the SecretVault. Private keys are decrypted only
//! during the signing operation and never returned to the caller.

use k256::ecdsa::{SigningKey, VerifyingKey};
use std::fmt;

use super::types::SecretError;
use super::vault::SecretVault;

/// Intent for what should be signed.
#[derive(Debug, Clone)]
pub enum SignIntent {
    /// EIP-191 personal_sign
    PersonalSign { message: Vec<u8> },
    /// EIP-712 typed data
    TypedData {
        domain_hash: [u8; 32],
        struct_hash: [u8; 32],
    },
    /// EIP-1559 transaction signing (simplified).
    ///
    /// **WARNING**: Uses simplified byte concatenation instead of proper RLP encoding.
    /// Signatures produced with this variant are NOT valid for on-chain submission.
    /// Use PersonalSign or TypedData for production signing. A future version will
    /// add proper RLP encoding for on-chain transaction signing.
    Transaction {
        chain_id: u64,
        to: [u8; 20],
        value: [u8; 32],
        data: Vec<u8>,
        nonce: u64,
        gas_limit: u64,
        max_fee_per_gas: u64,
        max_priority_fee_per_gas: u64,
    },
}

/// Result of a signing operation. NEVER contains private key.
#[derive(Clone)]
pub struct SignedResult {
    pub signature: Vec<u8>,
    pub recovery_id: u8,
    pub signer_address: [u8; 20],
}

impl fmt::Debug for SignedResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedResult")
            .field(
                "signature",
                &format!(
                    "0x{}...",
                    hex::encode(&self.signature[..4.min(self.signature.len())])
                ),
            )
            .field("recovery_id", &self.recovery_id)
            .field(
                "signer_address",
                &format!("0x{}", hex::encode(self.signer_address)),
            )
            .finish()
    }
}

impl fmt::Display for SignedResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SignedResult(signer=0x{}, sig_len={})",
            hex::encode(self.signer_address),
            self.signature.len()
        )
    }
}

/// EVM signer that reads private keys from the vault.
pub struct EvmSigner<'a> {
    vault: &'a SecretVault,
}

impl<'a> EvmSigner<'a> {
    pub fn new(vault: &'a SecretVault) -> Self {
        Self { vault }
    }

    /// Get the Ethereum address for a secret.
    pub fn get_address(&self, secret_name: &str) -> Result<[u8; 20], SecretError> {
        let secret = self.vault.get(secret_name)?;
        let signing_key = parse_private_key(secret.expose())?;
        let verifying_key = signing_key.verifying_key();
        Ok(eth_address_from_pubkey(verifying_key))
    }

    /// Sign an intent using the private key stored under `secret_name`.
    pub fn sign(
        &self,
        secret_name: &str,
        intent: &SignIntent,
    ) -> Result<SignedResult, SecretError> {
        let secret = self.vault.get(secret_name)?;
        let signing_key = parse_private_key(secret.expose())?;
        let verifying_key = signing_key.verifying_key();
        let address = eth_address_from_pubkey(verifying_key);

        let digest = compute_signing_digest(intent);
        let (signature, recovery_id) = signing_key
            .sign_prehash_recoverable(&digest)
            .map_err(|e| {
                SecretError::EncryptionFailed(format!("ECDSA signing failed: {}", e))
            })?;

        Ok(SignedResult {
            signature: signature.to_bytes().to_vec(),
            recovery_id: recovery_id.to_byte(),
            signer_address: address,
        })
    }
}

fn parse_private_key(hex_key: &str) -> Result<SigningKey, SecretError> {
    let key_str = hex_key.strip_prefix("0x").unwrap_or(hex_key);
    let key_bytes = hex::decode(key_str).map_err(|e| {
        SecretError::EncryptionFailed(format!("Invalid hex private key: {}", e))
    })?;
    SigningKey::from_bytes((&key_bytes[..]).into()).map_err(|e| {
        SecretError::EncryptionFailed(format!("Invalid secp256k1 private key: {}", e))
    })
}

fn eth_address_from_pubkey(pubkey: &VerifyingKey) -> [u8; 20] {
    let encoded = pubkey.to_encoded_point(false);
    let pubkey_bytes = &encoded.as_bytes()[1..]; // skip 0x04 prefix
    let hash = keccak256(pubkey_bytes);
    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[12..]);
    address
}

fn compute_signing_digest(intent: &SignIntent) -> [u8; 32] {
    match intent {
        SignIntent::PersonalSign { message } => {
            let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
            let mut data = prefix.into_bytes();
            data.extend_from_slice(message);
            keccak256(&data)
        }
        SignIntent::TypedData {
            domain_hash,
            struct_hash,
        } => {
            let mut data = vec![0x19, 0x01];
            data.extend_from_slice(domain_hash);
            data.extend_from_slice(struct_hash);
            keccak256(&data)
        }
        SignIntent::Transaction {
            chain_id,
            to,
            value,
            data,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        } => {
            let mut payload = Vec::new();
            payload.push(0x02); // EIP-1559 type
            payload.extend_from_slice(&chain_id.to_be_bytes());
            payload.extend_from_slice(&nonce.to_be_bytes());
            payload.extend_from_slice(&max_priority_fee_per_gas.to_be_bytes());
            payload.extend_from_slice(&max_fee_per_gas.to_be_bytes());
            payload.extend_from_slice(&gas_limit.to_be_bytes());
            payload.extend_from_slice(to);
            payload.extend_from_slice(value);
            payload.extend_from_slice(data);
            keccak256(&payload)
        }
    }
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut output = [0u8; 32];
    output.copy_from_slice(&result);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::types::EntryMetadata;
    use tempfile::TempDir;

    // Hardhat default account #0
    const TEST_PRIVATE_KEY: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn test_vault_with_key(dir: &TempDir) -> SecretVault {
        let path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&path, "test-master").unwrap();
        vault
            .set(
                "wallet_main",
                TEST_PRIVATE_KEY,
                EntryMetadata {
                    description: Some("Test wallet".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        vault
    }

    #[test]
    fn test_get_address() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let address = signer.get_address("wallet_main").unwrap();
        // Hardhat account #0 address: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
        let expected = hex::decode("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();
        assert_eq!(address, expected.as_slice());
    }

    #[test]
    fn test_personal_sign() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::PersonalSign {
            message: b"Hello Aleph".to_vec(),
        };
        let result = signer.sign("wallet_main", &intent).unwrap();

        assert_eq!(result.signature.len(), 64);
        assert!(result.recovery_id <= 1);
        let expected_addr =
            hex::decode("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();
        assert_eq!(result.signer_address, expected_addr.as_slice());
    }

    #[test]
    fn test_typed_data_sign() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::TypedData {
            domain_hash: [0xAA; 32],
            struct_hash: [0xBB; 32],
        };
        let result = signer.sign("wallet_main", &intent).unwrap();
        assert_eq!(result.signature.len(), 64);
    }

    #[test]
    fn test_transaction_sign() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::Transaction {
            chain_id: 1,
            to: [0x42; 20],
            value: [0; 32],
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
        };
        let result = signer.sign("wallet_main", &intent).unwrap();
        assert_eq!(result.signature.len(), 64);
    }

    #[test]
    fn test_sign_nonexistent_key_fails() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::PersonalSign {
            message: b"test".to_vec(),
        };
        let result = signer.sign("nonexistent", &intent);
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[test]
    fn test_sign_invalid_key_fails() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.vault");
        let mut vault = SecretVault::open(&path, "master").unwrap();
        vault
            .set("bad_key", "not-a-valid-hex-key", EntryMetadata::default())
            .unwrap();

        let signer = EvmSigner::new(&vault);
        let intent = SignIntent::PersonalSign {
            message: b"test".to_vec(),
        };
        let result = signer.sign("bad_key", &intent);
        assert!(result.is_err());
    }

    #[test]
    fn test_debug_never_shows_private_key() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::PersonalSign {
            message: b"test".to_vec(),
        };
        let result = signer.sign("wallet_main", &intent).unwrap();

        let debug_str = format!("{:?}", result);
        assert!(!debug_str.contains("ac0974bec39a"));
        assert!(debug_str.contains("SignedResult"));
    }

    #[test]
    fn test_display_never_shows_private_key() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::PersonalSign {
            message: b"test".to_vec(),
        };
        let result = signer.sign("wallet_main", &intent).unwrap();

        let display_str = format!("{}", result);
        assert!(!display_str.contains("ac0974bec39a"));
    }
}
