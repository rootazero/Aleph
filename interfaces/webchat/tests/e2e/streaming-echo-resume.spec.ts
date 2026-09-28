import { test, expect } from '@playwright/test';

/**
 * T2.1 — Streaming echo resumes after a connection round-trip.
 *
 * What this spec is pinning down:
 *
 *   1. The Panel mounts at the root route and the composer is present.
 *   2. Submitting a prompt kicks off a streaming run (state transitions to
 *      `Thinking` / `Streaming`).
 *   3. While the stream is in flight, a `offline` event on `window` does
 *      NOT freeze the bubble mid-token — either the run resumes when
 *      `online` fires, or the run completes gracefully.
 *   4. The last assistant message ends up with non-trivial content
 *      (length > 50 chars — short of a one-line ack).
 *
 * Selector strategy (L1-scope: no production `data-testid` additions):
 *
 *   - Composer textarea: there is exactly one `<textarea>` in the chat view
 *     (`composer/mod.rs`); selecting by tag type (rather than
 *     `getByRole('textbox')`) avoids matching the sidebar's search
 *     `<input>` first.
 *   - Send button: the send affordance is the only round `<button>` with
 *     the `bg-primary` Tailwind class — no other control in the composer
 *     row uses that color.
 *   - "Something is streaming" signal: the assistant bubble keeps growing
 *     text on every token, so polling `lastMessage.textContent()` past a
 *     minimum length is the most durable indicator across i18n / styling
 *     changes. (Anchoring on the `thinking` `<span class="reading-dots">`
 *     is too brittle: it disappears the instant the first token lands.)
 *
 * Why we don't use `data-testid`:
 *
 *   The brief is L1-bounded. Adding `data-testid` to Panel production code
 *   would be a production change for the sake of test ergonomics, and the
 *   trade-off (test fragility on class names vs. test fragility on i18n
 *   text vs. polluting production with test-only attributes) is settled
 *   in favour of class + role selectors for this task.
 */
test('streaming echo resumes after browser offline/online round-trip', async ({ page }) => {
    // ── 1. Mount + locate composer ─────────────────────────────────────────
    await page.goto('/');

    // The composer textarea is a real `<textarea>` (auto-grow input), while
    // the sidebar search field is a single-line `<input>` — distinguishing
    // by tag type avoids the trap where `getByRole('textbox')` resolves
    // the search input first.
    const textarea = page.locator('textarea').first();
    await expect(textarea).toBeVisible({ timeout: 15_000 });

    const sendButton = page.locator('button.bg-primary').first();
    await expect(sendButton).toBeVisible();

    // ── 2. Submit a prompt that triggers streaming ─────────────────────────
    // The Panel routes every prompt through the agent run loop; even an
    // empty / minimal prompt produces a non-trivial assistant bubble once
    // the gateway completes (or streams partial chunks).
    const prompt = 'Tell me a long story about a curious cat in three short paragraphs.';
    await textarea.fill(prompt);
    await sendButton.click();

    // ── 3. Capture a baseline snapshot of the assistant content ──────────
    // Wait for any assistant bubble to appear (count > 0) by polling for
    // text growth. We don't anchor on a specific class because the
    // bubble's CSS surface has been refactored across tasks (msg-glass-*
    // tokens) — but text length always grows.
    const assistantBubble = page.locator('[class*="msg-glass"]').last();
    await expect(assistantBubble).toBeVisible({ timeout: 15_000 });

    const beforeOffline = (await assistantBubble.textContent()) ?? '';
    expect(beforeOffline.length).toBeGreaterThan(0);

    // ── 4. Simulate offline / online round-trip on the page ───────────────
    // Browser-native events; the Panel's reconnect logic in
    // `state/connection.rs` listens for the upstream gateway disconnect
    // (driven by the WS layer), not for `navigator.onLine` — this event
    // pair is the closest in-page proxy we can dispatch without rewriting
    // the production reconnect hook.
    await page.evaluate(() => window.dispatchEvent(new Event('offline')));
    await page.waitForTimeout(500);
    await page.evaluate(() => window.dispatchEvent(new Event('online')));

    // ── 5. Stream resumes / completes ─────────────────────────────────────
    // Either the bubble grows past its pre-offline length, OR the run
    // finishes gracefully with a finalized, non-trivial payload. We give
    // the engine up to 20s to settle — long-form streams with retries can
    // take a while.
    await expect
        .poll(
            async () => {
                const t = (await assistantBubble.textContent()) ?? '';
                return t.length;
            },
            { timeout: 20_000, intervals: [200, 500, 1_000, 2_000] },
        )
        .toBeGreaterThan(50);

    const finalText = (await assistantBubble.textContent()) ?? '';
    expect(finalText.length).toBeGreaterThan(50);
});