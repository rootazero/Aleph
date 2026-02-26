---
name: http-client
description: API debugging and testing — RESTful requests, authentication, response analysis via curl
emoji: "🌐"
category: automation
cli-wrapper: true
requirements:
  binaries:
    - curl
triggers:
  - curl
  - API test
  - HTTP request
  - REST call
  - endpoint
  - webhook
---

# API Debugging (curl)

## When to Use

Invoke this skill when testing APIs, debugging HTTP requests, or validating endpoint behavior. Uses curl exclusively — pre-installed on macOS/Linux, zero additional dependencies.

## Core Operations

### GET Requests

```bash
# Basic GET
curl -s https://api.example.com/users | jq .

# With headers
curl -s -H "Authorization: Bearer $TOKEN" https://api.example.com/users | jq .

# With query parameters
curl -s "https://api.example.com/users?page=2&limit=10" | jq .

# Save response to file
curl -s https://api.example.com/data -o response.json
```

### POST / PUT / PATCH / DELETE

```bash
# POST JSON
curl -s -X POST https://api.example.com/users \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice", "email": "alice@example.com"}' | jq .

# PUT (full update)
curl -s -X PUT https://api.example.com/users/1 \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice Updated", "email": "alice@example.com"}' | jq .

# PATCH (partial update)
curl -s -X PATCH https://api.example.com/users/1 \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice Patched"}' | jq .

# DELETE
curl -s -X DELETE https://api.example.com/users/1 -w "\nHTTP %{http_code}\n"
```

### File Upload

```bash
# Multipart form upload
curl -s -X POST https://api.example.com/upload \
  -F "file=@/path/to/document.pdf" \
  -F "description=My document" | jq .

# Multiple files
curl -s -X POST https://api.example.com/upload \
  -F "files[]=@file1.png" \
  -F "files[]=@file2.png" | jq .
```

### Authentication Patterns

```bash
# Bearer token
curl -s -H "Authorization: Bearer $TOKEN" https://api.example.com/me | jq .

# Basic auth
curl -s -u "username:password" https://api.example.com/auth | jq .

# API key in header
curl -s -H "X-API-Key: $API_KEY" https://api.example.com/data | jq .

# API key in query parameter
curl -s "https://api.example.com/data?api_key=$API_KEY" | jq .

# OAuth2 token exchange
TOKEN=$(curl -s -X POST https://auth.example.com/oauth/token \
  -d "grant_type=client_credentials&client_id=$CLIENT_ID&client_secret=$CLIENT_SECRET" | jq -r '.access_token')
```

### Debugging

```bash
# Verbose mode — see full request/response headers
curl -v https://api.example.com/users 2>&1

# Show response headers only
curl -sI https://api.example.com/users

# Timing breakdown
curl -s -o /dev/null -w "DNS: %{time_namelookup}s\nConnect: %{time_connect}s\nTLS: %{time_appconnect}s\nFirst byte: %{time_starttransfer}s\nTotal: %{time_total}s\nHTTP code: %{http_code}\n" https://api.example.com/users

# Follow redirects
curl -sL https://short.url/abc | head -20

# Show both request and response with body
curl -v -X POST https://api.example.com/data -H "Content-Type: application/json" -d '{"key":"value"}' 2>&1
```

### Response Processing

```bash
# Extract specific field
curl -s https://api.example.com/users | jq '.data[0].email'

# Filter array
curl -s https://api.example.com/users | jq '.data[] | select(.role == "admin")'

# Count results
curl -s https://api.example.com/users | jq '.data | length'

# Extract headers + body
curl -sD - https://api.example.com/users | head -20

# Check status code
STATUS=$(curl -s -o /dev/null -w "%{http_code}" https://api.example.com/health)
echo "Status: $STATUS"
```

### Cookie Management

```bash
# Save cookies
curl -s -c cookies.txt https://api.example.com/login -d '{"user":"alice","pass":"secret"}'

# Use saved cookies
curl -s -b cookies.txt https://api.example.com/dashboard | jq .

# Both save and send
curl -s -b cookies.txt -c cookies.txt https://api.example.com/api/data | jq .
```

## Quick Reference

| Task | curl flags |
|------|-----------|
| Silent output | `-s` |
| JSON body | `-H "Content-Type: application/json" -d '{...}'` |
| Bearer auth | `-H "Authorization: Bearer $TOKEN"` |
| Status code only | `-o /dev/null -w "%{http_code}"` |
| Follow redirects | `-L` |
| Verbose debug | `-v` |
| Response headers | `-I` (HEAD) or `-D -` (with body) |
| Save to file | `-o filename` |
| Timeout | `--connect-timeout 5 --max-time 30` |

## Gotchas

- **JSON POST**: Always set `-H "Content-Type: application/json"` or server may reject
- **Special characters in data**: Use `--data-urlencode` for form data, single quotes for JSON
- **HTTPS cert issues**: Use `--cacert` to specify CA, never use `-k` in production
- **Large responses**: Pipe to `jq .` for formatting, or `| head -100` to preview
- **Binary responses**: Use `-o file.bin` to save, don't print to terminal

## Aleph Integration

- Synergy with `api-design` (W2): design API → `http-client` tests it
- Synergy with `security` (W5): security audit → `http-client` probes endpoints
- Use `jq` from `data-pipeline` (A5) for complex response processing
