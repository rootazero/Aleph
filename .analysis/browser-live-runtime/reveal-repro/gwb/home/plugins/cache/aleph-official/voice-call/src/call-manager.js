// call-manager.js — Call state machine and session management
//
// This module provides the core call management logic. Actual WebRTC/SIP
// integration would replace the stub sections marked with TODO comments.

const crypto = require("crypto");

// ---------------------------------------------------------------------------
// Call state enum
// ---------------------------------------------------------------------------

const CallState = Object.freeze({
  IDLE: "idle",
  RINGING: "ringing",
  CONNECTED: "connected",
  ON_HOLD: "on_hold",
  ENDED: "ended",
});

// Valid state transitions
const VALID_TRANSITIONS = {
  [CallState.IDLE]: [CallState.RINGING],
  [CallState.RINGING]: [CallState.CONNECTED, CallState.ENDED],
  [CallState.CONNECTED]: [CallState.ON_HOLD, CallState.ENDED],
  [CallState.ON_HOLD]: [CallState.CONNECTED, CallState.ENDED],
  [CallState.ENDED]: [],
};

// ---------------------------------------------------------------------------
// CallSession
// ---------------------------------------------------------------------------

class CallSession {
  constructor(id, target, options = {}) {
    this.id = id;
    this.target = target;
    this.callerId = options.callerId || null;
    this.record = options.record || false;
    this.state = CallState.IDLE;
    this.direction = options.direction || "outbound";
    this.startTime = null;
    this.endTime = null;
    this.connectTime = null;
    this.transcriptionBuffer = [];
  }

  /**
   * Transition to a new state with validation.
   * Throws if the transition is not allowed.
   */
  transitionTo(newState) {
    const allowed = VALID_TRANSITIONS[this.state] || [];
    if (!allowed.includes(newState)) {
      throw new Error(
        `Invalid state transition: ${this.state} -> ${newState} (call ${this.id})`,
      );
    }

    this.state = newState;

    if (newState === CallState.RINGING) {
      this.startTime = Date.now();
    } else if (newState === CallState.CONNECTED) {
      this.connectTime = Date.now();
    } else if (newState === CallState.ENDED) {
      this.endTime = Date.now();
    }
  }

  /**
   * Append a transcription segment to the buffer.
   */
  appendTranscription(text, timestamp) {
    this.transcriptionBuffer.push({
      text,
      timestamp: timestamp || Date.now(),
    });
  }

  /**
   * Return a JSON-serializable summary of this session.
   */
  toJSON() {
    return {
      id: this.id,
      target: this.target,
      caller_id: this.callerId,
      direction: this.direction,
      state: this.state,
      record: this.record,
      start_time: this.startTime,
      connect_time: this.connectTime,
      end_time: this.endTime,
      duration_sec: this._durationSec(),
      transcription_segments: this.transcriptionBuffer.length,
    };
  }

  _durationSec() {
    if (!this.connectTime) return null;
    const end = this.endTime || Date.now();
    return Math.round((end - this.connectTime) / 1000);
  }
}

// ---------------------------------------------------------------------------
// CallManager
// ---------------------------------------------------------------------------

class CallManager {
  constructor() {
    this.sessions = new Map();
    this._running = false;
  }

  /**
   * Start the call manager service.
   */
  start() {
    // TODO: Initialize WebRTC peer connection factory
    // TODO: Connect to SIP registrar if sip_server is configured
    // TODO: Set up STUN/TURN ICE candidates from stun_servers config
    this._running = true;
    return { status: "started" };
  }

  /**
   * Stop the call manager service and hang up all active calls.
   */
  stop() {
    // Hang up any active calls
    for (const session of this.sessions.values()) {
      if (session.state !== CallState.ENDED) {
        try {
          session.transitionTo(CallState.ENDED);
        } catch {
          // Already ended, ignore
        }
      }
    }

    // TODO: Close WebRTC peer connections
    // TODO: Unregister from SIP registrar
    // TODO: Release audio device handles

    this._running = false;
    return { status: "stopped", sessions_ended: this.sessions.size };
  }

