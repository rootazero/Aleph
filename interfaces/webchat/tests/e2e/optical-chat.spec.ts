import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';

const session = 'agent:main:main:s1';
const otherSession = 'agent:main:main:s2';
const run = 'optical-fixture-run';
const machine = { label: 'fixture-translate', revision: 'fixture-machine-1' };
const ai = { label: 'fixture-provider/fixture-model', revision: 'fixture-ai-1' };

const block = { session_key: session, run_id: run, block_id: 'settled-fixture' };

type Request = { id?: unknown; method: string; params?: Record<string, unknown> };
type Fixture = {
    requests: Request[];
    releaseDetail: () => void;
    translationDelta: (text: string) => void;
    releaseTranslation: () => void;
    disconnect: () => void;
    event: (topic: string, data: unknown) => void;
};

async function installFixture(page: Page, { deferDetail = false, deferTranslation = false, deferAi = false } = {}): Promise<Fixture> {
    const requests: Request[] = [];
    let finishDetail: (() => void) | undefined;
    let sendDelta: ((text: string) => void) | undefined;
    let finishTranslation: (() => void) | undefined;
    let connection: WebSocketRoute | undefined;
    let emit: ((topic: string, data: unknown) => void) | undefined;
    await page.routeWebSocket('**/ws', (socket: WebSocketRoute) => {
        connection = socket;
        const send = (value: unknown) => socket.send(JSON.stringify(value));
        emit = (topic, data) => send({ jsonrpc: '2.0', method: 'event', params: { topic, data } });
        socket.onMessage((frame) => {
            const request: Request = JSON.parse(frame.toString());
            requests.push(request);
            const respond = (result: unknown) => {
                if (request.id !== undefined) send({ jsonrpc: '2.0', id: request.id, result });
            };
            const key = request.params?.session_key;
            switch (request.method) {
                case 'connect':
                    respond({ authorized: true });
                    break;
                case 'agents.list':
                    respond({ agents: [{ id: 'main', name: 'Main', is_default: true }] });
                    break;
                case 'sessions.list':
                    respond({ sessions: [[session, 'Measured session'], [otherSession, 'Estimated session']].map(([key, topic]) => ({
                        key, topic, agent_id: 'main', session_type: 'main', message_count: 1,
                        created_at: '2026-11-27T10:00:00Z', last_active_at: '2026-11-27T10:00:00Z',
                    })) });
                    break;
                case 'chat.history':
                    respond({ messages: key === session ? [{
                        role: 'assistant', content: 'Measured fixture reply', run_id: run,
                        context_tokens: 128_000, context_window: 512_000,
                    }] : [{ role: 'user', content: 'Estimated fixture prompt' }], active_run: null });
                    break;
                case 'chat.context_estimate':
                    respond({ used_tokens: 64_000, window_tokens: 512_000 });
                    break;
                case 'context.breakdown': {
                    const finish = () => respond({ session_key: key, turn: 3, layers: [], tools: [] });
                    if (deferDetail) finishDetail = finish;
                    else finish();
                    break;
                }
                case 'trace.by_runs':
                    respond({ runs: key === session ? {
                        [run]: [{ type: 'reasoning_emitted', thought_block_ref: block, text: 'Original settled thought' }],
                    } : {} });
                    break;
                case 'thought.translation.engines':
                    respond({ machine, ai });
                    break;
                case 'thought.translation.start': {
                    const engine = request.params?.engine;
                    respond({});
                    const finish = () => emit?.('thought.translation.update', {
                        request_id: request.params?.request_id,
                        update: { type: 'completed', text: `${engine} fixture translation` },
                    });
                    if (deferTranslation || (deferAi && engine === 'ai')) {
                        sendDelta = (text) => emit?.('thought.translation.update', {
                            request_id: request.params?.request_id,
                            update: { type: 'delta', text },
                        });
                        finishTranslation = finish;
                    } else {
                        finish();
                    }
                    break;
                }
                default:
                    respond({});
            }
        });
    });
    return {
        requests,
        releaseDetail: () => finishDetail?.(),
        translationDelta: (text) => {
            if (!sendDelta) throw new Error('Fixture translation has not started');
            sendDelta(text);
        },
        releaseTranslation: () => finishTranslation?.(),
        disconnect: () => {
            if (!connection) throw new Error('WebSocket fixture is not connected');
            connection.close({ code: 1012, reason: 'Fixture restart' });
        },
        event: (topic, data) => {
            if (!emit) throw new Error('WebSocket fixture is not connected');
            emit(topic, data);
        },
    };
}

