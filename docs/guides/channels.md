# Channels Configuration Guide

## File Path
- Main: `~/.aleph/config.toml` section `[channels.<name>]`
- Channel secrets: encrypted vault (key: `channel:{instance_id}:{secret_field}`, e.g. `channel:telegram:bot_token`)

## Operation Rules
1. Before modification: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. Two ways to set a secret:
   - Put the secret field (e.g. `bot_token = "..."`) directly in the channel
     section — on next load it is auto-migrated into the vault and stripped from
     config; **or**
   - Store it explicitly: `vault_store(action="store", key="channel:<instance_id>:bot_token", secret="<token>")`
3. After modification: requires restart for channel connections

## Structure

Channel configs are opaque JSON — each platform has its own fields:

```toml
[channels.my-telegram-bot]
type = "telegram"          # Explicit type (also auto-inferred when the section name matches a platform)
bot_token = "123456:ABC..."   # Auto-migrated into vault on load, then stripped from config

[channels.my-discord-bot]
type = "discord"
bot_token = "MTIz..."
```

## Known Platform Types

Auto-inferred from channel section name:
telegram, discord, whatsapp, slack, imessage, email, matrix, signal, mattermost, irc, webhook, xmpp, nostr, feishu, qq

If the section name doesn't match any of these, add an explicit `type = "..."` field.

## Common Operations

### Add a Telegram bot
1. Store token: `vault_store(action="store", key="channel:telegram:mybot", secret="<bot_token>")`
2. Add config:
```toml
[channels.telegram]
type = "telegram"
# bot_token already in vault from step 1; omit it here (or paste it and let Aleph migrate it)
```
3. Restart Aleph for channel to connect

### Disable a channel
Remove or comment out the `[channels.<name>]` section. Restart required.

## Caveats
- Channel changes require server restart (not hot-reloaded)
- Bot tokens should always use vault_store, never plaintext in config
- Each channel type has platform-specific fields — consult platform docs
- Channel → Agent routing is configured via `[[bindings]]` section
