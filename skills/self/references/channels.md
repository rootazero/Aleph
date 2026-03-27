# Channel Configuration

## Telegram

```toml
[channels.MyBot]
type = "telegram"
bot_token = "123456:ABC..."          # REQUIRED (from @BotFather)
bot_username = "MyBot"                # Optional
allowed_users = []                    # Empty = allow all, or [user_id1, user_id2]
allowed_groups = []                   # Empty = allow all
dm_allowed = true                     # Default true
groups_allowed = true                 # Default true
polling_interval_secs = 1             # Default 1
send_typing = true                    # Default true
max_retries = 3                       # Default 3

# Optional webhook mode (default is long-polling)
[channels.MyBot.webhook]
url = "https://example.com/webhook"   # Public HTTPS URL
port = 8443                           # Default 8443
path = "/telegram/webhook"            # Default "/telegram/webhook"
certificate = "/path/to/cert.pem"     # Optional SSL cert
secret_token = "random-string"        # Optional webhook verification
```

## Discord

```toml
[channels.MyDiscordBot]
type = "discord"
bot_token = "MTIz..."                 # REQUIRED (from Developer Portal)
application_id = 123456789            # Optional (for slash commands)
allowed_guilds = []                   # Empty = allow all
allowed_channels = []                 # Empty = allow all
dm_allowed = true                     # Default true
command_prefix = "!"                  # Default "!"
respond_to_mentions = true            # Default true
slash_commands_enabled = true          # Default true
send_typing = true                    # Default true

[channels.MyDiscordBot.intents]
guild_messages = true                 # Default true
direct_messages = true                # Default true
message_content = true                # Default true (privileged intent)
guild_members = false                 # Default false
```

## Webhook (Generic)

```toml
[channels.MyWebhook]
type = "webhook"
secret = "hmac-secret"               # REQUIRED (HMAC-SHA256)
callback_url = "https://..."          # REQUIRED (POST target)
path = "/webhook/generic"            # Default, must start with /
allowed_senders = []                  # Empty = allow all
```
