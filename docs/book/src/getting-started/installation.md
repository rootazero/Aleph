# Installation

This guide covers installing Aether on your system.

## Prerequisites

- **Rust 1.75+** - Required for building from source
- **macOS 14+** or **Linux** - Primary supported platforms
- **SQLite 3.35+** - For session storage

## Installation Methods

### From Source (Recommended)

```bash
# Clone the repository
git clone https://github.com/anthropics/aether.git
cd aether

# Build the core library and CLI
cargo build --release -p aethecore --features gateway

# Install the CLI
cargo install --path core --bin aether-gateway
```

### Using Cargo

```bash
cargo install aether-cli
```

### Homebrew (macOS)

```bash
# Coming soon
brew install aether
```

## Verify Installation

```bash
# Check version
aether --version

# Run health check
aether health
```

## Directory Structure

Aether creates the following directories:

| Path | Purpose |
|------|---------|
| `~/.aether/` | Main configuration directory |
| `~/.aether/config.json` | Primary configuration file |
| `~/.aether/sessions/` | SQLite session databases |
| `~/.aether/logs/` | Application logs |
| `~/.aether/plugins/` | Installed plugins |

## Next Steps

- [Quick Start](./quick-start.md) - Get up and running
- [Configuration](./configuration.md) - Configure your setup
