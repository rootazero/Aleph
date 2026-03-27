# Email Recipes

## Table of Contents
- [Send: Basic](#basic-send)
- [Send: HTML](#html-email)
- [Send: With Attachment](#with-attachment)
- [Receive: IMAP](#receive-email-imap)
- [Templates](#email-templates)

## Basic Send

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

## HTML Email

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

## With Attachment

```bash
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
# Check mailbox status
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
