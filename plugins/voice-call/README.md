# Voice Call Plugin

A voice calling framework plugin for Aleph, providing call management with WebRTC/SIP stubs.

## Overview

This plugin implements a call state machine and provides tools for initiating, answering, and managing voice calls. It is designed as a **framework** — the core call management logic is complete, but actual WebRTC/SIP media transport and speech-to-text integration require real implementations at the marked TODO points.

## Tools

| Tool | Description |
|------|-------------|
| `call_initiate` | Start an outbound call to a phone number or SIP URI |
| `call_answer` | Answer an incoming voice call |
| `call_hangup` | End an active or ringing call |
| `call_status` | Get status of a specific call or all active calls |
| `call_transcribe` | Get live or completed transcription for a call |

## Services

- **call-manager** — Must be started before using any call tools. Manages call sessions, audio streams, and state transitions.

## Call States

```
idle -> ringing -> connected -> ended
                -> on_hold   -> connected
                             -> ended
```

## Configuration

| Key | Type | Description |
|-----|------|-------------|
| `sip_server` | string | SIP server address |
| `stun_servers` | string[] | STUN/TURN server URLs for WebRTC ICE |
| `default_codec` | string | Audio codec (default: "opus") |

## Integration Points

To make this plugin functional with real voice calls, implement the TODO sections in:

1. **`src/call-manager.js`** — WebRTC peer connection setup, SIP INVITE/BYE signaling, recording pipeline
2. **`src/audio-stream.js`** — Microphone capture, RTP streaming, speech-to-text engine integration

Recommended libraries:
- WebRTC: `wrtc` (node-webrtc) or `mediasoup`
- SIP: `jssip` or `sipjs`
- Speech-to-text: OpenAI Whisper, Google Cloud STT, Azure Speech
