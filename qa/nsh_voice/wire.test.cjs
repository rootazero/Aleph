const test = require('node:test');
const assert = require('node:assert/strict');
const { decode, requests, replies, events, traffic, stats, MAX_PCM, TRANSCRIBE } = require('./wire.cjs');

function packet(body) {
  const payload = Buffer.from(JSON.stringify(body));
  const header = Buffer.alloc(4);
  header.writeUInt32BE(payload.length);
  return Buffer.concat([header, payload]);
}

function wav(bytes) {
  const result = Buffer.alloc(44 + bytes);
  result.write('RIFF');
  result.writeUInt32LE(36 + bytes, 4);
  result.write('WAVEfmt ', 8);
  result.writeUInt32LE(16, 16);
  result.writeUInt32LE(0x10001, 20);
  result.writeUInt32LE(16000, 24);
  result.writeUInt32LE(32000, 28);
  result.writeUInt32LE(0x100002, 32);
  result.write('data', 36);
  result.writeUInt32LE(bytes, 40);
  return result;
}

const sent = (value, sock = 0) => decode(packet(value), 'sent', sock, 10);
const received = (value, sock = 0) => decode(packet(value), 'received', sock, 20);

test('bounded Web v1 frames report exact canonical PCM size without emitting audio', () => {
  for (const size of [MAX_PCM - 2, MAX_PCM, MAX_PCM + 2]) {
    const row = sent({ method: TRANSCRIBE, params: { audio_base64: wav(size).toString('base64'), turn_id: 'owned' } });
    assert.equal(row.inputPcmBytes, size <= MAX_PCM ? size : null);
    assert.deepEqual(JSON.parse(JSON.stringify(traffic([row]))), {
      sent: { [TRANSCRIBE]: { count: 1, framed_bytes: row.bytes } }, received: {},
    });
  }
  const bad = wav(10);
  bad.writeUInt32LE(22050, 24);
  assert.equal(sent({ method: TRANSCRIBE, params: { audio_base64: bad.toString('base64') } }).inputPcmBytes, null);
  assert.equal(decode(packet({ id: 1 }).subarray(1), 'received', 0), null);
  assert.equal(decode('only binary is Web v1', 'received', 0), null);
});

test('exact connection/request/terminal keys allow an early result and exclude unrelated new generations', () => {
  const send = sent({ id: 'fixture-id', method: 'chat.send', params: { message: 'fixture secret' } });
  const ack = { id: 'fixture-id', result: { run_id: 'wanted-run', session_key: 'owned' } };
  const terminal = (run, text, session) => ({ method: 'stream.run_complete', params: { run_id: run, session_key: session, summary: { final_response: text } } });
  const early = received(terminal('wanted-run', 'a real final', 'owned'));
  const rows = [send, early, received(ack, 1), received(terminal('wanted-run', 'foreign generation', 'owned'), 1),
    received(terminal('wanted-run', 'wrong owner', 'foreign')), received(terminal('foreign-run', 'other run', 'owned')), received(ack)];
  const wireAck = replies(rows, requests(rows, 'chat.send')[0]);
  assert.equal(wireAck.length, 1);
  assert.deepEqual(events(rows, 'stream.run_complete', wireAck[0]), [early]);
  assert.equal(early.final, sent({ method: 'voice.turn.synthesize', params: { text: 'a real final' } }).text);
  const summary = JSON.stringify(traffic(rows));
  for (const secret of ['fixture secret', 'fixture-id', 'owned', 'wanted-run', 'a real final', early.final]) assert.ok(!summary.includes(secret));
});

test('p50 and p95 use nearest rank over finite matching samples and absent stages stay null', () => {
  assert.deepEqual(stats([6, 1, 4, 2, NaN, 3, 5]), { n: 6, p50_ms: 3, p95_ms: 6 });
  assert.deepEqual(stats([]), { n: 0, p50_ms: null, p95_ms: null });
});
