// Channel definition data model and static definitions for all 13 messaging channels.
//
// This module provides the `ChannelDefinition` type describing each channel's metadata,
// brand identity, and configuration fields. The definitions drive the generic settings
// form renderer so that adding a new channel only requires a new entry here.

// ---------------------------------------------------------------------------
// Field & Channel types
// ---------------------------------------------------------------------------

/// Field input type for form rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldKind {
    Text,
    Secret,
    Url,
    Number { min: i32, max: i32 },
    Toggle,
    TagList,
    Select,
}

/// Describes a single configuration field inside a channel.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub placeholder: &'static str,
    pub help: &'static str,
    pub required: bool,
    pub default_value: &'static str,
    pub options: &'static [(&'static str, &'static str)],
}

/// Complete definition of a messaging channel: identity, brand, config schema.
#[derive(Debug, Clone)]
pub struct ChannelDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub icon_svg: &'static str,
    pub brand_color: &'static str,
    pub config_section: &'static str,
    pub fields: &'static [FieldDef],
    pub docs_url: &'static str,
}

impl ChannelDefinition {
    /// Look up a channel definition by its `id`.
    pub fn find(id: &str) -> Option<&'static ChannelDefinition> {
        ALL_CHANNELS.iter().find(|ch| ch.id == id)
    }
}

// ---------------------------------------------------------------------------
// SVG icon paths (24x24 viewBox)
// ---------------------------------------------------------------------------

const ICON_TELEGRAM: &str = r#"<path d="M21.2 4.4L2.9 11.3c-1.2.5-1.2 1.2-.2 1.5l4.7 1.5 1.8 5.6c.2.6.1.8.7.8.4 0 .6-.2.9-.4l2.1-2.1 4.4 3.3c.8.4 1.4.2 1.6-.8L22.4 5.6c.3-1.2-.5-1.7-1.2-1.2zM8.5 13.5l9.4-5.9c.4-.3.8-.1.5.2l-7.8 7-.3 3.2-1.8-4.5z"/>"#;

const ICON_DISCORD: &str = r#"<path d="M18.59 5.89c-1.23-.57-2.54-.99-3.92-1.23-.17.3-.37.71-.5 1.03-1.46-.22-2.91-.22-4.34 0-.14-.32-.34-.73-.51-1.03-1.38.24-2.69.66-3.92 1.23C2.18 10.73 1.34 15.44 1.76 20.09A18.07 18.07 0 0 0 7.2 22.5c.44-.6.83-1.24 1.17-1.91-.64-.24-1.26-.54-1.84-.89.15-.11.3-.23.45-.34a12.84 12.84 0 0 0 10.04 0c.15.12.3.23.45.34-.58.35-1.2.65-1.84.89.34.67.73 1.31 1.17 1.91a18 18 0 0 0 5.44-2.41c.49-5.15-.84-9.82-3.65-13.61zM8.35 17.24c-1.18 0-2.15-1.09-2.15-2.42s.95-2.42 2.15-2.42 2.17 1.09 2.15 2.42c0 1.33-.95 2.42-2.15 2.42zm6.3 0c-1.18 0-2.15-1.09-2.15-2.42s.95-2.42 2.15-2.42 2.17 1.09 2.15 2.42c0 1.33-.95 2.42-2.15 2.42z"/>"#;

const ICON_WHATSAPP: &str = r#"<path d="M17.47 14.38c-.29-.14-1.7-.84-1.96-.94-.27-.1-.46-.14-.65.14-.2.29-.75.94-.92 1.13-.17.2-.34.22-.63.07-.29-.14-1.22-.45-2.32-1.43-.86-.77-1.44-1.71-1.61-2-.17-.29-.02-.45.13-.59.13-.13.29-.34.44-.51.14-.17.2-.29.29-.48.1-.2.05-.37-.02-.51-.07-.15-.65-1.56-.89-2.14-.24-.56-.48-.49-.65-.49-.17 0-.37-.02-.56-.02-.2 0-.51.07-.78.37-.27.29-1.02 1-1.02 2.43 0 1.43 1.04 2.82 1.19 3.01.14.2 2.05 3.13 4.97 4.39.7.3 1.24.48 1.66.61.7.22 1.33.19 1.83.12.56-.08 1.7-.7 1.94-1.37.24-.68.24-1.26.17-1.38-.07-.12-.27-.2-.56-.34zM12 2C6.48 2 2 6.48 2 12c0 1.77.46 3.43 1.27 4.88L2 22l5.23-1.37A9.93 9.93 0 0 0 12 22c5.52 0 10-4.48 10-10S17.52 2 12 2z"/>"#;

