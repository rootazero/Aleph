// core/src/gateway/security/mod.rs

//! Security Module
//!
//! Provides authentication and authorization for Gateway connections.
//!
//! ## Architecture
//!
//! ```text
//! SecurityManager (unified entry point)
//!   ├── TokenManager (HMAC-signed tokens)
//!   ├── PairingManager (8-char Base32 codes)
//!   └── DeviceRegistry (Ed25519 public keys)
//!          │
//!          ▼
//!     SecurityStore (SQLite)
//! ```

pub mod crypto;
pub mod device;
pub mod identity_map;
pub mod pairing;
pub mod store;
pub mod token;

// Re-export commonly used types
pub use crypto::{
    generate_keypair, generate_pairing_code, generate_secret, hmac_sign, hmac_verify,
    sign_message, verify_signature, CryptoError, DeviceFingerprint, PAIRING_CODE_CHARSET,
    PAIRING_CODE_LENGTH,
};
pub use device::{Device, DeviceRole, DeviceType};
pub use identity_map::{IdentityMap, PlatformIdentity, UserId};
pub use pairing::{PairingError, PairingManager, PairingRequest};
pub use store::{DeviceRow, PairingRequestRow, SecurityStore, TokenRow};
pub use token::{SignedToken, TokenError, TokenManager, TokenValidation};
