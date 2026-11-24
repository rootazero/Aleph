// Drives the TSX Chat module in Chrome over Web v1 against the isolated
// Gateway started by run.py. Line protocol with run.py: printing
// RESTART_GATEWAY makes it restart the Gateway and answer one line on stdin,
// a fresh pairing ticket (Web cookie bindings live in Gateway memory, so the
// browser pairs again after a restart: design §7a B2).
const assert = require('node:assert/strict');
const { createHash, X509Certificate } = require('node:crypto');
const fs = require('node:fs/promises');
const path = require('node:path');
const readline = require('node:readline');
const { chromium } = require('playwright');

const HEADER_BYTES = 4;

/** One Web v1 binary message: 4-byte big-endian length, then UTF-8 JSON. */
function decode(payload) {
  if (typeof payload === 'string') return null;
  const bytes = Buffer.from(payload);
  if (bytes.length < HEADER_BYTES) return null;
  const length = bytes.readUInt32BE(0);
  if (bytes.length !== HEADER_BYTES + length) return null;
  try {
    return JSON.parse(bytes.subarray(HEADER_BYTES).toString('utf8'));
  } catch {
    return null;
  }
}

/** Every frame on every socket the page opens, in order. */
function recordFrames(page) {
  const frames = [];
  page.on('websocket', (socket) => {
    socket.on('framesent', ({ payload }) => frames.push({ dir: 'sent', frame: decode(payload) }));
    socket.on('framereceived', ({ payload }) => frames.push({ dir: 'received', frame: decode(payload) }));
  });
  return frames;
}

const sentMethods = (frames, from = 0) =>
  frames.slice(from).filter((f) => f.dir === 'sent' && f.frame).map((f) => f.frame.method);

/** Topics of id-less frames the Gateway pushed (stream.* use their method). */
const pushedTopics = (frames, from = 0) =>
  frames.slice(from)
    .filter((f) => f.dir === 'received' && f.frame && f.frame.method && f.frame.id === undefined)
    .map((f) => (f.frame.method === 'event' ? f.frame.params.topic : f.frame.method));

function count(list) {
  const counts = {};
  for (const item of list) counts[item] = (counts[item] || 0) + 1;
  return counts;
}

