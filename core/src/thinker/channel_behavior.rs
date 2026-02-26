//! Channel Behavior Configuration
//!
//! Provides per-channel behavioral guidance for the AI, including
//! message limits, reaction styles, and group chat rules.

use std::fmt;

/// Specific channel variant with platform-specific metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelVariant {
    Terminal,
    WebPanel,
    ControlPlane,
    Telegram { is_group: bool },
    Discord { is_guild: bool },
    IMessage,
    Cron,
    Heartbeat,
    Halo,
}

/// Message size and capability limits for a channel.
#[derive(Debug, Clone)]
pub struct MessageLimits {
    pub max_chars: usize,
    pub max_media_per_message: u8,
    pub supports_threading: bool,
    pub supports_editing: bool,
}

/// How aggressively to use emoji reactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionStyle {
    None,
    Minimal,
    Expressive,
}

/// Group chat behavioral rules.
#[derive(Debug, Clone)]
pub struct GroupBehavior {
    pub respond_triggers: Vec<ResponseTrigger>,
    pub silence_triggers: Vec<SilenceTrigger>,
    pub reaction_as_acknowledgment: bool,
}

/// Conditions under which the AI should respond in a group chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseTrigger {
    DirectMention,
    DirectReply,
    AddingValue,
    CorrectingMisinformation,
    ExplicitQuestion,
}

/// Conditions under which the AI should stay silent in a group chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SilenceTrigger {
    CasualBanter,
    AlreadyAnswered,
    ConversationFlowing,
    EmptyAcknowledgment,
    OffTopic,
}

/// Complete behavioral guide for a channel.
#[derive(Debug, Clone)]
pub struct ChannelBehaviorGuide {
    pub variant: ChannelVariant,
    pub message_limits: Option<MessageLimits>,
    pub reaction_style: ReactionStyle,
    pub supports_markdown: bool,
    pub inline_media: bool,
    pub inline_buttons: bool,
    pub typing_indicator: bool,
    pub group_behavior: Option<GroupBehavior>,
}

// ---------------------------------------------------------------------------
// Display implementations
// ---------------------------------------------------------------------------

impl fmt::Display for ChannelVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Terminal => write!(f, "Terminal"),
            Self::WebPanel => write!(f, "Web Panel"),
            Self::ControlPlane => write!(f, "Control Plane"),
            Self::Telegram { is_group: true } => write!(f, "Telegram Group"),
            Self::Telegram { is_group: false } => write!(f, "Telegram"),
            Self::Discord { is_guild: true } => write!(f, "Discord Server"),
            Self::Discord { is_guild: false } => write!(f, "Discord DM"),
            Self::IMessage => write!(f, "iMessage"),
            Self::Cron => write!(f, "Cron"),
            Self::Heartbeat => write!(f, "Heartbeat"),
            Self::Halo => write!(f, "Halo"),
        }
    }
}

impl fmt::Display for ResponseTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectMention => write!(f, "Someone directly @mentions you"),
            Self::DirectReply => write!(f, "Someone replies to your message"),
            Self::AddingValue => {
                write!(f, "You have unique knowledge that adds value to the conversation")
            }
            Self::CorrectingMisinformation => {
                write!(f, "Someone shares clearly incorrect information you can correct")
            }
            Self::ExplicitQuestion => {
                write!(f, "Someone asks a question clearly directed at you")
            }
        }
    }
}

