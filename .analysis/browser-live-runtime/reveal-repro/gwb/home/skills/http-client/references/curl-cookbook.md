# curl Cookbook

## GET Requests

```bash
curl -s https://api.example.com/users | jq .
curl -s -H "Authorization: Bearer $TOKEN" https://api.example.com/users | jq .
curl -s "https://api.example.com/users?page=2&limit=10" | jq .
curl -s https://api.example.com/data -o response.json
```

## POST / PUT / PATCH / DELETE

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

## File Upload

```bash
curl -s -X POST https://api.example.com/upload \
  -F "file=@/path/to/document.pdf" \
  -F "description=My document" | jq .

curl -s -X POST https://api.example.com/upload \
  -F "files[]=@file1.png" \
  -F "files[]=@file2.png" | jq .
```

## Authentication Patterns

```bash
# Bearer token
curl -s -H "Authorization: Bearer $TOKEN" https://api.example.com/me | jq .

# Basic auth
curl -s -u "username:password" https://api.example.com/auth | jq .

# API key in header
curl -s -H "X-API-Key: $API_KEY" https://api.example.com/data | jq .

# OAuth2 token exchange
TOKEN=$(curl -s -X POST https://auth.example.com/oauth/token \
  -d "grant_type=client_credentials&client_id=$CLIENT_ID&client_secret=$CLIENT_SECRET" | jq -r '.access_token')
```

## Debugging

```bash
# Verbose mode
curl -v https://api.example.com/users 2>&1

# Response headers only
curl -sI https://api.example.com/users

# Timing breakdown
curl -s -o /dev/null -w "DNS: %{time_namelookup}s\nConnect: %{time_connect}s\nTLS: %{time_appconnect}s\nFirst byte: %{time_starttransfer}s\nTotal: %{time_total}s\nHTTP code: %{http_code}\n" https://api.example.com/users

# Follow redirects
curl -sL https://short.url/abc | head -20
```

## Response Processing

```bash
curl -s https://api.example.com/users | jq '.data[0].email'        # extract field
curl -s https://api.example.com/users | jq '.data[] | select(.role == "admin")'  # filter
curl -s https://api.example.com/users | jq '.data | length'        # count
STATUS=$(curl -s -o /dev/null -w "%{http_code}" https://api.example.com/health)  # status code
```

## Cookie Management

```bash
curl -s -c cookies.txt https://api.example.com/login -d '{"user":"alice","pass":"secret"}'
curl -s -b cookies.txt https://api.example.com/dashboard | jq .
curl -s -b cookies.txt -c cookies.txt https://api.example.com/api/data | jq .
```
