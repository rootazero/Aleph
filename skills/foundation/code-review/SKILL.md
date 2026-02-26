---
name: code-review
description: Multi-dimensional code review with confidence-based filtering — security, logic, error handling
scope: standalone
---

# Code Review

## When to Use

Invoke this skill when reviewing code changes — pull requests, diffs, or files. It provides a structured, prioritized review that surfaces high-confidence issues first.

## Review Protocol

### Step 1: Understand Context

Before reviewing code, understand:
- What problem does this change solve?
- What's the scope? (new feature, bug fix, refactor, performance)
- Are there tests? Do they cover the change?

Read the PR description, linked issues, and relevant documentation first.

### Step 2: Review by Priority

Review in **descending priority**. Stop deeper if critical issues found.

#### P0 — Security (Blocking)

| Check | What to Look For |
|-------|-----------------|
| Secrets | Hardcoded API keys, passwords, tokens, connection strings |
| Injection | SQL injection, command injection, XSS, template injection |
| Auth bypass | Missing authentication/authorization checks on endpoints |
| Path traversal | User input used in file paths without sanitization |
| Deserialization | Unsafe deserialization of untrusted data |

**Rule:** Any P0 finding blocks the review. Fix before proceeding.

#### P1 — Logic (Blocking)

| Check | What to Look For |
|-------|-----------------|
| Correctness | Does the code actually do what it claims? |
| Edge cases | Empty inputs, null/None, boundary values, concurrent access |
| Error handling | Are errors caught? Are they handled correctly? Do they propagate? |
| State management | Race conditions, stale state, missing cleanup |
| Data integrity | Truncation, overflow, precision loss, encoding issues |

#### P2 — Error Handling (Important)

| Check | What to Look For |
|-------|-----------------|
| Missing error paths | What happens when this call fails? |
| Swallowed errors | Catch blocks that do nothing or just log |
| Error messages | Are they helpful for debugging? Do they leak internals? |
| Resource cleanup | Are connections, files, locks released on error? |

#### P3 — Performance (Informational)

| Check | What to Look For |
|-------|-----------------|
| N+1 queries | Loop with DB query inside |
| Unbounded growth | Lists/maps that grow without limit |
| Missing pagination | Returning all records from a large table |
| Blocking I/O | Synchronous I/O in async context |

#### P4 — Style (Optional, on request only)

Naming, formatting, idiomatic patterns. Only raise if significantly impacts readability.

### Step 3: Report Findings

For each finding, provide:

```
**[P0/P1/P2/P3] file:line — Title**
Description of the issue.
Suggested fix: (concrete code suggestion)
```

**Default behavior:** Report P0-P2 findings only. Mention P3-P4 count but don't detail unless asked.

## Confidence Filter

Only report findings where you have **high confidence** the issue is real.

| Confidence | Action |
|------------|--------|
| High (>80%) | Report with specific fix |
| Medium (50-80%) | Report as "potential issue, verify" |
| Low (<50%) | Do not report — avoid false positives |

**Rule:** 5 high-confidence findings > 20 speculative ones.

## Review Summary Template

```
## Review Summary

**Scope:** [feature/bugfix/refactor] — [brief description]
**Verdict:** [APPROVE / REQUEST CHANGES / NEEDS DISCUSSION]

### Findings
[P0-P2 findings listed]

### Observations
[P3-P4 count, general impressions, positive notes]

### Missing
[Tests not covering change, documentation not updated, etc.]
```

## Anti-Patterns

- **Nitpicking style over substance**: Don't flag formatting when there are logic bugs
- **Rubber-stamping**: "LGTM" without actually reading the code
- **Rewriting in review**: Suggesting complete rewrites instead of incremental improvements
- **Ignoring tests**: Reviewing implementation without checking test coverage
- **Scope creep**: Requesting changes unrelated to the PR's purpose
