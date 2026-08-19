//! `channel_outbox` — inspect and recover the durable outbound delivery queue.
//!
//! # Why this exists as a tool
//!
//! The queue in [`crate::gateway::delivery_queue`] is a three-stage trail:
//! `record_dead_letter` (preserve what was lost) → `recent_dead_letters` (read
//! it back) → `redrive_dead_letters` (send it again). Stages two and three were
//! built, tested, and then left with **no production caller at all** — the
//! `channels.dead_letters` / `channels.redrive_dead_letters` RPCs that consumed
//! them were removed as orphans (no client ever called them), which was the
//! right call for the RPC and the wrong outcome for the feature: the pipe was
//! cut but the plumbing behind it kept running. An operator could see the
//! *count* of permanently-undelivered pushes in `channels.list` and had no way
//! to learn which ones, to whom, or to get them back.
//!
//! Re-registering the RPCs would recreate the same dead surface. The consumer
//! that actually exists is the model (R8: everything Aleph can do about itself
//! is a tool), and the question is one a user genuinely asks in natural
//! language — *"did my message ever go out?"* — so this is where the trail
//! terminates.
//!
//! # Not read-only, on purpose
//!
//! `channel_directory` documents why reads are split off into their own tool
//! name: `ToolFacts::idempotent` is keyed on the tool NAME, so folding a read
//! into a mutating tool gates it under the `Ask` exec tier. This tool keeps
//! `status` / `dead_letters` next to `redrive` anyway, because splitting them
//! would put a recovery action and the *only* way to decide whether to run it
//! in two different tools — and the numbers that make the read cheap
//! (`pending`, `dead_lettered`) are already in `channels.list`, un-gated, for
//! the Panel. The trade-off: under `Ask`, listing dead letters prompts.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Ceiling on returned dead letters. Clamped at the boundary rather than
/// trusted from the model: each entry carries a text preview, and an unbounded
/// list of a wedged channel's backlog would be persisted-to-disk by the central
/// result budget instead of answered inline.
const MAX_LIMIT: usize = 25;
const DEFAULT_LIMIT: usize = 10;
/// Dead-letter text is echoed only as a preview. The point is recognition
/// ("that's the release note I asked for"), not recovering the payload — the
/// payload is still in the queue, and redrive replays it verbatim.
const PREVIEW_CHARS: usize = 200;

/// What to do with the outbound queue.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutboxAction {
    /// Queue depth, oldest backlog age, per-channel breakdown, dead-letter
    /// counts. Answers "is anything stuck?".
    Status,
    /// The most recent permanently-undelivered messages: channel, recipient,
    /// text preview, why it stopped being retried, and whether a redrive is
    /// safe. Answers "what was lost, and to whom?".
    DeadLetters,
    /// Move duplicate-safe dead letters back into the live queue; the drain
    /// task replays them on its next tick. Answers "the channel is back — send
    /// them now".
    Redrive,
}

/// Arguments for the `channel_outbox` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ChannelOutboxArgs {
    /// What to do.
    pub action: OutboxAction,

    /// Restrict to one channel (e.g. "telegram"). Omit for all channels.
    /// Applies to `dead_letters` and `redrive`.
    #[serde(default)]
    pub channel_id: Option<String>,

    /// Maximum dead letters to return (default 10, hard cap 25).
    #[serde(default)]
    pub limit: Option<usize>,

    // WHY (does not ship — a `//` comment is absent from the schemars schema,
    // while this argument's doc is sent to the model on every request that can
    // see this tool): a redrive replays permanently-failed outbound messages
    // verbatim to their target conversation, so a mistaken confirm turns a
    // dead-letter list into duplicate customer-visible messages. The model
    // needs the mechanics below to call the tool correctly; it does not need
    // the incident story in order to obey a required flag. Trimmed from seven
    // doc lines to two when `registry_schema_bytes_ratchet` caught the 510 B —
    // the same treatment the ledger on `REGISTRY_SCHEMA_CEILING_BYTES` records
    // for `loop_graph`'s variant doc.
    /// `redrive` only proceeds when this is `true`; omitted returns a preview
    /// of what would be redriven. Ignored by `status` and `dead_letters`.
    #[serde(default)]
    pub confirm_redrive: Option<bool>,
}

/// One permanently-undelivered outbound message, as the model sees it.
#[derive(Debug, Clone, Serialize)]
pub struct DeadLetterEntry {
    /// Transport it could not be delivered on.
    pub channel_id: String,
    /// Opaque conversation id it was addressed to.
    pub conversation_id: String,
    /// First [`PREVIEW_CHARS`] characters of the message text.
    pub preview: String,
    /// Delivery attempts made before it was retired.
    pub attempts: u32,
    /// Last transport error observed.
    pub last_error: String,
    /// Why retrying stopped: `exhausted` | `ambiguous` | `permanent` |
    /// `unknown_outcome` | `payload_too_large`.
    pub reason: String,
    /// `true` when a redrive provably cannot double-send this message. `false`
    /// means the outcome of the last attempt is unknown — `redrive` will not
    /// touch it, and resending is a judgement call for the user.
    pub replay_safe: bool,
    /// Seconds since the message was first queued.
    pub age_secs: i64,
}

