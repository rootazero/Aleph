// Configured-provider proof for exact run-only cancellation on the Classic wire.
// Fixed marker names are test identifiers; no request text or credentials are persisted.
const fs = require('node:fs/promises');
const path = require('node:path');
const { setTimeout: sleep } = require('node:timers/promises');
const WebSocket = require('ws');

class Check extends Error {}
const check = (condition, name) => { if (!condition) throw new Check(name); };

async function until(read, name, ms = 15000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    const value = await read();
    if (value) return value;
    await sleep(40);
  }
  throw new Check(name);
}

class Rpc {
  constructor(origin) {
    this.origin = origin;
    this.nextId = 0;
    this.pending = new Map();
    this.events = [];
  }

  async open() {
    this.socket = new WebSocket(`${this.origin.replace(/^https:/, 'wss:')}/ws`, { rejectUnauthorized: false });
    this.socket.on('message', (payload) => this.receive(JSON.parse(payload.toString('utf8'))));
    await new Promise((resolve, reject) => {
      this.socket.once('open', resolve);
      this.socket.once('error', reject);
    });
  }

  receive(frame) {
    if (frame.id !== undefined && (Object.hasOwn(frame, 'result') || Object.hasOwn(frame, 'error'))) {
      const pending = this.pending.get(frame.id);
      if (!pending) return;
      this.pending.delete(frame.id);
      clearTimeout(pending.timer);
      if (frame.error) pending.reject(new Check('exact_queue_rpc_error'));
      else pending.resolve(frame.result);
      return;
    }
    const event = frame.method === 'event' ? frame.params : frame;
    const data = event.data ?? event.params ?? event.result ?? {};
    if (typeof event.topic === 'string' || typeof event.method === 'string') {
      this.events.push({ topic: event.topic ?? event.method, data });
    }
  }

