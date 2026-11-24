// Shown when the Gateway refused the browser's session: exchange a one-time
// bootstrap ticket or a device token for a session (`aleph.pair`, which posts
// `/v1/session` same-origin and reconnects). The credential never leaves the
// host call; nothing here stores it.
import { host, signal, Text, View } from "nsh";
import { Button, Input } from "nsh/base";

type PairAnswer = { error?: { code: number; message: string } };

export function PairingForm() {
  const credential = signal("");
  const error = signal<string | null>(null);
  const busy = signal(false);

  const pair = async (kind: "bootstrap_ticket" | "device_token") => {
    busy.set(true);
    error.set(null);
    try {
      const answer = (await host.call("aleph.pair", { [kind]: credential().trim() })) as PairAnswer;
      if (answer.error) error.set(answer.error.message);
      else credential.set("");
    } catch (failure) {
      error.set((failure as Error).message);
    } finally {
      busy.set(false);
    }
  };

  const blank = () => busy() || !credential().trim();
  return (
    <View class="flex-1 items-center justify-center gap-3 p-6">
      <Text class="text-foreground">Pair this browser with Aleph</Text>
      <Input
        value={credential()}
        onChange={({ value }) => credential.set(value)}
        placeholder="Pairing ticket or device token"
        class="w-80 rounded-(--radius) border border-input px-3 py-2 text-foreground"
      />
      <View class="flex-row gap-2">
        <Button disabled={blank()} onPress={() => void pair("bootstrap_ticket")} class="rounded-(--radius) bg-primary px-4 py-2 disabled:opacity-40">
          <Text class="text-primary-foreground">Pair with ticket</Text>
        </Button>
        <Button disabled={blank()} onPress={() => void pair("device_token")} class="rounded-(--radius) border border-input px-4 py-2 disabled:opacity-40">
          <Text class="text-foreground">Use device token</Text>
        </Button>
      </View>
      <If when={error()}>
        <Text class="text-destructive">{error()}</Text>
      </If>
    </View>
  );
}
