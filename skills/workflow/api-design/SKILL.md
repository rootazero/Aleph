---
name: api-design
description: Contract-first API design — REST and GraphQL conventions, error handling, versioning
scope: standalone
---

# API Design

## When to Use

Invoke this skill when designing new API endpoints, reviewing API structure, or establishing API conventions for a project.

## Core Principle: Contract First

Define the API schema **before** writing implementation code:
- REST: Write OpenAPI spec first
- GraphQL: Write schema first
- gRPC: Write protobuf definitions first

The contract IS the documentation. Implementation follows the contract, not the other way around.

## REST Conventions

### HTTP Methods

| Method | Purpose | Idempotent | Body |
|--------|---------|------------|------|
| GET | Read resource(s) | Yes | No |
| POST | Create resource | No | Yes |
| PUT | Replace resource entirely | Yes | Yes |
| PATCH | Partial update | No | Yes |
| DELETE | Remove resource | Yes | No |

### URL Structure

```
GET    /api/v1/users           # list users
GET    /api/v1/users/:id       # get one user
POST   /api/v1/users           # create user
PUT    /api/v1/users/:id       # replace user
PATCH  /api/v1/users/:id       # partial update
DELETE /api/v1/users/:id       # delete user
GET    /api/v1/users/:id/posts # nested resource
```

**Rules:**
- Nouns, not verbs (`/users` not `/getUsers`)
- Plural (`/users` not `/user`)
- Lowercase, hyphens for multi-word (`/user-profiles`)
- No trailing slashes
- Max 3 levels of nesting

### Status Codes

| Code | Meaning | Use When |
|------|---------|----------|
| 200 | OK | Successful GET, PUT, PATCH |
| 201 | Created | Successful POST |
| 204 | No Content | Successful DELETE |
| 400 | Bad Request | Invalid input, validation failure |
| 401 | Unauthorized | Missing or invalid authentication |
| 403 | Forbidden | Authenticated but not authorized |
| 404 | Not Found | Resource doesn't exist |
| 409 | Conflict | Duplicate resource, version conflict |
| 422 | Unprocessable | Valid syntax but semantic error |
| 429 | Too Many Requests | Rate limit exceeded |
| 500 | Internal Error | Server bug (never expose details) |

### Error Response Format

```json
{
  "error": {
    "code": "VALIDATION_FAILED",
    "message": "Human-readable description",
    "details": [
      { "field": "email", "issue": "Invalid email format" }
    ]
  }
}
```

### Pagination

```
GET /api/v1/users?page=2&per_page=20

Response:
{
  "data": [...],
  "pagination": {
    "page": 2,
    "per_page": 20,
    "total": 150,
    "total_pages": 8
  }
}
```

For large datasets, prefer **cursor-based** pagination:
```
GET /api/v1/users?cursor=abc123&limit=20
```

### Filtering and Sorting

```
GET /api/v1/users?status=active&role=admin    # filtering
GET /api/v1/users?sort=-created_at,name       # sort (- = descending)
GET /api/v1/users?fields=id,name,email        # sparse fields
```

## Versioning

| Strategy | Example | Pros | Cons |
|----------|---------|------|------|
| URL path | `/api/v1/users` | Simple, explicit | URL changes |
| Header | `Accept: application/vnd.api+json;v=1` | Clean URLs | Hidden |
| Query param | `/api/users?version=1` | Easy to test | Ugly |

**Recommendation:** URL path versioning. It's the most discoverable.

## Authentication Patterns

| Method | Use When |
|--------|----------|
| API Key | Server-to-server, simple |
| JWT Bearer | User sessions, stateless |
| OAuth 2.0 | Third-party access |
| mTLS | Service mesh, high security |

## Rate Limiting

Include headers:
```
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 95
X-RateLimit-Reset: 1635724800
```

## Anti-Patterns

- **Verbs in URLs**: `/api/getUser/123` — use HTTP methods instead
- **Inconsistent naming**: `/users` and `/get_orders` — pick one convention
- **Nested too deep**: `/users/1/posts/2/comments/3/likes` — flatten with filters
- **Exposing internals**: Database IDs, SQL errors, stack traces in responses
- **Ignoring HATEOAS**: For hypermedia APIs, include `_links` for discoverability
- **No versioning**: Breaking changes destroy clients
