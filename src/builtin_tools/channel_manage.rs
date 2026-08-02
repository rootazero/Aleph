//! Channel pairing management tool — generate and list pairing codes.
//!
//! Exposes channel pairing operations as LLM-callable tools (R9 compliance).
//! The LLM can generate a pairing code or list active codes via natural language.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::channel::{ChannelId, PairingData};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args
// =============================================================================

/// Action to perform on channel pairing
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PairingAction {
    /// Generate a new pairing code for a channel
    Generate,
    /// List all active (non-expired) pairing codes for a channel
    List,
}

/// Arguments for the `channel_pairing` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ChannelPairingArgs {
    /// Action to perform
    pub action: PairingAction,

    /// Channel ID to operate on (e.g., "telegram").
    /// If omitted, defaults to the first Telegram channel found.
    #[serde(default)]
    pub channel_id: Option<String>,
}

// =============================================================================
// Output
// =============================================================================

/// A single active pairing code entry
#[derive(Debug, Clone, Serialize)]
pub struct PairingCodeEntry {
    /// The pairing code
    pub code: String,
    /// Remaining time-to-live in seconds
    pub remaining_ttl_secs: u64,
}

/// Output from `channel_pairing` tool
#[derive(Debug, Clone, Serialize)]
pub struct ChannelPairingOutput {
    /// Human-readable status message
    pub message: String,
    /// Generated pairing code (for generate action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Active pairing codes (for list action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codes: Option<Vec<PairingCodeEntry>>,
    /// Channel ID that was operated on
    pub channel_id: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool for managing channel pairing codes via natural language.
///
/// Supports generating new pairing codes and listing active ones.
/// Requires access to the `ChannelRegistry` to find and interact with channels.
#[derive(Clone)]
pub struct ChannelPairingTool {
    channel_registry: Arc<ChannelRegistry>,
}

impl ChannelPairingTool {
    pub const fn new(channel_registry: Arc<ChannelRegistry>) -> Self {
        Self { channel_registry }
    }

    /// Resolve the target channel ID.
    /// If provided, use it directly. Otherwise, find the first telegram channel.
    async fn resolve_channel_id(&self, requested: Option<String>) -> Result<ChannelId> {
        if let Some(id) = requested {
            return Ok(ChannelId::new(id));
        }

        // Find first telegram channel
        let telegram_channels = self.channel_registry.list_by_type("telegram").await;
        if let Some(ch) = telegram_channels.first() {
            return Ok(ch.id.clone());
        }

        Err(crate::error::AlephError::tool(
            "No Telegram channel found. Please specify a channel_id or configure a Telegram channel.",
        ))
    }
}

#[async_trait]
impl AlephTool for ChannelPairingTool {
    const NAME: &'static str = "channel_pairing";
    const DESCRIPTION: &'static str =
        "Manage channel pairing codes. Generate a new pairing code so a user can link \
         their Telegram account, or list currently active pairing codes. Use this when \
         the user asks to pair a new device/account or wants to see existing pairing codes.";

    type Args = ChannelPairingArgs;
    type Output = ChannelPairingOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let channel_id = self.resolve_channel_id(args.channel_id).await?;

        let channel_handle = self
            .channel_registry
            .get(&channel_id)
            .await
            .ok_or_else(|| {
                crate::error::AlephError::tool(format!(
                    "Channel '{channel_id}' not found in registry"
                ))
            })?;

        match args.action {
            PairingAction::Generate => {
                let channel = channel_handle.read().await;
                let pairing_data = channel.get_pairing_data().await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to generate pairing code: {e}"))
                })?;

                match pairing_data {
                    PairingData::Code(code) => {
                        info!(channel = %channel_id, code = %code, "Pairing code generated via tool");
                        Ok(ChannelPairingOutput {
                            message: format!(
                                "Pairing code generated for channel '{channel_id}': {code}. \
                                 The user should send this code to the Telegram bot in a DM. \
                                 Code expires in 1 hour."
                            ),
                            code: Some(code),
                            codes: None,
                            channel_id: channel_id.to_string(),
                        })
                    }
                    PairingData::QrCode(qr) => Ok(ChannelPairingOutput {
                        message: format!("QR code generated for channel '{channel_id}'"),
                        code: Some(qr),
                        codes: None,
                        channel_id: channel_id.to_string(),
                    }),
                    PairingData::None => Err(crate::error::AlephError::tool(format!(
                        "Channel '{channel_id}' does not support pairing"
                    ))),
                }
            }

            PairingAction::List => {
                let channel = channel_handle.read().await;
                let active_codes = channel.list_active_pairing_codes().await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to list pairing codes: {e}"))
                })?;

                let entries: Vec<PairingCodeEntry> = active_codes
                    .into_iter()
                    .map(|(code, remaining)| PairingCodeEntry {
                        code,
                        remaining_ttl_secs: remaining,
                    })
                    .collect();

                let count = entries.len();
                info!(channel = %channel_id, count = count, "Listed pairing codes via tool");

                Ok(ChannelPairingOutput {
                    message: if count == 0 {
                        format!("No active pairing codes for channel '{channel_id}'.")
                    } else {
                        format!("{count} active pairing code(s) for channel '{channel_id}'.")
                    },
                    code: None,
                    codes: Some(entries),
                    channel_id: channel_id.to_string(),
                })
            }
        }
    }
}
