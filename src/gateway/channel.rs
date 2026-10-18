//! Channel compatibility exports; pure contracts live in `aleph_slot_api`.
//! Core keeps channel paradigm selection and outbound chunk policy.

pub use aleph_slot_api::channel::*;

/// Chunk size for a channel that declares no cap
/// ([`ChannelCapabilities::max_message_length`] `== 0`). Not a limit anyone
/// asserts — just a bound so one enormous answer is never handed to a transport
/// in a single frame.
pub(crate) const DEFAULT_OUTBOUND_CHUNK_LEN: usize = 4000;

/// How long one outbound message may be for a channel that declared
/// `max_message_length`.
///
/// **The single answer for every outbound chunker.** Both the `ReplyEmitter`'s
/// text chokepoint and the origin fan-out used to hold their own `4000` — a
/// second answer to a question the capability already answers, and wrong for
/// every channel with a smaller cap. Discord declares 2000 and its
/// `Channel::send` does no splitting of its own, so an oversized frame came
/// back as `SendFailed`, which the durable queue refuses: the whole answer was
/// lost with one log line.
///
/// The cap is declared in characters and the splitter counts bytes. That is
/// left as-is and stated rather than converted: bytes ≥ chars in UTF-8, so
/// byte-splitting a character cap can only cut *earlier* than required. Erring
/// the other way would put us back over the limit.
#[must_use]
pub(crate) const fn outbound_chunk_len(declared: usize) -> usize {
    if declared == 0 {
        DEFAULT_OUTBOUND_CHUNK_LEN
    } else {
        declared
    }
}

/// Core-only private SMS operations. Metadata and arbitrary `Any` payloads
/// confer no authority; only the concrete Core adapter delegates to its live
/// transport's existing validation.
#[async_trait::async_trait]
pub(crate) trait CoreSmsChannelExt: Channel {
    async fn private_sms_lease(
        &self,
        ctx: &crate::gateway::telecom::TelecomPublicContext,
        recipient: &str,
    ) -> Option<crate::gateway::interfaces::sms::PrivateSmsLease>;

    async fn send_private_sms(
        &self,
        message: OutboundMessage,
        lease: &crate::gateway::interfaces::sms::PrivateSmsLease,
        cancel: &tokio_util::sync::CancellationToken,
        validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> ChannelResult<crate::gateway::telecom::public_run::ApprovedReplyOutcome>;
}

#[async_trait::async_trait]
impl<T: Channel + ?Sized> CoreSmsChannelExt for T {
    async fn private_sms_lease(
        &self,
        ctx: &crate::gateway::telecom::TelecomPublicContext,
        recipient: &str,
    ) -> Option<crate::gateway::interfaces::sms::PrivateSmsLease> {
        let sms = self
            .as_any()?
            .downcast_ref::<crate::gateway::interfaces::sms::SmsChannel>()?;
        sms.private_sms_lease(ctx, recipient).await
    }

