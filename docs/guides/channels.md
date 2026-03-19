# Channels Configuration Guide

## File Path
- Main: `~/.aleph/config.toml` section `[channels.<name>]`
- Channel secrets: encrypted vault (key: `channel:{type}:{id}`)

## Operation Rules
1. Before modification: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. Bot tokens via `vault_store(action="store", key="channel:<type>:<id>", secret="<token>")`
3. After modification: requires restart for channel connections

## Structure

Channel configs are opaque JSON — each platform has its own fields:

```toml
[channels.my-telegram-bot]
# Type auto-inferred from key if it matches a known platform
bot_token_key = "channel:telegram:my-telegram-bot"  # Vault key reference

[channels.my-discord-bot]
type = "discord"           # Required if key doesn't match known platform
guild_id = "123456789"
```

## Known Platform Types

Auto-inferred from channel key name:
telegram, discord, whatsapp, slack, imessage, email, matrix, signal, mattermost, irc, webhook, xmpp, nostr

If key doesn't match any of these, add explicit `type = "..."` field.

## Common Operations

### Add a Telegram bot
1. Store token: `vault_store(action="store", key="channel:telegram:mybot", secret="<bot_token>")`
2. Add config:
```toml
[channels.telegram]
bot_token_key = "channel:telegram:mybot"
```
3. Restart Aleph for channel to connect

### Disable a channel
Remove or comment out the `[channels.<name>]` section. Restart required.

## Caveats
- Channel changes require server restart (not hot-reloaded)
- Bot tokens should always use vault_store, never plaintext in config
- Each channel type has platform-specific fields — consult platform docs
- Channel → Agent routing is configured via `[[bindings]]` section
