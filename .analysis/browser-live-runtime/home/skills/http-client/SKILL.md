---
name: http-client
description: "API debugging and testing — RESTful requests, authentication, response analysis via curl. Use when testing APIs, debugging HTTP requests, validating endpoints, sending webhooks, or any HTTP/REST operation. Zero dependencies (curl pre-installed)."
---

# API Debugging (curl)

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

## Detailed Cookbook

For complete examples (GET/POST/PUT/DELETE, file upload, auth patterns, debugging, response processing, cookies), see [references/curl-cookbook.md](references/curl-cookbook.md).

## Gotchas

- **JSON POST**: Always set `-H "Content-Type: application/json"` or server may reject
- **Special characters in data**: Use `--data-urlencode` for form data, single quotes for JSON
- **HTTPS cert issues**: Use `--cacert` to specify CA, never use `-k` in production
- **Large responses**: Pipe to `jq .` for formatting, or `| head -100` to preview
- **Binary responses**: Use `-o file.bin` to save, don't print to terminal

## Aleph Integration

- Synergy with `api-design` (W2): design API -> `http-client` tests it
- Synergy with `security` (W5): security audit -> probe endpoints
- Use `jq` from `data-pipeline` (A5) for complex response processing