    async fn send_private_sms(
        &self,
        message: OutboundMessage,
        lease: &crate::gateway::interfaces::sms::PrivateSmsLease,
        cancel: &tokio_util::sync::CancellationToken,
        validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> ChannelResult<crate::gateway::telecom::public_run::ApprovedReplyOutcome> {
        let Some(sms) = self
            .as_any()
            .and_then(|value| value.downcast_ref::<crate::gateway::interfaces::sms::SmsChannel>())
        else {
            return Ok(crate::gateway::telecom::public_run::ApprovedReplyOutcome::Invalid);
        };
        sms.send_private_sms(message, lease, cancel, validate).await
    }
}

/// Provider of interaction manifest for a channel
///
/// Map a channel-type string (as reported by
/// [`Channel::channel_type`] / [`ChannelInfo::channel_type`]) to the
/// `InteractionParadigm` that best describes its UX shape.
///
/// Used by the gateway run-loop when constructing
/// `FlowRequest.interaction_manifest` for Phase 4 channel-aware prompt
/// assembly. The mapping is intentionally conservative — when a new
/// channel type is added without an entry, it falls through to
/// `Background`, which is the safest default (subagent-style execution
/// without user-facing chrome).
#[must_use]
pub fn paradigm_for_channel_type(
    channel_type: &str,
) -> crate::thinker::interaction::InteractionParadigm {
    use crate::thinker::interaction::InteractionParadigm;
    match channel_type {
        "cli" => InteractionParadigm::CLI,
        "webchat" | "webrich" | "web" | "webhook" => InteractionParadigm::WebRich,
        "telegram" | "feishu" | "slack" | "whatsapp" | "discord" | "irc" | "wechat" | "line" => {
            InteractionParadigm::Messaging
        }
        _ => InteractionParadigm::Background,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pretender {
        info: ChannelInfo,
        state: ChannelState,
        expose: bool,
    }

    impl Pretender {
        fn new(expose: bool) -> Self {
            Self {
                info: ChannelInfo {
                    id: ChannelId::new("pretender"),
                    name: "Pretend SMS".into(),
                    channel_type: "sms".into(),
                    status: ChannelStatus::Connected,
                    capabilities: ChannelCapabilities::default(),
                },
                state: ChannelState::new(1),
                expose,
            }
        }
    }

    #[async_trait::async_trait]
    impl Channel for Pretender {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }

        fn state(&self) -> &ChannelState {
            &self.state
        }

        fn as_any(&self) -> Option<&(dyn std::any::Any + Send + Sync)> {
            if self.expose {
                Some(self)
            } else {
                None
            }
        }

        fn outbound_delivery_policy(&self) -> OutboundDeliveryPolicy {
            OutboundDeliveryPolicy::ExplicitOnly
        }

        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }

        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }

        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            panic!("private SMS denial must not fall back to ordinary send")
        }
    }

    // Deliberately omit as_any to exercise the actual shared API default.
    struct DefaultAsAny(Pretender);

    #[async_trait::async_trait]
    impl Channel for DefaultAsAny {
        fn info(&self) -> &ChannelInfo {
            &self.0.info
        }

        fn state(&self) -> &ChannelState {
            &self.0.state
        }

        fn outbound_delivery_policy(&self) -> OutboundDeliveryPolicy {
            OutboundDeliveryPolicy::ExplicitOnly
        }

        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }

        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }

        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            panic!("private SMS denial must not fall back to ordinary send")
        }
    }

    #[tokio::test]
    async fn private_sms_bridge_denies_default_and_fake_any_without_transport_writes() {
        use crate::gateway::interfaces::sms::{test_private_channel, transport};
        use crate::gateway::telecom::public_run::ApprovedReplyOutcome;
        use tokio_util::sync::CancellationToken;

        let (channel, _transport, lease, peer) = test_private_channel("bridge-negative").await;
        let ctx = transport::test_private_context("bridge-negative");
        let default = DefaultAsAny(Pretender::new(false));
        let hidden = Pretender::new(false);
        let exposed = Pretender::new(true);
        assert!((&default as &dyn Channel).as_any().is_none());
        assert!((&hidden as &dyn Channel).as_any().is_none());
        assert!((&exposed as &dyn Channel)
            .as_any()
            .unwrap()
            .is::<Pretender>());
        assert!(peer.submissions().is_empty());

        for object in [&default as &dyn Channel, &hidden, &exposed] {
            assert_eq!(object.channel_type(), "sms");
            assert_eq!(
                object.outbound_delivery_policy(),
                OutboundDeliveryPolicy::ExplicitOnly
            );
            assert!(object
                .private_sms_lease(&ctx, "+15550000001")
                .await
                .is_none());
            let outcome = object
                .send_private_sms(
                    OutboundMessage::text("+15550000001", "body"),
                    &lease,
                    &CancellationToken::new(),
                    &|_| true,
                )
                .await
                .unwrap();
            assert!(matches!(outcome, ApprovedReplyOutcome::Invalid));
            assert!(peer.submissions().is_empty(), "denial must not write");
        }

        // Rejections leave the genuine adapter's live lease path intact.
        assert!((&channel as &dyn Channel)
            .private_sms_lease(&ctx, "+15550000001")
            .await
            .is_some());
        assert!(peer.submissions().is_empty());
    }

    /// Every `ChannelCapabilities` literal that claims `StreamProtocol::
    /// EditBased` must also claim `editing: true`.
    ///
    /// The two say the same thing — "this channel can rewrite a message it has
    /// already sent" — and until 2026-08-12 they contradicted each other on
    /// `line` and `wechat`, whose `edit` returns `UnsupportedFeature`
    /// unconditionally. The contradiction was not inert: the inbound router
    /// read `stream_protocol` to *force streaming on*, the emitter then
    /// discarded every `edit` error with no send fallback, and the reader got
    /// the first flush and nothing more. Now the router floors on `editing`,
    /// so a repeat of that exact lie is harmless — but the declarations would
    /// still disagree, and the next reader of `stream_protocol` would inherit
    /// the false one. Keep them in agreement.
    ///
    /// Runtime cannot check this (a `Channel` that overrides `edit` only to
    /// return `Err` is indistinguishable from one that does not override it —
    /// `feishu` is exactly that), so the check is over source text.
    #[test]
    fn an_edit_based_channel_declares_that_it_can_edit() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("gateway")
            .join("interfaces");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(
            files.len() > 15,
            "walk found {} sources under interfaces/ — expected one per channel",
            files.len()
        );

        let mut literals = 0usize;
        let mut edit_based = 0usize;
        let mut liars: Vec<String> = Vec::new();

        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            // Normalise line endings first: this checkout is CRLF and any
            // separator anchoring a bare `\n` matches nothing here
            // (CLAUDE.md §10). Nothing below anchors one, but the next
            // person to edit this scan should not have to notice.
            let text = text.replace("\r\n", "\n");
            let rel = file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let label = file
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| format!("{}/{rel}", n.to_string_lossy()))
                .unwrap_or(rel);

            // Brace-match each literal rather than guessing where it ends:
            // the field order is not uniform across channels and several
            // literals carry multi-line comments between the two fields.
            let mut rest = text.as_str();
            while let Some(at) = rest.find("ChannelCapabilities {") {
                let after = &rest[at + "ChannelCapabilities {".len()..];
                let mut depth = 1usize;
                let mut end = after.len();
                for (i, c) in after.char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let body = &after[..end];
                literals += 1;
                if body.contains("StreamProtocol::EditBased") {
                    edit_based += 1;
                    if !body.contains("editing: true") {
                        liars.push(label.clone());
                    }
                }
                rest = &after[end..];
            }
        }

        // Floors, not counts: they catch a scan that matched nothing while still
        // letting a channel legitimately come or go. There are exactly 3 EditBased
        // channels today (discord/matrix/telegram), so a `>= 3` floor would report a
        // legitimate removal as "the scan is broken" — pointing the next reader at
        // the scanner instead of at their own edit.
        assert!(
            literals >= 15,
            "found only {literals} ChannelCapabilities literals — the scan is broken, not the code"
        );
        assert!(
            edit_based >= 2,
            "found only {edit_based} EditBased declarations — the scan is broken, not the code"
        );
        assert!(
            liars.is_empty(),
            "these channels declare EditBased streaming but not `editing: true`, \
             so their `edit` is the trait default (an Err the streaming emitter \
             silently drops): {liars:?}"
        );
    }
}
