//! Borrowed browser port and pure value guards.
//!
//! Claims and matching reply tags do not confer authority or verify native
//! effects. Callers must compose these checks with Core authority checks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::error::BrowserError;
use super::operations::{BrowserFeature, BrowserOperation, BrowserReply};

/// Owned request values; the profile is a name, not a permission or lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRequest {
    pub profile: String,
    pub operation: BrowserOperation,
}

/// Declared method support, without authorization or native verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserCapabilities {
    pub supported: Vec<BrowserFeature>,
}

/// Lossless typed failures; wrapping an error does not create authority.
#[derive(Debug, thiserror::Error)]
pub enum BrowserContractError {
    #[error("{0}")]
    Native(#[from] BrowserError),

    #[error("browser request {}: {reason}", .by.head())]
    Refused {
        by: crate::tool::RefusedBy,
        reason: String,
    },

    #[error("unsupported browser feature: {}", .0.method_name())]
    Unsupported(BrowserFeature),

    #[error("{0}")]
    Aleph(#[from] crate::error::AlephError),
}

/// A pure port distinct from the native browser backend trait.
#[async_trait]
pub trait BrowserManagerContract: Send + Sync {
    fn capabilities(&self) -> BrowserCapabilities;

    async fn execute(&self, request: &BrowserRequest)
        -> Result<BrowserReply, BrowserContractError>;
}

/// Require every declared feature, returning the first missing one.
///
/// Empty requirements succeed. Duplicates do not grant additional authority.
pub fn validate_browser_claim(
    required: &[BrowserFeature],
    offered: &BrowserCapabilities,
) -> Result<(), BrowserContractError> {
    for feature in required {
        if !offered.supported.contains(feature) {
            return Err(BrowserContractError::Unsupported(*feature));
        }
    }
    Ok(())
}

/// Reject the native encoder injection seam before a port request is executed.
///
/// Other native policy and authority checks belong to Core.
pub fn validate_browser_request(request: &BrowserRequest) -> Result<(), BrowserContractError> {
    if let BrowserOperation::RecordStart { options, .. } = &request.operation {
        if options.ffmpeg.is_some() {
            return Err(crate::error::AlephError::Validation(
                "record_start: ffmpeg override is not allowed through the browser contract".into(),
            )
            .into());
        }
    }
    Ok(())
}

/// Validate only the exact operation/reply tag pairing, never native effects.
pub fn validate_browser_reply(
    operation: &BrowserOperation,
    reply: &BrowserReply,
) -> Result<(), BrowserContractError> {
    let expected = operation.feature();
    let actual = reply.feature();
    if expected != actual {
        return Err(crate::error::AlephError::Validation(format!(
            "browser reply mismatch: expected {}, actual {}",
            expected.method_name(),
            actual.method_name(),
        ))
        .into());
    }
    Ok(())
}