  call(method, params) {
    const id = `exact-${++this.nextId}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Check('exact_queue_rpc_timeout'));
      }, 15000);
      this.pending.set(id, { resolve, reject, timer });
      this.socket.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
    });
  }

  eventsFor(topic, runId) {
    return this.events.filter((event) => event.topic === topic && event.data.run_id === runId);
  }

  async close() {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Check('exact_queue_closed'));
    }
    this.pending.clear();
    if (this.socket.readyState === WebSocket.OPEN) {
      const closed = new Promise((resolve) => this.socket.once('close', resolve));
      this.socket.close();
      await closed;
    }
  }
}

async function run(fixturePath) {
  const fixture = JSON.parse(await fs.readFile(fixturePath, 'utf8'));
  const report = { passed: false, phase: 'connect', queued: {}, running: {} };
  const control = fixture.queue_control;
  check(typeof control === 'string' && path.isAbsolute(control), 'exact_queue_private_control');
  const statusPath = path.join(control, 'status.json');
  const rpc = new Rpc(fixture.origin);
  const seen = async () => {
    try {
      return JSON.parse(await fs.readFile(statusPath, 'utf8')).markers ?? [];
    } catch (error) {
      if (error.code === 'ENOENT') return [];
      throw error;
    }
  };
  const waitProvider = (marker) => until(async () => (await seen()).includes(marker), `exact_queue_provider_${marker.toLowerCase()}`);
  const release = (marker) => fs.writeFile(path.join(control, `release-${marker}`), 'released\n', { flag: 'wx' });
  const session = `agent:main:voice-exact-${process.pid}`;
  const runs = {};
  const runStates = () => Object.fromEntries(Object.entries(runs).map(([label, run]) => [label, {
    accepted: rpc.eventsFor('stream.run_accepted', run.run_id).length,
    queued: rpc.eventsFor('stream.run_queued', run.run_id).length,
    complete: rpc.eventsFor('stream.run_complete', run.run_id).length,
    errors: rpc.eventsFor('stream.run_error', run.run_id).map((event) => event.data.error_code),
  }]));
  const send = (marker) => rpc.call('chat.send', {
    message: `VOICE_QA_${marker}`, session_key: session, run_only: true, stream: true,
    // Plain text would steer into the active sibling. An attachment cannot use
    // that seam, so this follower exercises the real queued-receipt branch.
    ...(marker === 'QUEUED' ? { attachments: [{ name: 'queued.txt', mime_type: 'text/plain', data: 'cXVldWVk' }] } : {}),
  });
  const abort = (runId) => rpc.call('chat.abort', { run_id: runId, run_only: true });
  const terminal = async (runId, expected) => {
    await until(() => rpc.eventsFor(expected, runId).length === 1, `exact_queue_${expected}_${runId}`);
    check(rpc.eventsFor('stream.run_complete', runId).length === (expected === 'stream.run_complete' ? 1 : 0) &&
      rpc.eventsFor('stream.run_error', runId).length === (expected === 'stream.run_error' ? 1 : 0),
    'exact_queue_single_terminal');
    if (expected === 'stream.run_error') {
      check(rpc.eventsFor(expected, runId)[0].data.error_code === 'CANCELLED', 'exact_queue_cancelled_terminal');
    }
  };
  const checkAbort = (receipt, runId) => {
    report.last_abort = { run_matches: receipt.run_id === runId, aborted: receipt.aborted, dropped: receipt.dropped };
    check(receipt.run_id === runId && receipt.aborted === true && receipt.dropped === 0, 'exact_queue_abort_receipt');
  };

  try {
    await rpc.open();
    const connected = await rpc.call('connect', {});
    check(connected.authorized === true && connected.role === 'operator' && connected.needs_token === false,
      'exact_queue_credentialless_operator');

    report.phase = 'queued_sibling_active';
    const sibling = await send('SIBLING');
    await waitProvider('SIBLING');
    report.phase = 'queued_exact_abort';
    const queued = await send('QUEUED');
    Object.assign(runs, { sibling, queued });
    await until(() => rpc.eventsFor('stream.run_queued', queued.run_id).length === 1, 'exact_queue_queued_event');
    check(!(await seen()).includes('QUEUED'), 'exact_queue_queued_waited_without_provider');
    checkAbort(await abort(queued.run_id), queued.run_id);
    report.phase = 'queued_release_sibling';
    await release('SIBLING');
    await terminal(sibling.run_id, 'stream.run_complete');
    await terminal(queued.run_id, 'stream.run_error');
    const queuedSuccessor = await send('QUEUED_SUCCESSOR');
    await terminal(queuedSuccessor.run_id, 'stream.run_complete');
    report.queued = { sibling_completed: true, cancelled_before_provider: true, successor_completed: true };

    report.phase = 'running_exact_abort';
    const running = await send('RUNNING');
    runs.running = running;
    await waitProvider('RUNNING');
    const runningAbort = await abort(running.run_id);
    await release('RUNNING');
    checkAbort(runningAbort, running.run_id);
    await terminal(running.run_id, 'stream.run_error');
    const runningSuccessor = await send('RUNNING_SUCCESSOR');
    await terminal(runningSuccessor.run_id, 'stream.run_complete');
    report.running = { cancelled_at_provider: true, successor_completed: true };

    report.phase = 'provider_attribution';
    await waitProvider('QUEUED_SUCCESSOR');
    await waitProvider('RUNNING_SUCCESSOR');
    const markers = await seen();
    check(!markers.includes('QUEUED'), 'exact_queue_never_started_cancelled_queued_run');
    // Auxiliary model calls can retain the first-turn marker. Each real gated
    // input must arrive at least once; only the canceled queued run is forbidden.
    for (const marker of ['SIBLING', 'QUEUED_SUCCESSOR', 'RUNNING', 'RUNNING_SUCCESSOR']) {
      check(markers.includes(marker), `exact_queue_provider_${marker.toLowerCase()}`);
    }
    report.phase = 'complete';
    report.passed = true;
  } catch (error) {
    report.failure = error instanceof Check ? error.message : error.name;
    report.observed_events = rpc.events.reduce((counts, event) => {
      counts[event.topic] = (counts[event.topic] ?? 0) + 1;
      return counts;
    }, {});
    report.provider_markers = await seen();
    report.run_states = runStates();
    // Let a gated provider thread finish even when an assertion failed.
    await Promise.allSettled(['SIBLING', 'RUNNING'].map(async (marker) => {
      await fs.writeFile(path.join(control, `release-${marker}`), 'released\n');
    }));
  } finally {
    await rpc.close().catch(() => {});
    await fs.writeFile(fixture.exact_observation, JSON.stringify(report, null, 2));
  }
  return report.passed;
}

if (require.main === module) {
  run(process.argv[2]).then((passed) => { process.exitCode = passed ? 0 : 1; }, () => { process.exitCode = 1; });
}

module.exports = { run };
