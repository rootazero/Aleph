# TOOLS.md — Local Tool Notes

Skills define _how_ tools work. This file is for _your_ specifics — the stuff
that's unique to your setup and environment.

## What Goes Here

Environment-specific details that help the agent use tools effectively:

- SSH hosts and aliases
- Preferred voices for TTS
- Device nicknames and locations
- API endpoints and credentials references
- File paths and project locations
- Anything environment-specific

## Examples

```markdown
### SSH

- home-server → 192.168.1.100, user: admin
- dev-box → dev.example.com, key: ~/.ssh/dev_rsa

### Projects

- Main workspace: ~/Workspace/MyProject
- Config files: ~/.config/myapp/

### TTS

- Preferred voice: "Nova" (warm, slightly British)
- Default speaker: Kitchen HomePod
```

## Why Separate?

Skills are shared. Your setup is yours. Keeping them apart means you can update
skills without losing your notes, and share skills without leaking your
infrastructure.

---

_Add whatever helps you do your job. This is your cheat sheet._
