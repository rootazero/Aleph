# Playwright Cookbook

## Take a Screenshot

```bash
npx playwright screenshot https://example.com screenshot.png --full-page
npx playwright screenshot https://example.com mobile.png --viewport-size=375,812
npx playwright screenshot https://example.com loaded.png --wait-for-timeout=3000
```

## Record Interactions (Codegen)

```bash
npx playwright codegen https://example.com
npx playwright codegen https://example.com --output test-login.spec.ts
npx playwright codegen --device="iPhone 13" https://example.com
```

## Quick Script Template

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

## E2E Test Execution

```bash
npx playwright test                               # all tests
npx playwright test tests/login.spec.ts           # specific file
npx playwright test --headed                      # visible browser
npx playwright test --trace on-first-retry        # trace on failure
npx playwright show-trace trace.zip               # view trace
npx playwright test --reporter=html               # HTML report
npx playwright show-report
```

## Data Extraction Patterns

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
```

**Fill and submit a form:**
```javascript
await page.fill('#username', 'user@example.com');
await page.fill('#password', 'secret');
await page.click('button[type="submit"]');
await page.waitForURL('**/dashboard');
```
