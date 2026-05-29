//! Slack API Operations
//!
//! Low-level functions for interacting with the Slack Web API and Socket Mode.
//! These are separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ChannelResult, ConversationId, InboundMessage, InboundMessageSender,
    MessageId, MessageMeta, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use crate::sync_primitives::Arc;
use chrono::Utc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::config::SlackConfig;
use super::directory::UserDirectory;

const SLACK_API_BASE: &str = "https://slack.com/api";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Slack message length limit (characters).
pub(crate) const SLACK_MSG_LIMIT: usize = 3000;

mod api;
mod converter;
mod debouncer;
mod files;
mod socket;

#[cfg(test)]
mod tests;

pub use api::SlackMessageOps;
pub(crate) use debouncer::SlackDebouncer;
