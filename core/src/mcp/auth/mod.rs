//! OAuth Authentication for MCP
//!
//! Provides OAuth 2.0 authentication for remote MCP servers.
//!
//! # Components
//!
//! - **Storage** ([`OAuthStorage`]): Secure credential storage
//! - **Provider** (coming soon): OAuth flow implementation
//! - **Callback** (coming soon): Authorization code callback server
//!
//! # Usage
//!
//! ```ignore
//! use aether_core::mcp::auth::{OAuthStorage, OAuthTokens};
//!
//! // Create storage
//! let storage = OAuthStorage::new(OAuthStorage::default_path());
//!
//! // Check for existing tokens
//! if let Some(tokens) = storage.get_tokens("my-server").await? {
//!     if tokens.is_expired() && tokens.can_refresh() {
//!         // Refresh the token
//!     } else if !tokens.is_expired() {
//!         // Use the token
//!     }
//! }
//! ```

mod storage;

pub use storage::{ClientInfo, OAuthEntry, OAuthStorage, OAuthTokens};