const ICON_IMESSAGE: &str =
    r#"<path d="M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2z"/>"#;

const ICON_SLACK: &str = r#"<path d="M14.5 2c-.83 0-1.5.67-1.5 1.5v5c0 .83.67 1.5 1.5 1.5h5c.83 0 1.5-.67 1.5-1.5S20.33 7 19.5 7H16V3.5c0-.83-.67-1.5-1.5-1.5zm-5 0C8.67 2 8 2.67 8 3.5V7H4.5C3.67 7 3 7.67 3 8.5S3.67 10 4.5 10h5c.83 0 1.5-.67 1.5-1.5v-5C11 2.67 10.33 2 9.5 2zm5 12c-.83 0-1.5.67-1.5 1.5V17h-3.5c-.83 0-1.5.67-1.5 1.5s.67 1.5 1.5 1.5h5c.83 0 1.5-.67 1.5-1.5v-5c0-.83-.67-1.5-1.5-1.5zm-10 0c-.83 0-1.5.67-1.5 1.5s.67 1.5 1.5 1.5H8v3.5c0 .83.67 1.5 1.5 1.5s1.5-.67 1.5-1.5v-5c0-.83-.67-1.5-1.5-1.5h-5z"/>"#;

const ICON_EMAIL: &str = r#"<rect x="2" y="4" width="20" height="16" rx="2"/><polyline points="22,7 12,13 2,7"/>"#;

const ICON_MATRIX: &str = r#"<path d="M2 2v20h4V2H2zm16 0v20h4V2h-4zM7 4h1v2H7V4zm2 2h2v2H9V6zm4 0h2v2h-2V6zm3-2h1v2h-1V4zM7 18h1v2H7v-2zm2-2h2v2H9v-2zm4 0h2v2h-2v-2zm3 2h1v2h-1v-2z"/>"#;

const ICON_SIGNAL: &str = r#"<path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 15l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z"/>"#;

const ICON_MATTERMOST: &str = r#"<path d="M12 2C6.48 2 2 6.48 2 12c0 2.17.7 4.19 1.88 5.83L2 22l4.17-1.88C7.81 21.3 9.83 22 12 22c5.52 0 10-4.48 10-10S17.52 2 12 2z"/>"#;

const ICON_IRC: &str = r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#;

const ICON_WEBHOOK: &str = r#"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>"#;

const ICON_XMPP: &str = r#"<circle cx="12" cy="12" r="10"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/><line x1="9" y1="9" x2="9.01" y2="9"/><line x1="15" y1="9" x2="15.01" y2="9"/>"#;

const ICON_NOSTR: &str = r#"<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>"#;

// ---------------------------------------------------------------------------
// Per-channel field definitions
// ---------------------------------------------------------------------------

