const assert = require('node:assert/strict');
const test = require('node:test');
const { decode } = require('./wire.cjs');
const { hello, stopVoice, ownsChatAbort, cancelledChatTerminal, completedChat, completedVoiceTurn } = require('./web.cjs');

function frame(direction, value, at, socket = 0) {
  const json = Buffer.from(JSON.stringify(value));
  const size = Buffer.alloc(4);
  size.writeUInt32BE(json.length);
  return decode(Buffer.concat([size, json]), direction, socket, at);
}
const req = (id, method, params, at, socket) => frame('sent', { id, method, params }, at, socket);
const reply = (id, result, at, socket) => frame('received', { id, result }, at, socket);
const event = (method, params, at, socket) => frame('received', { method, params }, at, socket);
const handshake = hello([req(0, 'system.hello', {}, 0), reply(0, { features: ['live_events'] }, 1)]);
const conversation = { run_id: 'r', session_key: 's' };
const wav = Buffer.from('524946462600000057415645666d74201000000001000100803e0000007d00000200100064617461020000000000', 'hex');

function trace({ sttSocket = 0, chatSocket = 0, synthSocket = 0, idempotency = 'turn', acceptedSession = 's', runOnly = true } = {}) {
  const audio = reply(4, { audio_base64: 'YWJj' }, 10, synthSocket);
  return { audio, rows: [
    req(2, 'voice.turn.transcribe', { turn_id: 'turn', audio_base64: wav.toString('base64') }, 2, sttSocket),
    reply(2, { text: 'Synthetic input.' }, 3, sttSocket),
    req(3, 'chat.send', { message: 'Synthetic input.', voice_input: true, idempotency_key: idempotency, run_only: runOnly }, 4, chatSocket),
    event('stream.run_accepted', { run_id: 'r', session_key: acceptedSession }, 5, chatSocket),
    event('stream.run_complete', { run_id: 'r', summary: { final_response: 'Synthetic output.' } }, 6, chatSocket),
    reply(3, conversation, 7, chatSocket),
    req(4, 'voice.turn.synthesize', { turn_id: 'turn', text: 'Synthetic output.' }, 8, synthSocket), audio,
  ] };
}

const voice = ({ rows, audio }) => completedVoiceTurn(rows, handshake, audio.outputAudioFingerprint);

test('TSX cancellation clicks the exact installed Stop control', async () => {
  let clicks = 0;
  const page = { getByRole(role, options) {
    assert.equal(role, 'button');
    assert.deepEqual(options, { name: 'Stop voice conversation', exact: true });
    return { click: async () => { clicks++; } };
  } };
  await stopVoice(page);
  assert.equal(clicks, 1);
});

test('TSX chat cancellation uses exact owned run without a session queue purge', () => {
  const run = reply(1, conversation, 1);
  const abort = (params = { run_id: 'r', run_only: true }, socket = 0) => req(2, 'chat.abort', params, 2, socket);
  const ack = (result = { run_id: 'r', aborted: true, dropped: 0 }) => reply(2, result, 3);
  assert.ok(ownsChatAbort(abort(), ack(), run));
  for (const invalid of [
    { run_id: 'r', run_only: true, session_key: 's' }, { run_id: 'r', run_only: true, session_key: null },
    { run_id: 'other', run_only: true }, { run_id: 'r' }, { run_id: 'r', run_only: false },
  ]) {
    assert.ok(!ownsChatAbort(abort(invalid), ack(), run));
  }
  assert.ok(!ownsChatAbort(abort(undefined, 1), ack(), run));
  assert.ok(!ownsChatAbort(abort(), { ...ack(), socket: 1 }, run));
  assert.ok(!ownsChatAbort(abort(), { ...ack(), id: 'other-request' }, run));
  for (const invalid of [{ dropped: 1 }, { dropped: undefined }, { aborted: false }, { run_id: 'other' }]) {
    assert.ok(!ownsChatAbort(abort(), ack({ run_id: 'r', aborted: true, dropped: 0, ...invalid }), run));
  }
});

test('TSX cancellation needs one matching terminal after the abort request, even if it precedes the ACK', () => {
  const run = reply(1, conversation, 1);
  const abort = req(2, 'chat.abort', { run_id: 'r', run_only: true }, 3);
  const ack = reply(2, { run_id: 'r', aborted: true, dropped: 0 }, 5);
  const cancelled = (at, params = {}, socket = 0) =>
    event('stream.run_error', { run_id: 'r', error_code: 'CANCELLED', ...params }, at, socket);
  const terminal = cancelled(4);
  const result = { run_complete: 0, run_error: 1 };
  assert.deepEqual(cancelledChatTerminal([abort, terminal, ack], run, abort), result);
  assert.deepEqual(cancelledChatTerminal([abort, ack, cancelled(6, { session_key: 's' })], run, abort), result);
  for (const rows of [
    [cancelled(2), abort, ack],
    [abort, cancelled(4, { session_key: 'another' }), ack],
    [abort, cancelled(4, { session_key: null }), ack],
    [abort, cancelled(4, {}, 1), ack],
    [abort, cancelled(4, { run_id: 'another' }), ack],
    [abort, terminal, ack, cancelled(6)],
    [abort, terminal, ack, event('stream.run_complete', conversation, 6)],
    [terminal, ack],
  ]) {
    assert.equal(cancelledChatTerminal(rows, run, abort), null, JSON.stringify(rows));
  }
});

test('TSX success requires the original hello socket, exact turn key and unique session acceptance', () => {
  assert.equal(handshake.socket, 0);
  const turn = voice(trace());
  assert.ok(turn && turn.terminal.at < turn.reply.at && turn.terminal.session === undefined);
  for (const options of [{ sttSocket: 1 }, { chatSocket: 1 }, { synthSocket: 1 }, { idempotency: 'unrelated' },
    { acceptedSession: null }, { acceptedSession: 'other' }, { runOnly: false }]) {
    assert.equal(voice(trace(options)), null, JSON.stringify(options));
  }
  for (const session of ['s', 'other']) {
    const duplicate = trace();
    duplicate.rows.push(event('stream.run_accepted', { run_id: 'r', session_key: session }, 5.5));
    assert.equal(voice(duplicate), null);
  }
  for (const chatSocket of [0, 1]) {
    const { rows } = trace({ chatSocket });
    const ordinary = rows.slice(2, 6);
    ordinary[0] = req(3, 'chat.send', { message: 'Ordinary text.', idempotency_key: 'ordinary' }, 4, chatSocket);
    assert.equal(Boolean(completedChat(ordinary, handshake)), chatSocket === 0);
  }
});
