---
name: ci-cd
description: CI/CD pipeline design — GitHub Actions templates, caching, secrets, pipeline patterns
scope: standalone
---

# CI/CD Pipelines

## When to Use

Invoke this skill when setting up continuous integration, continuous deployment, or automating build/test/deploy workflows.

## GitHub Actions Fundamentals

### Workflow Structure
```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run tests
        run: cargo test
```

### Common Triggers

| Trigger | Use Case |
|---------|----------|
| `push` | Run on every push to specified branches |
| `pull_request` | Run on PR open/update |
| `schedule` | Cron-based (e.g., nightly builds) |
| `workflow_dispatch` | Manual trigger with inputs |
| `release` | On GitHub release creation |

## Pipeline Templates

### Rust CI
```yaml
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo check
      - run: cargo test
      - run: cargo clippy -- -D warnings
      - run: cargo fmt --check
```

### Node.js CI
```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
          cache: 'npm'
      - run: npm ci
      - run: npm run lint
      - run: npm test
      - run: npm run build
```

### Python CI
```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: '3.12'
      - run: pip install -e ".[dev]"
      - run: ruff check .
      - run: pytest --cov
```

## Caching Strategies

| What to Cache | Key | Restore |
|---------------|-----|---------|
| Rust target/ | `Cargo.lock` hash | `Swatinem/rust-cache` |
| node_modules/ | `package-lock.json` hash | `actions/cache` or setup-node cache |
| pip packages | `requirements.txt` hash | `actions/cache` |
| Docker layers | Dockerfile hash | `docker/build-push-action` with cache |

**Rule:** Cache dependencies, not build artifacts (unless builds are very slow).

## Pipeline Patterns

### Fan-Out / Fan-In
```yaml
jobs:
  lint:
    runs-on: ubuntu-latest
    steps: [...]
  test:
    runs-on: ubuntu-latest
    steps: [...]
  build:
    needs: [lint, test]    # runs after both pass
    steps: [...]
```

### Matrix Build
```yaml
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
        node: [18, 20]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/setup-node@v4
        with:
          node-version: ${{ matrix.node }}
```

### Conditional Deploy
```yaml
jobs:
  deploy:
    if: github.ref == 'refs/heads/main' && github.event_name == 'push'
    needs: [test]
    steps: [...]
```

## Secret Management

```yaml
steps:
  - name: Deploy
    env:
      API_KEY: ${{ secrets.API_KEY }}
    run: deploy.sh
```

**Rules:**
- Never echo secrets in logs
- Use environment-scoped secrets for production
- Rotate secrets regularly
- Use OIDC for cloud providers (no long-lived keys)

## Anti-Patterns

- **No caching**: Downloading dependencies every run wastes minutes
- **Testing only on push**: PRs should be tested before merge
- **Manual deploy steps**: If it's not in the pipeline, it's not repeatable
- **Ignoring flaky tests**: A flaky CI pipeline trains people to ignore failures
- **Secrets in workflow files**: Use GitHub Secrets, never hardcode
- **No timeout**: Jobs that hang forever waste runner minutes