static TELEGRAM_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "bot_token",
        label: "Bot Token",
        kind: FieldKind::Secret,
        placeholder: "123456:ABC-DEF1234...",
        help: "Obtain from @BotFather on Telegram",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "bot_username",
        label: "Bot Username",
        kind: FieldKind::Text,
        placeholder: "my_aleph_bot",
        help: "Username without the leading @",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "dm_allowed",
        label: "Allow DMs",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Accept direct messages from users",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "groups_allowed",
        label: "Allow Groups",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Accept messages from group chats",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "send_typing",
        label: "Send Typing Indicator",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Show typing indicator while processing",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "polling_interval_secs",
        label: "Polling Interval (seconds)",
        kind: FieldKind::Number { min: 1, max: 60 },
        placeholder: "1",
        help: "How often to poll for new messages",
        required: false,
        default_value: "1",
        options: &[],
    },
    FieldDef {
        key: "allowed_users",
        label: "Allowed Users",
        kind: FieldKind::TagList,
        placeholder: "Add user ID...",
        help: "Telegram user IDs allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "allowed_groups",
        label: "Allowed Groups",
        kind: FieldKind::TagList,
        placeholder: "Add group ID...",
        help: "Telegram group IDs allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

// Discord uses its own complex DiscordChannelView; no generic fields.
static DISCORD_FIELDS: &[FieldDef] = &[];

static WHATSAPP_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "phone_number",
        label: "Phone Number",
        kind: FieldKind::Text,
        placeholder: "+1234567890",
        help: "WhatsApp phone number with country code",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "send_typing",
        label: "Send Typing Indicator",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Show typing indicator while processing",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "mark_read",
        label: "Mark as Read",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Automatically mark incoming messages as read",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "bridge_binary",
        label: "Bridge Binary",
        kind: FieldKind::Text,
        placeholder: "/usr/local/bin/whatsapp-bridge",
        help: "Path to the WhatsApp bridge binary",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "max_restarts",
        label: "Max Restarts",
        kind: FieldKind::Number { min: 0, max: 20 },
        placeholder: "5",
        help: "Maximum number of bridge restart attempts",
        required: false,
        default_value: "5",
        options: &[],
    },
    FieldDef {
        key: "allowed_chats",
        label: "Allowed Chats",
        kind: FieldKind::TagList,
        placeholder: "Add chat ID...",
        help: "Chat IDs allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

static IMESSAGE_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "enabled",
        label: "Enabled",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Enable iMessage integration",
        required: false,
        default_value: "false",
        options: &[],
    },
    FieldDef {
        key: "db_path",
        label: "Database Path",
        kind: FieldKind::Text,
        placeholder: "~/Library/Messages/chat.db",
        help: "Path to the iMessage SQLite database",
        required: false,
        default_value: "~/Library/Messages/chat.db",
        options: &[],
    },
    FieldDef {
        key: "poll_interval_ms",
        label: "Poll Interval (ms)",
        kind: FieldKind::Number { min: 100, max: 10000 },
        placeholder: "1000",
        help: "Polling interval in milliseconds",
        required: false,
        default_value: "1000",
        options: &[],
    },
    FieldDef {
        key: "require_mention",
        label: "Require Mention",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Only respond when explicitly mentioned",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "bot_name",
        label: "Bot Name",
        kind: FieldKind::Text,
        placeholder: "Aleph",
        help: "Name used for mention detection",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "include_attachments",
        label: "Include Attachments",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Process message attachments",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "allow_from",
        label: "Allow From",
        kind: FieldKind::TagList,
        placeholder: "Add phone or email...",
        help: "Contacts allowed for DMs (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "group_allow_from",
        label: "Group Allow From",
        kind: FieldKind::TagList,
        placeholder: "Add group chat name...",
        help: "Group chats allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

static SLACK_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "app_token",
        label: "App Token",
        kind: FieldKind::Secret,
        placeholder: "xapp-1-...",
        help: "Slack app-level token for Socket Mode",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "bot_token",
        label: "Bot Token",
        kind: FieldKind::Secret,
        placeholder: "xoxb-...",
        help: "Slack bot user OAuth token",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "send_typing",
        label: "Send Typing Indicator",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Show typing indicator while processing",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "dm_allowed",
        label: "Allow DMs",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Accept direct messages from users",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "allowed_channels",
        label: "Allowed Channels",
        kind: FieldKind::TagList,
        placeholder: "Add channel ID...",
        help: "Channel IDs allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

static EMAIL_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "imap_host",
        label: "IMAP Host",
        kind: FieldKind::Text,
        placeholder: "imap.gmail.com",
        help: "IMAP server hostname",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "imap_port",
        label: "IMAP Port",
        kind: FieldKind::Number { min: 1, max: 65535 },
        placeholder: "993",
        help: "IMAP server port",
        required: false,
        default_value: "993",
        options: &[],
    },
    FieldDef {
        key: "smtp_host",
        label: "SMTP Host",
        kind: FieldKind::Text,
        placeholder: "smtp.gmail.com",
        help: "SMTP server hostname",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "smtp_port",
        label: "SMTP Port",
        kind: FieldKind::Number { min: 1, max: 65535 },
        placeholder: "587",
        help: "SMTP server port",
        required: false,
        default_value: "587",
        options: &[],
    },
    FieldDef {
        key: "username",
        label: "Username",
        kind: FieldKind::Text,
        placeholder: "user@example.com",
        help: "Email account username",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "password",
        label: "Password",
        kind: FieldKind::Secret,
        placeholder: "",
        help: "Email account password or app password",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "from_address",
        label: "From Address",
        kind: FieldKind::Text,
        placeholder: "aleph@example.com",
        help: "Email address used as the sender",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "use_tls",
        label: "Use TLS",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Enable TLS encryption for connections",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "poll_interval_secs",
        label: "Poll Interval (seconds)",
        kind: FieldKind::Number { min: 5, max: 3600 },
        placeholder: "30",
        help: "How often to check for new emails",
        required: false,
        default_value: "30",
        options: &[],
    },
    FieldDef {
        key: "folders",
        label: "Folders",
        kind: FieldKind::TagList,
        placeholder: "Add folder name...",
        help: "IMAP folders to monitor (empty = INBOX)",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "allowed_senders",
        label: "Allowed Senders",
        kind: FieldKind::TagList,
        placeholder: "Add email address...",
        help: "Sender addresses allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

static MATRIX_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "homeserver_url",
        label: "Homeserver URL",
        kind: FieldKind::Url,
        placeholder: "https://matrix.org",
        help: "Matrix homeserver URL",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "access_token",
        label: "Access Token",
        kind: FieldKind::Secret,
        placeholder: "",
        help: "Matrix access token for authentication",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "display_name",
        label: "Display Name",
        kind: FieldKind::Text,
        placeholder: "Aleph",
        help: "Bot display name in Matrix rooms",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "send_typing",
        label: "Send Typing Indicator",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Show typing indicator while processing",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "allowed_rooms",
        label: "Allowed Rooms",
        kind: FieldKind::TagList,
        placeholder: "Add room ID...",
        help: "Room IDs allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

static SIGNAL_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "api_url",
        label: "API URL",
        kind: FieldKind::Url,
        placeholder: "http://localhost:8080",
        help: "Signal CLI REST API endpoint",
        required: false,
        default_value: "http://localhost:8080",
        options: &[],
    },
    FieldDef {
        key: "phone_number",
        label: "Phone Number",
        kind: FieldKind::Text,
        placeholder: "+1234567890",
        help: "Registered Signal phone number",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "send_typing",
        label: "Send Typing Indicator",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Show typing indicator while processing",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "poll_interval_secs",
        label: "Poll Interval (seconds)",
        kind: FieldKind::Number { min: 1, max: 60 },
        placeholder: "2",
        help: "How often to poll for new messages",
        required: false,
        default_value: "2",
        options: &[],
    },
    FieldDef {
        key: "allowed_users",
        label: "Allowed Users",
        kind: FieldKind::TagList,
        placeholder: "Add phone number...",
        help: "Phone numbers allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

static MATTERMOST_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "server_url",
        label: "Server URL",
        kind: FieldKind::Url,
        placeholder: "https://mattermost.example.com",
        help: "Mattermost server URL",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "bot_token",
        label: "Bot Token",
        kind: FieldKind::Secret,
        placeholder: "",
        help: "Mattermost bot access token",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "send_typing",
        label: "Send Typing Indicator",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Show typing indicator while processing",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "allowed_channels",
        label: "Allowed Channels",
        kind: FieldKind::TagList,
        placeholder: "Add channel ID...",
        help: "Channel IDs allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

static IRC_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "server",
        label: "Server",
        kind: FieldKind::Text,
        placeholder: "irc.libera.chat",
        help: "IRC server hostname",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "port",
        label: "Port",
        kind: FieldKind::Number { min: 1, max: 65535 },
        placeholder: "6667",
        help: "IRC server port",
        required: false,
        default_value: "6667",
        options: &[],
    },
    FieldDef {
        key: "nick",
        label: "Nickname",
        kind: FieldKind::Text,
        placeholder: "aleph",
        help: "IRC nickname",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "password",
        label: "Password",
        kind: FieldKind::Secret,
        placeholder: "",
        help: "Server or NickServ password (optional)",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "use_tls",
        label: "Use TLS",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Enable TLS encryption",
        required: false,
        default_value: "false",
        options: &[],
    },
    FieldDef {
        key: "realname",
        label: "Real Name",
        kind: FieldKind::Text,
        placeholder: "Aleph Bot",
        help: "IRC real name / GECOS field",
        required: false,
        default_value: "Aleph Bot",
        options: &[],
    },
    FieldDef {
        key: "channels",
        label: "Channels",
        kind: FieldKind::TagList,
        placeholder: "Add channel (e.g. #general)...",
        help: "IRC channels to join",
        required: true,
        default_value: "",
        options: &[],
    },
];

