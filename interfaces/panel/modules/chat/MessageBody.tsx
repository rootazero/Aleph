import { Canvas, openUrl, Scroll, Text, View } from "nsh";
import { TextView, type TextViewCustomNode } from "nsh/base";

type Node = { label: string; x: number; y: number; width: number; height: number };
type Scene = { width: number; height: number; left_to_right: boolean; nodes: Node[]; edges: { from: number; to: number }[] };
export type Diagram = { scene: Scene } | { reason: string };

const MERMAID = [{ name: "mermaid", language: "mermaid" }, { name: "mermaid", language: "mmd" }];

function Flowchart(props: { scene: Scene }) {
  const scene = props.scene;
  return (
    <Scroll horizontal class="rounded bg-muted">
      <Canvas style={{ width: scene.width, height: scene.height }}
        accessibilityLabel={`Mermaid flowchart: ${scene.nodes.map((node) => node.label).join(", ")}`}
        draw={(cx) => {
          cx.fillStyle = "#f4f6fb";
          cx.fillRect(0, 0, scene.width, scene.height);
          cx.strokeStyle = "#455775";
          cx.lineWidth = 2;
          for (const edge of scene.edges) {
            const from = scene.nodes[edge.from];
            const to = scene.nodes[edge.to];
            const horizontal = scene.left_to_right;
            const x1 = from.x + (horizontal ? from.width : from.width / 2);
            const y1 = from.y + (horizontal ? from.height / 2 : from.height);
            const x2 = to.x + (horizontal ? 0 : to.width / 2);
            const y2 = to.y + (horizontal ? to.height / 2 : 0);
            const angle = Math.atan2(y2 - y1, x2 - x1);
            cx.beginPath();
            cx.moveTo(x1, y1);
            cx.lineTo(x2, y2);
            cx.moveTo(x2 - 8 * Math.cos(angle - 0.5), y2 - 8 * Math.sin(angle - 0.5));
            cx.lineTo(x2, y2);
            cx.lineTo(x2 - 8 * Math.cos(angle + 0.5), y2 - 8 * Math.sin(angle + 0.5));
            cx.stroke();
          }
          cx.font = "14px sans-serif";
          cx.textAlign = "center";
          cx.textBaseline = "middle";
          for (const node of scene.nodes) {
            cx.fillStyle = "#ffffff";
            cx.fillRect(node.x, node.y, node.width, node.height);
            cx.strokeRect(node.x, node.y, node.width, node.height);
            cx.fillStyle = "#122342";
            cx.fillText(node.label, node.x + node.width / 2, node.y + node.height / 2, node.width - 12);
          }
        }} />
    </Scroll>
  );
}

export function MessageBody(props: { text: string; streaming: boolean; layout: (source: string) => Diagram }) {
  const renderDiagram = (node: TextViewCustomNode) => {
    const source = node.text ?? "";
    const result = props.layout(source);
    if ("scene" in result) return <Flowchart scene={result.scene} />;
    return (
      <View class="gap-1 rounded bg-muted p-2" selectable>
        <Text class="text-muted-foreground">Mermaid source · {result.reason}</Text>
        <Text class="font-mono text-sm">{source}</Text>
      </View>
    );
  };
  return (
    <TextView markdown={props.text} options={{ html: false, mdx: false, alerts: true, streaming: props.streaming }}
      extensions={props.streaming ? [] : MERMAID} renderCustom={renderDiagram} selectable
      renderImage={({ alt }) => <Text class="text-muted-foreground">{alt || "Image not loaded"}</Text>}
      onLinkPress={({ url }) => {
        if (/^https?:\/\/[^\s\\/]/i.test(url)) void openUrl(url).catch(() => undefined);
      }} />
  );
}
