import { mount, signal, Text, View } from "nsh";
import { Badge, Counter, Format, Probe, Pulse, Tracked } from "aleph-s2-spike/native";

// Its count lives in Rust (`src/lib.rs`).
const counter = new Counter();
// Spike probes: a Rust object JS holds, and a JS timer reporting to Rust.
const tracked = new Tracked();
setInterval(() => Probe.tick(), 50);

function App() {
  const count = signal(counter.count);
  const fired = signal(false);
  Probe.report(`app-rendered:${tracked ? "tracked" : "none"}`);
  return (
    <View
      style={{
        flex: 1,
        paddingTop: "env(safe-area-inset-top)",
        paddingRight: "env(safe-area-inset-right)",
        paddingBottom: "env(safe-area-inset-bottom)",
        paddingLeft: "env(safe-area-inset-left)",
        alignItems: "center", justifyContent: "center", gap: 16,
      }}
    >
      <Text class="text-foreground" style={{ fontSize: 22 }}>{count() ? `Pressed ${Format.times(count())}` : "Not pressed yet"}</Text>
      <Badge label="Press me" onPress={() => count.set(counter.increment())} />
      <Pulse label="pulse-from-tsx" onFire={() => { fired.set(true); Probe.report("pulse-fired"); }} />
      <Text>{fired() ? "Pulse fired" : "Pulse waiting"}</Text>
    </View>
  );
}

mount(App);
