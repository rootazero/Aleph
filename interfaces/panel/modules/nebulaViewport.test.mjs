import assert from "node:assert/strict";
import test from "node:test";
import { drawEdge, drawSlots, pickedNode, project } from "./nebulaViewport.ts";

const edge = (from, to, kind = null) => ({ from, to, kind });

test("reciprocal incident rows take the drawing budget before background rows", () => {
  const edges = [edge(0, 1), edge(1, 0), edge(2, 3), edge(3, 4), edge(4, 3)];
  assert.deepEqual(drawSlots(edges, 4, 3), [edges[0], edges[3], edges[4]]);
});

test("similarity is drawn without an arrow and with a distinct color", () => {
  const drawn = [];
  const pen = {
    beginPath: () => {},
    moveTo: () => {},
    lineTo: (...point) => drawn.push(point),
    stroke: () => {},
  };
  const positions = [[10, 10], [100, 10]];
  drawEdge(pen, positions, edge(0, 1, "related_similarity"), 0);
  assert.deepEqual(drawn, [[100, 10]]);
  const similarityColor = pen.strokeStyle;
  drawn.length = 0;
  drawEdge(pen, positions, edge(0, 1, "refers"), 0);
  assert.equal(drawn.length, 3);
  assert.notEqual(pen.strokeStyle, similarityColor);
});

test("picking uses the projected positions at every supported zoom", () => {
  const points = [[-60, 20], [10, -25], [50, 30]];
  for (const zoom of [0.5, 1, 1.25, 4]) {
    const drawn = project(points, 360, 190, zoom);
    for (const [index, [x, y]] of drawn.entries()) {
      assert.equal(pickedNode(drawn, x, y), index);
    }
  }
  assert.equal(pickedNode([], 180, 95), -1);
});