/// Output of the `channel_outbox` tool. Every field is optional so one shape
/// serves all three actions without inventing zeroes for facts not requested.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelOutboxOutput {
    /// Human-readable summary of what happened.
    pub message: String,
    /// `false` when no durable queue is configured — nothing is being retried
    /// at all, which is a different answer from "nothing is stuck".
    pub durable_queue: bool,
    /// `status`: live queue depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<i64>,
    /// `status`: subset of `pending` already due for another attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_now: Option<i64>,
    /// `status`: age of the oldest queued message, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_age_secs: Option<i64>,
    /// `status`: pending count per channel, busiest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_channel: Option<Vec<(String, i64)>>,
    /// `status`: total dead letters retained.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead_lettered: Option<i64>,
    /// `status`: how many of those a redrive would actually replay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead_lettered_replayable: Option<i64>,
    /// `dead_letters`: the entries themselves, newest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead_letters: Option<Vec<DeadLetterEntry>>,
    /// `redrive`: messages moved back into the live queue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redriven: Option<u64>,
    /// `redrive`: left behind because the live queue is full — retry later.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_queue_full: Option<u64>,
    /// `redrive`: left behind because replaying them could double-send.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_not_replay_safe: Option<u64>,
}

impl ChannelOutboxOutput {
    fn blank(message: impl Into<String>, durable_queue: bool) -> Self {
        Self {
            message: message.into(),
            durable_queue,
            pending: None,
            due_now: None,
            oldest_age_secs: None,
            per_channel: None,
            dead_lettered: None,
            dead_lettered_replayable: None,
            dead_letters: None,
            redriven: None,
            skipped_queue_full: None,
            skipped_not_replay_safe: None,
        }
    }
}

/// Tool for inspecting and recovering the durable outbound delivery queue.
#[derive(Clone)]
pub struct ChannelOutboxTool {
    registry: Arc<ChannelRegistry>,
}

impl ChannelOutboxTool {
    #[must_use]
    pub const fn new(registry: Arc<ChannelRegistry>) -> Self {
        Self { registry }
    }

    fn status(&self) -> ChannelOutboxOutput {
        let Some(stats) = self.registry.delivery_queue_stats() else {
            return ChannelOutboxOutput::blank(
                "No durable delivery queue is configured, so failed outbound sends are \
                 not retried or retained. Nothing to inspect.",
                false,
            );
        };
        let mut out = ChannelOutboxOutput::blank(
            format!(
                "{} message(s) queued for retry ({} due now), {} dead-lettered ({} redrivable).",
                stats.pending, stats.due_now, stats.dead_lettered, stats.dead_lettered_replayable
            ),
            true,
        );
        out.pending = Some(stats.pending);
        out.due_now = Some(stats.due_now);
        out.oldest_age_secs = stats.oldest_age_secs;
        out.per_channel = Some(stats.per_channel);
        out.dead_lettered = Some(stats.dead_lettered);
        out.dead_lettered_replayable = Some(stats.dead_lettered_replayable);
        out
    }

    fn dead_letters(&self, channel: Option<&str>, limit: usize) -> ChannelOutboxOutput {
        // Fetch the cap regardless of the channel filter, then narrow: the
        // store indexes by died_at, not by channel, and a per-channel view of a
        // handful of rows is not worth a second query path.
        let Some(letters) = self.registry.recent_dead_letters(MAX_LIMIT) else {
            return ChannelOutboxOutput::blank(
                "No durable delivery queue is configured, so no dead letters are retained.",
                false,
            );
        };
        let now = crate::gateway::delivery_queue::now_secs();
        let entries: Vec<DeadLetterEntry> = letters
            .into_iter()
            .filter(|dl| channel.is_none_or(|c| dl.channel_id == c))
            .take(limit)
            .map(|dl| DeadLetterEntry {
                channel_id: dl.channel_id,
                conversation_id: dl.message.conversation_id.as_str().to_string(),
                preview: dl.message.text.chars().take(PREVIEW_CHARS).collect(),
                attempts: dl.attempts,
                last_error: dl.last_error,
                reason: dl.reason.as_str().to_string(),
                replay_safe: dl.reason.replay_safe(),
                age_secs: (now - dl.created_at).max(0),
            })
            .collect();

        let mut out = ChannelOutboxOutput::blank(
            if entries.is_empty() {
                "No permanently-undelivered messages.".to_string()
            } else {
                format!("{} permanently-undelivered message(s).", entries.len())
            },
            true,
        );
        out.dead_letters = Some(entries);
        out
    }

    fn redrive(&self, channel: Option<&str>) -> ChannelOutboxOutput {
        let Some(outcome) = self.registry.redrive_dead_letters(channel) else {
            return ChannelOutboxOutput::blank(
                "No durable delivery queue is configured, so there is nothing to redrive.",
                false,
            );
        };
        let mut out = ChannelOutboxOutput::blank(
            format!(
                "Requeued {} message(s) for delivery; {} left (live queue full), \
                 {} left (outcome unknown — resending could duplicate).",
                outcome.moved, outcome.skipped_capacity, outcome.skipped_unsafe
            ),
            true,
        );
        out.redriven = Some(outcome.moved);
        out.skipped_queue_full = Some(outcome.skipped_capacity);
        out.skipped_not_replay_safe = Some(outcome.skipped_unsafe);
        out
    }
}

