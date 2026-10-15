#!/usr/bin/env node
// Real lifecycle driver for the browser-live viewer cases `target-destroy` and
// `ttl-expiry`. It uses only the production Gateway surface:
//   agent.run (mock-LLM fixture turn -> real browser_open -> real
//   browser.live.resource) -> browser.viewer.resource.decide(safe) ->
//   browser.viewer.grant.issue(frames) -> spawn `run.sh --case <case>` with the
//   launch URL passed through the ENVIRONMENT only (never argv/file/log).
// then performs / observes the real lifecycle effect:
//   target-destroy: browser_tabs list (exact tab_id === bound target_id, else
//                   abort) -> browser_tabs close {tab_id}; expects the terminal
//                   event browser.live.closed reason=target_closed.
//   ttl-expiry:     does nothing; waits for the REAL fixed 600 s resource TTL;
//                   expects browser.live.closed reason=expired. No clock mock.
// No credential, launch URL or raw WS frame is ever printed: child output is
// scrubbed of URL fragments; only sanitized numeric timestamps are reported.
//
// Env: ALEPH_BROWSER_LIVE_AUTH_FILE   device-token JSON (consumed in-process)
//      ALEPH_BROWSER_LIVE_GATEWAY_WS  default ws://127.0.0.1:18790/ws
//      ALEPH_BROWSER_LIVE_FIXTURE_URL default http://127.0.0.1:18791/page
//      ALEPH_BROWSER_LIVE_CHROME_PATH passed through to Playwright
// Usage: node qa/browser_live/lifecycle-driver.cjs <target-destroy|ttl-expiry> [--probe]
// Exit: 0 all ok | 2 usage/setup | 3 no resource | 4 decide/grant | 5 target
//       correlation not proven | 6 tool close failed | 7 no/wrong terminal
//       event | 8 no ready marker | 10 playwright failed (see pw_exit) | 9 fatal
'use strict';
const path = require('path');
const fs = require('fs');
const os = require('os');
const { spawn } = require('child_process');

