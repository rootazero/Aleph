---
name: email
description: "Email automation — send and receive emails via curl using SMTP/IMAP. Use when sending emails (deployment notifications, automated reports, code review requests), checking inbox, searching mail, or any email task. Zero dependencies beyond curl."
---

# Email Automation (curl)

## Security Rules

- **Never use main password** -- always use App Passwords or OAuth tokens
- **Store credentials in env vars** -- never hardcode in commands
- **TLS mandatory** -- always use `smtps://` (port 465) or STARTTLS (port 587)
- **Test with your own address first**

## Provider Configuration

| Provider | SMTP Server | Port | IMAP Server | Port |
|----------|------------|------|-------------|------|
| Gmail | smtp.gmail.com | 465 | imap.gmail.com | 993 |
| Outlook | smtp.office365.com | 587 | outlook.office365.com | 993 |
| iCloud | smtp.mail.me.com | 587 | imap.mail.me.com | 993 |
| Yahoo | smtp.mail.yahoo.com | 465 | imap.mail.yahoo.com | 993 |
| 163.com | smtp.163.com | 465 | imap.163.com | 993 |
| QQ Mail | smtp.qq.com | 465 | imap.qq.com | 993 |

**Gmail App Password**: https://myaccount.google.com/apppasswords -> generate for "Mail" -> `export EMAIL_PASSWORD="xxxx xxxx xxxx xxxx"`

## Send/Receive Recipes

For complete examples (basic send, HTML, attachments, IMAP receive, templates), see [references/email-recipes.md](references/email-recipes.md).

## Gotchas

- **Gmail blocks "less secure apps"**: Must use App Password, not account password
- **curl IMAP quirks**: Limited for complex inbox ops; consider Python `imaplib`
- **Rate limits**: Gmail limits ~500 emails/day for personal accounts
- **Encoding**: Always set `charset=utf-8` for international text

## Aleph Integration

- Synergy with `notification` (A7): email for formal communications, ntfy for quick alerts
- Synergy with `deploy` (W4): deployment -> email stakeholders
- Synergy with `github` (A1): PR events -> email notifications
