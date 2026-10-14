---
name: github
description: "GitHub CLI automation — PR lifecycle, issue management, CI debugging, release management, and API queries via gh. Use when creating/reviewing pull requests, managing issues, debugging CI failures, checking workflow runs, querying GitHub API, managing releases, or auditing repo security."
---

# GitHub CLI Automation

## Prerequisites

```bash
brew install gh && gh auth login
```

## Quick Reference

| Task | Command |
|------|---------|
| PR status | `gh pr status` |
| Create PR | `gh pr create --title "..." --body "..."` |
| PR checks | `gh pr checks <number>` |
| My PRs | `gh pr list --author @me` |
| Merge PR | `gh pr merge <number> --squash --delete-branch` |
| Failed CI logs | `gh run view <id> --log-failed` |
| Rerun CI | `gh run rerun <id> --failed` |
| Close issue | `gh issue close <number>` |
| Repo clone | `gh repo clone owner/repo` |

For detailed examples of all operations, see [references/gh-cookbook.md](references/gh-cookbook.md).

## Gotchas

- **Auth scope**: `gh auth login` with `repo` scope needed for private repos
- **Rate limits**: `gh api --header 'X-RateLimit-Remaining'` to check remaining quota
- **JSON output**: Most commands support `--json field1,field2 --jq '.expression'`
- **Non-git directory**: Use `--repo owner/repo` flag when not inside a git repo

## Aleph Integration

- Synergy with `git` (F4): local git -> `github` for remote operations
- Synergy with `code-review` (F3): review methodology -> `github` submits reviews
- Synergy with `ci-cd` (W7): pipeline design -> `github` debugs failures
