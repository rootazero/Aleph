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
  platforms:
    - macos
    - linux
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
