---
name: security
description: Security audit — OWASP checklist, credential leak response, dependency scanning, input validation
scope: standalone
---

# Security Audit

## When to Use

Invoke this skill when reviewing code for security vulnerabilities, responding to credential leaks, or establishing security practices for a project.

## OWASP Top 10 Checklist

| # | Vulnerability | What to Check |
|---|--------------|---------------|
| 1 | **Injection** | SQL, command, template injection — parameterize all queries |
| 2 | **Broken Auth** | Weak passwords, missing MFA, session fixation |
| 3 | **Sensitive Data** | Secrets in code, unencrypted PII, verbose error messages |
| 4 | **XXE** | XML parser with external entities enabled |
| 5 | **Broken Access Control** | Missing authorization checks, IDOR, privilege escalation |
| 6 | **Misconfiguration** | Default credentials, debug mode in production, open CORS |
| 7 | **XSS** | Unescaped user input in HTML output |
| 8 | **Insecure Deserialization** | Deserializing untrusted data without validation |
| 9 | **Known Vulnerabilities** | Outdated dependencies with known CVEs |
| 10 | **Insufficient Logging** | No audit trail for security-relevant actions |

## Credential Leak Response

**A credential is compromised the instant it's pushed publicly. History cleanup does NOT undo exposure.**

### Immediate Response (in this order):

```
1. REVOKE the credential immediately (API dashboard, password reset)
2. ROTATE to a new credential
3. AUDIT logs for unauthorized use during exposure window
4. CLEAN history (only after steps 1-3)
   git filter-branch or BFG Repo Cleaner
5. PREVENT future leaks (pre-commit hooks, .gitignore)
```

### Prevention

```bash
# Install git-secrets or similar pre-commit hook
# .gitignore essentials:
.env
.env.*
*.pem
*.key
credentials.json
```

**Never commit:** API keys, passwords, tokens, private keys, connection strings with credentials.

## Input Validation

### At System Boundaries

Validate ALL input at the point it enters your system:

| Input Source | Validation |
|-------------|------------|
| HTTP request params | Type, length, range, format |
| HTTP headers | Expected values only |
| File uploads | Type, size, content validation |
| Database results | Schema validation (defensive) |
| External API responses | Schema validation |
| Environment variables | Required checks at startup |

### Validation Rules

```
- Whitelist over blacklist (allow known-good, reject everything else)
- Validate type before value (is it a number? then check range)
- Validate length before content (reject oversized input early)
- Sanitize for context (HTML-encode for HTML, SQL-parameterize for SQL)
- Never trust client-side validation alone
```

## Authentication Checklist

- [ ] Passwords hashed with bcrypt/argon2 (not MD5/SHA1)
- [ ] JWT tokens have reasonable expiry (< 1 hour for access tokens)
- [ ] Refresh tokens stored securely, rotated on use
- [ ] Failed login attempts rate-limited
- [ ] Session tokens invalidated on logout
- [ ] Password reset tokens single-use and time-limited

## Dependency Scanning

```bash
# Check for known vulnerabilities
npm audit                          # Node.js
cargo audit                        # Rust
pip-audit                          # Python
gh api /repos/:owner/:repo/vulnerability-alerts  # GitHub
```

**Rules:**
- Run `audit` in CI on every build
- Pin dependency versions in production
- Update dependencies regularly (monthly minimum)
- Review changelogs before major version bumps

## Anti-Patterns

- **Security through obscurity**: Hiding the endpoint doesn't protect it
- **Rolling your own crypto**: Use established libraries (ring, openssl, libsodium)
- **Storing secrets in code**: Use environment variables or secret managers
- **Trusting internal networks**: Zero-trust — validate even internal requests
- **Logging sensitive data**: Never log passwords, tokens, PII, credit cards
- **Ignoring audit findings**: A known unpatched vulnerability is negligence