impl fmt::Display for SilenceTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CasualBanter => {
                write!(f, "Humans are having casual banter or social conversation")
            }
            Self::AlreadyAnswered => {
                write!(f, "The question has already been answered by someone else")
            }
            Self::ConversationFlowing => {
                write!(f, "The conversation is flowing naturally without needing your input")
            }
            Self::EmptyAcknowledgment => {
                write!(f, "Your response would just be an empty acknowledgment (e.g., \"ok\", \"got it\")")
            }
            Self::OffTopic => {
                write!(f, "The topic is outside your expertise or not relevant to you")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Default for GroupBehavior
// ---------------------------------------------------------------------------

impl Default for GroupBehavior {
    fn default() -> Self {
        Self {
            respond_triggers: vec![
                ResponseTrigger::DirectMention,
                ResponseTrigger::DirectReply,
                ResponseTrigger::AddingValue,
                ResponseTrigger::CorrectingMisinformation,
                ResponseTrigger::ExplicitQuestion,
            ],
            silence_triggers: vec![
                SilenceTrigger::CasualBanter,
                SilenceTrigger::AlreadyAnswered,
                SilenceTrigger::ConversationFlowing,
                SilenceTrigger::EmptyAcknowledgment,
                SilenceTrigger::OffTopic,
            ],
            reaction_as_acknowledgment: true,
        }
    }
}

// ---------------------------------------------------------------------------
// ChannelBehaviorGuide implementation
// ---------------------------------------------------------------------------

impl ChannelBehaviorGuide {
    /// Create a behavior guide with sensible defaults for the given channel variant.
    pub fn for_channel(variant: ChannelVariant) -> Self {
        match &variant {
            ChannelVariant::Terminal => Self {
                variant,
                message_limits: None,
                reaction_style: ReactionStyle::None,
                supports_markdown: false,
                inline_media: false,
                inline_buttons: false,
                typing_indicator: false,
                group_behavior: None,
            },
            ChannelVariant::WebPanel => Self {
                variant,
                message_limits: None,
                reaction_style: ReactionStyle::None,
                supports_markdown: true,
                inline_media: true,
                inline_buttons: true,
                typing_indicator: true,
                group_behavior: None,
            },
            ChannelVariant::ControlPlane => Self {
                variant,
                message_limits: None,
                reaction_style: ReactionStyle::None,
                supports_markdown: true,
                inline_media: false,
                inline_buttons: true,
                typing_indicator: true,
                group_behavior: None,
            },
            ChannelVariant::Telegram { is_group } => Self {
                message_limits: Some(MessageLimits {
                    max_chars: 4096,
                    max_media_per_message: 10,
                    supports_threading: true,
                    supports_editing: true,
                }),
                reaction_style: if *is_group {
                    ReactionStyle::Minimal
                } else {
                    ReactionStyle::None
                },
                supports_markdown: true,
                inline_media: true,
                inline_buttons: true,
                typing_indicator: true,
                group_behavior: if *is_group {
                    Some(GroupBehavior::default())
                } else {
                    None
                },
                variant,
            },
            ChannelVariant::Discord { is_guild } => Self {
                message_limits: Some(MessageLimits {
                    max_chars: 2000,
                    max_media_per_message: 10,
                    supports_threading: true,
                    supports_editing: true,
                }),
                reaction_style: if *is_guild {
                    ReactionStyle::Expressive
                } else {
                    ReactionStyle::None
                },
                supports_markdown: true,
                inline_media: true,
                inline_buttons: false,
                typing_indicator: true,
                group_behavior: if *is_guild {
                    Some(GroupBehavior::default())
                } else {
                    None
                },
                variant,
            },
            ChannelVariant::IMessage => Self {
                variant,
                message_limits: Some(MessageLimits {
                    max_chars: 20000,
                    max_media_per_message: 5,
                    supports_threading: false,
                    supports_editing: false,
                }),
                reaction_style: ReactionStyle::Minimal,
                supports_markdown: false,
                inline_media: true,
                inline_buttons: false,
                typing_indicator: true,
                group_behavior: None,
            },
            ChannelVariant::Cron | ChannelVariant::Heartbeat => Self {
                variant,
                message_limits: None,
                reaction_style: ReactionStyle::None,
                supports_markdown: false,
                inline_media: false,
                inline_buttons: false,
                typing_indicator: false,
                group_behavior: None,
            },
            ChannelVariant::Halo => Self {
                variant,
                message_limits: Some(MessageLimits {
                    max_chars: 500,
                    max_media_per_message: 1,
                    supports_threading: false,
                    supports_editing: true,
                }),
                reaction_style: ReactionStyle::None,
                supports_markdown: true,
                inline_media: false,
                inline_buttons: true,
                typing_indicator: true,
                group_behavior: None,
            },
        }
    }

    /// Generate the complete prompt section describing this channel's behavior rules.
    pub fn to_prompt_section(&self) -> String {
        let mut lines = Vec::new();

        lines.push(format!("## Channel: {}", self.variant));
        lines.push(String::new());

        // Communication style
        lines.push("### Communication Style".to_string());
        if self.supports_markdown {
            lines.push("- Messages support Markdown formatting".to_string());
        } else {
            lines.push("- Plain text only".to_string());
        }
        if self.inline_media {
            lines.push("- Images can be sent inline".to_string());
        }
        if self.inline_buttons {
            lines.push("- Inline buttons available for options".to_string());
        }
        if self.typing_indicator {
            lines.push("- Typing indicator will be shown".to_string());
        }

        // Message limits
        if let Some(ref limits) = self.message_limits {
            lines.push(String::new());
            lines.push("### Message Limits".to_string());
            lines.push(format!("- Maximum: {} characters per message", limits.max_chars));
            if limits.max_chars <= 2000 {
                lines.push(
                    "- If your response exceeds the limit, split into logical sections".to_string(),
                );
            }
            if limits.supports_editing {
                lines.push("- You can edit previously sent messages".to_string());
            }
        }

        // Reaction guidance
        match self.reaction_style {
            ReactionStyle::None => {}
            ReactionStyle::Minimal => {
                lines.push(String::new());
                lines.push("### Reaction Guidance".to_string());
                lines.push(
                    "- Use reactions sparingly — roughly 1 per 5-10 messages".to_string(),
                );
                lines.push("- Preferred reactions: 👍 ❤️ 🤔".to_string());
            }
            ReactionStyle::Expressive => {
                lines.push(String::new());
                lines.push("### Reaction Guidance".to_string());
                lines.push("- Use reactions liberally to engage with the community".to_string());
                lines.push("- Preferred reactions: 👍 ❤️ 🎉 🤔 💡 😂 💀".to_string());
            }
        }

        // Group chat rules
        if let Some(ref group) = self.group_behavior {
            lines.push(String::new());
            lines.push("### Group Chat Rules".to_string());
            lines.push("RESPOND when:".to_string());
            for trigger in &group.respond_triggers {
                lines.push(format!("- {trigger}"));
            }
            lines.push(String::new());
            lines.push("STAY SILENT (use ALEPH_NO_REPLY) when:".to_string());
            for trigger in &group.silence_triggers {
                lines.push(format!("- {trigger}"));
            }
            lines.push(String::new());
            lines.push(
                "Remember: Humans don't respond to everything. Neither should you.".to_string(),
            );
            lines.push(
                "Use emoji reactions as lightweight acknowledgment instead of full messages."
                    .to_string(),
            );
        }

        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telegram_group_defaults() {
        let guide = ChannelBehaviorGuide::for_channel(ChannelVariant::Telegram { is_group: true });
        assert_eq!(guide.reaction_style, ReactionStyle::Minimal);
        assert!(guide.supports_markdown);
        assert!(guide.inline_buttons);
        assert!(guide.group_behavior.is_some());
        assert_eq!(guide.message_limits.as_ref().unwrap().max_chars, 4096);
    }

    #[test]
    fn test_telegram_private_no_group_behavior() {
        let guide =
            ChannelBehaviorGuide::for_channel(ChannelVariant::Telegram { is_group: false });
        assert!(guide.group_behavior.is_none());
    }

    #[test]
    fn test_discord_guild_defaults() {
        let guide =
            ChannelBehaviorGuide::for_channel(ChannelVariant::Discord { is_guild: true });
        assert_eq!(guide.reaction_style, ReactionStyle::Expressive);
        assert!(guide.supports_markdown);
        assert!(guide.group_behavior.is_some());
        assert_eq!(guide.message_limits.as_ref().unwrap().max_chars, 2000);
    }

    #[test]
    fn test_terminal_no_reactions() {
        let guide = ChannelBehaviorGuide::for_channel(ChannelVariant::Terminal);
        assert_eq!(guide.reaction_style, ReactionStyle::None);
        assert!(guide.group_behavior.is_none());
        assert!(guide.message_limits.is_none());
    }

    #[test]
    fn test_prompt_section_contains_channel_name() {
        let guide = ChannelBehaviorGuide::for_channel(ChannelVariant::Telegram { is_group: true });
        let section = guide.to_prompt_section();
        assert!(section.contains("## Channel: Telegram Group"));
    }

    #[test]
    fn test_prompt_section_contains_group_rules() {
        let guide = ChannelBehaviorGuide::for_channel(ChannelVariant::Telegram { is_group: true });
        let section = guide.to_prompt_section();
        assert!(section.contains("RESPOND when"));
        assert!(section.contains("STAY SILENT"));
        assert!(section.contains("ALEPH_NO_REPLY"));
    }

    #[test]
    fn test_prompt_section_omits_group_rules_for_dm() {
        let guide =
            ChannelBehaviorGuide::for_channel(ChannelVariant::Telegram { is_group: false });
        let section = guide.to_prompt_section();
        assert!(!section.contains("STAY SILENT"));
    }

    #[test]
    fn test_prompt_section_contains_message_limits() {
        let guide =
            ChannelBehaviorGuide::for_channel(ChannelVariant::Discord { is_guild: false });
        let section = guide.to_prompt_section();
        assert!(section.contains("2000"));
    }

    #[test]
    fn test_default_group_behavior() {
        let gb = GroupBehavior::default();
        assert!(gb.respond_triggers.contains(&ResponseTrigger::DirectMention));
        assert!(gb.silence_triggers.contains(&SilenceTrigger::CasualBanter));
        assert!(gb.reaction_as_acknowledgment);
    }

    #[test]
    fn test_channel_variant_display() {
        assert_eq!(
            format!("{}", ChannelVariant::Telegram { is_group: true }),
            "Telegram Group"
        );
        assert_eq!(
            format!("{}", ChannelVariant::Telegram { is_group: false }),
            "Telegram"
        );
        assert_eq!(format!("{}", ChannelVariant::Terminal), "Terminal");
    }

    #[test]
    fn test_halo_small_limits() {
        let guide = ChannelBehaviorGuide::for_channel(ChannelVariant::Halo);
        assert_eq!(guide.message_limits.as_ref().unwrap().max_chars, 500);
        assert!(guide.supports_markdown);
        assert!(guide.inline_buttons);
    }
}
