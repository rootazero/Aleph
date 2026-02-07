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
pub mod activity_log;
pub mod activity_logger;
pub mod guest_session_manager;
pub mod identity_map;
pub mod invitation_manager;
pub mod pairing;
pub mod policy_engine;
pub mod store;
pub mod token;

// Re-export commonly used types
pub use crypto::{
    generate_keypair, generate_pairing_code, generate_secret, hmac_sign, hmac_verify,
    sign_message, verify_signature, CryptoError, DeviceFingerprint, PAIRING_CODE_CHARSET,
    PAIRING_CODE_LENGTH,
};
pub use device::{Device, DeviceRole, DeviceType};
pub use activity_log::{
    ActivityLogQuery, ActivityLogQueryResult, ActivityStatus, ActivityType, GuestActivityLog,
};
pub use activity_logger::GuestActivityLogger;
pub use guest_session_manager::{GuestSession, GuestSessionError, GuestSessionManager};
pub use identity_map::{IdentityMap, PlatformIdentity, UserId};
pub use invitation_manager::{InvitationError, InvitationManager};
pub use pairing::{PairingError, PairingManager, PairingRequest};
pub use policy_engine::{PermissionResult, PolicyEngine};
pub use store::{DeviceRow, PairingRequestRow, SecurityStore, TokenRow};
pub use token::{SignedToken, TokenError, TokenManager, TokenValidation};
