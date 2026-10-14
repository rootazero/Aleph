---
name: notification
description: "Multi-channel notifications — macOS native alerts, ntfy.sh push, Telegram bot, webhooks (Slack/Discord) via curl and osascript. Use when notifying about task completion, errors, events, or any alert/reminder. Supports voice synthesis (say), push to phone, and webhook integrations."
---

# Multi-Channel Notifications

## Channel Overview

| Channel | Setup Required | Best For |
|---------|---------------|----------|
| macOS native | None | Immediate local alerts |
| ntfy.sh | None (free, no signup) | Phone/desktop push |
| Telegram Bot | Bot token + chat ID | Remote messaging |
| Webhook | URL only | Integration with any service |

## Quick Reference

| Task | Command |
|------|---------|
| Local popup | `osascript -e 'display notification "msg" with title "T"'` |
| With sound | `osascript -e 'display notification "msg" with title "T" sound name "Glass"'` |
| Push to phone | `curl -d "msg" ntfy.sh/my-topic` |
| Urgent push | `curl -H "Priority: urgent" -d "msg" ntfy.sh/my-topic` |
| Speak aloud | `say "message"` or `say -v Samantha "message"` |

For detailed examples per channel (macOS, ntfy, Telegram, webhooks, voice), see [references/notification-channels.md](references/notification-channels.md).

## Routing by Urgency

| Urgency | Channels | Example |
|---------|----------|---------|
| **Urgent** | macOS + ntfy (urgent) + Telegram | Server down, security breach |
| **High** | macOS + ntfy (high) | Build failed, test regression |
| **Normal** | ntfy (default) | Task complete, PR merged |
| **Low** | Batched, defer to quiet hours | Info updates, metrics |

## Anti-Patterns

- **Notification fatigue**: Don't notify for every small event. Batch low-priority items.
- **Quiet hours**: Defer non-urgent notifications.
- **Duplicate channels**: Don't send the same alert to all channels simultaneously.
- **Secrets in notifications**: Never include passwords, tokens, or keys in notification text.

## Aleph Integration

- Synergy with `deploy` (W4): deploy complete -> notify
- Synergy with `ci-cd` (W7): CI failure -> notify
- Synergy with any long-running task: completion -> notify user
