---
name: email
description: Email automation — send (SMTP) and receive (IMAP) emails via curl
emoji: "📧"
category: automation
cli-wrapper: true
requirements:
  binaries:
    - curl
  platforms:
    - macos
    - linux
triggers:
  - email
  - send email
  - check mail
  - SMTP
  - IMAP
---

# Email Automation (curl)

## When to Use

Invoke this skill when you need to send or receive emails programmatically — deployment notifications, automated reports, code review requests, or inbox checking. Uses curl only, zero additional dependencies.

## Security Rules

- **Never use main password** — always use App Passwords or OAuth tokens
- **Store credentials in env vars** — never hardcode in commands or scripts
- **TLS mandatory** — always use `smtps://` (port 465) or STARTTLS (port 587)
- **Test with your own address first** — verify before sending to others

## Provider Configuration

| Provider | SMTP Server | Port | IMAP Server | Port |
|----------|------------|------|-------------|------|
| Gmail | smtp.gmail.com | 465 | imap.gmail.com | 993 |
| Outlook | smtp.office365.com | 587 | outlook.office365.com | 993 |
| iCloud | smtp.mail.me.com | 587 | imap.mail.me.com | 993 |
| Yahoo | smtp.mail.yahoo.com | 465 | imap.mail.yahoo.com | 993 |
| 163.com | smtp.163.com | 465 | imap.163.com | 993 |
| QQ Mail | smtp.qq.com | 465 | imap.qq.com | 993 |

### Gmail App Password Setup

1. Go to https://myaccount.google.com/apppasswords
2. Generate an App Password for "Mail"
3. Store it: `export EMAIL_PASSWORD="xxxx xxxx xxxx xxxx"`

## Send Email (SMTP)

### Basic Send

```bash
curl --ssl-reqd \
  --url "smtps://smtp.gmail.com:465" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  --mail-from "$EMAIL_USER" \
  --mail-rcpt "recipient@example.com" \
  -T - <<EOF
From: $EMAIL_USER
To: recipient@example.com
Subject: Build Report - $(date +%Y-%m-%d)
Content-Type: text/plain; charset=utf-8

Build #42 completed successfully.

- Tests: 142 passed, 0 failed
- Coverage: 87.3%
- Duration: 4m 32s
EOF
```

### HTML Email

```bash
curl --ssl-reqd \
  --url "smtps://smtp.gmail.com:465" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  --mail-from "$EMAIL_USER" \
  --mail-rcpt "recipient@example.com" \
  -T - <<EOF
From: $EMAIL_USER
To: recipient@example.com
Subject: Weekly Report
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8

<h1>Weekly Summary</h1>
<ul>
  <li>PRs merged: 5</li>
  <li>Issues closed: 8</li>
  <li>Deploys: 2</li>
</ul>
EOF
```

### With Attachment

```bash
# Generate MIME with boundary
BOUNDARY="==boundary_$(date +%s)=="
curl --ssl-reqd \
  --url "smtps://smtp.gmail.com:465" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  --mail-from "$EMAIL_USER" \
  --mail-rcpt "recipient@example.com" \
  -T - <<EOF
From: $EMAIL_USER
To: recipient@example.com
Subject: Report with attachment
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="$BOUNDARY"

--$BOUNDARY
Content-Type: text/plain; charset=utf-8

Please find the report attached.

--$BOUNDARY
Content-Type: application/octet-stream; name="report.csv"
Content-Disposition: attachment; filename="report.csv"
Content-Transfer-Encoding: base64

$(base64 report.csv)
--$BOUNDARY--
EOF
```

## Receive Email (IMAP)

```bash
# Check mailbox status (message count)
curl -s --url "imaps://imap.gmail.com:993/INBOX" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  -X "STATUS INBOX (MESSAGES UNSEEN)"

# List recent message subjects (last 5)
curl -s --url "imaps://imap.gmail.com:993/INBOX" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  -X "FETCH 1:5 (BODY[HEADER.FIELDS (FROM SUBJECT DATE)])"

# Search for messages
curl -s --url "imaps://imap.gmail.com:993/INBOX" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  -X "SEARCH UNSEEN FROM \"github.com\""

# Fetch specific message body
curl -s --url "imaps://imap.gmail.com:993/INBOX;UID=123" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD"
```

## Email Templates

### Code Review Request
```
Subject: Code Review Request: [PR Title]

Hi [Reviewer],

PR #[number] is ready for review.

Changes: [1-2 sentence summary]
Link: [PR URL]
Estimated review time: [X minutes]

Key areas to focus on:
- [Area 1]
- [Area 2]
```

### Deployment Notification
```
Subject: [ENV] Deployed v[version] - [status]

Deployment Summary:
- Environment: [staging/production]
- Version: [version]
- Status: [success/failed]
- Time: [timestamp]
- Deployer: [name]

Changes included:
- [commit 1]
- [commit 2]
```

## Gotchas

- **Gmail blocks "less secure apps"**: Must use App Password, not account password
- **curl IMAP quirks**: IMAP commands via curl are limited; for complex inbox operations, consider Python's `imaplib`
- **Rate limits**: Gmail limits to ~500 emails/day for personal accounts
- **Encoding**: Always set `charset=utf-8` in headers for international text

## Aleph Integration

- Synergy with `notification` (A7): email for formal communications, ntfy for quick alerts
- Synergy with `deploy` (W4): deployment → email stakeholders
- Synergy with `github` (A1): PR events → email notifications
