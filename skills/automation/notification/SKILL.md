---
name: notification
description: Multi-channel notifications — macOS native, ntfy.sh, Telegram, webhooks via curl and osascript
emoji: "🔔"
category: automation
allowed-tools:
  - Bash
requirements:
  binaries:
    - curl
    - osascript
  platforms:
    - macos
triggers:
  - notify
  - alert
  - notification
  - remind me
  - send message
---

# Multi-Channel Notifications

## When to Use

Invoke this skill when you need to notify the user about task completion, errors, or events. Supports multiple channels with zero additional dependencies on macOS.

## Channel Overview

| Channel | Setup Required | Best For |
|---------|---------------|----------|
| macOS native | None | Immediate local alerts |
| ntfy.sh | None (free, no signup) | Phone/desktop push notifications |
| Telegram Bot | Bot token + chat ID | Remote messaging |
| Webhook | URL only | Integration with any service |

## Core Operations

### macOS Native Notifications

```bash
# Basic notification
osascript -e 'display notification "Build completed successfully" with title "Aleph" subtitle "CI/CD"'

# With sound
osascript -e 'display notification "Tests passed" with title "Aleph" sound name "Glass"'

# Voice synthesis (speaks aloud)
say "Build complete. All tests passing."

# Voice with specific voice
say -v Samantha "Your deployment is ready."
```

### ntfy.sh (Zero-Config Push)

```bash
# Send notification (anyone with the topic can receive)
curl -s -d "Build #42 completed" ntfy.sh/my-aleph-alerts

# With title and priority
curl -s -H "Title: Deploy Complete" -H "Priority: high" -d "v2.1.0 deployed to production" ntfy.sh/my-aleph-alerts

# With tags (emoji)
curl -s -H "Title: Tests Passed" -H "Tags: white_check_mark" -d "All 142 tests green" ntfy.sh/my-aleph-alerts

# Urgent (bypasses DND on phones)
curl -s -H "Priority: urgent" -H "Tags: rotating_light" -d "Production server down!" ntfy.sh/my-aleph-alerts

# With click URL
curl -s -H "Click: https://github.com/org/repo/pull/123" -d "PR #123 ready for review" ntfy.sh/my-aleph-alerts
```

Subscribe at: `https://ntfy.sh/my-aleph-alerts` (web) or ntfy app (iOS/Android).

### Telegram Bot

```bash
# Send message (requires BOT_TOKEN and CHAT_ID)
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendMessage" \
  -H "Content-Type: application/json" \
  -d "{\"chat_id\": \"$TELEGRAM_CHAT_ID\", \"text\": \"Build complete ✅\", \"parse_mode\": \"Markdown\"}"

# Send with inline keyboard
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendMessage" \
  -H "Content-Type: application/json" \
  -d "{\"chat_id\": \"$TELEGRAM_CHAT_ID\", \"text\": \"PR #123 ready\", \"reply_markup\": {\"inline_keyboard\": [[{\"text\": \"View PR\", \"url\": \"https://github.com/org/repo/pull/123\"}]]}}"

# Send document
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendDocument" \
  -F "chat_id=$TELEGRAM_CHAT_ID" -F "document=@report.pdf"
```

### Generic Webhook

```bash
# Slack-compatible webhook
curl -s -X POST "$WEBHOOK_URL" \
  -H "Content-Type: application/json" \
  -d '{"text": "Deployment complete: v2.1.0 → production"}'

# Discord webhook
curl -s -X POST "$DISCORD_WEBHOOK" \
  -H "Content-Type: application/json" \
  -d '{"content": "Build #42 passed all checks ✅"}'
```

## Notification Routing by Urgency

| Urgency | Channels | Example |
|---------|----------|---------|
| **Urgent** | macOS + ntfy (urgent) + Telegram | Server down, security breach |
| **High** | macOS + ntfy (high) | Build failed, test regression |
| **Normal** | ntfy (default) | Task complete, PR merged |
| **Low** | Batched, defer to quiet hours | Info updates, metrics |

## Anti-Patterns

- **Notification fatigue**: Don't notify for every small event. Batch low-priority items.
- **Quiet hours**: Respect sleep schedules — defer non-urgent notifications.
- **Duplicate channels**: Don't send the same alert to all channels simultaneously.
- **Secrets in notifications**: Never include passwords, tokens, or keys in notification text.

## Aleph Integration

- Aleph R6 principle ("AI Comes to You"): notifications are a core delivery mechanism
- Synergy with `deploy` (W4): deploy complete → notify
- Synergy with `ci-cd` (W7): CI failure → notify
- Synergy with any long-running task: completion → notify user
