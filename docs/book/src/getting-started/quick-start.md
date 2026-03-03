# Quick Start

Get Aleph running in under 5 minutes.

## 1. Run the Setup Wizard

The setup wizard guides you through initial configuration:

```bash
aleph wizard
```

This will:
- Configure your AI provider (Claude, GPT, Gemini, etc.)
- Set up API credentials
- Choose your preferred model and thinking level
- Optionally connect messaging apps

## 2. Start the Gateway

```bash
# Start in foreground
aleph gateway run

# Or start as daemon
aleph gateway run --daemon
```

The gateway listens on `ws://127.0.0.1:18790/ws` by default.

## 3. Chat with Aleph

```bash
# Simple chat
aleph chat "What's the weather like today?"

# With specific thinking level
aleph chat --thinking high "Explain quantum computing"

# Interactive mode
aleph chat --interactive
```

## 4. Connect a Messaging App (Optional)

### Telegram

```bash
# Set your bot token
aleph config set channels.telegram.token "YOUR_BOT_TOKEN"

# Start the channel
aleph channels start telegram
```

### Discord

```bash
# Set your bot token
aleph config set channels.discord.token "YOUR_BOT_TOKEN"

# Start the channel
aleph channels start discord
```

## Next Steps

- [Configuration](./configuration.md) - Customize your setup
- [Gateway RPC](../gateway/protocol.md) - Learn the API
- [Interfaces](../interfaces/overview.md) - Connect more platforms
