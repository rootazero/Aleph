# Aether

> *"This is the first time in human history that a machine's soul has been given a body."*
> — Ghost in the Shell

**Aether** is a powerful personal AI assistant built in Rust, designed to give AI the ability to interact with the world. It combines a high-performance Gateway control plane with multi-channel messaging support, enabling AI agents to work across various platforms seamlessly.

## Key Features

- **Multi-Provider AI** - Claude, GPT, Gemini, Ollama with automatic failover
- **Multi-Channel** - Telegram, Discord, iMessage, Slack, WebChat
- **Native Performance** - 100% Rust core, no interpreted languages
- **Secure Execution** - Three-tier security model for command execution
- **Extensible** - MCP integration, plugin system, custom tools
- **Self-Hosted** - Run on your own hardware, full control

## Architecture

```
                     Channels
         ┌─────────────────────────────┐
         │ Telegram │ Discord │ iMessage│
         └──────────────┬──────────────┘
                        │
              ┌─────────▼─────────┐
              │      Gateway      │
              │  ws://127.0.0.1   │
              │    JSON-RPC 2.0   │
              └─────────┬─────────┘
                        │
         ┌──────────────┼──────────────┐
         │              │              │
    ┌────▼────┐   ┌─────▼─────┐  ┌────▼────┐
    │  Agent  │   │  Session  │  │  Tools  │
    │  Loop   │   │  Manager  │  │ Registry│
    └─────────┘   └───────────┘  └─────────┘
```

## Quick Start

```bash
# Install via cargo
cargo install aether-cli

# Run the setup wizard
aether wizard

# Start chatting
aether chat "Hello, Aether!"
```

## Documentation Sections

| Section | Description |
|---------|-------------|
| [Getting Started](./getting-started/installation.md) | Installation and initial setup |
| [Architecture](./architecture/overview.md) | System design and components |
| [Gateway RPC](./gateway/protocol.md) | WebSocket API reference |
| [Channels](./channels/overview.md) | Messaging platform integrations |
| [Security](./security/exec-approval.md) | Security model and approvals |
| [CLI Reference](./cli/commands.md) | Command-line interface |
| [Development](./development/building.md) | Building and contributing |

## Philosophy

Aether is built on a five-layer emergence architecture:

1. **Sea of Knowledge** - AI's pre-training foundation
2. **Domain Classification** - Organized expertise
3. **Atomic Skills** - Know-what → Know-how
4. **Functional Modules** - Composable capabilities
5. **Polymorphic Agents** - Soul gains body

The goal is not just to build a tool, but to explore a path toward AGI - when intelligence gains the ability to act.

## License

Apache 2.0
