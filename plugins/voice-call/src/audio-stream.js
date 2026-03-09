// audio-stream.js — Audio stream abstraction (stub)
//
// Provides AudioStream and TranscriptionStream classes as integration points
// for real WebRTC audio handling and speech-to-text. All methods are stubs
// returning placeholder data.

// ---------------------------------------------------------------------------
// AudioStream
// ---------------------------------------------------------------------------

class AudioStream {
  constructor(options = {}) {
    this.sampleRate = options.sampleRate || 48000;
    this.channels = options.channels || 1;
    this.codec = options.codec || "opus";
    this._active = false;
    this._pipedTo = [];
  }

  /**
   * Start capturing or receiving audio.
   *
   * TODO: Initialize microphone capture via system audio API
   * TODO: Or accept incoming RTP/WebRTC audio track
   */
  start() {
    this._active = true;
    return {
      status: "stub",
      message: "Audio stream started (no real capture — stub implementation)",
      config: {
        sample_rate: this.sampleRate,
        channels: this.channels,
        codec: this.codec,
      },
    };
  }

  /**
   * Stop the audio stream.
   *
   * TODO: Release microphone / close RTP receiver
   * TODO: Flush any buffered audio data
   */
  stop() {
    this._active = false;
    this._pipedTo = [];
    return {
      status: "stub",
      message: "Audio stream stopped",
    };
  }

  /**
   * Pipe audio data to another stream (e.g. TranscriptionStream, recording).
   *
   * TODO: Set up audio data forwarding pipeline
   * TODO: Handle backpressure if consumer is slow
   */
  pipe(destination) {
    this._pipedTo.push(destination);
    return {
      status: "stub",
      message: `Audio piped to ${destination.constructor.name}`,
    };
  }

  /**
   * Check if the stream is currently active.
   */
  isActive() {
    return this._active;
  }
}

// ---------------------------------------------------------------------------
// TranscriptionStream
// ---------------------------------------------------------------------------

class TranscriptionStream {
  constructor(options = {}) {
    this.language = options.language || "auto";
    this.model = options.model || "default";
    this._active = false;
    this._segments = [];
    this._onSegment = options.onSegment || null;
  }

  /**
   * Start the transcription pipeline.
   *
   * TODO: Initialize speech-to-text engine (e.g. Whisper, Google STT, Azure)
   * TODO: Set up streaming recognition session
   * TODO: Configure language and model parameters
   */
  start() {
    this._active = true;
    return {
      status: "stub",
      message: "Transcription stream started (no real STT — stub implementation)",
      config: {
        language: this.language,
        model: this.model,
      },
    };
  }

  /**
   * Stop the transcription pipeline.
   *
   * TODO: Close STT session
   * TODO: Flush any pending audio buffers for final transcription
   */
  stop() {
    this._active = false;
    return {
      status: "stub",
      message: "Transcription stream stopped",
      segments_captured: this._segments.length,
    };
  }

  /**
   * Receive audio data for transcription.
   *
   * TODO: Forward audio buffer to STT engine
   * TODO: Emit recognized segments via onSegment callback
   *
   * @param {Buffer} audioData - Raw audio data chunk
   */
  write(audioData) {
    if (!this._active) {
      return { status: "error", message: "Transcription stream is not active" };
    }

    // Stub: In a real implementation, this would feed audio to the STT engine
    // and asynchronously emit transcription segments
    return {
      status: "stub",
      message: "Audio data received (not processed — stub implementation)",
      bytes: audioData ? audioData.length : 0,
    };
  }

  /**
   * Get all captured transcription segments.
   */
  getSegments() {
    return this._segments;
  }
}

module.exports = { AudioStream, TranscriptionStream };