const ROOT = path.resolve(__dirname, '../..');
const WebSocket = require(path.join(ROOT, 'node_modules', 'ws'));
const CASE = process.argv[2];
const PROBE = process.argv.includes('--probe');
const CASES = {
  'target-destroy': { reason: 'target_closed', closedWaitMs: 60_000, readyWaitMs: 90_000 },
  'ttl-expiry': { reason: 'expired', closedWaitMs: 720_000, readyWaitMs: 90_000 },
};
if (!CASES[CASE]) {
  console.error('usage: lifecycle-driver.cjs <target-destroy|ttl-expiry> [--probe]');
  process.exit(2);
}
const WS_URL = process.env.ALEPH_BROWSER_LIVE_GATEWAY_WS || 'ws://127.0.0.1:18790/ws';
const ORIGIN = new URL(WS_URL).origin.replace(/^ws/, 'http');
const FIXTURE = process.env.ALEPH_BROWSER_LIVE_FIXTURE_URL || 'http://127.0.0.1:18791/page';
const AUTH_FILE = process.env.ALEPH_BROWSER_LIVE_AUTH_FILE
  || path.join(ROOT, '.analysis', 'browser-live-runtime', 'device-token.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const rpcError = (e) => (e ? `code=${e.code ?? '?'} message=${String(e.message ?? '').slice(0, 160)}` : '');

function client() {
  const ws = new WebSocket(WS_URL, { headers: { Origin: ORIGIN } });
  let id = 0; const pending = new Map(); const listeners = [];
  ws.on('message', (raw) => {
    let m; try { m = JSON.parse(raw.toString()); } catch { return; }
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
    listeners.forEach((l) => l(m));
  });
  return {
    ws, onEvent: (f) => listeners.push(f),
    open: () => new Promise((r, j) => { ws.once('open', r); ws.once('error', j); }),
    call: (method, params) => new Promise((resolve) => {
      const rid = ++id; pending.set(rid, resolve);
      ws.send(JSON.stringify({ jsonrpc: '2.0', id: rid, method, params }));
    }),
  };
}

// browser_tabs output may reach us as an object, a JSON string, or wrapped;
// find the `tabs` array / `success` flag without assuming the wrapper shape.
function walk(v, pick, depth = 0) {
  if (depth > 6 || v == null) return undefined;
  if (typeof v === 'string') { try { return walk(JSON.parse(v), pick, depth + 1); } catch { return undefined; } }
  if (typeof v === 'object' && !Array.isArray(v)) {
    const direct = pick(v);
    if (direct !== undefined) return direct;
    for (const k of Object.keys(v)) { const r = walk(v[k], pick, depth + 1); if (r !== undefined) return r; }
  }
  return undefined;
}
const findTabs = (v) => walk(v, (o) => (Array.isArray(o.tabs) ? o.tabs : undefined)) ?? null;
const findSuccess = (v) => walk(v, (o) => (typeof o.success === 'boolean' ? o.success : undefined));

(async () => {
  const spec = CASES[CASE];
  const auth = JSON.parse(fs.readFileSync(AUTH_FILE, 'utf8')); // consumed in memory only
  const c = client(); await c.open();
  const conn = await c.call('connect', { client_info: { name: 'e2e-lifecycle' }, device_token: auth.device_token, device_id: auth.device_id });
  if (conn.error || !conn.result?.authorized) { console.error('driver: connect failed', rpcError(conn.error)); process.exit(2); }

  await c.call('events.subscribe', { topics: ['browser.live.*'], except: [] });
  const resources = []; const closed = [];
  c.onEvent((m) => {
    const d = m.params?.data;
    if (m.params?.topic === 'browser.live.resource' && d?.resource) resources.push({ r: d.resource, at: Date.now() });
    if (m.params?.topic === 'browser.live.closed' && d?.closed) closed.push({ c: d.closed, at: Date.now() });
  });

  const run = await c.call('agent.run', { input: `OPEN_FIXTURE:${FIXTURE}`, session_key: `agent:main:lifecycle-${CASE}-${Date.now()}`, stream: true });
  if (run.error) { console.error('driver: agent.run failed', rpcError(run.error)); process.exit(3); }
  const runId = run.result.run_id;
  const find = () => resources.find((x) => x.r.binding?.viewer?.task?.run_id === runId);
  for (let i = 0; i < 120 && !find(); i++) await sleep(500);
  const hit = find();
  if (!hit) { console.error('driver: no browser.live.resource for this run'); process.exit(3); }
  const res = hit.r;
  const boundTarget = res.binding.viewer.target; // {profile, browser_session_id, target_id}

  if (PROBE) {
    const l = await c.call('tools.invoke', { tool_name: 'browser_tabs', arguments: { profile: boundTarget.profile, action: 'list' } });
    const tabs = findTabs(l.result ?? null);
    console.log(JSON.stringify({ probe: true, list_error: rpcError(l.error) || null, tab_ids_match_target: !!tabs && tabs.some((t) => t.id === boundTarget.target_id), tab_count: tabs ? tabs.length : null, result_keys: l.result && typeof l.result === 'object' ? Object.keys(l.result) : typeof l.result }));
    process.exit(0);
  }

  const dec = await c.call('browser.viewer.resource.decide', { resource_id: res.resource_id, expected_revision: res.revision, decision: 'safe' });
  if (dec.error) { console.error('driver: decide failed', rpcError(dec.error)); process.exit(4); }
  const grantAskedAt = Date.now();
  const grant = await c.call('browser.viewer.grant.issue', { resource_id: res.resource_id, permissions: ['frames'] });
  if (grant.error) { console.error('driver: grant.issue failed', rpcError(grant.error)); process.exit(4); }
  const grantIssuedAt = Date.now();
  const launchUrl = grant.result.launch_url; // memory + child env only
  const grantExpiresAtMs = grant.result.expires_at_ms;
  const resourceExpiresAtMs = res.expires_at_ms;

  const signalDir = fs.mkdtempSync(path.join(os.tmpdir(), `aleph-live-${CASE}-`)); // private per-case marker dir
  const readyFile = path.join(signalDir, `${CASE}.ready`);
  const scrub = (s) => {
    let o = String(s).split(launchUrl).join('<URL>');
    const frag = new URL(launchUrl).hash; if (frag.length > 1) o = o.split(frag.slice(1)).join('<FRAG>');
    return o.replace(/#[A-Za-z0-9_.~%=&-]{20,}/g, '#<REDACTED>');
  };
  const child = spawn('bash', ['qa/browser_live/run.sh', '--case', CASE], {
    cwd: ROOT,
    env: { ...process.env, ALEPH_BROWSER_LIVE_VIEWER_URL: launchUrl, ALEPH_BROWSER_LIVE_SIGNAL_DIR: signalDir },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let pwOut = '';
  const grab = (b) => { pwOut += scrub(b); };
  child.stdout.on('data', grab); child.stderr.on('data', grab);
  let pwExit = null;
  const pwDone = new Promise((r) => child.on('close', (code, sig) => { pwExit = code ?? (sig ? 128 : 1); r(); }));

  const out = { case: CASE, expected_reason: spec.reason, resource_id: res.resource_id, target_id: boundTarget.target_id, resource_expires_at_ms: resourceExpiresAtMs, grant_expires_at_ms: grantExpiresAtMs, resource_event_at_ms: hit.at, grant_issued_at_ms: grantIssuedAt, grant_request_ms: grantIssuedAt - grantAskedAt, driver_rc: 0 };
  let rc = 0;
  const fail = (code, msg) => { if (rc === 0) rc = code; out.driver_error = out.driver_error || msg; };

  // 1. initial live frame: Playwright writes <case>.ready only after a real JPEG frame is on screen.
  let readyAt = null;
  for (let i = 0; i < spec.readyWaitMs / 250 && pwExit === null && !fs.existsSync(readyFile); i++) await sleep(250);
  if (fs.existsSync(readyFile)) { readyAt = Date.now(); out.initial_frame_ready_at_ms = Number(fs.readFileSync(readyFile, 'utf8')) || readyAt; }
  else fail(8, 'no ready marker (no initial live frame)');

  const matching = () => closed.find((x) => x.c.resource_id === res.resource_id);
  if (readyAt) {
    if (CASE === 'target-destroy') {
      const l = await c.call('tools.invoke', { tool_name: 'browser_tabs', arguments: { profile: boundTarget.profile, action: 'list' } });
      const tabs = l.error ? null : findTabs(l.result ?? null);
      const exact = tabs ? tabs.filter((t) => t.id === boundTarget.target_id) : [];
      out.tab_count = tabs ? tabs.length : null;
      out.exact_tab_matches = exact.length;
      if (exact.length !== 1) {
        fail(5, `target correlation not proven (list_error=${rpcError(l.error) || 'none'})`);
      } else {
        out.close_requested_at_ms = Date.now();
        const r = await c.call('tools.invoke', { tool_name: 'browser_tabs', arguments: { profile: boundTarget.profile, action: { close: { tab_id: exact[0].id } } } });
        out.close_rpc_error = rpcError(r.error) || null;
        const okFlag = findSuccess(r.result ?? null);
        out.close_tool_success = okFlag ?? null;
        if (r.error || okFlag === false) fail(6, 'browser_tabs close failed');
      }
    } else {
      out.ttl_wait_started_at_ms = Date.now();
      out.remaining_resource_ttl_ms_at_ready = resourceExpiresAtMs - Date.now();
    }
    if (rc === 0) {
      const t0 = Date.now();
      while (!matching() && Date.now() - t0 < spec.closedWaitMs && pwExit === null) await sleep(250);
      // Playwright may finish just before we see the event; give the event a short grace.
      for (let i = 0; i < 20 && !matching(); i++) await sleep(250);
    }
  }
  const ev = matching();
  if (ev) {
    out.closed_event = { reason: ev.c.reason, at_ms: ev.c.at_ms, received_at_ms: ev.at, resource_id_matches: ev.c.resource_id === res.resource_id, target_matches: JSON.stringify(ev.c.binding?.viewer?.target) === JSON.stringify(boundTarget) };
    if (CASE === 'target-destroy') out.close_to_event_ms = ev.at - out.close_requested_at_ms;
    if (CASE === 'ttl-expiry') {
      out.expiry_event_minus_resource_expires_ms = ev.c.at_ms - resourceExpiresAtMs;
      out.resource_ttl_ms_from_resource_event = resourceExpiresAtMs - hit.at; // ~600000 (event arrival lags publish slightly)
      out.event_minus_resource_event_ms = ev.c.at_ms - hit.at;
      out.event_minus_grant_expires_ms = ev.c.at_ms - grantExpiresAtMs;
    }
    if (ev.c.reason !== spec.reason || !out.closed_event.target_matches) fail(7, `unexpected terminal event (reason=${ev.c.reason})`);
  } else if (rc === 0) fail(7, 'no browser.live.closed event for this resource');
  out.other_closed_events_seen = closed.filter((x) => x.c.resource_id !== res.resource_id).length;

  await pwDone;
  out.pw_exit = pwExit;
  if (pwExit !== 0 && rc === 0) fail(10, 'playwright failed (viewer-side assertions)');
  out.playwright_tail = pwOut.split(/\r?\n/).filter((l) => /✓|✘|passed|failed|Error|expect|Received|Expected|Timeout/.test(l)).slice(0, 12);
  out.driver_rc = rc;
  try { fs.rmSync(signalDir, { recursive: true, force: true }); } catch { /* private tmp marker dir only */ }
  console.log(JSON.stringify(out, null, 1));
  c.ws.close();
  process.exit(rc);
})().catch((e) => { console.error('driver fatal', String(e && e.message).slice(0, 200)); process.exit(9); });
