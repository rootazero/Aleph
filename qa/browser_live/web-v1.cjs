// Invoked by the isolated Rust native-TLS fixture, never against an installed Gateway.
const assert = require('node:assert/strict');
const { createHash, X509Certificate } = require('node:crypto');
const fs = require('node:fs/promises');
const path = require('node:path');
const { chromium } = require('playwright');

async function run(fixturePath) {
  const fixture = JSON.parse(await fs.readFile(fixturePath, 'utf8'));
  assert.equal(new URL(fixture.origin).hostname, '127.0.0.1');
  const publicKey = new X509Certificate(fixture.certificate).publicKey;
  const spki = createHash('sha256').update(publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
  const browser = await chromium.launch({
    executablePath: process.env.ALEPH_BROWSER_LIVE_CHROME_PATH || undefined,
    args: [`--ignore-certificate-errors-spki-list=${spki}`],
  });
  try {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto(`${fixture.origin}/health`);
    const result = await page.evaluate(async ({ token, ownKey, foreignKey }) => {
      const protocol = 'aleph.protocol.v1';
      function expect(condition, message) {
        if (!condition) throw new Error(message);
      }
      function bounded(register) {
        return new Promise((resolve, reject) => {
          const timer = setTimeout(() => reject(new Error('browser operation timed out')), 4000);
          register((result) => {
            clearTimeout(timer);
            resolve(result);
          });
        });
      }
      async function open(url = '/v1/ws', requested = protocol) {
        const socket = new WebSocket(url, requested);
        socket.binaryType = 'arraybuffer';
        const accepted = await bounded((done) => {
          socket.addEventListener('open', () => done(true), { once: true });
          socket.addEventListener('error', () => done(false), { once: true });
        });
        if (!accepted) return null;
        if (requested === protocol) expect(socket.protocol === protocol, 'wrong negotiated subprotocol');
        return socket;
      }
      function frame(message) {
        const payload = new TextEncoder().encode(JSON.stringify(message));
        const packet = new Uint8Array(4 + payload.length);
        new DataView(packet.buffer).setUint32(0, payload.length);
        packet.set(payload, 4);
        return packet;
      }
      function call(socket, id, method, params, legacy = false) {
        return bounded((done) => {
          function settle(response) {
            socket.removeEventListener('close', closed);
            socket.removeEventListener('message', received);
            done(response);
          }
          function closed() { settle(null); }
          function received(event) {
            if (legacy) {
              // Delivery of a legacy event after connect is independent of RPC replies.
              const response = JSON.parse(event.data);
              if (response.id === id) settle(response);
              return;
            }
            const bytes = new Uint8Array(event.data);
            if (bytes.length < 4 || new DataView(bytes.buffer).getUint32(0) !== bytes.length - 4) {
              settle({ malformed: true });
              return;
            }
            settle(JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(4))));
          }
          socket.addEventListener('close', closed, { once: true });
          socket.addEventListener('message', received);
          const message = { jsonrpc: '2.0', id, method, params };
          socket.send(legacy ? JSON.stringify(message) : frame(message));
        });
      }
      expect(await open() === null, 'upgrade without credentials succeeded');
      const exchange = await fetch('/v1/session', {
        method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ device_token: token }),
      });
      expect(exchange.status === 204 && await exchange.text() === '', 'explicit exchange did not yield an empty 204');
      expect(!document.cookie.includes('__Host-aleph-v1'), 'Cookie leaked to browser script');
      expect(await open('/v1/ws', 'other.protocol') === null, 'unsupported subprotocol accepted');
      const unready = await open();
      expect(unready !== null, 'credentialed WS upgrade failed');
      const secondPending = await open();
      expect(secondPending !== null, 'second pending socket was refused');
      expect(await open() === null, 'pending hellos did not count against the Gateway cap');
      for (const pending of [unready, secondPending]) {
        const closed = bounded((done) => pending.addEventListener('close', done, { once: true }));
        const denied = await call(pending, 1, 'health');
        expect(denied?.id === 1 && denied?.error?.code === -32000, 'business request admitted before hello');
        await closed;
      }

      const client = await open();
      expect(client !== null, 'second v1 upgrade failed');
      const hello = await call(client, 'browser-hello', 'system.hello', {
        protocol: { major: 1, minor: 12 },
        client: { name: 'qa', version: '1', platform: 'chromium', ui_runtime: 'browser' },
        features: ['snapshot', 'event_replay', 'live_events'],
      });
      // Only `live_events` is implemented; the request loop now also serves
      // the event feed, which must not disturb request/response pairing.
      expect(hello?.id === 'browser-hello' && hello?.result?.protocol?.major === 1 &&
        hello?.result?.protocol?.minor === 0 &&
        JSON.stringify(hello?.result?.features) === '["live_events"]' &&
        hello?.result?.limits?.max_inflight_requests === 1, 'invalid v1 hello result');
      const liveHealth = await call(client, 'live-health', 'health');
      expect(liveHealth?.id === 'live-health' && liveHealth?.result?.status === 'healthy', 'authenticated v1 health failed');
      const list = await call(client, 2, 'sessions.list');
      const sessions = list?.result?.sessions;
      expect(list?.id === 2 && sessions?.length === 1 && sessions[0].key === ownKey,
        'session scope differs from original member');
      // chat.* is mounted since S3; other writes are still refused before Core.
      const write = await call(client, 3, 'agent.run');
      expect(write?.id === 3 && write?.error?.code === -32601 &&
        write?.error?.message === 'method unavailable on Web v1', 'unsupported write entered Core');
      const select = await call(client, 4, 'panel.project.select', { project_id: null });
      expect(select?.id === 4 && select?.error?.code === -32601, 'project selection is mounted on Web v1');
      const snapshot = await call(client, 5, 'panel.capabilities.snapshot');
      expect(snapshot?.id === 5 && snapshot?.error?.code !== -32601, 'capability snapshot not mounted on Web v1');

      const transcriptReads = [
        ['chat.transcript_details', { run_ids: ['foreign-run', 'browser-run', 'absent-run'] }],
        ['trace.tool_output', { run_id: 'browser-run', tool_call_id: 'browser-result' }],
        ['context.breakdown', {}],
      ];
      for (const [index, [method, fields]] of transcriptReads.entries()) {
        const id = 40 + index;
        const request = (session_key) => call(client, id, method, { session_key, ...fields });
        const own = await request(ownKey);
        expect(own?.id === id && own?.result, `${method} did not return an owned result`);
        if (method === 'chat.transcript_details') {
          const [run] = own.result.runs ?? [];
          expect(own.result.runs?.length === 1 && run?.run_id === 'browser-run', 'owned/foreign replay mismatch');
          expect(run.tools?.length === 2 && run.tools[0].change.kind === 'started' &&
            run.tools[1].change.kind === 'finished' && run.tools.every(t => t.tool_call_id === 'browser-result'),
          'owned tool deltas missing or reordered');
          expect(!JSON.stringify(run).includes('excluded'), 'raw trace content leaked');
        } else if (method === 'trace.tool_output') {
          expect(own.result.text === 'browser-owned output', 'unexpected masked output page');
        } else {
          expect(own.result.provider_reported == null, 'provider count without matched evidence');
        }
        const other = await request(foreignKey);
        const missing = await request('agent:missing-web-v1-fixture:main');
        expect(other?.id === id && missing?.id === id && !('result' in other) && !('result' in missing) &&
          other?.error && JSON.stringify(other.error) === JSON.stringify(missing.error),
          `${method} distinguished a foreign session from a missing one`);
      }
      const rawTrace = await call(client, 44, 'trace.by_runs', { session_key: ownKey, run_ids: [] });
      expect(rawTrace?.error?.code === -32601, 'legacy trace is mounted on Web v1');

      const graphReads = ['graph.query', 'graph.search', 'graph.node_detail'];
      const graphWrites = ['graph.update_note', 'graph.rename_note', 'graph.delete_note'];
      for (const [offset, method] of graphReads.entries()) {
        const id = 20 + offset;
        const response = await call(client, id, method);
        expect(response?.id === id && response?.result?.entered === method, `${method} did not enter Core on Web v1`);
      }
      for (const [offset, method] of graphWrites.entries()) {
        const id = 30 + offset;
        const response = await call(client, id, method);
        expect(response?.id === id && response?.error?.code === -32601, `${method} unexpectedly entered Core on Web v1`);
      }

      const legacy = await open('/ws', []);
      expect(legacy !== null, 'legacy WS upgrade failed');
      const oldGate = await call(legacy, 1, 'health', undefined, true);
      expect(oldGate?.error?.code === -32000, 'v1 Cookie authorized legacy without connect');
      legacy.close();
      const oldClient = await open('/ws', []);
      const connected = await call(oldClient, 1, 'connect', { channel_kind: 'desktop' }, true);
      expect(connected?.result?.authorized === true, 'legacy connect no longer works');
      const health = await call(oldClient, 2, 'health', undefined, true);
      expect(health?.id === 2 && health?.result?.status === 'healthy', 'legacy health no longer works');
      oldClient.close();

      const idleClose = new Promise((resolve) => client.addEventListener('close', () => resolve(true), { once: true }));
      window.webV1Probe = async () => {
        expect(await bounded((done) => idleClose.then(done)), 'revoked idle socket remained open');
        expect(await open() === null, 'revoked Cookie upgraded again');
      };
      return { connectionId: hello.result.connection_id };
    }, { token: fixture.token, ownKey: fixture.ownKey, foreignKey: fixture.foreignKey });
    assert.equal(typeof result.connectionId, 'string');
    const cookies = await context.cookies(fixture.origin);
    const hints = cookies.filter(({ name }) => name === '__Host-aleph-v1');
    assert.equal(hints.length, 1);
    const { httpOnly, secure, sameSite, domain, path: cookiePath } = hints[0];
    assert.deepEqual({ httpOnly, secure, sameSite, domain, cookiePath }, {
      httpOnly: true, secure: true, sameSite: 'Strict', domain: '127.0.0.1', cookiePath: '/',
    });
    const alternatePage = await context.newPage();
    assert.equal(new URL(fixture.alternateOrigin).hostname, new URL(fixture.origin).hostname);
    assert.notEqual(new URL(fixture.alternateOrigin).port, new URL(fixture.origin).port);
    await alternatePage.goto(`${fixture.alternateOrigin}/health`);
    const crossOriginRejected = await alternatePage.evaluate((origin) => new Promise((resolve) => {
      const socket = new WebSocket(`${origin.replace('https:', 'wss:')}/v1/ws`, 'aleph.protocol.v1');
      const timer = setTimeout(() => { socket.close(); resolve(false); }, 4000);
      socket.onerror = () => { clearTimeout(timer); resolve(true); };
      socket.onopen = () => { clearTimeout(timer); socket.close(); resolve(false); };
    }), fixture.origin);
    assert(crossOriginRejected, 'cross-origin browser upgrade accepted');

    const root = path.dirname(fixturePath);
    await fs.writeFile(path.join(root, 'revoke.tmp'), result.connectionId);
    await fs.rename(path.join(root, 'revoke.tmp'), path.join(root, 'revoke'));
    let revoked = false;
    for (let elapsed = 0; elapsed < 3000 && !revoked; elapsed += 25) {
      revoked = await fs.access(path.join(root, 'revoked')).then(() => true, () => false);
      if (!revoked) await new Promise((resolve) => setTimeout(resolve, 25));
    }
    assert(revoked, 'fixture did not revoke the exact device token');
    await page.evaluate(() => window.webV1Probe());
    console.log('Web v1 browser: explicit HTTPS exchange, hello, member scope, revocation, and legacy WS passed');
  } finally {
    await browser.close();
  }
}

run(process.argv[2]).catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
