# Changelog

All notable changes to the Aleph project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Added "Conversation Modes" section to CLAUDE.md documenting single-turn (FROZEN) and multi-turn (ACTIVE) mode boundaries
- Added "Conversation Modes" chapter to ARCHITECTURE.md with detailed implementation and data flow diagrams
- Added conversation history saving logic in Agent Loop for multi-turn mode (`agent_loop.rs:319-352`)
- Added automatic history trimming (max 50 messages = 25 turns) to prevent memory bloat
- Added 🔒 FROZEN and ✅ ACTIVE emoji markers throughout codebase to clarify mode boundaries

### Fixed
- **Critical**: Fixed missing conversation history saving in Agent Loop, enabling proper context injection for multi-turn conversations
- Multi-turn conversations now correctly accumulate and inject conversation history from previous turns

### Changed
- Enhanced `ProcessOptions.topic_id` documentation with comprehensive mode explanation
- Improved code comments in `orchestration.rs`, `agent_loop.rs`, and `prompt_helpers.rs` with mode-specific annotations
- Updated function documentation for `build_history_summary_from_conversations` to clarify single-turn vs multi-turn behavior

### Developer Notes
- **Single-turn mode** is now feature-locked (FROZEN). All future enhancements target multi-turn mode.
- Development constraint: Modifications to single-turn code paths require explicit approval (bug fixes only)
- Multi-turn mode is the active development focus for AI agent capabilities

---

## [0.1.0] - 2026-01-27

### Project Status
- Phase 9 Complete: Agent Loop Hardening
- Established architectural boundary between single-turn and multi-turn conversation modes
- Clarified development direction: single-turn frozen, multi-turn active

---

## Format Guidelines

### Types of Changes
- **Added** for new features
- **Changed** for changes in existing functionality
- **Deprecated** for soon-to-be removed features
- **Removed** for now removed features
- **Fixed** for any bug fixes
- **Security** in case of vulnerabilities

### Commit Message Convention
```
<type>(<scope>): <subject>

<body>

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`
