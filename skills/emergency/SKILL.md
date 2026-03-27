---
name: emergency
description: "Developer emergency rescue — recover from Git disasters, disk full, DB locks, credential leaks, and deployment failures. Use when something is broken and needs immediate recovery: lost commits, force-push accidents, corrupted repos, exposed secrets, stuck queries, failed migrations, or rollback needed."
---

# Emergency Rescue

**Rule:** Stabilize first, investigate later. Don't debug under fire.

## Triage

| Emergency | First Action |
|-----------|-------------|
| Lost commits / force-push | `git reflog` -> find and recover |
| Credential leak | REVOKE immediately, then rotate |
| Disk full | `df -h`, `docker system prune -af` |
| DB deadlock | Find blocking query, `pg_cancel_backend` |
| Deploy failure | Rollback immediately |

For detailed recovery recipes, see [references/recovery-recipes.md](references/recovery-recipes.md).

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
