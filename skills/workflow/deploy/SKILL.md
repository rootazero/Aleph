---
name: deploy
description: Deployment lifecycle — containerization, release strategies, monitoring, rollback procedures
scope: standalone
---

# Deployment & Operations

## When to Use

Invoke this skill when containerizing applications, setting up deployment pipelines, planning release strategies, or handling rollbacks.

## Containerization

### Dockerfile Best Practices

```dockerfile
# Multi-stage build (smaller final image)
FROM rust:1.77 AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/myapp /usr/local/bin/
EXPOSE 8080
CMD ["myapp"]
```

**Rules:**
- Use multi-stage builds to reduce image size
- Pin base image versions (no `latest` in production)
- Order layers from least to most frequently changed (deps before code)
- Use `.dockerignore` to exclude `target/`, `node_modules/`, `.git/`
- Run as non-root user
- One process per container

### Docker Compose for Local Dev

```yaml
services:
  app:
    build: .
    ports: ["8080:8080"]
    environment:
      DATABASE_URL: postgres://user:pass@db:5432/mydb
    depends_on:
      db:
        condition: service_healthy
  db:
    image: postgres:16
    environment:
      POSTGRES_PASSWORD: pass
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U user"]
      interval: 5s
      retries: 3
    volumes:
      - pgdata:/var/lib/postgresql/data
volumes:
  pgdata:
```

## Release Strategies

| Strategy | Risk | Downtime | Complexity | Best For |
|----------|------|----------|------------|----------|
| **Rolling update** | Low | Zero | Low | Stateless services |
| **Blue-green** | Low | Near-zero | Medium | Critical services |
| **Canary** | Very low | Zero | High | High-traffic services |
| **Recreate** | High | Yes | None | Dev/staging only |

### Rolling Update
Replace instances one at a time. Requires backward-compatible changes.

### Blue-Green
Run two identical environments. Switch traffic from blue (old) to green (new). Keep blue for instant rollback.

### Canary
Route small percentage (1-5%) of traffic to new version. Monitor metrics. Gradually increase if healthy.

## Health Checks

Every deployed service must have:

```
GET /health          → 200 OK (service is running)
GET /health/ready    → 200 OK (service can handle traffic)
GET /health/live     → 200 OK (service is not deadlocked)
```

## Monitoring Essentials

### The Four Golden Signals

| Signal | What to Measure | Alert When |
|--------|----------------|------------|
| **Latency** | Request duration (p50, p95, p99) | p99 > 2x baseline |
| **Traffic** | Requests per second | Sudden drop > 50% |
| **Errors** | Error rate (5xx / total) | > 1% of requests |
| **Saturation** | CPU, memory, disk, connections | > 80% utilization |

## Rollback Procedures

### Application Rollback
```bash
# Git-based
git revert <bad-commit-sha>
git push

# Container-based
docker pull <image>:<previous-tag>
docker compose up -d

# Kubernetes
kubectl rollout undo deployment/<name>
```

### Database Rollback
```bash
# Run the down migration
<migration-tool> migrate down

# If no down migration: apply a corrective migration
# NEVER: drop the database and recreate
```

### When to Rollback
- Error rate spikes above threshold
- Latency degrades significantly
- Health checks failing
- Data corruption detected

**Rule:** Rollback first, investigate later. Don't debug in production under fire.

## Anti-Patterns

- **No health checks**: Can't tell if deployment succeeded
- **No rollback plan**: "We'll figure it out" is not a plan
- **Deploying Friday afternoon**: Save risky deploys for early in the week
- **No monitoring**: If you can't see it, you can't fix it
- **`latest` tag in production**: No way to know what's running or rollback
- **Manual deployment**: If it's not automated, it's not repeatable
