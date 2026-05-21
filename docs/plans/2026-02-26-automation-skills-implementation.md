# Automation Skills Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create 10 automation SKILL.md files (skills #21-30) with script execution capabilities, extending Aleph's official collection from 20 to 30.

**Architecture:** Each skill is a SKILL.md file with YAML frontmatter declaring `cli-wrapper: true` or `allowed-tools: [Bash]` plus `requirements.binaries`. Skills wrap real CLI tools (gh, ffmpeg, curl, etc.) and provide scene-driven command templates. Aleph's `CliWrapperValidator` enforces binary whitelists for cli-wrapper skills.

**Tech Stack:** Markdown + YAML frontmatter. Key frontmatter fields: `name`, `description`, `cli-wrapper`, `allowed-tools`, `requirements` (binaries, platforms, install), `triggers`, `emoji`, `category`.

**Design Doc:** `docs/plans/2026-02-26-automation-skills-design.md`

**Target Directory:** `skills/automation/` at project root (parallel to existing `skills/foundation/`, `skills/workflow/`, `skills/specialist/`)

---

## Task 1: Create directory structure

**Files:**
- Create: `skills/automation/github/SKILL.md` (placeholder)
- Create: `skills/automation/playwright/SKILL.md` (placeholder)
- Create: `skills/automation/http-client/SKILL.md` (placeholder)
- Create: `skills/automation/web-scraper/SKILL.md` (placeholder)
- Create: `skills/automation/data-pipeline/SKILL.md` (placeholder)
- Create: `skills/automation/media-tools/SKILL.md` (placeholder)
- Create: `skills/automation/notification/SKILL.md` (placeholder)
- Create: `skills/automation/email/SKILL.md` (placeholder)
- Create: `skills/automation/ssh/SKILL.md` (placeholder)
- Create: `skills/automation/typeset/SKILL.md` (placeholder)

**Step 1: Create all directories**

```bash
mkdir -p skills/automation/{github,playwright,http-client,web-scraper,data-pipeline,media-tools,notification,email,ssh,typeset}
```

**Step 2: Verify structure**

```bash
find skills/automation -type d | sort
```

Expected:
```
skills/automation
skills/automation/data-pipeline
skills/automation/email
skills/automation/github
skills/automation/http-client
skills/automation/media-tools
skills/automation/notification
skills/automation/playwright
skills/automation/ssh
skills/automation/typeset
skills/automation/web-scraper
```

**Step 3: Commit**

```bash
git add skills/automation/
git commit -m "skills: create directory structure for 10 automation skills"
```

---

## Task 2: A1 — github (GitHub CLI Automation)

**Files:**
- Create: `skills/automation/github/SKILL.md`

**Step 1: Write the skill**

```markdown
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
```

**Step 2: Verify skill parses correctly**

```bash
cd core && cargo test test_parse_skill -- --nocapture 2>&1 | head -20
```

**Step 3: Commit**

```bash
git add skills/automation/github/SKILL.md
git commit -m "skills: add A1 github automation skill"
```

---

## Task 3: A2 — playwright (Browser Automation)

**Files:**
- Create: `skills/automation/playwright/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: playwright
description: Browser automation — E2E testing, screenshots, form filling, and data extraction via Playwright
emoji: "🎭"
category: automation
cli-wrapper: true
allowed-tools:
  - Bash
requirements:
  binaries:
    - npx
  install:
    - manager: brew
      package: node
triggers:
  - playwright
  - browser
  - screenshot
  - e2e test
  - scrape page
  - automate browser
---

# Browser Automation (Playwright)

## When to Use

Invoke this skill for browser automation: E2E testing, taking screenshots, filling forms, extracting data from rendered pages, or recording user interactions. Use this instead of `WebFetch` when you need JavaScript rendering, user interaction simulation, or visual screenshots.

## Prerequisites

```bash
# Install Playwright
npm install -g playwright

# Install browser (Chromium is sufficient for most tasks)
npx playwright install chromium

# Verify
npx playwright --version
```

## Decision Matrix

| Need | Tool |
|------|------|
| Fetch static HTML/text | `WebFetch` (Aleph built-in) — no install needed |
| Fetch API JSON | `curl` via `http-client` skill |
| JS-rendered content | **Playwright** — this skill |
| Screenshots | **Playwright** — this skill |
| Form interaction | **Playwright** — this skill |
| E2E test suite | **Playwright** — this skill |

## Core Operations

### Take a Screenshot

```bash
# Full page screenshot
npx playwright screenshot https://example.com screenshot.png --full-page

# Specific viewport size
npx playwright screenshot https://example.com mobile.png --viewport-size=375,812

# Wait for network idle before capture
npx playwright screenshot https://example.com loaded.png --wait-for-timeout=3000
```

### Record Interactions (Codegen)

```bash
# Launch codegen — interact with the page, Playwright generates code
npx playwright codegen https://example.com

# Save generated code to file
npx playwright codegen https://example.com --output test-login.spec.ts

# With specific device emulation
npx playwright codegen --device="iPhone 13" https://example.com
```

### Run a Quick Script

Write a temporary Node.js script and execute it:

```javascript
// tmp/scrape.mjs
import { chromium } from 'playwright';

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();
await page.goto('https://example.com');

// Extract text content
const title = await page.textContent('h1');
console.log('Title:', title);

// Extract all links
const links = await page.$$eval('a[href]', els => els.map(e => ({ text: e.textContent.trim(), href: e.href })));
console.log(JSON.stringify(links, null, 2));

await browser.close();
```

```bash
node tmp/scrape.mjs
```

### E2E Test Execution

```bash
# Run all tests
npx playwright test

# Run specific test file
npx playwright test tests/login.spec.ts

# Run with headed browser (visible)
npx playwright test --headed

# Run with trace on failure
npx playwright test --trace on-first-retry

# View trace
npx playwright show-trace trace.zip

