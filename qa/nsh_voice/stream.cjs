// Deterministic loopback Deepgram dialect for the real Gateway streaming adapter.
const { WebSocketServer } = require('ws');
const { writeFileSync } = require('node:fs');

const [ready, mode] = process.argv.slice(2);
const server = new WebSocketServer({ host: '127.0.0.1', port: 0, path: '/v1/listen', maxPayload: 65536 });
server.on('listening', () => writeFileSync(ready, String(server.address().port), { flag: 'wx' }));
server.on('connection', (socket, request) => {
  const query = new URL(request.url, 'http://127.0.0.1').searchParams;
  if (query.get('sample_rate') !== '16000' || query.get('encoding') !== 'linear16') {
    socket.close(1008);
    return;
  }
  let sent = false;
  socket.on('error', () => socket.terminate());
  socket.on('message', (pcm, binary) => {
    if (!binary || sent || !pcm.some((byte) => byte !== 0)) return;
    sent = true;
    const transcript = mode === 'stream-empty' ? '' : 'Fixture streaming input.';
    socket.send(JSON.stringify({ type: 'Results', channel: { alternatives: [{ transcript }] }, is_final: true }));
  });
});
