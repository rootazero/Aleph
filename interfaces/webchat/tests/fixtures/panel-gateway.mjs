import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, resolve, sep } from 'node:path';
import { WebSocketServer } from 'ws';

const mime = {
  '.css': 'text/css',
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.json': 'application/json',
  '.wasm': 'application/wasm',
};

export async function createPanelGatewayFixture(staged) {
  const state = {
    httpPaths: [],
    rpc: [],
    sockets: [],
    subscriptions: new Map(),
    sessions: [{
      session_id: 'production-fixture-session',
      shell: 'fixture-shell',
      cwd: '/tmp/terminal-fixture',
      created_at: 1700000000,
      closed: false,
    }],
  };

  const patch = (text, options = {}) => ({
    rows: [{ row: 0, runs: [{ text }] }],
    cursor: [0, text.length],
    ...options,
  });

  const serveStatic = async (request, response) => {
    const url = new URL(request.url ?? '/', 'http://127.0.0.1');
    state.httpPaths.push(url.pathname);
    if (url.pathname === '/health') {
      response.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' });
      response.end('ok');
      return;
    }
    const prefix = url.pathname === '/panel' || url.pathname.startsWith('/panel/') ? '/panel' : '';
    const requested = url.pathname.slice(prefix.length).replace(/^\//, '');
    const relative = requested && extname(requested) ? requested : 'index.html';
    const file = resolve(staged, relative);
    if (!file.startsWith(staged + sep)) {
      response.writeHead(403);
      response.end('forbidden');
      return;
    }
    try {
      const info = await stat(file);
      if (!info.isFile()) throw Error('not a file');
      response.writeHead(200, { 'content-type': mime[extname(file)] ?? 'application/octet-stream' });
      response.end(await readFile(file));
    } catch {
      response.writeHead(404);
      response.end('missing');
    }
  };

  const httpServer = createServer(serveStatic);
  const websocketServer = new WebSocketServer({ noServer: true });
  httpServer.on('upgrade', (request, socket, head) => {
    if (new URL(request.url ?? '/', 'http://127.0.0.1').pathname !== '/ws') {
      socket.destroy();
      return;
    }
    websocketServer.handleUpgrade(request, socket, head, (client) => websocketServer.emit('connection', client, request));
  });

  const send = (client, message) => client.send(JSON.stringify(message));
  const reply = (client, id, result) => send(client, { jsonrpc: '2.0', id, result });
  const reject = (client, id, message, code = -32602) => send(client, { jsonrpc: '2.0', id, error: { code, message } });
  const event = (client, topic, data) => send(client, { jsonrpc: '2.0', method: 'event', params: { topic, data } });
  const validSession = (sessionId) => state.sessions.some((session) => session.session_id === sessionId && !session.closed);
  const validGeometry = (params) => Number.isInteger(params?.rows) && params.rows > 0 && Number.isInteger(params?.cols) && params.cols > 0;

  const harmlessMethods = new Set([
    'behavior_config.get',
    'canvas.list',
    'exec.approvals.pending',
    'gateway.metrics.run_concurrency',
    'health',
    'memory.stats',
    'subagent.tree',
    'telecom.approval.pending',
  ]);

  websocketServer.on('connection', (client) => {
    state.sockets.push(client);
    state.subscriptions.set(client, new Set());
    client.on('message', (raw) => {
      const request = JSON.parse(raw.toString());
      state.rpc.push(request);
      const params = request.params ?? {};
      if (request.method === 'connect') {
        reply(client, request.id, { authorized: true, needs_token: false, user: { id: 'production-fixture-user' } });
        return;
      }
      if (request.method === 'pty.list') {
        reply(client, request.id, { sessions: state.sessions });
        return;
      }
      if (request.method === 'pty.attach') {
        if (!validSession(params.session_id)) {
          reject(client, request.id, 'unknown PTY session');
          return;
        }
        reply(client, request.id, {
          seq: 0,
          rows: 24,
          cols: 80,
          patch: patch('Production host screen ✓', { bracketed_paste: true }),
          scrollback_len: 0,
        });
        return;
      }
      if (request.method === 'pty.spawn') {
        const session = {
          session_id: 'production-spawned-session',
          shell: 'fixture-shell',
          cwd: request.params?.cwd ?? '',
          created_at: 1700000001,
          closed: false,
        };
        state.sessions.push(session);
        reply(client, request.id, { session_id: session.session_id, shell: session.shell, seq: 0, rows: request.params?.rows ?? 24, cols: request.params?.cols ?? 80 });
        return;
      }
      if (request.method === 'config.get') {
        reply(client, request.id, { terminal: { font_family: 'monospace', font_size_px: 14 } });
        return;
      }
      if (request.method === 'pty.resize') {
        if (!validSession(params.session_id) || !validGeometry(params)) {
          reject(client, request.id, 'invalid PTY resize');
          return;
        }
        reply(client, request.id, {});
        return;
      }
      if (request.method === 'pty.input') {
        if (!validSession(params.session_id) || typeof params.data !== 'string' || params.base64 !== true) {
          reject(client, request.id, 'invalid PTY input');
          return;
        }
        reply(client, request.id, {});
        return;
      }
      if (request.method === 'pty.close') {
        if (!validSession(params.session_id)) {
          reject(client, request.id, 'unknown PTY session');
          return;
        }
        reply(client, request.id, {});
        return;
      }
      if (request.method === 'events.subscribe' || request.method === 'events.unsubscribe') {
        if (!Array.isArray(params.topics) || params.topics.some((topic) => typeof topic !== 'string' || topic.length === 0)) {
          reject(client, request.id, 'invalid event topics');
          return;
        }
        const topics = state.subscriptions.get(client);
        params.topics.forEach((topic) => request.method === 'events.subscribe' ? topics.add(topic) : topics.delete(topic));
        reply(client, request.id, {});
        return;
      }
      if (harmlessMethods.has(request.method)) {
        reply(client, request.id, {});
        return;
      }
      reject(client, request.id, `unsupported fixture RPC: ${request.method}`, -32601);
    });
    client.on('close', () => state.subscriptions.delete(client));
  });

  await new Promise((resolve) => httpServer.listen(0, '127.0.0.1', resolve));
  const address = httpServer.address();
  const origin = `http://127.0.0.1:${address.port}`;
  return {
    origin,
    state,
    emit(topic, data) {
      state.sockets.forEach((client) => {
        if (state.subscriptions.get(client)?.has(topic)) event(client, topic, data);
      });
    },
    async close() {
      await Promise.all(state.sockets.map((client) => new Promise((resolve) => {
        if (client.readyState === client.CLOSED) {
          resolve();
          return;
        }
        client.once('close', resolve);
        client.close();
      })));
      await new Promise((resolve) => websocketServer.close(resolve));
      await new Promise((resolve) => httpServer.close(resolve));
    },
  };
}

export function terminalPatch(text, options = {}) {
  return {
    rows: [{ row: 0, runs: [{ text }] }],
    cursor: [0, text.length],
    ...options,
  };
}