# Generate HTML report
npx playwright test --reporter=html
npx playwright show-report
```

### Data Extraction Patterns

**Extract a table:**
```javascript
const rows = await page.$$eval('table tr', rows =>
  rows.map(row => [...row.querySelectorAll('td,th')].map(cell => cell.textContent.trim()))
);
console.log(JSON.stringify(rows));
```

**Extract structured data:**
```javascript
const products = await page.$$eval('.product-card', cards => cards.map(card => ({
  name: card.querySelector('.name')?.textContent.trim(),
  price: card.querySelector('.price')?.textContent.trim(),
  rating: card.querySelector('.rating')?.textContent.trim(),
})));
console.log(JSON.stringify(products, null, 2));
```

**Fill and submit a form:**
```javascript
await page.fill('#username', 'user@example.com');
await page.fill('#password', 'secret');
await page.click('button[type="submit"]');
await page.waitForURL('**/dashboard');
```

## Selector Priority (Always Follow This Order)

1. `page.getByRole('button', { name: 'Submit' })` — accessible, most resilient
2. `page.getByTestId('submit-btn')` — explicit, stable
3. `page.getByLabel('Email')` — form elements
4. `page.getByPlaceholder('Enter email')` — input hints
5. `page.getByText('Click here')` — visible content
6. `page.locator('css=.class')` — **last resort**, avoid nth-child and generated classes

## Critical Rules

- **Never use `page.waitForTimeout()`** — use `waitForSelector`, `waitForURL`, or `expect` with polling
- **Always close browser** — `await browser.close()` to prevent memory leaks
- **Use headless by default** — set `headless: false` only for debugging
- **Trace on failure only** — `trace: 'on-first-retry'` in config, not always-on

## Gotchas

| Problem | Solution |
|---------|----------|
| Element not found | Use `await locator.waitFor()` before interaction |
| Flaky clicks | `await locator.click({ force: true })` or wait for visible state first |
| Timeout in CI | Increase timeout, add `expect.poll()`, check viewport size |
| Auth lost between tests | Use `storageState` to persist cookies/localStorage |
| SPA never reaches networkidle | Use DOM-based waits instead of `waitForLoadState('networkidle')` |

## Aleph Integration

- Synergy with `test` (F2): TDD methodology → Playwright executes E2E tests
- Synergy with `web-scraper` (A4): Playwright for JS-rendered pages, curl for static
- Use Playwright when Aleph's built-in `WebFetch` can't handle JS-rendered content
```

**Step 2: Commit**

```bash
git add skills/automation/playwright/SKILL.md
git commit -m "skills: add A2 playwright browser automation skill"
```

---

## Task 4: A3 — http-client (API Debugging)

**Files:**
- Create: `skills/automation/http-client/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: http-client
description: API debugging and testing — RESTful requests, authentication, response analysis via curl
emoji: "🌐"
category: automation
cli-wrapper: true
requirements:
  binaries:
    - curl
triggers:
  - curl
  - API test
  - HTTP request
  - REST call
  - endpoint
  - webhook
---

# API Debugging (curl)

## When to Use

Invoke this skill when testing APIs, debugging HTTP requests, or validating endpoint behavior. Uses curl exclusively — pre-installed on macOS/Linux, zero additional dependencies.

## Core Operations

### GET Requests

```bash
# Basic GET
curl -s https://api.example.com/users | jq .

# With headers
curl -s -H "Authorization: Bearer $TOKEN" https://api.example.com/users | jq .

# With query parameters
curl -s "https://api.example.com/users?page=2&limit=10" | jq .

# Save response to file
curl -s https://api.example.com/data -o response.json
```

### POST / PUT / PATCH / DELETE

```bash
# POST JSON
curl -s -X POST https://api.example.com/users \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice", "email": "alice@example.com"}' | jq .

# PUT (full update)
curl -s -X PUT https://api.example.com/users/1 \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice Updated", "email": "alice@example.com"}' | jq .

# PATCH (partial update)
curl -s -X PATCH https://api.example.com/users/1 \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice Patched"}' | jq .

# DELETE
curl -s -X DELETE https://api.example.com/users/1 -w "\nHTTP %{http_code}\n"
```

### File Upload

```bash
# Multipart form upload
curl -s -X POST https://api.example.com/upload \
  -F "file=@/path/to/document.pdf" \
  -F "description=My document" | jq .

# Multiple files
curl -s -X POST https://api.example.com/upload \
  -F "files[]=@file1.png" \
  -F "files[]=@file2.png" | jq .
```

### Authentication Patterns

```bash
# Bearer token
curl -s -H "Authorization: Bearer $TOKEN" https://api.example.com/me | jq .

# Basic auth
curl -s -u "username:password" https://api.example.com/auth | jq .

# API key in header
curl -s -H "X-API-Key: $API_KEY" https://api.example.com/data | jq .

# API key in query parameter
curl -s "https://api.example.com/data?api_key=$API_KEY" | jq .

# OAuth2 token exchange
TOKEN=$(curl -s -X POST https://auth.example.com/oauth/token \
  -d "grant_type=client_credentials&client_id=$CLIENT_ID&client_secret=$CLIENT_SECRET" | jq -r '.access_token')
```

### Debugging

```bash
# Verbose mode — see full request/response headers
curl -v https://api.example.com/users 2>&1

# Show response headers only
curl -sI https://api.example.com/users

# Timing breakdown
curl -s -o /dev/null -w "DNS: %{time_namelookup}s\nConnect: %{time_connect}s\nTLS: %{time_appconnect}s\nFirst byte: %{time_starttransfer}s\nTotal: %{time_total}s\nHTTP code: %{http_code}\n" https://api.example.com/users

# Follow redirects
curl -sL https://short.url/abc | head -20

# Show both request and response with body
curl -v -X POST https://api.example.com/data -H "Content-Type: application/json" -d '{"key":"value"}' 2>&1
```

### Response Processing

```bash
# Extract specific field
curl -s https://api.example.com/users | jq '.data[0].email'

# Filter array
curl -s https://api.example.com/users | jq '.data[] | select(.role == "admin")'

# Count results
curl -s https://api.example.com/users | jq '.data | length'

# Extract headers + body
curl -sD - https://api.example.com/users | head -20

