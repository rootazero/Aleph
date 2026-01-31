//! Security Module
//!
//! Provides authentication and authorization for Gateway connections.

pub mod crypto;
pub mod token;
pub mod pairing;

pub use crypto::{
    generate_keypair, generate_pairing_code, generate_secret, hmac_sign, hmac_verify, sign_message,
    verify_signature, CryptoError, DeviceFingerprint, PAIRING_CODE_CHARSET, PAIRING_CODE_LENGTH,
};
pub use token::TokenManager;
pub use pairing::PairingManager;
