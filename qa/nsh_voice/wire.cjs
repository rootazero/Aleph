// Web v1 and Classic observation. Keep content fingerprints in memory only.
const { createHash } = require('node:crypto');
const { performance } = require('node:perf_hooks');

const TRANSCRIBE = 'voice.turn.transcribe';
const SYNTHESIZE = 'voice.turn.synthesize';
const CANCEL = 'voice.turn.cancel';
const FORBIDDEN = /^(voice\.stream\.|voice\.format$|voice\.synthesize$|voice\.transcribe\.delta$)/;
const digest = (value) => typeof value === 'string' && value.length > 0
  ? createHash('sha256').update(value, 'utf8').digest('hex') : null;
const length = (value) => typeof value === 'string' ? Buffer.byteLength(value) : 0;
const MAX_PCM = 20 * 16000 * 2;
const MAX_FRAME = 2 * 1024 * 1024;
const MAX_BASE64 = Math.ceil((MAX_PCM + 44) / 3) * 4;

function pcmInfo(encoded, classic) {
  if (typeof encoded !== 'string' || encoded.length < 60 || encoded.length > (classic ? MAX_FRAME : MAX_BASE64) ||
      encoded.length % 4) return null;
  const pad = encoded.endsWith('==') ? 2 : encoded.endsWith('=') ? 1 : 0;
  const size = encoded.length / 4 * 3 - pad;
  const header = Buffer.from(encoded.slice(0, 60), 'base64');
  if (header.length < 44 || header.toString('ascii', 0, 4) !== 'RIFF' || header.readUInt32LE(4) !== size - 8 ||
      header.toString('ascii', 8, 16) !== 'WAVEfmt ' || header.readUInt32LE(16) !== 16 ||
      header.readUInt32LE(20) !== 0x10001 || header.readUInt32LE(32) !== 0x100002 ||
      header.toString('ascii', 36, 40) !== 'data' || header.readUInt32LE(40) !== size - 44 || size <= 44 || size % 2) return null;
  // Classic labels PCM with the AudioContext's native rate; Voice v1 is fixed at 16 kHz.
  const rate = header.readUInt32LE(24);
  if ((classic ? rate < 8000 || rate > 192000 : rate !== 16000) || header.readUInt32LE(28) !== rate * 2) return null;
  return { bytes: size - 44, rate };
}

function decode(payload, direction, socket, at = performance.now(), classic = false) {
  const bytes = typeof payload === 'string' ? Buffer.byteLength(payload) : payload.length;
  if (bytes < 4 || bytes > MAX_FRAME) return null;
  if (classic ? typeof payload !== 'string' : typeof payload === 'string' || payload.readUInt32BE(0) !== bytes - 4) return null;
  const { id, method, params: p, result: r, error, topic, data } = JSON.parse(classic ? payload : payload.subarray(4).toString('utf8'));
  const body = direction === 'sent' ? p : r ?? (method === 'event' ? p?.data : data ?? p);
  const pcm = pcmInfo(p?.audio_base64, classic);
  return {
    direction, socket, at, bytes,
    id, method, topic: method === 'event' ? p?.topic : topic ?? method,
    turn: p?.turn_id, run: body?.run_id,
    runOnly: p?.run_only === true,
    session: body?.session_key, sessionPresent: direction === 'sent' && Object.hasOwn(p ?? {}, 'session_key'),
    voice: p?.voice_input === true,
    idempotencyKey: typeof p?.idempotency_key === 'string' && p.idempotency_key.length > 0 ? p.idempotency_key : null,
    text: digest(p?.message ?? p?.input ?? p?.text ?? r?.text), final: digest(body?.summary?.final_response),
    captureId: p != null && Object.keys(p).some((k) => /capture.*id|clip.*id/i.test(k)),
    inputAudioBytes: length(p?.audio_base64), inputPcmBytes: pcm?.bytes ?? null, inputSampleRate: pcm?.rate ?? null,
    outputAudioBytes: length(r?.audio_base64), outputAudioFingerprint: digest(r?.audio_base64),
    errorKind: error?.data?.kind, runErrorCode: body?.error_code,
    failed: !!error, cancelled: r?.cancelled, aborted: r?.aborted, dropped: r?.dropped,
    features: Array.isArray(r?.features) ? r.features : [],
    ...(classic ? {
      credentialPresent: method === 'connect' && ['token', 'device_token', 'bootstrap_ticket'].some((key) => Object.hasOwn(p ?? {}, key)),
      loopbackOperator: r?.authorized === true && r?.role === 'operator' && r?.needs_token === false &&
        r?.device_token == null && r?.device_id == null && r?.user_id == null,
      streamingDisabled: r?.stream_id === null,
    } : {}),
  };
}

function observe(page, origin, classic = false) {
  const frames = [];
  let sockets = 0;
  let invalid = false;
  const closures = [];
  page.on('websocket', (ws) => {
    if (ws.url() !== `${origin.replace(/^https:/, 'wss:')}${classic ? '/ws' : '/v1/ws'}`) {
      invalid = true;
      return;
    }
    const socket = sockets++;
    ws.on('close', () => closures.push(socket));
    for (const [event, direction] of [['framesent', 'sent'], ['framereceived', 'received']]) {
      ws.on(event, ({ payload }) => {
        try {
          if (frames.length >= 1024) throw new Error();
          const frame = decode(payload, direction, socket, performance.now(), classic);
          if (!frame) throw new Error();
          frames.push(frame);
        } catch {
          invalid = true;
        }
      });
    }
  });
  return { frames, closures, valid: () => !invalid };
}

const requests = (rows, method) => rows.filter((row) => row.direction === 'sent' && row.method === method);
const replies = (rows, req) => req?.id == null ? [] : rows.filter((row) =>
  row.direction === 'received' && row.socket === req.socket && row.id === req.id && row.method == null);
const runEvents = (rows, method, run) => rows.filter((row) =>
  run?.run && row.direction === 'received' && row.socket === run.socket && row.topic === method && row.run === run.run);
const events = (rows, method, run) => runEvents(rows, method, run).filter((row) => !row.session || row.session === run.session);

function traffic(rows) {
  const sent = Object.create(null), received = Object.create(null);
  for (const row of rows) {
    // Report only method/topic names, never envelope payloads or identifiers.
    const candidate = row.direction === 'sent' ? row.method : row.topic;
    const name = /^[a-z][a-z0-9_.]{0,70}$/i.test(candidate ?? '') ? candidate : '(reply-or-other)';
    const counts = row.direction === 'sent' ? sent : received;
    const entry = counts[name] ??= { count: 0, framed_bytes: 0 };
    entry.count++;
    entry.framed_bytes += row.bytes;
  }
  return { sent, received };
}

function stats(samples) {
  const values = samples.filter(Number.isFinite).sort((a, b) => a - b);
  const p = (q) => values.length ? Math.round(values[Math.ceil(q * values.length) - 1] * 100) / 100 : null;
  return { n: values.length, p50_ms: p(0.5), p95_ms: p(0.95) };
}

module.exports = { TRANSCRIBE, SYNTHESIZE, CANCEL, FORBIDDEN, MAX_PCM, decode, observe, requests, replies, runEvents, events, traffic, stats };