async function waitFor(predicate, timeout, what) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${what}`);
}

/** Pairs, then waits for `ready` (the active tab's content). */
async function pair(page, ticket, ready) {
  await page.getByText('Pair this browser with Aleph').waitFor({ timeout: 30000 });
  await page.getByPlaceholder('Pairing ticket or device token').fill(ticket);
  await page.getByText('Pair with ticket').click();
  await ready.waitFor({ timeout: 30000 });
}

/** Sessions → the first session row → Chat shows its transcript. */
async function openFirstSession(page, expectedText) {
  await page.getByLabel('Open Sessions').click();
  const row = page.locator('[aria-label^="Open chat "]').first();
  await row.waitFor({ timeout: 15000 });
  const label = await row.getAttribute('aria-label');
  await row.click();
  await page.getByText(expectedText, { exact: true }).first().waitFor({ timeout: 15000 });
  return label;
}

async function run(fixturePath) {
  const fixture = JSON.parse(await fs.readFile(fixturePath, 'utf8'));
  assert.equal(new URL(fixture.origin).hostname, '127.0.0.1');
  const lines = readline.createInterface({ input: process.stdin })[Symbol.asyncIterator]();
  const publicKey = new X509Certificate(fixture.certificate).publicKey;
  const spki = createHash('sha256').update(publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
  const browser = await chromium.launch({
    executablePath: process.env.ALEPH_BROWSER_LIVE_CHROME_PATH || undefined,
    args: [`--ignore-certificate-errors-spki-list=${spki}`],
  });
  const report = { console: [], steps: [] };
  const shot = (name) => path.join(fixture.out, `web-${name}.png`);
  const step = (name, detail = {}) => {
    report.steps.push({ name, ...detail });
    console.log(`step ${name}`);
  };
  try {
    const context = await browser.newContext({ viewport: { width: 1100, height: 760 } });
    const page = await context.newPage();
    page.on('console', (message) => {
      if (message.type() === 'error' || message.type() === 'warning') {
        report.console.push(`${message.type()}: ${message.text()}`);
      }
    });
    const frames = recordFrames(page);

    // 1. Pair; the hello asks for live_events and gets it.
    await page.goto(`${fixture.origin}/v1/panel/`);
    await pair(page, fixture.ticket, page.getByText('Gateway: healthy'));
    await waitFor(() => frames.some((f) => f.dir === 'received' && f.frame?.id === 1), 10000, 'hello reply');
    const hello = frames.find((f) => f.dir === 'received' && f.frame?.id === 1).frame;
    assert.deepEqual(hello.result.features, ['live_events']);
    step('paired_live_events_granted', { features: hello.result.features, subject: hello.result.subject });

    // 2. The session the desktop shell created: same subject, same transcript.
    const row = await openFirstSession(page, fixture.desktop_message);
    await page.getByText(/This response is from/).first().waitFor({ timeout: 15000 });
    const copies = await page.getByText(fixture.desktop_message, { exact: true }).count();
    assert.equal(copies, 1, `the desktop message reached Core ${copies} times`);
    await page.screenshot({ path: shot('history') });
    step('cross_surface_history', { row, desktop_message_copies: copies });

    // 3. Return while an input method composes does not send.
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await composer.click();
    const cdp = await context.newCDPSession(page);
    const beforeIme = frames.length;
    await cdp.send('Input.imeSetComposition', { text: 'nihao', selectionStart: 5, selectionEnd: 5 });
    await page.waitForTimeout(300);
    await page.keyboard.press('Enter');
    await page.waitForTimeout(1500);
    const imeSent = sentMethods(frames, beforeIme).filter((m) => m === 'chat.send').length;
    await page.screenshot({ path: shot('ime-composing') });
    assert.equal(imeSent, 0, 'Return during a composition sent the message');
    // Commit the candidate the way an input method does.
    await cdp.send('Input.insertText', { text: '你好' });
    await page.waitForTimeout(300);
    const committed = await composer.inputValue();
    assert.ok(committed.includes('你好'), `composer holds ${JSON.stringify(committed)}`);
    step('ime_return_does_not_send', { committed });

    // 4. Return sends; the reply streams in as live frames and lands in history.
    const beforeSend = frames.length;
    await composer.press('Enter');
    await waitFor(() => sentMethods(frames, beforeSend).includes('chat.send'), 10000, 'chat.send');
    const sendFrame = frames.slice(beforeSend).find((f) => f.dir === 'sent' && f.frame?.method === 'chat.send').frame;
    assert.equal(typeof sendFrame.params.idempotency_key, 'string');
    assert.equal(sendFrame.params.message, committed.trim());
    await waitFor(() => pushedTopics(frames, beforeSend).includes('stream.run_complete'), 30000, 'run_complete');
    await page.getByText(/This response is from/).first().waitFor({ timeout: 15000 });
    assert.equal(await page.getByText('Stop', { exact: true }).count(), 0);
    const topics = count(pushedTopics(frames, beforeSend));
    assert.ok(topics['stream.response_chunk'] >= 1, `no chunks: ${JSON.stringify(topics)}`);
    assert.ok(topics['stream.run_accepted'] >= 1, `no run_accepted: ${JSON.stringify(topics)}`);
    // Only the narrowed v1 topics reach the browser.
    const allowed = new Set(['stream.run_accepted', 'stream.response_chunk', 'stream.run_complete',
      'stream.run_error', 'connection.warning']);
    const stray = Object.keys(topics).filter((topic) => !allowed.has(topic));
    assert.deepEqual(stray, []);
    await page.screenshot({ path: shot('streamed') });
    step('send_streams_and_completes', {
      topics,
      methods_after_send: count(sentMethods(frames, beforeSend)),
      idempotency_key_present: true,
    });

    // 5. Stop: the simulated engine finishes in well under a second, so this
    // records whether Stop could be pressed in time; it is not asserted.
    const beforeStop = frames.length;
    await composer.fill('Stop probe');
    await composer.press('Enter');
    let stopOutcome = 'run_finished_before_stop';
    try {
      await page.getByText('Stop', { exact: true }).click({ timeout: 1500 });
      stopOutcome = 'stop_pressed';
    } catch {
      // Already finished.
    }
    await waitFor(
      () => pushedTopics(frames, beforeStop).some((t) => t === 'stream.run_complete' || t === 'stream.run_error'),
      30000, 'second run end',
    );
    const abortReply = frames.slice(beforeStop).find((f) => f.dir === 'sent' && f.frame?.method === 'chat.abort');
    step('stop_probe', {
      outcome: stopOutcome,
      abort_sent: Boolean(abortReply),
      topics: count(pushedTopics(frames, beforeStop)),
    });

    // 6. Gateway restart: the socket drops, Reconnect lands on pairing (the
    // cookie binding was in the old Gateway's memory), pairing again brings
    // the transcript back from disk.
    console.log('RESTART_GATEWAY');
    const next = await lines.next();
    assert.ok(!next.done && next.value.startsWith('aleph-bt-'), 'run.py sent no ticket');
    await page.getByText('Disconnected from Aleph').waitFor({ timeout: 60000 });
    await page.screenshot({ path: shot('disconnected') });
    await page.getByText('Reconnect', { exact: true }).click();
    // The layout kept Chat active: the composer shows once connected.
    await pair(page, next.value.trim(), page.getByRole('textbox', { name: 'Message', exact: true }));
    const rowAgain = await openFirstSession(page, fixture.desktop_message);
    await page.getByText(committed.trim(), { exact: true }).first().waitFor({ timeout: 15000 });
    await page.screenshot({ path: shot('after-restart') });
    step('gateway_restart_repair_restores_history', { row: rowAgain });
    report.ok = true;
  } finally {
    await browser.close();
    await fs.writeFile(path.join(fixture.out, 'web-report.json'), `${JSON.stringify(report, null, 2)}\n`);
  }
}

run(process.argv[2]).then(
  () => process.exit(0),
  (error) => {
    console.error(error);
    process.exit(1);
  },
);
