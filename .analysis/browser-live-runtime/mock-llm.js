// Local scripted OpenAI-compatible mock LLM for browser-live E2E.
// Only the LLM is mocked; Gateway, agent loop, tools, CDP and Chrome are real.
const http = require('http');
const fs = require('fs');
const path = require('path');
const PORT = 18791;
const LOG = path.join(__dirname, 'mock-llm.log');
const log = (m) => fs.appendFileSync(LOG, `${new Date().toISOString()} ${m}\n`);

const PAGE = `<!doctype html><html><head><title>Aleph Live Fixture</title></head>
<body style="background:#1e6;font:32px sans-serif"><h1 id="fixture">Aleph browser-live fixture</h1>
<p id="clock"></p><script>setInterval(()=>{document.getElementById('clock').textContent=new Date().toISOString()},250)</script>
<button id="btn" onclick="this.textContent='clicked'">click me</button></body></html>`;

function decide(body) {
  const msgs = body.messages || [];
  const tools = (body.tools || []).map((t) => t.function && t.function.name).filter(Boolean);
  log(`request stream=${!!body.stream} msgs=${msgs.length} tools=${tools.length} has_browser_open=${tools.includes('browser_open')}`);
  const lastUser = [...msgs].reverse().find((m) => m.role === 'user');
  const userText = JSON.stringify(msgs.filter((m) => m.role === 'user').map((m) => m.content)); log('user: ' + userText.slice(0, 200));
  const toolResults = msgs.filter((m) => m.role === 'tool');
  const lastIdx = msgs.length - 1; log('tail: ' + JSON.stringify(msgs.slice(-3)).slice(0, 1500));
  const lastIsTool = lastIdx >= 0 && msgs[lastIdx].role === 'tool';
  if (toolResults.length === 0 && /OPEN_FIXTURE/.test(userText) && tools.includes('browser_open')) {
    const m = userText.match(/OPEN_FIXTURE(?::(\S+))?/);
    const url = (m && m[1]) || `http://127.0.0.1:${PORT}/page`;
    return { tool: { name: 'browser_open', args: { url, engine: 'chromium', profile: 'default' } } };
  }
  if (toolResults.length) log(`tool result: ${String(msgs[lastIdx].content).slice(0, 300)}`);
  return { text: toolResults.length ? 'Fixture opened.' : `mock reply (${toolResults.length})` };
}

function sendNonStream(res, d) {
  const message = d.tool
    ? { role: 'assistant', content: null, tool_calls: [{ id: 'call_' + Date.now(), type: 'function', function: { name: d.tool.name, arguments: JSON.stringify(d.tool.args) } }] }
    : { role: 'assistant', content: d.text };
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(JSON.stringify({ id: 'cmpl', object: 'chat.completion', created: Math.floor(Date.now() / 1000), model: 'mock', choices: [{ index: 0, message, finish_reason: d.tool ? 'tool_calls' : 'stop' }], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } }));
}

function sendStream(res, d) {
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
  const base = { id: 'cmpl', object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000), model: 'mock' };
  const w = (delta, finish) => res.write(`data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta, finish_reason: finish || null }] })}\n\n`);
  if (d.tool) {
    w({ role: 'assistant', tool_calls: [{ index: 0, id: 'call_' + Date.now(), type: 'function', function: { name: d.tool.name, arguments: '' } }] });
    w({ tool_calls: [{ index: 0, function: { arguments: JSON.stringify(d.tool.args) } }] });
    w({}, 'tool_calls');
  } else {
    w({ role: 'assistant', content: d.text });
    w({}, 'stop');
  }
  res.write(`data: ${JSON.stringify({ ...base, choices: [], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } })}\n\n`);
  res.end('data: [DONE]\n\n');
}

http.createServer((req, res) => {
  if (req.method === 'GET' && req.url.startsWith('/page')) {
    res.writeHead(200, { 'content-type': 'text/html' });
    return res.end(PAGE);
  }
  if (req.method === 'GET' && req.url.startsWith('/v1/models')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ object: 'list', data: [{ id: 'mock-model', object: 'model' }] }));
  }
  let raw = '';
  req.on('data', (c) => (raw += c));
  req.on('end', () => {
    log(`${req.method} ${req.url}`);
    if (!req.url.includes('chat/completions')) { res.writeHead(404); return res.end(); }
    let body = {};
    try { body = JSON.parse(raw); } catch (e) { log('bad json'); }
    const d = decide(body);
    log(`decision ${JSON.stringify(d)}`);
    body.stream ? sendStream(res, d) : sendNonStream(res, d);
  });
}).listen(PORT, '127.0.0.1', () => log(`listening ${PORT}`));
