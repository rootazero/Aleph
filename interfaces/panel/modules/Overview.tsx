// Gateway health for the current connection; asks again whenever the
// connection reports a change while this module is mounted.
import { signal, Text, View } from "nsh";
import type { PanelSdk } from "../shell/sdk";

type Health = { status?: string; timestamp?: string };

export function Overview(props: { sdk: PanelSdk }) {
  const { sdk } = props;
  const health = signal<Health | null>(null);
  const error = signal<string | null>(null);

  const refresh = () =>
    sdk.invoke("health").then(
      (result) => health.set(result as Health),
      (failure: Error) => {
        if (failure.message !== "disposed") error.set(failure.message);
      },
    );
  refresh();
  sdk.subscribe((state) => {
    if (state.state === "connected") refresh();
  });

  return (
    <View class="gap-2 p-4">
      <Text class="text-foreground">Gateway: {health()?.status ?? (error() ? "unreachable" : "checking…")}</Text>
      <Text class="text-muted-foreground">{sdk.connection().endpoint}</Text>
      <If when={error()}>
        <Text class="text-destructive">{error()}</Text>
      </If>
    </View>
  );
}