static WEBHOOK_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "secret",
        label: "Secret",
        kind: FieldKind::Secret,
        placeholder: "",
        help: "Shared secret for request signature verification",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "callback_url",
        label: "Callback URL",
        kind: FieldKind::Url,
        placeholder: "https://example.com/callback",
        help: "URL to send outgoing messages to",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "path",
        label: "Webhook Path",
        kind: FieldKind::Text,
        placeholder: "/webhook/generic",
        help: "Path where incoming webhooks are received",
        required: false,
        default_value: "/webhook/generic",
        options: &[],
    },
    FieldDef {
        key: "allowed_senders",
        label: "Allowed Senders",
        kind: FieldKind::TagList,
        placeholder: "Add sender ID...",
        help: "Sender identifiers allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

static XMPP_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "jid",
        label: "JID",
        kind: FieldKind::Text,
        placeholder: "aleph@jabber.org",
        help: "XMPP Jabber ID (user@domain)",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "password",
        label: "Password",
        kind: FieldKind::Secret,
        placeholder: "",
        help: "XMPP account password",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "server",
        label: "Server",
        kind: FieldKind::Text,
        placeholder: "jabber.org",
        help: "XMPP server (overrides domain in JID if set)",
        required: false,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "port",
        label: "Port",
        kind: FieldKind::Number { min: 1, max: 65535 },
        placeholder: "5222",
        help: "XMPP server port",
        required: false,
        default_value: "5222",
        options: &[],
    },
    FieldDef {
        key: "use_tls",
        label: "Use TLS",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Enable TLS encryption (STARTTLS)",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "nick",
        label: "Nickname",
        kind: FieldKind::Text,
        placeholder: "aleph",
        help: "Nickname for multi-user chat rooms",
        required: false,
        default_value: "aleph",
        options: &[],
    },
    FieldDef {
        key: "muc_rooms",
        label: "MUC Rooms",
        kind: FieldKind::TagList,
        placeholder: "Add room JID...",
        help: "Multi-user chat rooms to join",
        required: false,
        default_value: "",
        options: &[],
    },
];

