import { cleanup, host, signal, Text, View } from "nsh";
import { Button } from "nsh/base";
import type { PanelSdk } from "../shell/sdk";
import { bindVoiceHostAudio, type VoiceCaptureTransport } from "./voiceHostAudio";
import { createVoiceTurn, type VoiceTurnStatus } from "./voiceTurn";

const PHASE_LABEL = {
  idle: "Ready to start",
  opening: "Waiting for microphone",
  listening: "Listening",
  finishing: "Finishing capture",
  thinking: "Waiting for reply",
  playing: "Reply playback",
  stopping: "Stopping voice turn",
} as const;

const ERROR_LABEL: Record<Extract<VoiceTurnStatus, { phase: "error" }>["reason"], string> = {
  audio: "Microphone capture failed",
  transcription: "Transcription failed",
  send: "Could not send voice turn",
  reply: "No usable reply",
  synthesis: "Speech synthesis failed",
  playback: "Reply playback failed",
  connection: "Voice turn stopped: connection lost",
};

export function Voice(props: { sdk: PanelSdk }) {
  const availability = signal<"checking" | "unavailable" | VoiceCaptureTransport>("checking");
  const transport = () => {
    const state = availability();
    return state === "desktop" || state === "web" ? state : null;
  };
  const sdk = props.sdk;
  const audio = bindVoiceHostAudio(sdk, transport);
  const turn = createVoiceTurn(sdk, audio);
  const status = signal(turn.status());
  const visible = signal(true);
  let mounted = true;

  const release = turn.subscribe(status.set);
  const unsubscribeVisibility = host.on<{ visible: boolean }>("aleph.voice.visibility", (event) => {
    if (typeof event?.visible !== "boolean") return;
    visible.set(event.visible);
    if (!event.visible) turn.cancel();
  });
  cleanup(() => {
    mounted = false;
    unsubscribeVisibility();
    release();
    turn.dispose();
  });
  void host.call("aleph.voice.availability", null).then((result) => {
    const found = result as { capture?: boolean; playback?: boolean; capture_transport?: VoiceCaptureTransport } | null;
    const source = found?.capture_transport;
    if (mounted) availability.set(found?.capture === true && found.playback === true && (source === "desktop" || source === "web")
      ? source : "unavailable");
  }, () => {
    if (mounted) availability.set("unavailable");
  });

  const canAct = () => visible() && transport() !== null;
  const inactive = () => status().phase === "idle" || status().phase === "error";
  const label = () => {
    const next = status();
    return next.phase === "error" ? ERROR_LABEL[next.reason] : PHASE_LABEL[next.phase];
  };

  return (
    <View class="flex-1 gap-4 p-6">
      <Text class="text-xl font-semibold text-foreground">Voice</Text>
      <If when={transport() === null}>
        <Text class="text-muted-foreground">
          {availability() === "checking" ? "Checking microphone and playback availability" : "Microphone and playback are not available in this host"}
        </Text>
      </If>
      <If when={transport() !== null}>
        <Text class="text-foreground">{label()}</Text>
      </If>
      <If when={status().limitReached}>
        <Text class="text-muted-foreground">Capture limit reached</Text>
      </If>
      <If when={canAct()}>
        <View class="flex-row gap-2">
          <If when={inactive()}>
            <Button
              class="rounded-(--radius) bg-primary px-4 py-2"
              accessibilityLabel="Start voice conversation"
              onPress={() => { if (canAct()) turn.start(); }}
            >
              <Text class="text-primary-foreground">Start</Text>
            </Button>
          </If>
          <If when={status().phase === "listening"}>
            <Button class="rounded-(--radius) border border-input px-4 py-2" accessibilityLabel="Finish speaking" onPress={() => turn.finish()}>
              <Text class="text-foreground">Finish speaking</Text>
            </Button>
          </If>
          <If when={status().phase === "playing"}>
            <Button
              class="rounded-(--radius) border border-input px-4 py-2"
              accessibilityLabel="Interrupt reply and listen again"
              onPress={() => { if (canAct()) turn.interrupt(); }}
            >
              <Text class="text-foreground">Interrupt</Text>
            </Button>
          </If>
          <If when={!inactive() && status().phase !== "stopping"}>
            <Button class="rounded-(--radius) border border-input px-4 py-2" accessibilityLabel="Stop voice conversation" onPress={() => turn.cancel()}>
              <Text class="text-foreground">Stop</Text>
            </Button>
          </If>
        </View>
      </If>
    </View>
  );
}
