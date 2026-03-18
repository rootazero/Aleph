/**
 * E2E tests for the Reranking Provider settings page.
 *
 * Stub file — to be fleshed out once UI selectors are confirmed.
 * Run `just wasm` before running these tests.
 */

import { test, expect } from '@playwright/test';

test.describe('Reranking Provider Settings', () => {
  test('page loads without errors', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (err) => errors.push(err.message));

    await page.goto('/');
    await page.waitForLoadState('networkidle');

    const realErrors = errors.filter(
      (e) => !e.includes('wasm') && !e.includes('WebSocket')
    );
    expect(realErrors).toEqual([]);
  });
});
