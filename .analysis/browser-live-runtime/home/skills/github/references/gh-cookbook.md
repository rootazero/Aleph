# GitHub CLI Cookbook

## Table of Contents
- [PR Lifecycle](#pr-lifecycle)
- [Issue Management](#issue-management)
- [CI / Workflow Debugging](#ci--workflow-debugging)
- [Advanced API Queries](#advanced-api-queries)
- [Security Checks](#security-checks)
- [Release Management](#release-management)

## PR Lifecycle

**Create a PR:**
```bash
gh pr create --title "feat: add user auth" --body "## Summary\n- Adds JWT auth\n- Adds login endpoint"
gh pr create --draft --title "WIP: refactoring auth" --body "Work in progress"
gh pr create --title "fix: null check" --reviewer alice,bob --label bug,p1
```

**Review a PR:**
```bash
gh pr view 123
gh pr diff 123
gh api repos/{owner}/{repo}/pulls/123/comments --jq '.[] | "\(.path):\(.line) - \(.body)"'
gh pr review 123 --approve --body "LGTM"
gh pr review 123 --request-changes --body "Please fix the null check on line 42"
gh pr review 123 --comment --body "Looks good overall"
```

**Merge and cleanup:**
```bash
gh pr merge 123 --squash --delete-branch
gh pr merge 123 --merge    # merge commit
gh pr merge 123 --rebase   # rebase
```

## Issue Management

```bash
gh issue create --title "Bug: login fails" --body "## Steps\n1. ..." --label bug
gh issue list --label bug --state open
gh issue close 45 --comment "Fixed in #123"
gh issue transfer 45 target-repo
```

## CI / Workflow Debugging

```bash
gh run list --limit 10                    # list recent runs
gh run view <run-id>                      # view specific run
gh run view <run-id> --log-failed         # failed step logs
gh run rerun <run-id> --failed            # re-run failed jobs
gh run download <run-id> -n artifact-name # download artifacts
```

**CI debugging workflow:**
1. `gh pr checks <pr-number>` -- see which checks failed
2. `gh run view <run-id>` -- identify which job failed
3. `gh run view <run-id> --log-failed` -- read the failure logs
4. Fix code, push, repeat

## Advanced API Queries

```bash
# Get PR with specific fields
gh api repos/{owner}/{repo}/pulls/123 --jq '{title: .title, state: .state, author: .user.login}'

# List open PRs by author
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

## Security Checks

```bash
gh secret list
gh api repos/{owner}/{repo}/dependabot/alerts --jq '.[] | "\(.state): \(.security_advisory.summary)"'
gh api repos/{owner}/{repo}/branches/main/protection --jq '{
  enforce_admins: .enforce_admins.enabled,
  required_reviews: .required_pull_request_reviews.required_approving_review_count,
  require_ci: .required_status_checks.strict
}'
gh api repos/{owner}/{repo}/keys --jq '.[] | "\(.title) (read_only: \(.read_only))"'
```

## Release Management

```bash
gh release create v1.0.0 --title "v1.0.0" --notes "## Changes\n- Feature A" --latest
gh release upload v1.0.0 ./build/app.tar.gz
gh release list --limit 5
```
