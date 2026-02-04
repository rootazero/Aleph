# Aleph Documentation

## Current Documentation

All current project guidance is in the root **CLAUDE.md** file.

**CLAUDE.md** is the single source of truth for:
- Project vision (Rust 版 Moltbot)
- Architecture design (Gateway 控制面)
- Technical decisions
- Development guidelines

## Directory Structure

```
docs/
├── README.md          # This file
├── assets/            # Images and static assets
└── legacy/            # Archived documentation (pre-Moltbot evolution)
    ├── *.md           # Old documentation files
    ├── architecture/  # Old architecture docs
    ├── core/          # Old core module docs
    ├── plans/         # Old design plans
    └── archive-plans/ # Previously archived plans
```

## Legacy Documentation

The `legacy/` directory contains all documentation from before the Moltbot-inspired architecture evolution (2026-01-28).

These documents are preserved for historical reference but should **NOT** be used as guidance for new development.

## New Development

All new development should follow **CLAUDE.md** exclusively.