#[async_trait]
impl AlephTool for ChannelOutboxTool {
    const NAME: &'static str = "channel_outbox";
    const DESCRIPTION: &'static str = concat!(
        "Inspect and recover Aleph's durable outbound delivery queue — the ",
        "messages it tried to push to a chat channel (Telegram, Slack, Discord, ",
        "...) but could not deliver. Use it when the user asks whether something ",
        "was actually sent, why a notification never arrived, or to resend what ",
        "was lost after a channel came back.\n\n",
        "action=status: queue depth, oldest backlog age, which channel is backed ",
        "up, and how many messages were given up on.\n",
        "action=dead_letters: the given-up messages themselves — recipient, text ",
        "preview, attempts, last error, and why retrying stopped.\n",
        "action=redrive: put them back in the queue for another delivery pass.\n\n",
        "Redrive only replays failures that provably never reached the ",
        "recipient. Entries with replay_safe=false (reason 'ambiguous' or ",
        "'unknown_outcome') may already have been delivered, so they are left ",
        "alone and reported as skipped_not_replay_safe — tell the user and let ",
        "them decide rather than resending silently."
    );

    type Args = ChannelOutboxArgs;
    type Output = ChannelOutboxOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let channel = args
            .channel_id
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty());
        if let Some(c) = channel {
            // A typo'd channel id must not read as "nothing was lost".
            if self
                .registry
                .get(&crate::gateway::channel::ChannelId::new(c))
                .await
                .is_none()
            {
                return Err(AlephError::tool(format!(
                    "unknown channel '{c}' — call channels list first, or omit channel_id to \
                     cover every channel"
                )));
            }
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        Ok(match args.action {
            OutboxAction::Status => self.status(),
            OutboxAction::DeadLetters => self.dead_letters(channel, limit),
            OutboxAction::Redrive => {
                // BT-C-R4-07: require explicit confirmation before redrive
                // replays dead letters. The previous shape used the same
                // single-arg dispatch as `dead_letters`, so a model could
                // pass action='redrive' with no confirmation and the loop
                // would silently send duplicate customer-visible messages
                // — a one-call DoS for any channel whose outbound queue
                // has accumulated a few failures. Two-step preview-then-
                // confirm pattern: when confirm_redrive is not set or
                // false, return a `status`-shaped preview that names the
                // dead letters that *would* be replayed and tells the
                // caller to re-issue with confirm_redrive=true.
                if !args.confirm_redrive.unwrap_or(false) {
                    let preview = self.status();
                    let preview_count = preview.dead_lettered.unwrap_or(0);
                    let preview_msg = if preview_count == 0 {
                        "redrive preview: no dead letters to replay; nothing would be sent. \
                         Pass confirm_redrive=true to issue the (no-op) replay anyway."
                            .to_string()
                    } else {
                        format!(
                            "redrive preview: {} dead letter(s) would be replayed. \
                             Pass confirm_redrive=true to proceed; pass action='dead_letters' \
                             first to inspect the entries.",
                            preview_count
                        )
                    };
                    return Ok(ChannelOutboxOutput::blank(
                        preview_msg,
                        preview.durable_queue,
                    ));
                }
                self.redrive(channel)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_store_every_action_says_so_instead_of_reporting_zero() {
        // "No queue configured" and "queue configured, nothing stuck" are
        // different answers; reporting 0 for the first would tell the user
        // their pushes are fine when nothing is being retained at all.
        let tool = ChannelOutboxTool::new(Arc::new(ChannelRegistry::new()));
        for action in [
            OutboxAction::Status,
            OutboxAction::DeadLetters,
            OutboxAction::Redrive,
        ] {
            let out = tool
                .call(ChannelOutboxArgs {
                    action,
                    channel_id: None,
                    limit: None,
                    // The redrive arm gates on this; `None` is the shape a
                    // caller that has not confirmed sends, which is what the
                    // no-store assertion below is about.
                    confirm_redrive: None,
                })
                .await
                .expect("no store is not an error");
            assert!(!out.durable_queue);
            assert!(out.pending.is_none());
            assert!(out.dead_letters.is_none());
            assert!(out.redriven.is_none());
        }
    }

    #[tokio::test]
    async fn unknown_channel_is_rejected_rather_than_answered_empty() {
        let tool = ChannelOutboxTool::new(Arc::new(ChannelRegistry::new()));
        let err = tool
            .call(ChannelOutboxArgs {
                action: OutboxAction::DeadLetters,
                channel_id: Some("teelgram".to_string()),
                limit: None,
                confirm_redrive: None,
            })
            .await
            .expect_err("a typo'd channel must not look like an empty result");
        assert!(err.to_string().contains("unknown channel"));
    }
}
