// Classic `/ws` has text JSON-RPC and a credential-less loopback owner connect.
const { requests, replies, events } = require('./wire.cjs');

const BATCH_STT = 'voice.transcribe';
const TAIL_TTS = 'voice.synthesize';
const STREAM_PROBE = 'voice.stream.start';

function operatorConnect(rows) {
  const [connect, extra] = requests(rows, 'connect');
  const [reply, duplicate] = replies(rows, connect);
  return !extra && !duplicate && !connect?.credentialPresent && !reply?.failed && reply?.loopbackOperator ? reply : null;
}

function classicTurn(rows, operator, replyFingerprint) {
  const single = (method) => {
    const [req, extra] = requests(rows, method);
    const [reply, duplicate] = replies(rows, req);
    return !extra && !duplicate && req?.socket === operator.socket && reply && !reply.failed && { req, reply };
  };
  const stt = single(BATCH_STT), send = single('chat.send'), tts = single(TAIL_TTS), stream = single(STREAM_PROBE);
  if (!stt || !send || !tts || !stream || !stream.reply.streamingDisabled || stream.reply.at > stt.req.at ||
      !stt.req.inputPcmBytes || !stt.reply.text || !send.req.voice || send.req.text !== stt.reply.text ||
      send.req.at < stt.reply.at || !send.reply.session || !send.reply.run ||
      rows.some((row) => row.topic !== STREAM_PROBE && /^(voice\.(turn|stream)\.|voice\.format$|voice\.transcribe\.delta$)/.test(row.topic ?? ''))) return null;
  const [accepted, duplicateAccepted] = events(rows, 'stream.run_accepted', send.reply);
  const [terminal, duplicateTerminal] = events(rows, 'stream.run_complete', send.reply);
  if (!accepted || duplicateAccepted || !terminal?.final || duplicateTerminal ||
      accepted.session !== send.reply.session ||
      accepted.at < send.req.at || terminal.at < accepted.at ||
      events(rows, 'stream.run_error', send.reply).length || tts.req.at < Math.max(terminal.at, send.reply.at) ||
      tts.req.text !== terminal.final || !tts.reply.outputAudioBytes || tts.reply.outputAudioFingerprint !== replyFingerprint) return null;
  return { stt, send, accepted, terminal, tts };
}

module.exports = { classicTurn, operatorConnect };