async function selectSession(page: Page, name: string) {
    await page.getByText(name, { exact: true }).first().click();
}

test.use({ locale: 'en-US' });

test('measured fill survives reduced motion and keyboard-operated request details', async ({ page }) => {
    await installFixture(page);
    await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'dark' });
    await page.addInitScript(() => { HTMLCanvasElement.prototype.getContext = () => null; });
    await page.goto('/');
    await selectSession(page, 'Measured session');
    const vial = page.getByRole('button', { name: /25%.*128000.*512000/ });
    await expect(vial).toBeVisible();
    await expect(vial.locator('[data-level]')).toHaveAttribute('data-level', 'measured');
    await expect(vial.locator('.panel-context-vial-fill')).toHaveCSS('height', /^\d+(\.\d+)?px$/);
    await expect(vial.locator('.panel-context-vial-fill')).toHaveAttribute('style', /height:\s*25%/);
    await expect(vial.locator('canvas')).toBeHidden();
    await vial.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByText(/Last measured request.*3/)).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(vial).toBeFocused();
    await expect(vial).toHaveAttribute('aria-expanded', 'false');

    const composer = page.locator('.panel-composer-optic:visible, .phone-composer-optic:visible');
    const dark = await composer.evaluate((element) => getComputedStyle(element).backgroundImage);
    expect(dark).toContain('linear-gradient');
    await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'light' });
    await expect.poll(() => composer.evaluate((element) => getComputedStyle(element).backgroundImage)).not.toBe(dark);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(vial).toBeVisible();
    await expect(vial.locator('.panel-context-vial-fill')).toHaveAttribute('style', /height:\s*25%/);
});

test('switching sessions drops late details and keeps estimates out of the fill', async ({ page }) => {
    const fixture = await installFixture(page, { deferDetail: true });
    await page.goto('/');
    await selectSession(page, 'Measured session');
    await page.getByRole('button', { name: /25%.*128000.*512000/ }).click();
    await expect.poll(() => fixture.requests.some((request) => request.method === 'context.breakdown')).toBe(true);
    await selectSession(page, 'Estimated session');
    fixture.releaseDetail();
    const vial = page.getByRole('button', { name: /Estimated context usage.*13%/ });
    await expect(vial).toBeVisible();
    await expect(vial.locator('[data-level]')).toHaveAttribute('data-level', 'unknown');
    await expect(page.getByRole('region', { name: 'Context details' })).toHaveCount(0);
});

test('compaction replaces the measured level and hiding the page stops both animations', async ({ page }) => {
    const fixture = await installFixture(page);
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.goto('/');
    await selectSession(page, 'Measured session');
    await expect(page.getByRole('button', { name: /25%.*128000.*512000/ })).toBeVisible();
    const live = { run_id: 'compaction-fixture', session_key: session };
    fixture.event('run.run_accepted', live);
    fixture.event('run.context_gauge', { ...live, context_tokens: 256_000, context_window: 512_000 });
    await expect(page.getByRole('button', { name: /50%.*256000.*512000/ })).toBeVisible();
    fixture.event('run.context_gauge', { ...live, context_tokens: 51_200, context_window: 512_000 });
    const vial = page.getByRole('button', { name: /10%.*51200.*512000/ });
    await expect(vial.locator('canvas')).toBeVisible();
    const companion = page.locator('.panel-pixel-companion');
    await expect(companion).toHaveAttribute('data-pose', 'working');
    await page.evaluate(() => {
        Object.defineProperty(document, 'hidden', { configurable: true, value: true });
        document.dispatchEvent(new Event('visibilitychange'));
    });
    await expect(vial.locator('canvas')).toBeHidden();
    await expect(vial.locator('.panel-context-vial-fill')).toHaveAttribute('style', /height:\s*10%/);
    await expect(companion.locator('.panel-pixel-sprite')).toHaveAttribute('style', /--panel-pixel-lift:\s*-3(?:\.00)?px/);
});

