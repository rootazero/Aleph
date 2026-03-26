# Recovery Recipes

## Table of Contents
- [Git Disasters](#git-disasters)
- [Credential Leak](#credential-leak)
- [Disk Full](#disk-full)
- [Database Issues](#database-issues)
- [Deployment Failure](#deployment-failure)
- [Universal Diagnostic](#universal-diagnostic)

## Git Disasters

### Accidentally Force-Pushed / Lost Commits

```bash
git reflog                            # find the lost commit
git checkout <lost-sha>               # inspect it
git branch recovery <lost-sha>        # save it
git reset --hard <lost-sha>           # restore branch to this point
```

### Committed to Wrong Branch

```bash
git log -1                            # note the SHA
git checkout correct-branch
git cherry-pick <sha>
git checkout wrong-branch
git reset --hard HEAD~1               # careful: destroys local changes
```

### Merge Conflict Nightmare

```bash
git merge --abort                     # or git rebase --abort

# Accept theirs / ours for specific files
git checkout --theirs <file>
git checkout --ours <file>
git add <file>
```

### Corrupted Repository

```bash
git fsck --full                       # verify integrity
# If corrupted, re-clone:
git clone <remote-url> fresh-clone
```

## Credential Leak

**A credential is compromised the moment it appears in ANY public history.**

### Response Checklist (in order)

1. **REVOKE** -- Invalidate the credential NOW
2. **ROTATE** -- Generate a new credential
3. **AUDIT** -- Check logs for unauthorized access
4. **CLEAN** -- Remove from history (BFG or git filter-repo)
5. **PREVENT** -- Add pre-commit hook, update .gitignore

```bash
bfg --delete-files credentials.json
bfg --replace-text passwords.txt
git reflog expire --expire=now --all
git gc --prune=now
git push --force
```

## Disk Full

```bash
# Find what's consuming space
df -h
du -sh /* 2>/dev/null | sort -rh | head -20

# Quick wins
docker system prune -af               # Docker cleanup
rm -rf /tmp/*                          # temp files
journalctl --vacuum-size=100M          # trim logs (Linux)

# Language-specific
cargo clean                            # Rust
rm -rf node_modules && npm ci          # Node.js
pip cache purge                        # Python

# Find large files
find / -type f -size +100M 2>/dev/null | head -20
```

**Warning:** Don't delete log files -- truncate: `> /var/log/large-logfile.log`

## Database Issues

### Deadlock / Stuck Queries

```sql
-- PostgreSQL: find blocking queries
SELECT pid, state, query, wait_event_type
FROM pg_stat_activity
WHERE state != 'idle'
ORDER BY query_start;

-- Kill stuck query
SELECT pg_cancel_backend(<pid>);       -- gentle
SELECT pg_terminate_backend(<pid>);    -- force
```

### Failed Migration

```bash
<migration-tool> status                # check current state
<migration-tool> down                  # rollback
# If manually broken: fix DB state, update version table, create corrective migration
```

### Connection Pool Exhaustion

```sql
SELECT count(*) FROM pg_stat_activity;
SELECT max_connections FROM pg_settings WHERE name = 'max_connections';

-- Kill idle connections
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE state = 'idle' AND query_start < now() - interval '10 minutes';
```

## Deployment Failure

```bash
# Container rollback
docker compose down
docker compose -f docker-compose.previous.yml up -d

# Kubernetes
kubectl rollout undo deployment/<name>

# Git-based deploy
git revert <bad-commit> && git push
```

## Universal Diagnostic

When you don't know what's wrong:

```bash
ps aux | grep <service-name>                       # 1. process running?
lsof -i :<expected-port>                           # 2. ports open?
journalctl -u <service> -n 100 --no-pager          # 3. logs (systemd)
docker logs --tail 100 <container>                  # 3. logs (Docker)
curl -v http://localhost:<dep-port>/health          # 4. dependencies?
df -h && free -h && top -bn1 | head -5             # 5. resources?
```