# Check status code
STATUS=$(curl -s -o /dev/null -w "%{http_code}" https://api.example.com/health)
echo "Status: $STATUS"
```

### Cookie Management

```bash
# Save cookies
curl -s -c cookies.txt https://api.example.com/login -d '{"user":"alice","pass":"secret"}'

# Use saved cookies
curl -s -b cookies.txt https://api.example.com/dashboard | jq .

# Both save and send
curl -s -b cookies.txt -c cookies.txt https://api.example.com/api/data | jq .
```

## Quick Reference

| Task | curl flags |
|------|-----------|
| Silent output | `-s` |
| JSON body | `-H "Content-Type: application/json" -d '{...}'` |
| Bearer auth | `-H "Authorization: Bearer $TOKEN"` |
| Status code only | `-o /dev/null -w "%{http_code}"` |
| Follow redirects | `-L` |
| Verbose debug | `-v` |
| Response headers | `-I` (HEAD) or `-D -` (with body) |
| Save to file | `-o filename` |
| Timeout | `--connect-timeout 5 --max-time 30` |

## Gotchas

- **JSON POST**: Always set `-H "Content-Type: application/json"` or server may reject
- **Special characters in data**: Use `--data-urlencode` for form data, single quotes for JSON
- **HTTPS cert issues**: Use `--cacert` to specify CA, never use `-k` in production
- **Large responses**: Pipe to `jq .` for formatting, or `| head -100` to preview
- **Binary responses**: Use `-o file.bin` to save, don't print to terminal

## Aleph Integration

- Synergy with `api-design` (W2): design API → `http-client` tests it
- Synergy with `security` (W5): security audit → `http-client` probes endpoints
- Use `jq` from `data-pipeline` (A5) for complex response processing
```

**Step 2: Commit**

```bash
git add skills/automation/http-client/SKILL.md
git commit -m "skills: add A3 http-client API debugging skill"
```

---

## Task 5: A4 — web-scraper (Web Data Extraction)

**Files:**
- Create: `skills/automation/web-scraper/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: web-scraper
description: Web data extraction — structured scraping, search, PDF download via curl and Python
emoji: "🕷️"
category: automation
allowed-tools:
  - Bash
requirements:
  binaries:
    - curl
    - python3
  install:
    - manager: pip
      package: beautifulsoup4
    - manager: pip
      package: requests
triggers:
  - scrape
  - extract data
  - crawl
  - parse HTML
  - download page
---

# Web Data Extraction

## When to Use

Invoke this skill when you need to extract structured data from web pages, download files, or perform web searches without API keys. Use this for data extraction tasks; for testing and interaction, use `playwright` (A2) instead.

## Decision Guide

| Need | Tool |
|------|------|
| Quick text from a page | `WebFetch` (Aleph built-in) |
| JS-rendered content | `playwright` (A2) |
| Structured data extraction | **This skill** |
| Bulk downloads | **This skill** |
| Search without API key | **This skill** |

## Prerequisites

```bash
# Core (optional — curl alone works for basic scraping)
pip install requests beautifulsoup4

# PDF extraction (optional)
pip install pdfplumber

# Verify
python3 -c "from bs4 import BeautifulSoup; print('BS4 OK')"
```

## Core Operations

### Extract Structured Data from HTML

```python
#!/usr/bin/env python3
"""Extract structured data from a web page."""
import requests
from bs4 import BeautifulSoup
import json, sys

url = sys.argv[1] if len(sys.argv) > 1 else "https://example.com"
headers = {"User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"}
resp = requests.get(url, headers=headers, timeout=30)
soup = BeautifulSoup(resp.text, "html.parser")

# Extract all links
links = [{"text": a.get_text(strip=True), "href": a.get("href")}
         for a in soup.select("a[href]") if a.get_text(strip=True)]

# Extract tables
tables = []
for table in soup.select("table"):
    rows = []
    for tr in table.select("tr"):
        cells = [td.get_text(strip=True) for td in tr.select("td,th")]
        if cells:
            rows.append(cells)
    if rows:
        tables.append(rows)

print(json.dumps({"links": links[:20], "tables": tables}, indent=2, ensure_ascii=False))
```

Save as `tmp/scrape.py` and run: `python3 tmp/scrape.py "https://target-url"`

### Curl-Only Scraping (No Python)

```bash
# Download page and extract with grep/sed
curl -s "https://example.com" | grep -oP 'href="\K[^"]+' | head -20

# Extract title
curl -s "https://example.com" | grep -oP '<title>\K[^<]+'

# Download and convert to text (via lynx or w3m if available)
curl -s "https://example.com" | python3 -c "
import sys
from html.parser import HTMLParser
class T(HTMLParser):
    def __init__(self): super().__init__(); self.text = []
    def handle_data(self, d): self.text.append(d.strip())
t = T(); t.feed(sys.stdin.read()); print(' '.join(filter(None, t.text)))
"
```

### Search Without API Keys

```bash
# DuckDuckGo HTML search
curl -s "https://html.duckduckgo.com/html/?q=python+web+scraping" \
  -H "User-Agent: Mozilla/5.0" | python3 -c "
from bs4 import BeautifulSoup
import sys, json
soup = BeautifulSoup(sys.stdin.read(), 'html.parser')
results = [{'title': r.get_text(strip=True), 'url': r.get('href', '')}
           for r in soup.select('.result__a')]
print(json.dumps(results[:10], indent=2))
"
```

### Download Files

```bash
# Download with auto-filename
curl -sLOJ "https://example.com/report.pdf"

# Download to specific path
curl -sL "https://example.com/data.csv" -o downloads/data.csv

# Download multiple URLs
cat urls.txt | xargs -I{} curl -sLOJ "{}"
```

### PDF Text Extraction

```python
#!/usr/bin/env python3
"""Download a PDF and extract text."""
import requests, sys
try:
    import pdfplumber
except ImportError:
    print("Install: pip install pdfplumber"); sys.exit(1)

url = sys.argv[1]
resp = requests.get(url, timeout=30)
with open("/tmp/doc.pdf", "wb") as f:
    f.write(resp.content)

with pdfplumber.open("/tmp/doc.pdf") as pdf:
    for i, page in enumerate(pdf.pages):
        text = page.extract_text()
        if text:
            print(f"--- Page {i+1} ---")
            print(text)
```

## Best Practices

- **Respect robots.txt**: Check `curl -s https://example.com/robots.txt` before scraping
- **Rate limiting**: Add `time.sleep(1)` between requests to avoid being blocked
- **User-Agent**: Always set a realistic User-Agent header
- **Error handling**: Check HTTP status codes, handle timeouts and connection errors
- **Output as JSON**: Default to JSON output for downstream processing with `data-pipeline` (A5)

## Gotchas

- **JS-rendered content**: curl/requests only get raw HTML. Use `playwright` (A2) for SPAs
- **Anti-bot**: Some sites block automated requests. Rotate User-Agents, add delays
- **Encoding**: Check `resp.encoding` and force UTF-8 if needed: `resp.encoding = 'utf-8'`
- **Relative URLs**: Use `urllib.parse.urljoin(base_url, relative_url)` to resolve

## Aleph Integration

- Synergy with `data-pipeline` (A5): scrape → JSON → jq/python processing
- Synergy with `search` (F7): `search` finds information, `web-scraper` extracts data
- Synergy with `playwright` (A2): curl for static, Playwright for JS-rendered
```

**Step 2: Commit**

```bash
git add skills/automation/web-scraper/SKILL.md
git commit -m "skills: add A4 web-scraper data extraction skill"
```

---

## Task 6: A5 — data-pipeline (Data Processing)

**Files:**
- Create: `skills/automation/data-pipeline/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: data-pipeline
description: Data processing — JSON/CSV/TSV transformation, filtering, aggregation via jq and Python
emoji: "🔀"
category: automation
cli-wrapper: true
allowed-tools:
  - Bash
requirements:
  binaries:
    - jq
    - python3
  install:
    - manager: brew
      package: jq
triggers:
  - jq
  - JSON process
  - CSV
  - data transform
  - filter data
  - parse data
---

# Data Processing Pipeline

## When to Use

Invoke this skill for any data transformation task: JSON filtering, CSV manipulation, format conversion, data cleaning, or report generation. Uses jq for quick queries and Python for complex transformations.

## Prerequisites

```bash
# jq (JSON processor)
brew install jq
jq --version

# Python 3 (usually pre-installed)
python3 --version
```

## Dual Path Strategy

| Complexity | Tool | When |
|-----------|------|------|
| Quick filter/extract | `jq` | One-liner, JSON data |
| Multi-step pipeline | `bash` (jq + awk + sort) | Chaining simple operations |
| Complex transform | `python3` | Joins, aggregation, validation, Excel |

## jq Quick Reference

### Basics
```bash
# Pretty print
cat data.json | jq .

# Extract field
jq '.name' data.json

# Nested access
jq '.user.address.city' data.json

# Array first element
jq '.[0]' data.json

# Iterate array
jq '.[]' data.json
```

### Filtering
```bash
# Select by condition
jq '.[] | select(.age > 30)' users.json

# Select by string match
jq '.[] | select(.name | test("^A"))' users.json

# Multiple conditions
jq '.[] | select(.active == true and .role == "admin")' users.json
```

### Transformation
```bash
# Reshape objects
jq '.[] | {name: .full_name, email: .contact.email}' users.json

# Add/modify fields
jq '.[] | . + {status: "active"}' users.json

# Delete fields
jq '.[] | del(.password, .internal_id)' users.json

# Map array
jq '[.[] | .name]' users.json

# Group by
jq 'group_by(.department) | map({dept: .[0].department, count: length})' users.json
```

### Aggregation
```bash
# Count
jq '.data | length' response.json

# Sum
jq '[.[] | .amount] | add' transactions.json

# Min/Max
jq '[.[] | .price] | min' products.json
jq '[.[] | .price] | max' products.json

# Unique values
jq '[.[] | .category] | unique' items.json
```

### Format Conversion
```bash
# JSON → CSV
jq -r '.[] | [.name, .email, .age] | @csv' users.json

# JSON → TSV
jq -r '.[] | [.name, .email, .age] | @tsv' users.json

# JSON Lines → JSON array
jq -s '.' data.jsonl

# JSON array → JSON Lines
jq -c '.[]' data.json
```

### Flags
| Flag | Purpose |
|------|---------|
| `-r` | Raw output (no quotes around strings) |
| `-c` | Compact output (one line per object) |
| `-s` | Slurp: read all inputs into array |
| `-S` | Sort object keys |
| `-e` | Exit with error if output is null/false |
| `--arg k v` | Pass variable: `jq --arg name "Alice" '.[] \| select(.name == $name)'` |

## CSV Operations (Python)

### Read and Filter
```python
#!/usr/bin/env python3
import csv, json, sys

with open(sys.argv[1]) as f:
    reader = csv.DictReader(f)
    rows = [row for row in reader]

# Filter
filtered = [r for r in rows if float(r.get('amount', 0)) > 100]
print(json.dumps(filtered, indent=2))
```

### Aggregate
```python
from collections import Counter, defaultdict

# Group and sum
totals = defaultdict(float)
for row in rows:
    totals[row['category']] += float(row['amount'])
print(json.dumps(dict(totals), indent=2))

# Count by field
counts = Counter(row['status'] for row in rows)
print(json.dumps(dict(counts), indent=2))
```

### Join Two CSVs
```python
import csv

def read_csv(path):
    with open(path) as f:
        return list(csv.DictReader(f))

left = {r['id']: r for r in read_csv('users.csv')}
right = read_csv('orders.csv')
joined = [{**left.get(r['user_id'], {}), **r} for r in right if r['user_id'] in left]
```

### Format Conversion
```bash
# CSV → JSON
python3 -c "import csv,json,sys; print(json.dumps(list(csv.DictReader(open(sys.argv[1]))),indent=2))" data.csv

# JSON → CSV
python3 -c "
import csv,json,sys
data = json.load(open(sys.argv[1]))
if data:
    w = csv.DictWriter(sys.stdout, fieldnames=data[0].keys())
    w.writeheader(); w.writerows(data)
" data.json

# Excel → JSON (requires openpyxl)
python3 -c "
import json
from openpyxl import load_workbook
wb = load_workbook('data.xlsx', read_only=True)
ws = wb.active
rows = list(ws.iter_rows(values_only=True))
headers = [str(h) for h in rows[0]]
data = [dict(zip(headers, row)) for row in rows[1:]]
print(json.dumps(data, indent=2, default=str))
"
```

## Large File Handling

```bash
# Stream JSON Lines (don't load all into memory)
cat huge.jsonl | jq -c 'select(.status == "error")' > errors.jsonl

# Process CSV in chunks
python3 -c "
import csv, sys
with open(sys.argv[1]) as f:
    reader = csv.reader(f)
    header = next(reader)
    count = sum(1 for _ in reader)
    print(f'Rows: {count}, Columns: {len(header)}')
    print(f'Headers: {header}')
" huge.csv
```

## Gotchas

- **jq string interpolation**: Use `\(expr)` inside strings, e.g., `"\(.name) is \(.age)"`
- **Null handling**: Use `// "default"` for fallback, e.g., `jq '.name // "unknown"'`
- **CSV encoding**: Force UTF-8: `open(f, encoding='utf-8')` or `iconv -f latin1 -t utf-8`
- **Floating point**: Use `decimal.Decimal` for financial data in Python

## Aleph Integration

- Synergy with `web-scraper` (A4): extract data → process with jq/python
- Synergy with `http-client` (A3): API response → jq pipeline for analysis
- Synergy with `doc` (F8): processed data → Markdown tables for documentation
```

**Step 2: Commit**

```bash
git add skills/automation/data-pipeline/SKILL.md
git commit -m "skills: add A5 data-pipeline processing skill"
```

---

## Task 7: A6 — media-tools (Media Processing)

**Files:**
- Create: `skills/automation/media-tools/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: media-tools
description: Media processing — video, audio, image manipulation via ffmpeg
emoji: "🎬"
category: automation
cli-wrapper: true
requirements:
  binaries:
    - ffmpeg
  install:
    - manager: brew
      package: ffmpeg
triggers:
  - ffmpeg
  - video
  - audio
  - convert video
  - compress
  - GIF
  - thumbnail
---

# Media Processing (ffmpeg)

## When to Use

Invoke this skill for any video, audio, or image processing task: format conversion, compression, clipping, merging, GIF creation, thumbnail extraction, or metadata inspection.

## Prerequisites

```bash
brew install ffmpeg
ffmpeg -version
```

## Core Operations

### Inspect Media Info

```bash
# Show all streams and metadata
ffprobe -v quiet -print_format json -show_format -show_streams input.mp4 | jq '{
  duration: .format.duration,
  size: .format.size,
  video: .streams[] | select(.codec_type=="video") | {codec: .codec_name, width: .width, height: .height, fps: .r_frame_rate},
  audio: .streams[] | select(.codec_type=="audio") | {codec: .codec_name, sample_rate: .sample_rate, channels: .channels}
}'
```

### Video Operations

```bash
# Cut a clip (fast, no re-encoding — -ss BEFORE -i for speed)
ffmpeg -ss 00:01:30 -i input.mp4 -t 00:00:30 -c copy clip.mp4

# Compress video (CRF: 18=high quality, 23=default, 28=small file)
ffmpeg -i input.mp4 -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k output.mp4

# Change resolution
ffmpeg -i input.mp4 -vf "scale=1280:720" -c:a copy output_720p.mp4

# Change speed (2x faster)
ffmpeg -i input.mp4 -filter:v "setpts=0.5*PTS" -filter:a "atempo=2.0" fast.mp4

# Merge videos (same codec)
echo "file 'part1.mp4'" > list.txt
echo "file 'part2.mp4'" >> list.txt
ffmpeg -f concat -safe 0 -i list.txt -c copy merged.mp4

# Add watermark
ffmpeg -i input.mp4 -i logo.png -filter_complex "overlay=W-w-10:H-h-10" watermarked.mp4

# Add subtitles
ffmpeg -i input.mp4 -vf "subtitles=subs.srt" -c:a copy subtitled.mp4

# Remove audio
ffmpeg -i input.mp4 -an -c:v copy silent.mp4
```

### Audio Operations

```bash
# Extract audio from video
ffmpeg -i video.mp4 -vn -c:a copy audio.aac
ffmpeg -i video.mp4 -vn -c:a libmp3lame -q:a 2 audio.mp3

# Convert audio format
ffmpeg -i input.wav -c:a libmp3lame -q:a 2 output.mp3
ffmpeg -i input.mp3 -c:a aac -b:a 192k output.m4a

# Adjust volume
ffmpeg -i input.mp3 -filter:a "volume=1.5" louder.mp3
ffmpeg -i input.mp3 -filter:a "volume=0.5" quieter.mp3

# Trim audio
ffmpeg -ss 00:00:10 -i input.mp3 -t 00:00:30 -c copy clip.mp3
```

### Image Operations

```bash
# Extract frame at timestamp
ffmpeg -ss 00:00:05 -i video.mp4 -frames:v 1 frame.jpg

# Generate thumbnail grid
ffmpeg -i video.mp4 -vf "fps=1/10,scale=320:-1,tile=5x4" thumbnails.jpg

# Convert image format
ffmpeg -i input.png output.jpg
ffmpeg -i input.jpg -q:v 2 output.webp

# Resize image
ffmpeg -i input.png -vf "scale=800:-1" resized.png

# Create GIF from video
ffmpeg -ss 00:00:05 -i video.mp4 -t 5 -vf "fps=15,scale=480:-1:flags=lanczos" -loop 0 output.gif
```

## Quality Control

| Parameter | Meaning | Range |
|-----------|---------|-------|
| `-crf` | Constant Rate Factor (H.264) | 0=lossless, 18=high, 23=default, 28=low |
| `-preset` | Encoding speed vs compression | ultrafast, fast, medium, slow, veryslow |
| `-q:a` | MP3 quality (LAME) | 0=best, 2=good, 5=ok, 9=worst |
| `-b:a` | Audio bitrate | 128k=standard, 192k=good, 320k=high |
| `-b:v` | Video bitrate | 1M, 2.5M, 5M (use CRF instead when possible) |

## Critical Gotchas

- **`-ss` position matters**: Before `-i` = fast seek (keyframe), after `-i` = precise but slow
- **`-c copy` + `-vf` incompatible**: Filters require re-encoding. Drop `-c copy` when using `-vf`
- **Overwrite**: Add `-y` to auto-overwrite, or ffmpeg will prompt and block
- **Container vs codec**: `.mp4` is container, `h264` is codec. Wrong combination = errors
- **Audio filters need `-filter:a`**: Not `-vf` (video filter)

## Quick Reference

| Task | Command Pattern |
|------|----------------|
| Get info | `ffprobe -v quiet -print_format json -show_streams FILE` |
| Cut clip | `ffmpeg -ss START -i FILE -t DURATION -c copy OUT` |
| Compress | `ffmpeg -i FILE -c:v libx264 -crf 23 OUT` |
| Extract audio | `ffmpeg -i FILE -vn -c:a copy OUT` |
| Extract frame | `ffmpeg -ss TIME -i FILE -frames:v 1 OUT.jpg` |
| Make GIF | `ffmpeg -ss START -i FILE -t DUR -vf "fps=15,scale=480:-1" OUT.gif` |
| Convert format | `ffmpeg -i FILE OUT.newext` |

## Aleph Integration

- Synergy with `data-pipeline` (A5): ffprobe JSON output → jq processing
- Synergy with `notification` (A7): "Video processing complete" → notify user
```

**Step 2: Commit**

```bash
git add skills/automation/media-tools/SKILL.md
git commit -m "skills: add A6 media-tools ffmpeg processing skill"
```

---

## Task 8: A7 — notification (Multi-Channel Notifications)

**Files:**
- Create: `skills/automation/notification/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: notification
description: Multi-channel notifications — macOS native, ntfy.sh, Telegram, webhooks via curl and osascript
emoji: "🔔"
category: automation
allowed-tools:
  - Bash
requirements:
  binaries:
    - curl
    - osascript
  platforms:
    - macos
triggers:
  - notify
  - alert
  - notification
  - remind me
  - send message
---

# Multi-Channel Notifications

## When to Use

Invoke this skill when you need to notify the user about task completion, errors, or events. Supports multiple channels with zero additional dependencies on macOS.

## Channel Overview

| Channel | Setup Required | Best For |
|---------|---------------|----------|
| macOS native | None | Immediate local alerts |
| ntfy.sh | None (free, no signup) | Phone/desktop push notifications |
| Telegram Bot | Bot token + chat ID | Remote messaging |
| Webhook | URL only | Integration with any service |

## Core Operations

### macOS Native Notifications

```bash
# Basic notification
osascript -e 'display notification "Build completed successfully" with title "Aleph" subtitle "CI/CD"'

# With sound
osascript -e 'display notification "Tests passed" with title "Aleph" sound name "Glass"'

# Voice synthesis (speaks aloud)
say "Build complete. All tests passing."

# Voice with specific voice
say -v Samantha "Your deployment is ready."
```

### ntfy.sh (Zero-Config Push)

```bash
# Send notification (anyone with the topic can receive)
curl -s -d "Build #42 completed" ntfy.sh/my-aleph-alerts

# With title and priority
curl -s -H "Title: Deploy Complete" -H "Priority: high" -d "v2.1.0 deployed to production" ntfy.sh/my-aleph-alerts

# With tags (emoji)
curl -s -H "Title: Tests Passed" -H "Tags: white_check_mark" -d "All 142 tests green" ntfy.sh/my-aleph-alerts

# Urgent (bypasses DND on phones)
curl -s -H "Priority: urgent" -H "Tags: rotating_light" -d "Production server down!" ntfy.sh/my-aleph-alerts

# With click URL
curl -s -H "Click: https://github.com/org/repo/pull/123" -d "PR #123 ready for review" ntfy.sh/my-aleph-alerts
```

Subscribe at: `https://ntfy.sh/my-aleph-alerts` (web) or ntfy app (iOS/Android).

### Telegram Bot

```bash
# Send message (requires BOT_TOKEN and CHAT_ID)
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendMessage" \
  -H "Content-Type: application/json" \
  -d "{\"chat_id\": \"$TELEGRAM_CHAT_ID\", \"text\": \"Build complete ✅\", \"parse_mode\": \"Markdown\"}"

# Send with inline keyboard
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendMessage" \
  -H "Content-Type: application/json" \
  -d "{\"chat_id\": \"$TELEGRAM_CHAT_ID\", \"text\": \"PR #123 ready\", \"reply_markup\": {\"inline_keyboard\": [[{\"text\": \"View PR\", \"url\": \"https://github.com/org/repo/pull/123\"}]]}}"

# Send document
curl -s -X POST "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendDocument" \
  -F "chat_id=$TELEGRAM_CHAT_ID" -F "document=@report.pdf"
```

### Generic Webhook

```bash
# Slack-compatible webhook
curl -s -X POST "$WEBHOOK_URL" \
  -H "Content-Type: application/json" \
  -d '{"text": "Deployment complete: v2.1.0 → production"}'

# Discord webhook
curl -s -X POST "$DISCORD_WEBHOOK" \
  -H "Content-Type: application/json" \
  -d '{"content": "Build #42 passed all checks ✅"}'
```

## Notification Routing by Urgency

| Urgency | Channels | Example |
|---------|----------|---------|
| **Urgent** | macOS + ntfy (urgent) + Telegram | Server down, security breach |
| **High** | macOS + ntfy (high) | Build failed, test regression |
| **Normal** | ntfy (default) | Task complete, PR merged |
| **Low** | Batched, defer to quiet hours | Info updates, metrics |

## Anti-Patterns

- **Notification fatigue**: Don't notify for every small event. Batch low-priority items.
- **Quiet hours**: Respect sleep schedules — defer non-urgent notifications.
- **Duplicate channels**: Don't send the same alert to all channels simultaneously.
- **Secrets in notifications**: Never include passwords, tokens, or keys in notification text.

## Aleph Integration

- Aleph R6 principle ("AI Comes to You"): notifications are a core delivery mechanism
- Synergy with `deploy` (W4): deploy complete → notify
- Synergy with `ci-cd` (W7): CI failure → notify
- Synergy with any long-running task: completion → notify user
```

**Step 2: Commit**

```bash
git add skills/automation/notification/SKILL.md
git commit -m "skills: add A7 notification multi-channel skill"
```

---

## Task 9: A8 — email (Email Automation)

**Files:**
- Create: `skills/automation/email/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: email
description: Email automation — send (SMTP) and receive (IMAP) emails via curl
emoji: "📧"
category: automation
cli-wrapper: true
requirements:
  binaries:
    - curl
triggers:
  - email
  - send email
  - check mail
  - SMTP
  - IMAP
---

# Email Automation (curl)

## When to Use

Invoke this skill when you need to send or receive emails programmatically — deployment notifications, automated reports, code review requests, or inbox checking. Uses curl only, zero additional dependencies.

## Security Rules

- **Never use main password** — always use App Passwords or OAuth tokens
- **Store credentials in env vars** — never hardcode in commands or scripts
- **TLS mandatory** — always use `smtps://` (port 465) or STARTTLS (port 587)
- **Test with your own address first** — verify before sending to others

## Provider Configuration

| Provider | SMTP Server | Port | IMAP Server | Port |
|----------|------------|------|-------------|------|
| Gmail | smtp.gmail.com | 465 | imap.gmail.com | 993 |
| Outlook | smtp.office365.com | 587 | outlook.office365.com | 993 |
| iCloud | smtp.mail.me.com | 587 | imap.mail.me.com | 993 |
| Yahoo | smtp.mail.yahoo.com | 465 | imap.mail.yahoo.com | 993 |
| 163.com | smtp.163.com | 465 | imap.163.com | 993 |
| QQ Mail | smtp.qq.com | 465 | imap.qq.com | 993 |

### Gmail App Password Setup

1. Go to https://myaccount.google.com/apppasswords
2. Generate an App Password for "Mail"
3. Store it: `export EMAIL_PASSWORD="xxxx xxxx xxxx xxxx"`

## Send Email (SMTP)

### Basic Send

```bash
curl --ssl-reqd \
  --url "smtps://smtp.gmail.com:465" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  --mail-from "$EMAIL_USER" \
  --mail-rcpt "recipient@example.com" \
  -T - <<EOF
From: $EMAIL_USER
To: recipient@example.com
Subject: Build Report - $(date +%Y-%m-%d)
Content-Type: text/plain; charset=utf-8

Build #42 completed successfully.

- Tests: 142 passed, 0 failed
- Coverage: 87.3%
- Duration: 4m 32s
EOF
```

### HTML Email

```bash
curl --ssl-reqd \
  --url "smtps://smtp.gmail.com:465" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  --mail-from "$EMAIL_USER" \
  --mail-rcpt "recipient@example.com" \
  -T - <<EOF
From: $EMAIL_USER
To: recipient@example.com
Subject: Weekly Report
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8

<h1>Weekly Summary</h1>
<ul>
  <li>PRs merged: 5</li>
  <li>Issues closed: 8</li>
  <li>Deploys: 2</li>
</ul>
EOF
```

### With Attachment

```bash
# Generate MIME with boundary
BOUNDARY="==boundary_$(date +%s)=="
curl --ssl-reqd \
  --url "smtps://smtp.gmail.com:465" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  --mail-from "$EMAIL_USER" \
  --mail-rcpt "recipient@example.com" \
  -T - <<EOF
From: $EMAIL_USER
To: recipient@example.com
Subject: Report with attachment
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="$BOUNDARY"

--$BOUNDARY
Content-Type: text/plain; charset=utf-8

Please find the report attached.

--$BOUNDARY
Content-Type: application/octet-stream; name="report.csv"
Content-Disposition: attachment; filename="report.csv"
Content-Transfer-Encoding: base64

$(base64 report.csv)
--$BOUNDARY--
EOF
```

## Receive Email (IMAP)

```bash
# Check mailbox status (message count)
curl -s --url "imaps://imap.gmail.com:993/INBOX" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  -X "STATUS INBOX (MESSAGES UNSEEN)"

# List recent message subjects (last 5)
curl -s --url "imaps://imap.gmail.com:993/INBOX" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  -X "FETCH 1:5 (BODY[HEADER.FIELDS (FROM SUBJECT DATE)])"

# Search for messages
curl -s --url "imaps://imap.gmail.com:993/INBOX" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD" \
  -X "SEARCH UNSEEN FROM \"github.com\""

# Fetch specific message body
curl -s --url "imaps://imap.gmail.com:993/INBOX;UID=123" \
  --user "$EMAIL_USER:$EMAIL_PASSWORD"
```

## Email Templates

### Code Review Request
```
Subject: Code Review Request: [PR Title]

Hi [Reviewer],

PR #[number] is ready for review.

Changes: [1-2 sentence summary]
Link: [PR URL]
Estimated review time: [X minutes]

Key areas to focus on:
- [Area 1]
- [Area 2]
```

### Deployment Notification
```
Subject: [ENV] Deployed v[version] - [status]

Deployment Summary:
- Environment: [staging/production]
- Version: [version]
- Status: [success/failed]
- Time: [timestamp]
- Deployer: [name]

Changes included:
- [commit 1]
- [commit 2]
```

## Gotchas

- **Gmail blocks "less secure apps"**: Must use App Password, not account password
- **curl IMAP quirks**: IMAP commands via curl are limited; for complex inbox operations, consider Python's `imaplib`
- **Rate limits**: Gmail limits to ~500 emails/day for personal accounts
- **Encoding**: Always set `charset=utf-8` in headers for international text

## Aleph Integration

- Synergy with `notification` (A7): email for formal communications, ntfy for quick alerts
- Synergy with `deploy` (W4): deployment → email stakeholders
- Synergy with `github` (A1): PR events → email notifications
```

**Step 2: Commit**

```bash
git add skills/automation/email/SKILL.md
git commit -m "skills: add A8 email automation skill"
```

---

## Task 10: A9 — ssh (SSH Management)

**Files:**
- Create: `skills/automation/ssh/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: ssh
description: SSH management — connections, port forwarding, file transfer, jump hosts, key management
emoji: "🔑"
category: automation
cli-wrapper: true
requirements:
  binaries:
    - ssh
    - scp
    - rsync
triggers:
  - ssh
  - remote
  - tunnel
  - port forward
  - scp
  - rsync
  - jump host
---

# SSH Management

## When to Use

Invoke this skill for remote server operations: connecting, port forwarding, file transfers, jump host configuration, or key management. All tools are pre-installed on macOS/Linux.

## SSH Config Best Practices

Always use `~/.ssh/config` instead of long command-line flags:

```
# ~/.ssh/config

# Default settings for all hosts
Host *
    AddKeysToAgent yes
    IdentitiesOnly yes
    ServerAliveInterval 60
    ServerAliveCountMax 3

# Production server
Host prod
    HostName 10.0.1.100
    User deploy
    IdentityFile ~/.ssh/id_ed25519_prod
    Port 22

# Jump through bastion
Host internal-db
    HostName 10.0.2.50
    User dbadmin
    ProxyJump bastion

Host bastion
    HostName bastion.example.com
    User jump
    IdentityFile ~/.ssh/id_ed25519_bastion

# Connection multiplexing (reuse connections)
Host *.example.com
    ControlMaster auto
    ControlPath ~/.ssh/sockets/%r@%h-%p
    ControlPersist 600
```

```bash
# Create sockets directory
mkdir -p ~/.ssh/sockets
chmod 700 ~/.ssh/sockets
```

## Core Operations

### Connect

```bash
# Basic connection
ssh user@host

# With specific key
ssh -i ~/.ssh/id_ed25519 user@host

# With port
ssh -p 2222 user@host

# Using config alias
ssh prod
```

### Port Forwarding

```bash
# Local forward: access remote service on local port
# "I want to access remote DB (port 5432) at localhost:15432"
ssh -L 15432:localhost:5432 prod
# Now: psql -h localhost -p 15432

# Remote forward: expose local service to remote
# "Let the remote server access my local dev server (port 3000)"
ssh -R 8080:localhost:3000 prod
# Remote can now access: curl localhost:8080

# Dynamic forward (SOCKS proxy)
# "Route all traffic through remote server"
ssh -D 1080 prod
# Configure browser: SOCKS5 proxy → localhost:1080

# Background tunnel (no interactive shell)
ssh -fNL 15432:localhost:5432 prod
# Kill later: kill $(lsof -ti:15432)
```

### File Transfer

```bash
# SCP: simple file copy
scp file.txt user@host:/remote/path/
scp user@host:/remote/file.txt ./local/
scp -r ./local-dir/ user@host:/remote/dir/

# Rsync: incremental sync (preferred for large transfers)
rsync -avz --progress ./local-dir/ user@host:/remote/dir/
rsync -avz --progress user@host:/remote/dir/ ./local-dir/

# Rsync with exclusions
rsync -avz --exclude='node_modules' --exclude='.git' ./project/ user@host:/deploy/

# Rsync dry run (preview changes)
rsync -avzn ./local/ user@host:/remote/
```

**When to use which:**
| Tool | Best For |
|------|----------|
| `scp` | Quick single file copy |
| `rsync` | Large directories, incremental sync, bandwidth efficiency |
| `sftp` | Interactive file browsing |

### Jump Hosts (Bastion)

```bash
# ProxyJump (modern, preferred)
ssh -J bastion@jump.example.com admin@internal-server

# Multi-hop
ssh -J bastion1,bastion2 admin@deep-internal

# Via config (see SSH Config section above)
ssh internal-db
```

### Key Management

```bash
# Generate key (ed25519 recommended)
ssh-keygen -t ed25519 -C "your@email.com" -f ~/.ssh/id_ed25519_purpose

# Copy public key to server
ssh-copy-id -i ~/.ssh/id_ed25519_purpose.pub user@host

# Add key to agent
ssh-add ~/.ssh/id_ed25519_purpose

# List loaded keys
ssh-add -l

# Remove all keys from agent
ssh-add -D
```

### Remote Command Execution

```bash
# Run single command
ssh prod "uname -a"

# Run script remotely
ssh prod "bash -s" < local-script.sh

# Run with sudo
ssh prod "sudo systemctl restart nginx"

# Pipe data through SSH
cat local.sql | ssh prod "psql -U postgres mydb"
tar czf - ./project | ssh prod "tar xzf - -C /deploy/"
```

## Troubleshooting

```bash
# Verbose connection debugging
ssh -v user@host    # level 1
ssh -vv user@host   # level 2
ssh -vvv user@host  # level 3

# Test connection
ssh -o ConnectTimeout=5 user@host echo "Connected"

# Fix permissions (SSH is strict about this)
chmod 700 ~/.ssh
chmod 600 ~/.ssh/id_ed25519
chmod 644 ~/.ssh/id_ed25519.pub
chmod 600 ~/.ssh/config
chmod 600 ~/.ssh/authorized_keys

# Clear known_hosts entry (after server rebuild)
ssh-keygen -R hostname
```

| Problem | Solution |
|---------|----------|
| Permission denied (publickey) | Check key permissions (600), verify key is in authorized_keys |
| Connection refused | Verify sshd running, check firewall, verify port |
| Connection timed out | Check network, verify host is reachable, check security groups |
| Host key verification failed | Server was rebuilt — `ssh-keygen -R host` to clear old key |
| Broken pipe | Add `ServerAliveInterval 60` to config |

## Aleph Integration

- Synergy with `deploy` (W4): SSH for deployment to remote servers
- Synergy with `shell` (F6): local shell → remote shell via SSH
- Synergy with `security` (W5): key management, audit SSH config
```

**Step 2: Commit**

```bash
git add skills/automation/ssh/SKILL.md
git commit -m "skills: add A9 ssh management skill"
```

---

## Task 11: A10 — typeset (Document Rendering)

**Files:**
- Create: `skills/automation/typeset/SKILL.md`

**Step 1: Write the skill**

```markdown
---
name: typeset
description: Document rendering — Typst/LaTeX to PDF compilation, math rendering, templates
emoji: "📄"
category: automation
allowed-tools:
  - Bash
requirements:
  binaries:
    - curl
  install:
    - manager: brew
      package: typst
triggers:
  - typeset
  - LaTeX
  - Typst
  - PDF
  - render document
  - compile document
---

# Document Rendering (Typst/LaTeX)

## When to Use

Invoke this skill when you need to create professional documents (PDF): papers, reports, resumes, presentations, or math-heavy content. Supports Typst (recommended, modern) and LaTeX (legacy compatible).

## Why Typst Over LaTeX

| Aspect | Typst | LaTeX |
|--------|-------|-------|
| Syntax complexity | Simple, markdown-like | Verbose, lots of backslashes |
| Compilation speed | Instant (<1s) | Slow (seconds to minutes) |
| Error messages | Clear, human-readable | Cryptic, hard to debug |
| Package management | Built-in | texlive (GB-sized) |
| Feature parity | 95%+ for most documents | 100% (decades of packages) |

**Recommendation**: Use Typst for new documents. Use LaTeX only for existing LaTeX projects or when a specific LaTeX package is required.

## Prerequisites

```bash
# Option A: Local Typst (recommended)
brew install typst
typst --version

# Option B: Remote API (no install needed)
# Uses TypeTex API — just needs curl (pre-installed)
```

## Typst Quick Start

### Create a Document

```typst
// document.typ
#set page(paper: "a4", margin: 2cm)
#set text(font: "New Computer Modern", size: 11pt)
#set heading(numbering: "1.1")

= My Document Title

== Introduction

This is a paragraph with *bold* and _italic_ text.

=== Math Support

The quadratic formula: $ x = (-b plus.minus sqrt(b^2 - 4a c)) / (2a) $

Display math:
$ integral_0^infinity e^(-x^2) d x = sqrt(pi) / 2 $

=== Tables

#table(
  columns: (auto, auto, auto),
  [*Name*], [*Role*], [*Score*],
  [Alice], [Engineer], [95],
  [Bob], [Designer], [88],
)

=== Code

```rust
fn main() {
    println!("Hello, Typst!");
}
`` `

=== Images

#image("figure.png", width: 80%)
```

### Compile

```bash
# Typst → PDF
typst compile document.typ

# Watch mode (auto-recompile on save)
typst watch document.typ

# Custom output path
typst compile document.typ output.pdf

# With font path
typst compile --font-path ./fonts document.typ
```

## Remote Compilation (Zero Install)

Using TypeTex API (no API key needed):

```bash
# Compile Typst → PDF
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/typst" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat document.typ | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > output.pdf

# Compile LaTeX → PDF
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/latex" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat document.tex | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > output.pdf
```

## Templates

### Resume
```typst
#set page(paper: "a4", margin: (x: 1.5cm, y: 2cm))
#set text(font: "New Computer Modern", size: 10pt)

#align(center)[
  #text(size: 20pt, weight: "bold")[Your Name]
  #linebreak()
  email\@example.com | github.com/you | +1-234-567-8900
]

#line(length: 100%)

== Experience

*Senior Engineer* | Company Name #h(1fr) 2024 -- Present
- Led team of 5 engineers on core infrastructure
- Reduced build times by 60% through caching strategy

== Education

*M.S. Computer Science* | University Name #h(1fr) 2020 -- 2022
```

### Paper
```typst
#set page(paper: "a4", margin: 2.5cm)
#set text(font: "New Computer Modern", size: 11pt)
#set par(justify: true)
#set heading(numbering: "1.1")

#align(center)[
  #text(size: 16pt, weight: "bold")[Paper Title]
  #linebreak()
  #text(size: 12pt)[Author Name]
  #linebreak()
  #text(size: 10pt, style: "italic")[Institution]
]

#set text(size: 10pt)
*Abstract.* #lorem(80)

= Introduction
#lorem(120)

= Related Work
#lorem(100)

= Methodology
#lorem(150)

= Results
#lorem(100)

= Conclusion
#lorem(60)
```

## LaTeX Quick Reference (for existing projects)

```bash
# Compile LaTeX locally (requires texlive)
# brew install --cask mactex  # WARNING: ~4GB download
pdflatex document.tex
bibtex document
pdflatex document.tex
pdflatex document.tex  # yes, run twice for references
```

### LaTeX Common Gotchas

| Issue | Fix |
|-------|-----|
| Special chars: `# $ % & _ { } ~ ^` | Escape with backslash: `\#`, `\$`, etc. |
| Quotes: "wrong" | Use `` `single' `` or ` ``double'' ` |
| Missing package | `\usepackage{packagename}` in preamble |
| Float placement | Use `[htbp]` or `\usepackage{float}` with `[H]` |
| Table too wide | Use `tabularx` with `X` column type |
| Encoding error | Add `\usepackage[utf8]{inputenc}` |

## Math Rendering to Image

For inline use in chat or documentation:

```bash
# Using TypeTex API
echo '$ E = m c^2 $' > /tmp/math.typ
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/typst" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat /tmp/math.typ | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > /tmp/math.pdf
```

## Aleph Integration

- Synergy with `doc` (F8): co-author content → `typeset` renders to PDF
- Synergy with `data-pipeline` (A5): process data → insert into Typst tables
- Synergy with `email` (A8): render report PDF → email as attachment
```

**Step 2: Commit**

```bash
git add skills/automation/typeset/SKILL.md
git commit -m "skills: add A10 typeset document rendering skill"
```

---

## Task 12: Final Verification

**Step 1: Verify all 10 skills exist**

```bash
find skills/automation -name "SKILL.md" | sort
```

Expected:
```
skills/automation/data-pipeline/SKILL.md
skills/automation/email/SKILL.md
skills/automation/github/SKILL.md
skills/automation/http-client/SKILL.md
skills/automation/media-tools/SKILL.md
skills/automation/notification/SKILL.md
skills/automation/playwright/SKILL.md
skills/automation/ssh/SKILL.md
skills/automation/typeset/SKILL.md
skills/automation/web-scraper/SKILL.md
```

**Step 2: Verify each skill has valid YAML frontmatter**

```bash
for f in skills/automation/*/SKILL.md; do
  name=$(grep "^name:" "$f" | head -1 | sed 's/name: *//')
  desc=$(grep "^description:" "$f" | head -1 | cut -c1-60)
  lines=$(wc -l < "$f")
  echo "✓ $name ($lines lines) — $desc"
done
```

**Step 3: Run Aleph skill parser tests**

```bash
cd core && cargo test skills -- --nocapture 2>&1 | tail -20
```

**Step 4: Count total skills**

```bash
echo "Foundation: $(find skills/foundation -name SKILL.md | wc -l)"
echo "Workflow: $(find skills/workflow -name SKILL.md | wc -l)"
echo "Specialist: $(find skills/specialist -name SKILL.md | wc -l)"
echo "Automation: $(find skills/automation -name SKILL.md | wc -l)"
echo "Total: $(find skills -name SKILL.md | wc -l)"
```

Expected:
```
Foundation: 8
Workflow: 7
Specialist: 5
Automation: 10
Total: 30
```

**Step 5: Final commit**

```bash
git add -A
git commit -m "skills: complete 30 official skills (20 knowledge + 10 automation)"
```
