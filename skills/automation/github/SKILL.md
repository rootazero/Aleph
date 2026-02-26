---
name: github
description: GitHub CLI automation — PR lifecycle, issue management, CI debugging, and advanced API queries via gh
emoji: "🐙"
category: automation
cli-wrapper: true
requirements:
  binaries:
    - gh
  platforms:
    - macos
    - linux
  install:
    - manager: brew
      package: gh
triggers:
  - github
  - gh
  - pull request
  - PR
  - issue
  - workflow
  - CI status
---

# GitHub CLI Automation

## When to Use

Invoke this skill when you need to interact with GitHub: creating/reviewing PRs, managing issues, debugging CI failures, or querying repository data. This skill wraps the `gh` CLI with intelligent automation patterns.

## Prerequisites

```bash
# Install
brew install gh

# Authenticate
gh auth login

# Verify
gh auth status
```

## Core Operations

### PR Lifecycle

**Create a PR:**
```bash
# From current branch
gh pr create --title "feat: add user auth" --body "## Summary\n- Adds JWT auth\n- Adds login endpoint"

# Draft PR (not ready for review)
gh pr create --draft --title "WIP: refactoring auth" --body "Work in progress"

# With reviewers and labels
gh pr create --title "fix: null check" --reviewer alice,bob --label bug,p1
```

**Review a PR:**
```bash
# View PR details
gh pr view 123

# Check diff
gh pr diff 123

# View review comments
gh api repos/{owner}/{repo}/pulls/123/comments --jq '.[] | "\(.path):\(.line) - \(.body)"'

# Submit review
gh pr review 123 --approve --body "LGTM"
gh pr review 123 --request-changes --body "Please fix the null check on line 42"
gh pr review 123 --comment --body "Looks good overall, minor suggestion on error handling"
```

**Merge and cleanup:**
```bash
# Merge (squash by default for clean history)
gh pr merge 123 --squash --delete-branch

# Merge with specific strategy
gh pr merge 123 --merge    # merge commit
gh pr merge 123 --rebase   # rebase
```

### Issue Management

```bash
# Create issue with template
gh issue create --title "Bug: login fails on Safari" --body "## Steps\n1. Go to login\n2. Enter credentials\n3. Click submit\n\n## Expected\nRedirect to dashboard\n\n## Actual\n500 error" --label bug

# List open issues by label
gh issue list --label bug --state open

# Close with comment
gh issue close 45 --comment "Fixed in #123"

# Transfer issue
gh issue transfer 45 target-repo
```

### CI / Workflow Debugging

```bash
# List recent runs
gh run list --limit 10

# View a specific run
gh run view <run-id>

# See failed step logs (most useful for debugging)
gh run view <run-id> --log-failed

# Re-run failed jobs only
gh run rerun <run-id> --failed

# Download artifacts
gh run download <run-id> -n artifact-name
```

**CI debugging workflow:**
1. `gh pr checks <pr-number>` — see which checks failed
2. `gh run view <run-id>` — identify which job failed
3. `gh run view <run-id> --log-failed` — read the failure logs
4. Fix code, push, repeat

### Advanced API Queries

```bash
# Get PR with specific fields
gh api repos/{owner}/{repo}/pulls/123 --jq '{title: .title, state: .state, author: .user.login, reviews: .requested_reviewers | length}'

# List all open PRs by author
gh api repos/{owner}/{repo}/pulls --jq '.[] | select(.user.login == "alice") | "\(.number): \(.title)"'

# Repo statistics
gh api repos/{owner}/{repo} --jq '{stars: .stargazers_count, forks: .forks_count, issues: .open_issues_count}'

# Search across repos
gh api search/code -X GET -f q="filename:SKILL.md org:aleph" --jq '.items[] | "\(.repository.full_name): \(.path)"'

# GraphQL for complex queries
gh api graphql -f query='
  query {
    repository(owner: "owner", name: "repo") {
      pullRequests(last: 5, states: OPEN) {
        nodes { number title additions deletions }
      }
    }
  }
' --jq '.data.repository.pullRequests.nodes[] | "\(.number): \(.title) (+\(.additions)/-\(.deletions))"'
```

### Security Checks

```bash
# List repository secrets (names only, values are never exposed)
gh secret list

# Check Dependabot alerts for dependency vulnerabilities
gh api repos/{owner}/{repo}/dependabot/alerts --jq '.[] | "\(.state): \(.security_advisory.summary) [\(.dependency.package.name)]"'

# Audit branch protection rules on main
gh api repos/{owner}/{repo}/branches/main/protection --jq '{
  enforce_admins: .enforce_admins.enabled,
  required_reviews: .required_pull_request_reviews.required_approving_review_count,
  dismiss_stale: .required_pull_request_reviews.dismiss_stale_reviews,
  require_ci: .required_status_checks.strict,
  ci_contexts: .required_status_checks.contexts
}'

# List deploy keys
gh api repos/{owner}/{repo}/keys --jq '.[] | "\(.title) (read_only: \(.read_only))"'
```

### Release Management

```bash
# Create release from tag
gh release create v1.0.0 --title "v1.0.0" --notes "## Changes\n- Feature A\n- Fix B" --latest

# Upload assets
gh release upload v1.0.0 ./build/app.tar.gz

# List releases
gh release list --limit 5
```

## Quick Reference

| Task | Command |
|------|---------|
| PR status | `gh pr status` |
| PR checks | `gh pr checks <number>` |
| My PRs | `gh pr list --author @me` |
| Failed CI logs | `gh run view <id> --log-failed` |
| Rerun CI | `gh run rerun <id> --failed` |
| Close issue | `gh issue close <number>` |
| Repo clone | `gh repo clone owner/repo` |

## Gotchas

- **Auth scope**: `gh auth login` with `repo` scope needed for private repos
- **Rate limits**: `gh api --header 'X-RateLimit-Remaining'` to check remaining quota
- **JSON output**: Most commands support `--json field1,field2 --jq '.expression'`
- **Non-git directory**: Use `--repo owner/repo` flag when not inside a git repo

## Aleph Integration

- Synergy with `git` (F4): local git → `github` for remote operations
- Synergy with `code-review` (F3): review methodology → `github` submits reviews
- Synergy with `ci-cd` (W7): pipeline design → `github` debugs failures
