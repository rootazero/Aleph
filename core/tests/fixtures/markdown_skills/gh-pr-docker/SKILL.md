---
name: gh-pr-docker
description: GitHub PR operations (Aleph enhanced with Docker)
metadata:
  requires:
    bins: ["gh"]
  aleph:
    security:
      sandbox: docker
      confirmation: write
      network: internet
    docker:
      image: "ghcr.io/cli/cli:latest"
      env_vars:
        - GITHUB_TOKEN
        - GH_TOKEN
    input_hints:
      action:
        type: string
        values: ["list", "view", "create"]
        optional: false
      repo:
        type: string
        pattern: "^[^/]+/[^/]+$"
        description: "Repository in format owner/name"
        optional: false
      number:
        type: integer
        description: "PR number"
        optional: true
---

# GitHub PR Tool (Docker Sandbox)

Manage GitHub Pull Requests with Docker isolation.

## Examples

```bash
gh pr list --repo anthropics/anthropic-sdk-typescript
gh pr view 123
```

## Security

This tool runs in Docker with:
- Read-only root filesystem
- Limited tmpfs (100MB, noexec)
- GITHUB_TOKEN passed from host environment
