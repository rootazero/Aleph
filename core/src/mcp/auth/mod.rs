//! OAuth Authentication for MCP
//!
//! Provides OAuth 2.0 authentication for remote MCP servers.
//!
//! # Components
//!
//! - **Storage** ([`OAuthStorage`]): Secure credential storage
//! - **Provider** ([`OAuthProvider`]): OAuth flow implementation
//! - **Callback** (coming soon): Authorization code callback server
//!
//! # Usage
//!
//! ```ignore
//! use alephcore::mcp::auth::{OAuthStorage, OAuthTokens, OAuthProvider};
//!
//! // Create storage
//! let storage = Arc::new(OAuthStorage::new(OAuthStorage::default_path()));
//!
//! // Create provider
//! let provider = OAuthProvider::new(
//!     storage.clone(),
//!     "my-server",
//!     "https://api.example.com",
//!     "http://localhost:19877/callback",
//! );
//!
//! // Start authorization flow
//! let metadata = provider.discover_metadata().await?;
//! let auth_req = provider.start_authorization(&metadata, "client_id", None).await?;
//! println!("Open in browser: {}", auth_req.authorization_url);
//!
//! // After user authorizes, exchange code for tokens
//! let tokens = provider.finish_authorization(&metadata, "client_id", &code, &state).await?;
//! ```

mod callback;
mod provider;
mod refresh;
mod storage;

pub use callback::{CallbackResult, CallbackServer, DEFAULT_CALLBACK_PORT, DEFAULT_CALLBACK_TIMEOUT};
pub use provider::{AuthorizationRequest, OAuthProvider, OAuthServerMetadata};
pub use refresh::{TokenRefreshConfig, TokenRefreshManager};
pub use storage::{ClientInfo, OAuthEntry, OAuthStorage, OAuthTokens};