static NOSTR_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "private_key",
        label: "Private Key",
        kind: FieldKind::Secret,
        placeholder: "nsec1...",
        help: "Nostr private key (nsec or hex)",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "relays",
        label: "Relays",
        kind: FieldKind::TagList,
        placeholder: "Add relay URL (wss://...)...",
        help: "Nostr relay URLs to connect to",
        required: true,
        default_value: "",
        options: &[],
    },
    FieldDef {
        key: "allowed_pubkeys",
        label: "Allowed Public Keys",
        kind: FieldKind::TagList,
        placeholder: "Add npub or hex pubkey...",
        help: "Public keys allowed to interact (empty = all)",
        required: false,
        default_value: "",
        options: &[],
    },
];

// ---------------------------------------------------------------------------
// Static channel registry
// ---------------------------------------------------------------------------

/// All supported messaging channel definitions, ordered for the overview grid.
pub static ALL_CHANNELS: &[ChannelDefinition] = &[
    // 1. Telegram
    ChannelDefinition {
        id: "telegram",
        name: "Telegram",
        description: "Connect to Telegram via Bot API with polling",
        icon_svg: ICON_TELEGRAM,
        brand_color: "#26A5E4",
        config_section: "channels.telegram",
        fields: TELEGRAM_FIELDS,
        docs_url: "https://core.telegram.org/bots/api",
    },
    // 2. Discord
    ChannelDefinition {
        id: "discord",
        name: "Discord",
        description: "Connect to Discord via Gateway with slash commands",
        icon_svg: ICON_DISCORD,
        brand_color: "#5865F2",
        config_section: "channels.discord",
        fields: DISCORD_FIELDS,
        docs_url: "https://discord.com/developers/docs",
    },
    // 3. WhatsApp
    ChannelDefinition {
        id: "whatsapp",
        name: "WhatsApp",
        description: "Connect to WhatsApp via bridge binary",
        icon_svg: ICON_WHATSAPP,
        brand_color: "#25D366",
        config_section: "channels.whatsapp",
        fields: WHATSAPP_FIELDS,
        docs_url: "https://developers.facebook.com/docs/whatsapp",
    },
    // 4. iMessage
    ChannelDefinition {
        id: "imessage",
        name: "iMessage",
        description: "Native macOS iMessage integration via chat.db",
        icon_svg: ICON_IMESSAGE,
        brand_color: "#34C759",
        config_section: "channels.imessage",
        fields: IMESSAGE_FIELDS,
        docs_url: "https://support.apple.com/messages",
    },
    // 5. Slack
    ChannelDefinition {
        id: "slack",
        name: "Slack",
        description: "Connect to Slack via Socket Mode",
        icon_svg: ICON_SLACK,
        brand_color: "#4A154B",
        config_section: "channels.slack",
        fields: SLACK_FIELDS,
        docs_url: "https://api.slack.com/apis/socket-mode",
    },
    // 6. Email
    ChannelDefinition {
        id: "email",
        name: "Email",
        description: "IMAP/SMTP email integration",
        icon_svg: ICON_EMAIL,
        brand_color: "#EA4335",
        config_section: "channels.email",
        fields: EMAIL_FIELDS,
        docs_url: "https://datatracker.ietf.org/doc/html/rfc3501",
    },
    // 7. Matrix
    ChannelDefinition {
        id: "matrix",
        name: "Matrix",
        description: "Connect to Matrix via Client-Server API",
        icon_svg: ICON_MATRIX,
        brand_color: "#0DBD8B",
        config_section: "channels.matrix",
        fields: MATRIX_FIELDS,
        docs_url: "https://spec.matrix.org/latest/client-server-api/",
    },
    // 8. Signal
    ChannelDefinition {
        id: "signal",
        name: "Signal",
        description: "Connect to Signal via signal-cli REST API",
        icon_svg: ICON_SIGNAL,
        brand_color: "#3A76F0",
        config_section: "channels.signal",
        fields: SIGNAL_FIELDS,
        docs_url: "https://github.com/bbernhard/signal-cli-rest-api",
    },
    // 9. Mattermost
    ChannelDefinition {
        id: "mattermost",
        name: "Mattermost",
        description: "Connect to Mattermost via Bot API",
        icon_svg: ICON_MATTERMOST,
        brand_color: "#0058CC",
        config_section: "channels.mattermost",
        fields: MATTERMOST_FIELDS,
        docs_url: "https://developers.mattermost.com/integrate/reference/bot/",
    },
    // 10. IRC
    ChannelDefinition {
        id: "irc",
        name: "IRC",
        description: "Classic IRC protocol with TLS support",
        icon_svg: ICON_IRC,
        brand_color: "#6B7280",
        config_section: "channels.irc",
        fields: IRC_FIELDS,
        docs_url: "https://datatracker.ietf.org/doc/html/rfc2812",
    },
    // 11. Webhook
    ChannelDefinition {
        id: "webhook",
        name: "Webhook",
        description: "Generic HTTP webhook for custom integrations",
        icon_svg: ICON_WEBHOOK,
        brand_color: "#8B5CF6",
        config_section: "channels.webhook",
        fields: WEBHOOK_FIELDS,
        docs_url: "https://en.wikipedia.org/wiki/Webhook",
    },
    // 12. XMPP
    ChannelDefinition {
        id: "xmpp",
        name: "XMPP",
        description: "Connect via XMPP/Jabber protocol with MUC support",
        icon_svg: ICON_XMPP,
        brand_color: "#002B5C",
        config_section: "channels.xmpp",
        fields: XMPP_FIELDS,
        docs_url: "https://xmpp.org/extensions/",
    },
    // 13. Nostr
    ChannelDefinition {
        id: "nostr",
        name: "Nostr",
        description: "Connect to Nostr decentralized network via relays",
        icon_svg: ICON_NOSTR,
        brand_color: "#8B5CF6",
        config_section: "channels.nostr",
        fields: NOSTR_FIELDS,
        docs_url: "https://github.com/nostr-protocol/nips",
    },
];
