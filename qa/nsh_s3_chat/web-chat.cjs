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
    if (await predicate()) return;
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

    // 1. Pair with the transcript presentation negotiated alongside live events.
    await page.goto(`${fixture.origin}/v1/panel/`);
    await pair(page, fixture.ticket, page.getByText('Gateway: healthy'));
    await waitFor(() => frames.some((f) => f.dir === 'received' && f.frame?.id === 1), 10000, 'hello reply');
    const hello = frames.find((f) => f.dir === 'received' && f.frame?.id === 1).frame;
    assert.deepEqual([...hello.result.features].sort(), ['chat_transcript', 'live_events']);
    step('paired_live_events_granted', { features: hello.result.features, subject: hello.result.subject });

    // 2. The session the desktop shell created: same subject, same transcript.
    // A configured system IME may transform the initial key sequence, but the
    // fixture's final ASCII marker survives composition. Reopen the exact row later.
    const desktopMarker = fixture.desktop_marker;
    assert.match(desktopMarker, /^[a-z]+$/);
    const row = await openFirstSession(page, new RegExp(`${desktopMarker}$`));
    const desktopMessage = await page.getByText(new RegExp(`${desktopMarker}$`)).first().innerText();
    const reply = () => page.getByText(fixture.reply).first();
    await reply().waitFor({ timeout: 15000 });
    const copies = await page.getByText(desktopMessage, { exact: true }).count();
    assert.equal(copies, 1, `the desktop message reached Core ${copies} times`);
    await page.screenshot({ path: shot('history') });
    step('cross_surface_history', { row, desktop_message_copies: copies });
    if (fixture.rich) {
      const detailCall = frames.find((f) => f.dir === 'sent' && f.frame?.method === 'chat.transcript_details');
      const detail = frames.find((f) => f.dir === 'received' && f.frame?.id === detailCall?.frame?.id)?.frame;
      const historyCall = frames.find((f) => f.dir === 'sent' && f.frame?.method === 'chat.history');
      const history = frames.find((f) => f.dir === 'received' && f.frame?.id === historyCall?.frame?.id)?.frame;
      step('rich_gateway_summary', {
        history: history?.result?.messages?.map((message) => ({
          role: message.role, has_run: Boolean(message.run_id),
          rich_reply: message.content.includes('S3 transcript callout'),
        })),
        replay: detail?.result?.runs?.map((run) => run.tools.map((tool) => ({
          kind: tool.change.kind, diff: Boolean(tool.change.presentation),
        }))),
      });
      await page.getByText('S3 transcript callout', { exact: true }).first().waitFor();
      await page.getByLabel('Mermaid flowchart: S3 Start, S3 Finish').first().waitFor();
      await page.getByText('https://example.com/s3-transcript', { exact: true }).first().waitFor();
      await page.getByLabel(`Expand Write · ${fixture.file}`, { exact: true }).click();
      await page.getByText('s3 original line', { exact: true }).first().waitFor();
      await page.getByLabel(`Collapse Write · ${fixture.file}`, { exact: true }).click();
      const editCard = page.getByLabel(`Expand Edit · ${fixture.file}`, { exact: true });
      await editCard.scrollIntoViewIfNeeded();
      const background = (card) => card.evaluate((node) => getComputedStyle(node).backgroundColor);
      await page.mouse.move(0, 0);
      const restingBackground = await background(editCard);
      await editCard.hover();
      await waitFor(async () => (await background(editCard)) !== restingBackground, 3000, 'tool hover highlight');
      await editCard.click();
      await page.getByText('s3 edited line', { exact: true }).first().waitFor();
      const launcher = page.getByText('Open full screen', { exact: true });
      await launcher.click();
      await page.getByText('Close reader', { exact: true }).waitFor();
      await page.screenshot({ path: shot('rich-diff') });
      await page.keyboard.press('Escape');
      await launcher.waitFor();
      const launcherElement = await launcher.elementHandle();
      const restoredFocus = await page.waitForFunction((el) => el.contains(document.activeElement) ||
        (document.activeElement !== document.body && document.activeElement?.contains(el)),
      launcherElement, { timeout: 1500 }).then(() => true, () => false);
      await page.getByLabel(`Collapse Edit · ${fixture.file}`, { exact: true }).click();
      await page.getByText('Context', { exact: true }).click();
      await page.getByText(/System Prompt · [0-9]+ bytes sent/).waitFor();
      await page.getByText(/Tool schemas · [0-9]+ bytes/).waitFor();
      await page.screenshot({ path: shot('rich-context') });
      await page.getByText('Close Context', { exact: true }).click();
      await page.setViewportSize({ width: 1100, height: 480 });
      await page.mouse.move(450, 220);
      await page.mouse.wheel(0, -2000);
      const jump = page.getByText('Jump to bottom', { exact: true });
      await jump.waitFor({ timeout: 10000 });
      await jump.click();
      await jump.waitFor({ state: 'hidden', timeout: 10000 });
      await page.setViewportSize({ width: 1100, height: 760 });
      step('browser_jumps_back_to_transcript_tail');
      for (const method of ['trace.tool_output', 'chat.transcript_details', 'context.breakdown']) {
        assert.ok(sentMethods(frames).includes(method), `${method} was not called`);
      }
      assert.ok(restoredFocus, 'Escape did not return keyboard focus to the reader launcher');
      step('rich_desktop_history_in_web', { restored_focus: restoredFocus, hover_highlight: true });
    }

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
    if (fixture.live_tool) {
      const live = fixture.live_tool;
      const proseBefore = page.getByText(live.before, { exact: true });
      const card = page.getByLabel(`Expand Write · ${live.file}`, { exact: true });
      const proseAfter = page.getByText(live.after, { exact: true });
      await proseBefore.waitFor({ timeout: 15000 });
      await card.waitFor({ timeout: 15000 });
      await proseAfter.waitFor({ timeout: 15000 });
      // Virtual-list slots may be recycled out of DOM order. Check the visible layout.
      const [first, tool, last] = await Promise.all([proseBefore, card, proseAfter].map((item) => item.boundingBox()));
      assert.ok(first && tool && last && first.y < tool.y && tool.y < last.y,
        `unexpected live layout: ${JSON.stringify({ first, tool, last })}`);
      assert.match(await card.innerText(), /Done/);
      assert.ok(await page.getByText('Stop', { exact: true }).isVisible());
      const liveTopics = count(pushedTopics(frames, beforeSend));
      assert.ok(liveTopics['stream.chat_tool'] >= 2, 'missing live tool start or finish');
      assert.ok(!liveTopics['stream.run_complete']);
      await page.screenshot({ path: shot('live-tool-order') });
      step('live_text_tool_text_before_completion', { chat_tool_frames: liveTopics['stream.chat_tool'] });
      await fs.writeFile(live.release, 'seen');
    }
    await waitFor(() => pushedTopics(frames, beforeSend).includes('stream.run_complete'), 30000, 'run_complete');
    await (fixture.live_tool ? page.getByText(fixture.live_tool.after).first() : reply()).waitFor({ timeout: 15000 });
    await page.getByText('Stop', { exact: true }).waitFor({ state: 'hidden' });
    const topics = count(pushedTopics(frames, beforeSend));
    assert.ok(topics['stream.response_chunk'] >= 1, `no chunks: ${JSON.stringify(topics)}`);
    assert.ok(topics['stream.run_accepted'] >= 1, `no run_accepted: ${JSON.stringify(topics)}`);
    // Only the narrowed v1 topics reach the browser.
    const allowed = new Set(['stream.run_accepted', 'stream.response_chunk', 'stream.run_complete',
      'stream.run_error', 'stream.chat_tool', 'connection.warning']);
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
    const rowAgain = await openFirstSession(page, desktopMessage);
    await page.getByText(committed.trim(), { exact: true }).first().waitFor({ timeout: 15000 });
    if (fixture.live_tool) {
      await page.getByLabel(`Expand Write · ${fixture.live_tool.file}`, { exact: true }).waitFor({ timeout: 15000 });
    }
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
