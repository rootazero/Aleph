---
name: playwright
description: "Browser automation — E2E testing, screenshots, form filling, and data extraction via Playwright. Use when needing JS-rendered content, browser screenshots, user interaction simulation, E2E test suites, or scraping dynamic pages. Use instead of WebFetch when JavaScript rendering is required."
---

# Browser Automation (Playwright)

## Prerequisites

```bash
npm install -g playwright
npx playwright install chromium
```

## Decision Matrix

| Need | Tool |
|------|------|
| Fetch static HTML/text | `WebFetch` (built-in) |
| Fetch API JSON | `curl` via `http-client` skill |
| JS-rendered content | **Playwright** |
| Screenshots / Form interaction / E2E | **Playwright** |

## Selector Priority (always follow this order)

1. `page.getByRole('button', { name: 'Submit' })` -- accessible, most resilient
2. `page.getByTestId('submit-btn')` -- explicit, stable
3. `page.getByLabel('Email')` -- form elements
4. `page.getByPlaceholder('Enter email')` -- input hints
5. `page.getByText('Click here')` -- visible content
6. `page.locator('css=.class')` -- **last resort**

## Core Operations

For detailed examples (screenshots, codegen, scripts, E2E, data extraction, forms), see [references/playwright-cookbook.md](references/playwright-cookbook.md).

## Critical Rules

- **Never use `page.waitForTimeout()`** -- use `waitForSelector`, `waitForURL`, or `expect`
- **Always close browser** -- `await browser.close()`
- **Use headless by default** -- `headless: false` only for debugging
- **Trace on failure only** -- `trace: 'on-first-retry'` in config

## Gotchas

| Problem | Solution |
|---------|----------|
| Element not found | `await locator.waitFor()` before interaction |
| Flaky clicks | `click({ force: true })` or wait for visible state |
| Timeout in CI | Increase timeout, add `expect.poll()` |
| Auth lost between tests | Use `storageState` to persist cookies |
| SPA never reaches networkidle | Use DOM-based waits instead |

## Aleph Integration

- Synergy with `test` (F2): TDD methodology -> Playwright E2E tests
- Synergy with `web-scraper` (A4): Playwright for JS-rendered, curl for static
