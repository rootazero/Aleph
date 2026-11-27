import { cleanup, host } from "nsh";
import { BrowserCaptureVad } from "aleph/panel";
import type { PanelSdk } from "../shell/sdk";
import type { VoiceAudio, VoiceCaptureResult } from "./voiceTurn";

export type VoiceCaptureTransport = "desktop" | "web";

// The NSH transport must not expire before the hosts' bounded media leases.
export const VOICE_CAPTURE_HOST_TIMEOUT_MS = 55_000;
const VOICE_PLAYBACK_HOST_TIMEOUT_MS = 125_000;

type VadRequest = { id: string; seq: number } & (
  | { op: "probe" | "finish" }
  | { op: "observe"; end_sample: number; rms: number }
);

/** Bind platform I/O to the mounted surface; only the controller starts a capture. */
export function bindVoiceHostAudio(sdk: PanelSdk, transport: () => VoiceCaptureTransport | null): VoiceAudio {
  let browserCapture: { id: string; onReady: () => void; vad: BrowserCaptureVad } | undefined;
  cleanup(host.on<{ id: string }>("aleph.voice.ready", ({ id }) => {
    if (browserCapture?.id === id) browserCapture.onReady();
  }));
  cleanup(host.on<VadRequest>("aleph.voice.vad", (request) => {
    const capture = browserCapture;
    if (capture?.id !== request.id) return;
    const value = request.op === "probe" ? BrowserCaptureVad.config()
      : request.op === "observe" ? capture.vad.observe(request.end_sample, request.rms)
        : capture.vad.finish();
    void host.call("aleph.voice.vadReply", { id: request.id, seq: request.seq, value }).catch(() => {
      // The browser host owns the timeout and closes an unanswered capture.
    });
  }));

  return {
    async capture(id, onReady) {
      const source = transport();
      if (source === "web") {
        const capture = { id, onReady, vad: new BrowserCaptureVad() };
        browserCapture = capture;
        try {
          return (await host.call("aleph.voice.capture", { id }, { timeout: VOICE_CAPTURE_HOST_TIMEOUT_MS })) as VoiceCaptureResult;
        } finally {
          if (browserCapture === capture) browserCapture = undefined;
        }
      }
      if (source !== "desktop") throw new Error("Voice capture is unavailable");
      const params = { capture_id: id };
      const sealed = sdk.invokeVoice("voice.turn.capture", params) as Promise<VoiceCaptureResult>;
      let pending = true;
      const ready = sdk.invokeVoice("voice.turn.capture_ready", params).then((reply) => {
        if ((reply as { ready?: boolean } | null)?.ready !== true) throw new Error("Voice capture was not ready");
        if (pending) onReady();
        return sealed;
      });
      try {
        return await Promise.race([sealed, ready]);
      } finally {
        pending = false;
      }
    },
    async finish(id) {
      if (transport() === "web") await host.call("aleph.voice.finish", { id });
      else await sdk.invokeVoice("voice.turn.capture_finish", { capture_id: id });
    },
    async cancel(id) {
      if (transport() === "web") {
        if (browserCapture?.id === id) browserCapture = undefined;
        await host.call("aleph.voice.cancel", { id });
      } else {
        // SDK invalidation notifies the controller synchronously. Submit using its
        // original connection binding before yielding; IPC EOF owns lost connections.
        await sdk.invokeVoice("voice.turn.capture_cancel", { capture_id: id });
      }
    },
    async play(id, result) {
      await host.call("aleph.voice.play", { playback_id: id, ...result }, { timeout: VOICE_PLAYBACK_HOST_TIMEOUT_MS });
    },
    async stop(id) {
      await host.call("aleph.voice.stop", { playback_id: id });
    },
  };
}
