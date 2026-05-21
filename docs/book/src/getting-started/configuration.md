# Configuration

Aleph uses a JSON5 configuration file at `~/.aleph/config.json`.

## Basic Structure

```json5
{
  // Agent configuration
  agents: {
    defaults: {
      workspace: "~/aleph-workspace",
      model: "anthropic/claude-sonnet-4",
      thinking: "medium",
    },
    list: [{
      id: "main",
      identity: "You are Aleph, a helpful personal AI assistant.",
    }],
  },

  // Gateway configuration
  gateway: {
    port: 18790,
    bind: "loopback",  // or "all"
    require_auth: false,
  },

  // Channel configuration
  channels: {
    telegram: {
      enabled: false,
      token: "BOT_TOKEN",
    },
    discord: {
      enabled: false,
      token: "BOT_TOKEN",
    },
  },

  // Session configuration
  session: {
    dmScope: "per-peer",
    autoResetHour: 4,
    expiryDays: 30,
  },
}
```

## Agent Configuration

### Model Selection

```json5
agents: {
  defaults: {
    // Provider/model format
    model: "anthropic/claude-opus-4-5",

    // Failover chain
    models: [
      "anthropic/claude-opus-4-5",
      "openai/gpt-4o",
      "google/gemini-pro"
    ],
  }
}
```

### Thinking Levels

| Level | Description |
|-------|-------------|
| `off` | No extended thinking |
| `minimal` | Brief reasoning |
| `low` | Quick analysis |
| `medium` | Balanced (default) |
| `high` | Deep analysis |
| `xhigh` | Maximum depth |

### Identity

```json5
agents: {
  list: [{
    id: "main",
    identity: "You are Aleph, a personal AI assistant. Be helpful, concise, and accurate.",

    // Group chat behavior
    groupChat: {
      requireMention: true,
      prefix: "@aleph"
    }
  }]
}
```

## Gateway Configuration

```json5
gateway: {
  // Port to listen on
  port: 18790,

  // Bind address
  bind: "loopback",  // localhost only
  // bind: "all",    // all interfaces

  // Authentication
  require_auth: true,

  // TLS (optional)
  tls: {
    cert: "/path/to/cert.pem",
    key: "/path/to/key.pem"
  }
}
```

## Channel Configuration

### Telegram

```json5
channels: {
  telegram: {
    enabled: true,
    token: "123456:ABC...",

    // Allowlist (optional)
    allowFrom: ["+1234567890"],

    // Group behavior
    groups: {
      "*": { requireMention: true }
    }
  }
}
```

### Discord

```json5
channels: {
  discord: {
    enabled: true,
    token: "YOUR_BOT_TOKEN",

    // Guild/server restrictions
    allowGuilds: ["guild-id-1", "guild-id-2"],
  }
}
```

## Session Configuration

```json5
session: {
  // DM scope strategy
  dmScope: "per-peer",  // or "main", "per-channel-peer"

  // Auto-reset hour (0-23, local time)
  autoResetHour: 4,

  // Session expiry
  expiryDays: 30,

  // Compaction
  compaction: {
    enabled: true,
    threshold: 50000,  // tokens
  }
}
```

## Environment Variables

Configuration values can use environment variables:

```json5
{
  channels: {
    telegram: {
      token: "${TELEGRAM_BOT_TOKEN}"
    }
  }
}
```

## Hot Reload

Configuration changes are automatically detected and applied:

```bash
# Manual reload
aleph config reload

# Watch for changes
aleph gateway run --watch-config
```

## CLI Configuration

```bash
# Get a value
aleph config get gateway.port

# Set a value
aleph config set gateway.port 18790

# Open in editor
aleph config edit

# Validate configuration
aleph config validate
```
