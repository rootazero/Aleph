const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const test = require('node:test');
const { classicTurn, operatorConnect } = require('./classic.cjs');
const { decode, traffic } = require('./wire.cjs');

const frame = (direction, data, at = 0, socket = 0) => decode(JSON.stringify(data), direction, socket, at, true);
const req = (id, method, params, at) => frame('sent', { jsonrpc: '2.0', id, method, params }, at);
const reply = (id, result, at) => frame('received', { jsonrpc: '2.0', id, result }, at);
const notify = (method, params, at) => frame('received', { jsonrpc: '2.0', method, params }, at);
const operator = () => reply(1, { role: 'operator', authorized: true, needs_token: false }, 1);
const audio = Buffer.from('synthetic response').toString('base64');
const fingerprint = createHash('sha256').update(audio).digest('hex');
const wav = Buffer.from('524946462600000057415645666d7420100000000100010080bb0000007701000200100064617461020000000000', 'hex');

function trace(earlyTerminal = false) {
  // Acceptance can precede the chat receipt. Classic arms the speaker only after the receipt.
  return [
    req(2, 'voice.stream.start', { language: null }, 2), reply(2, { stream_id: null }, 3),
    req(3, 'voice.transcribe', { audio_base64: wav.toString('base64') }, 4), reply(3, { text: 'Synthetic phrase.' }, 5),
    req(4, 'chat.send', { message: 'Synthetic phrase.', voice_input: true }, 6),
    notify('stream.run_accepted', { run_id: 'owned', session_key: 'session' }, 7),
    ...[
      req(5, 'voice.synthesize', { text: 'PHASE9_RUNTIME_PROOF' }, 10),
      notify('stream.run_complete', { run_id: 'owned', summary: { final_response: 'PHASE9_RUNTIME_PROOF' } }, earlyTerminal ? 7.5 : 9),
      reply(4, { run_id: 'owned', session_key: 'session' }, 8),
    ].sort((a, b) => a.at - b.at),
    reply(5, { audio_base64: audio, mime_type: 'audio/mpeg' }, 11),
  ];
}

test('Classic connection uses text JSON-RPC and proves credential-less loopback authorization', () => {
  const connect = req(1, 'connect', { device_id: 'client-claim' }, 0);
  assert.equal(operatorConnect([connect, operator()]).socket, connect.socket);
  for (const key of ['token', 'device_token', 'bootstrap_ticket']) {
    assert.equal(operatorConnect([req(1, 'connect', { [key]: 'do-not-export' }, 0), operator()]), null);
  }
  assert.equal(operatorConnect([connect, reply(1, { role: 'operator', authorized: true, user_id: 'device-user' }, 1)]), null);
  assert.equal(operatorConnect([connect, { ...operator(), socket: 1 }]), null);
  const text = '{"method":"connect","id":"非 ASCII"}';
  assert.equal(decode(text, 'sent', 0, 0), null);
  assert.equal(decode(Buffer.from(text), 'sent', 0, 0, true), null);
  assert.equal(decode(text, 'sent', 0, 0, true).bytes, Buffer.byteLength(text));
});

test('Classic unpunctuated fixture tail is synthesized only after the exact terminal and send reply', () => {
  const rows = trace();
  const turn = classicTurn(rows, operator(), fingerprint);
  assert.equal(turn.stt.req.inputPcmBytes, 2);
  assert.equal(turn.stt.req.inputSampleRate, 48000);
  const modern = Buffer.from(JSON.stringify({ method: 'voice.turn.transcribe', params: { audio_base64: wav.toString('base64') } }));
  const length = Buffer.alloc(4);
  length.writeUInt32BE(modern.length);
  assert.equal(decode(Buffer.concat([length, modern]), 'sent', 0).inputPcmBytes, null);
  assert.ok(turn.send.reply.at < turn.terminal.at && turn.terminal.at < turn.tts.req.at);
  const early = classicTurn(trace(true), operator(), fingerprint);
  assert.ok(early.accepted.at < early.terminal.at && early.terminal.at < early.send.reply.at && early.send.reply.at < early.tts.req.at);
  const json = JSON.stringify(traffic(rows));
  assert.match(json, /voice\.stream\.start/);
  for (const secret of ['Synthetic phrase.', 'PHASE9_RUNTIME_PROOF', 'owned', audio, fingerprint]) {
    assert.ok(!json.includes(secret));
  }
});

test('Classic rejects ambiguous attribution, streaming fallback drift and different TTS bytes', () => {
  const variants = [
    (r) => r.splice(1, 1, reply(2, { stream_id: 'enabled' }, 3)),
    (r) => r.push(req(6, 'voice.stream.audio', {}, 12)),
    (r) => r.push(req(6, 'voice.turn.synthesize', {}, 12)),
    (r) => r.push({ ...r.find((row) => row.topic === 'stream.run_complete') }),
    (r) => { r.find((row) => row.topic === 'stream.run_complete').socket = 1; },
    (r) => { r.find((row) => row.topic === 'stream.run_complete').run = 'unrelated'; },
    (r) => { r[5].session = 'unrelated'; },
    (r) => { r[4].text = 'unrelated'; },
    (r) => { r[9].outputAudioFingerprint = 'unrelated'; },
    (r) => { r.find((row) => row.topic === 'voice.synthesize').at = 8.5; },
  ];
  for (const vary of variants) {
    const rows = trace();
    vary(rows);
    assert.equal(classicTurn(rows, operator(), fingerprint), null);
  }
});
