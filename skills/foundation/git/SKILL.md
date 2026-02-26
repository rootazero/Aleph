---
name: git
description: Git workflow — daily operations, advanced techniques, and Aleph conventions
scope: standalone
---

# Git Workflow

## When to Use

Invoke this skill for any Git operation — committing, branching, merging, recovering from mistakes, or working with advanced features like bisect, reflog, and worktrees.

## Commit Convention

Aleph uses the format: `<scope>: <description>`

```
gateway: add WebSocket server foundation
memory: fix LanceDB connection pool leak
tools: implement shell execution with timeout
```

- Scope = module/component being changed
- Description = imperative mood, lowercase, no period
- English commit messages always

## Daily Operations

### Stage and Commit

```bash
git add <specific-files>           # stage specific files
git diff --staged                  # review what will be committed
git commit -m "scope: description" # commit with message
```

**Never** use `git add .` or `git add -A` blindly — review what you're staging.

### Branching

```bash
git checkout -b feature/name       # create and switch to new branch
git branch -d feature/name         # delete merged branch
git branch -D feature/name         # force delete unmerged branch (careful!)
```

### Sync with Remote

```bash
git fetch origin                   # update remote tracking branches
git pull --rebase origin main      # rebase local changes on top of remote
git push -u origin feature/name    # push and set upstream
```

## Advanced Operations

### Bisect — Find the Commit That Broke It

```bash
git bisect start
git bisect bad                     # current is broken
git bisect good <known-good-sha>   # last known working commit
# Git checks out midpoint — test it, then:
git bisect good   # or   git bisect bad
# Repeat until found. Automate with:
git bisect run <test-script>
git bisect reset                   # exit bisect
```

### Reflog — Recover from Mistakes

The reflog records every HEAD movement. Nothing is truly lost (for ~30 days).

```bash
git reflog                         # see recent HEAD history
git checkout <reflog-sha>          # go to any previous state
git branch recovery <reflog-sha>   # save as a branch
```

Use when: accidentally deleted a branch, reset --hard too far, lost commits after rebase.

### Cherry-Pick — Apply Specific Commits

```bash
git cherry-pick <sha>              # apply one commit
git cherry-pick <sha1>..<sha3>     # apply a range
git cherry-pick --no-commit <sha>  # stage changes without committing
```

### Interactive Rebase — Clean Up History

```bash
git rebase -i HEAD~5               # rewrite last 5 commits
# In editor: pick/squash/reword/edit/drop
# After editing:
git rebase --continue              # or --abort to cancel
```

**Warning:** Never rebase commits that have been pushed to a shared branch.

### Worktree — Parallel Branches

```bash
git worktree add ../feature-branch feature/name
# Work in ../feature-branch independently
git worktree remove ../feature-branch
git worktree prune                 # clean up stale entries
```

**Aleph-specific rule:** Never delete a worktree from within it. The CWD lock will corrupt your shell. Always `cd` to the main repo or use a separate terminal.

### Sparse Checkout — Partial Clone

```bash
git sparse-checkout init --cone
git sparse-checkout set src/core src/gateway
```

Useful for large monorepos when you only need specific directories.

## Conflict Resolution

```bash
# During merge/rebase with conflicts:
git status                         # see conflicting files
# Edit files — look for <<<<<<< / ======= / >>>>>>>
git add <resolved-file>            # mark as resolved
git merge --continue               # or git rebase --continue
```

**Strategy:** If conflicts are large, consider `git rerere` to record resolutions:
```bash
git config rerere.enabled true     # auto-record conflict resolutions
```

## Useful Aliases

```bash
git config --global alias.st 'status -sb'
git config --global alias.lg 'log --oneline --graph --all -20'
git config --global alias.unstage 'reset HEAD --'
git config --global alias.last 'log -1 HEAD --stat'
```

## Anti-Patterns

- **Force push to shared branches**: Destroys other people's work. Only force-push your own feature branches.
- **Giant commits**: "implement everything" — break into logical units
- **Vague messages**: "fix stuff", "update", "WIP" — describe what and why
- **Committing secrets**: Even after removal, they exist in history. Use `git-secrets` or pre-commit hooks.
- **Never rebasing**: Merge commits clutter history. Rebase feature branches before merge.