test('thought reader discloses the destination before each engine sends an addressed block', async ({ page }) => {
    const fixture = await installFixture(page, { deferAi: true });
    const { requests } = fixture;
    await page.goto('/');
    await selectSession(page, 'Measured session');
    await page.getByRole('button', { name: 'Read thoughts' }).click();
    await expect(page.getByText('Original settled thought')).toBeVisible();
    await expect(page.getByText('Sends this block to fixture-translate')).toBeVisible();
    expect(requests.filter(({ method }) => method === 'thought.translation.start')).toHaveLength(0);
    await page.getByLabel('Reading mode').selectOption('bilingual');
    await page.getByRole('button', { name: 'Translate this block' }).click();
    await expect(page.getByText('machine fixture translation')).toBeVisible();
    await expect(page.getByText('Original settled thought')).toBeVisible();
    await page.getByLabel('Translation engine').selectOption('ai');
    await expect(page.getByText('Sends this block to fixture-provider/fixture-model')).toBeVisible();
    await page.getByRole('button', { name: 'Translate this block' }).click();
    await expect.poll(() => requests.some(({ method, params }) =>
        method === 'thought.translation.start' && params?.engine === 'ai')).toBe(true);
    fixture.translationDelta('ai partial');
    await expect(page.getByText('ai partial')).toBeVisible();
    fixture.releaseTranslation();
    await expect(page.getByText('ai fixture translation')).toBeVisible();
    await page.getByLabel('Reading mode').selectOption('translated');
    await expect(page.getByText('Original settled thought')).toHaveCount(0);
    expect(requests.filter(({ method }) => method === 'thought.translation.start').map(({ params }) => {
        const { request_id, ...payload } = params ?? {};
        expect(request_id).toEqual(expect.any(String));
        return payload;
    })).toEqual([
        { block, engine: 'machine', target: 'en', expected_destination: machine },
        { block, engine: 'ai', target: 'en', expected_destination: ai },
    ]);
});

test('reconnect restores measured occupancy and never accepts the old translation request', async ({ page }) => {
    const fixture = await installFixture(page, { deferTranslation: true });
    await page.goto('/');
    await selectSession(page, 'Measured session');
    const vial = page.getByRole('button', { name: /25%.*128000.*512000/ });
    const read = page.getByRole('button', { name: 'Read thoughts' });
    const translate = page.getByRole('button', { name: 'Translate this block' });
    const starts = () => fixture.requests.filter(({ method }) => method === 'thought.translation.start');
    await expect(vial).toBeVisible();
    await read.click();
    await translate.click();
    await expect.poll(() => starts().length).toBe(1);
    const oldId = starts()[0].params?.request_id;
    fixture.disconnect();
    await expect.poll(() => fixture.requests.filter(({ method }) => method === 'connect').length).toBe(2);
    await expect(vial).toBeVisible();
    if (!(await translate.isVisible())) await read.click();
    await translate.click();
    await expect.poll(() => starts().length).toBe(2);
    expect(starts()[1].params?.request_id).not.toBe(oldId);
    fixture.event('thought.translation.update', {
        request_id: oldId,
        update: { type: 'completed', text: 'Old connection translation' },
    });
    fixture.releaseTranslation();
    await expect(page.getByText('machine fixture translation')).toBeVisible();
    await expect(page.getByText('Old connection translation')).toHaveCount(0);
});

test.describe('Chinese phone reader', () => {
    test.use({ locale: 'zh-CN', viewport: { width: 390, height: 844 } });

    test('cancelling a translation preserves the original and drops late completion', async ({ page }) => {
        const fixture = await installFixture(page, { deferTranslation: true });
        await page.goto('/');
        await page.getByRole('button', { name: 'History' }).click();
        await selectSession(page, 'Measured session');
        await page.getByRole('button', { name: '阅读思考' }).click();
        await expect(page.getByText('将此思考块发送给 fixture-translate')).toBeVisible();
        await page.getByRole('button', { name: '翻译此块' }).click();
        await page.getByRole('button', { name: '取消翻译' }).click();
        await expect.poll(() => fixture.requests.find(({ method }) => method === 'thought.translation.cancel')?.params)
            .toEqual({ request_id: expect.any(String) });
        const start = fixture.requests.find(({ method }) => method === 'thought.translation.start')?.params;
        const cancel = fixture.requests.find(({ method }) => method === 'thought.translation.cancel')?.params;
        expect(start).toMatchObject({ target: 'zh', request_id: cancel?.request_id });
        fixture.releaseTranslation();
        await expect(page.getByText('Original settled thought')).toBeVisible();
        await expect(page.getByText('machine fixture translation')).toHaveCount(0);
    });
});
