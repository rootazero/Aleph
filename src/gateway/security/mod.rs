// core/src/gateway/security/mod.rs

//! Security Module — secret vault + cluster device records.
//!
//! The device-auth layer (auth tokens, HTTP sessions, device pairing,
//! brute-force tracking, guest sessions, invitations, policy engine,
//! identity map, activity logs) was removed in the LAN-trust
//! architecture revert (2026-06). What remains:
//!
//! ```text
//! SharedTokenManager (vault manager; token doubles as vault master key)
//!   ├── SecretVault (encrypted secrets file)
//!   └── SecurityStore (SQLite)
//! crypto (HMAC / Ed25519 / pairing-code primitives — consumed by
//!         secrets vault, Feishu webhook, WhatsApp vault store)
//! token_readonly (read-only shared-token lookup for the admin IPC client)
//! artifact_caps (session-scoped capabilities for the /artifact byte route)
//! ```
//!
//! `SecurityStore` persists the vault master key / HMAC secret **plus**
//! cluster-node device records — the `devices` CRUD is alive and
//! load-bearing for cluster federation (`handlers/cluster.rs`, WS node
//! enrollment) — as well as approved senders, channel DM policies, and
//! the security audit log. See `store/mod.rs` for per-table status,
//! including the legacy tables kept only for migration compatibility.

pub mod artifact_caps;
pub mod crypto;
pub mod device_token_manager;
pub mod shared_token;
pub mod store;
pub mod token_readonly;

// Re-export commonly used types
pub use artifact_caps::ArtifactCapabilities;
pub use crypto::{
    generate_keypair, generate_secret, hmac_sign, hmac_verify, sign_message, verify_signature,
    CryptoError, DeviceFingerprint,
};
pub use device_token_manager::{DeviceTokenError, DeviceTokenManager, PANEL_DEVICE_TYPE};
pub use shared_token::{SharedTokenError, SharedTokenManager};
pub use store::{DeviceUpsertData, SecurityStore};
pub use token_readonly::read_current_token_readonly;
