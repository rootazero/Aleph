---
name: emergency
description: Developer emergency rescue — Git disasters, disk full, DB locks, credential leaks, deployment failures
scope: standalone
---

# Emergency Rescue

## When to Use

Invoke this skill when something is broken and needs immediate recovery — Git catastrophes, system failures, data corruption, or credential exposure.

**Rule:** Stabilize first, investigate later. Don't debug under fire.

## Git Disasters

### Accidentally Force-Pushed / Lost Commits

```bash
# Find the lost commit in reflog
git reflog

# The commit is still there (for ~30 days)
git checkout <lost-sha>          # inspect it
git branch recovery <lost-sha>   # save it
git reset --hard <lost-sha>      # restore branch to this point
```

### Committed to Wrong Branch

```bash
# Save the commit hash
git log -1                        # note the SHA

# Move to correct branch
git checkout correct-branch
git cherry-pick <sha>

# Remove from wrong branch
git checkout wrong-branch
git reset --hard HEAD~1           # careful: destroys local changes
```

### Merge Conflict Nightmare

```bash
# Option 1: Abort and start over
git merge --abort                  # or git rebase --abort

# Option 2: Accept theirs / ours for specific files
git checkout --theirs <file>       # accept their version
git checkout --ours <file>         # keep our version
git add <file>
```

### Corrupted Repository

```bash
# Verify integrity
git fsck --full

# If objects are corrupted, re-clone
git clone <remote-url> fresh-clone
# Copy your uncommitted changes manually
```

## Credential Leak

**A credential is compromised the moment it appears in ANY public history.**

### Response Checklist (in order)

1. **REVOKE** — Invalidate the credential NOW (API dashboard, admin panel)
2. **ROTATE** — Generate a new credential
3. **AUDIT** — Check logs for unauthorized access during exposure window
4. **CLEAN** — Remove from history (BFG Repo Cleaner or git filter-repo)
5. **PREVENT** — Add pre-commit hook, update .gitignore

```bash
# Clean history with BFG (after revoking!)
bfg --delete-files credentials.json
bfg --replace-text passwords.txt
git reflog expire --expire=now --all
git gc --prune=now
git push --force
```

## Disk Full

### Immediate Relief

```bash
# Find what's consuming space
df -h                              # overview
du -sh /* 2>/dev/null | sort -rh | head -20  # top consumers

# Quick wins
docker system prune -af            # Docker cleanup (aggressive)
rm -rf /tmp/*                      # temp files
journalctl --vacuum-size=100M      # trim logs (Linux)

# Language-specific
cargo clean                        # Rust
rm -rf node_modules && npm ci      # Node.js
pip cache purge                    # Python

# Find large files
find / -type f -size +100M 2>/dev/null | head -20
```

**Warning:** Don't delete log files — truncate them instead:
```bash
> /var/log/large-logfile.log       # truncate to zero
```

## Database Issues

### Deadlock / Stuck Queries

```sql
-- PostgreSQL: find blocking queries
SELECT pid, state, query, wait_event_type
FROM pg_stat_activity
WHERE state != 'idle'
ORDER BY query_start;

-- Kill stuck query
SELECT pg_cancel_backend(<pid>);     -- gentle
SELECT pg_terminate_backend(<pid>);  -- force
```

### Failed Migration

```bash
# Check current migration state
<migration-tool> status

# Rollback the failed migration
<migration-tool> down

# If manually broken:
# 1. Fix the database state manually
# 2. Update the migration version table
# 3. Create a corrective migration going forward
```

### Connection Pool Exhaustion

Symptoms: "too many connections", timeouts, new connections refused

```bash
# Check active connections (PostgreSQL)
SELECT count(*) FROM pg_stat_activity;
SELECT max_connections FROM pg_settings WHERE name = 'max_connections';

# Kill idle connections
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE state = 'idle' AND query_start < now() - interval '10 minutes';
```

## Deployment Failure

### Rollback Immediately

```bash
# Container rollback
docker compose down
docker compose -f docker-compose.previous.yml up -d

# Kubernetes
kubectl rollout undo deployment/<name>

# Git-based deploy
git revert <bad-commit>
git push
```

### "I Don't Know What's Wrong"

Universal diagnostic:
```bash
# 1. Is the process running?
ps aux | grep <service-name>

# 2. Are ports open?
lsof -i :<expected-port>

# 3. What do the logs say?
journalctl -u <service> -n 100 --no-pager  # systemd
docker logs --tail 100 <container>           # Docker

# 4. Can it reach dependencies?
curl -v http://localhost:<dep-port>/health

# 5. Resource exhaustion?
df -h && free -h && top -bn1 | head -5
```

## Post-Mortem Template

After every emergency, fill this out (POE Evaluation Phase):

```
## Post-Mortem: [Incident Title]
Date: YYYY-MM-DD
Duration: X hours
Severity: P0/P1/P2

### What Happened
[Timeline of events]

### Root Cause
[Technical explanation of why]

### What We Did
[Actions taken to resolve]

### What We'll Change
[Preventive measures]

### Detection
[How we found out — monitoring? user report? accident?]
[How can we detect this faster next time?]
```
