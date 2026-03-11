# Contributing to Aleph

We welcome contributions to Aleph! Before you begin, please read through this guide.

## Contributor License Agreement (CLA)

All contributors must agree to our [Contributor License Agreement](CLA.md). By submitting a pull request, you automatically indicate your agreement to the CLA terms. Your Git commit metadata (name and email) serves as your electronic signature.

The CLA ensures that:
- You have the right to make the contribution
- The Maintainer can continue to distribute the project under AGPL-3.0 and offer commercial licenses

## Getting Started

1. Fork the repository
2. Create a branch for your changes
3. Make your changes following the guidelines below
4. Submit a pull request

## Development Guidelines

### Language

- **Code comments**: English
- **Commit messages**: English, format: `<scope>: <description>`
- **Discussion**: Chinese or English

### Code Quality

- Run `cargo check -p alephcore` before submitting
- Run `cargo test -p alephcore --lib` to verify tests pass
- Run `just clippy` for linting

### Architecture

Please review [CLAUDE.md](CLAUDE.md) for architectural redlines and design principles. Key rules:

- Core must not depend on platform-specific APIs (R1)
- Business logic lives in Leptos/WASM, not Tauri (R2)
- Prefer Skills/MCP Servers over adding heavy dependencies to core (R3)

## License

By contributing, you agree that your contributions will be licensed under the [AGPL-3.0-or-later](LICENSE) license, subject to the terms of the [CLA](CLA.md).
