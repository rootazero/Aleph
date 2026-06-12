// core/src/gateway/security/mod.rs

//! Security Module — vault + master-key persistence.
//!
//! The device-auth machinery (tokens, pairing, devices, brute-force,
//! guest sessions, invitations, policy engine, identity map, activity
//! logs) was removed in the LAN-trust architecture revert (2026-06).
//! What remains is the encrypted secret vault and its supporting
//! infrastructure:
//!
//! ```text
//! SharedTokenManager (vault manager; token doubles as vault master key)
//!   ├── SecretVault (encrypted secrets file)
//!   └── SecurityStore (SQLite: master-key/HMAC persistence)
//! crypto (HMAC / Ed25519 / pairing-code primitives — consumed by
//!         secrets vault, Feishu webhook, WhatsApp vault store)
//! token_readonly (read-only shared-token lookup for the admin IPC client)
//! ```

pub mod crypto;
pub mod shared_token;
pub mod store;
pub mod token_readonly;

// Re-export commonly used types
pub use crypto::{
    generate_keypair, generate_pairing_code, generate_secret, hmac_sign, hmac_verify, sign_message,
    verify_signature, CryptoError, DeviceFingerprint, PAIRING_CODE_CHARSET, PAIRING_CODE_LENGTH,
};
pub use shared_token::{SharedTokenError, SharedTokenManager};
pub use store::{DeviceUpsertData, SecurityStore};
pub use token_readonly::read_current_token_readonly;
