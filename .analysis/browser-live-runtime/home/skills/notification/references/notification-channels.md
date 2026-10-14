# Notification Channel Details

## macOS Native Notifications

```bash
# Basic notification
osascript -e 'display notification "Build completed successfully" with title "Aleph" subtitle "CI/CD"'

# With sound
osascript -e 'display notification "Tests passed" with title "Aleph" sound name "Glass"'

# Voice synthesis
say "Build complete. All tests passing."
say -v Samantha "Your deployment is ready."
say -v Alex "Server restart completed."
say -v Ting-Ting "构建完成，所有测试通过。"

# Adjust speaking rate (words per minute, default ~175)
say -r 250 "Quick notification, build passed."
say -r 100 "Slow and clear: deployment failed, please investigate."

# Save speech to audio file
say -v Samantha -o alert.aiff "Build 42 failed. Check CI logs."

# Multi-modal alert
osascript -e 'display notification "Build failed" with title "Aleph" sound name "Basso"' && say -v Samantha "Build failed."

# List all available voices
say -v '?'
```

### Voice Selection

| Use Case | Voice | Why |
|----------|-------|-----|
| Quick status alerts | `Fred` | Fast, low-overhead |
| Important announcements | `Samantha` / `Alex` | Clear, natural |
| Chinese content | `Ting-Ting` | Native Mandarin |
| Urgent warnings | `say -r 100` | Forces attention |
| Audio logging | `say -o file.aiff` | Record for async review |

## ntfy.sh (Zero-Config Push)

```bash
curl -s -d "Build #42 completed" ntfy.sh/my-aleph-alerts
curl -s -H "Title: Deploy Complete" -H "Priority: high" -d "v2.1.0 deployed" ntfy.sh/my-aleph-alerts
curl -s -H "Title: Tests Passed" -H "Tags: white_check_mark" -d "All 142 tests green" ntfy.sh/my-aleph-alerts
curl -s -H "Priority: urgent" -H "Tags: rotating_light" -d "Production server down!" ntfy.sh/my-aleph-alerts
curl -s -H "Click: https://github.com/org/repo/pull/123" -d "PR #123 ready" ntfy.sh/my-aleph-alerts
```

Subscribe: `https://ntfy.sh/my-aleph-alerts` (web) or ntfy app (iOS/Android).

## Telegram Bot

```bash
# Send message
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendMessage" \
  -H "Content-Type: application/json" \
  -d "{\"chat_id\": \"$TELEGRAM_CHAT_ID\", \"text\": \"Build complete\", \"parse_mode\": \"Markdown\"}"

# Send with inline keyboard
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendMessage" \
  -H "Content-Type: application/json" \
  -d "{\"chat_id\": \"$TELEGRAM_CHAT_ID\", \"text\": \"PR #123 ready\", \"reply_markup\": {\"inline_keyboard\": [[{\"text\": \"View PR\", \"url\": \"https://github.com/org/repo/pull/123\"}]]}}"

# Send document
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendDocument" \
  -F "chat_id=$TELEGRAM_CHAT_ID" -F "document=@report.pdf"
```

## Generic Webhook

```bash
# Slack
curl -s -X POST "$WEBHOOK_URL" \
  -H "Content-Type: application/json" \
  -d '{"text": "Deployment complete: v2.1.0"}'

# Discord
curl -s -X POST "$DISCORD_WEBHOOK" \
  -H "Content-Type: application/json" \
  -d '{"content": "Build #42 passed all checks"}'
```