  /**
   * Generate a unique call ID.
   */
  generateId() {
    return `call-${crypto.randomUUID()}`;
  }

  /**
   * Initiate an outbound call.
   */
  initiate(target, options = {}) {
    if (!this._running) {
      throw new Error("CallManager is not running. Start the call-manager service first.");
    }

    if (!target) {
      throw new Error("target is required");
    }

    const id = this.generateId();
    const session = new CallSession(id, target, {
      callerId: options.caller_id,
      record: options.record || false,
      direction: "outbound",
    });

    // Transition: idle -> ringing
    session.transitionTo(CallState.RINGING);

    // TODO: Create WebRTC offer or send SIP INVITE
    // TODO: Set up local audio stream capture
    // TODO: If record=true, start recording pipeline

    this.sessions.set(id, session);

    // Simulate the call connecting after a brief period
    // In a real implementation, this would happen via WebRTC/SIP signaling
    // TODO: Replace with actual signaling callback
    setTimeout(() => {
      if (session.state === CallState.RINGING) {
        try {
          session.transitionTo(CallState.CONNECTED);
        } catch {
          // May have been hung up during ringing
        }
      }
    }, 100);

    return session.toJSON();
  }

  /**
   * Answer an incoming call.
   */
  answer(callId, options = {}) {
    if (!this._running) {
      throw new Error("CallManager is not running. Start the call-manager service first.");
    }

    const session = this._getSession(callId);

    if (session.state !== CallState.RINGING) {
      throw new Error(
        `Cannot answer call ${callId}: current state is ${session.state}, expected ringing`,
      );
    }

    if (options.record) {
      session.record = true;
    }

    // Transition: ringing -> connected
    session.transitionTo(CallState.CONNECTED);

    // TODO: Send SIP 200 OK / WebRTC answer SDP
    // TODO: Start audio stream playback
    // TODO: If record=true, start recording pipeline

    return session.toJSON();
  }

  /**
   * Hang up a call.
   */
  hangup(callId) {
    const session = this._getSession(callId);

    if (session.state === CallState.ENDED) {
      throw new Error(`Call ${callId} has already ended`);
    }

    // Transition to ended from any active state
    session.transitionTo(CallState.ENDED);

    // TODO: Send SIP BYE / close WebRTC peer connection
    // TODO: Stop audio streams
    // TODO: Finalize recording if active
    // TODO: Flush transcription buffer

    return session.toJSON();
  }

  /**
   * Get status of a specific call or all calls.
   */
  getStatus(callId) {
    if (callId) {
      const session = this._getSession(callId);
      return session.toJSON();
    }

    // Return all sessions
    const all = [];
    for (const session of this.sessions.values()) {
      all.push(session.toJSON());
    }
    return { calls: all, total: all.length };
  }

  /**
   * Get transcription for a call.
   */
  getTranscription(callId, language) {
    const session = this._getSession(callId);

    // TODO: If call is active, pull latest segments from TranscriptionStream
    // TODO: Apply language detection or forced language from `language` param
    // TODO: Integrate with speech-to-text service (Whisper, Google STT, etc.)

    if (session.transcriptionBuffer.length === 0) {
      return {
        call_id: callId,
        language: language || "auto",
        status: "stub",
        text: "[Transcription not available — speech-to-text integration required]",
        segments: [],
        note: "This is a stub. Integrate a speech-to-text service for real transcription.",
      };
    }

    const fullText = session.transcriptionBuffer.map((s) => s.text).join(" ");
    return {
      call_id: callId,
      language: language || "auto",
      status: "complete",
      text: fullText,
      segments: session.transcriptionBuffer,
    };
  }

  /**
   * Look up a session by ID, throwing if not found.
   */
  _getSession(callId) {
    const session = this.sessions.get(callId);
    if (!session) {
      throw new Error(`Call not found: ${callId}`);
    }
    return session;
  }
}

module.exports = { CallState, CallSession, CallManager };
