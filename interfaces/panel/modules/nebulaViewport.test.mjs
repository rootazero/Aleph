import assert from "node:assert/strict";
import test from "node:test";
import { beginDrag, clipEdge, drawEdge, drawSegment, drawSlots, endDrag, fitViewport, inverseProject, moveDrag, pickOnRelease, pickedNode, project, zoomAt } from "./nebulaViewport.ts";

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

test("inverse projection preserves the pointer anchor through zoom", () => {
  const before = { scale: 2, x: -40, y: 30 };
  const anchor = [121, 88];
  const world = inverseProject(anchor, before);
  const after = zoomAt(before, anchor, 1.75);
  const projected = [after.x + world[0] * after.scale, after.y + world[1] * after.scale];
  assert.ok(Math.abs(projected[0] - anchor[0]) < 1e-9);
  assert.ok(Math.abs(projected[1] - anchor[1]) < 1e-9);
});

test("clipped edges retain crossings through the viewport", () => {
  const clipped = clipEdge([[-30, 50], [130, 50]], edge(0, 1), 100, 100);
  assert.deepEqual(clipped, [[12, 50], [88, 50]]);
  assert.equal(clipEdge([[-30, -30], [-10, -10]], edge(0, 1), 100, 100), null);
});

test("drag release never picks even when it ends over a node", () => {
  let drag = beginDrag(10, 10);
  const moved = moveDrag(drag, 40, 10);
  drag = moved.drag;
  const release = endDrag(drag, 50, 10);
  assert.equal(release.dragged, true);
  assert.equal(pickedNode([[50, 10]], ...release.point), 0);
  assert.equal(pickOnRelease(drag, [[50, 10]], ...release.point), -1);
  assert.equal(endDrag(beginDrag(10, 10), 12, 11).dragged, false);
});

test("drawSegment renders clipped endpoints instead of stale graph indices", () => {
  const drawn = [];
  const pen = { beginPath: () => {}, moveTo: (...point) => drawn.push(point), lineTo: (...point) => drawn.push(point), stroke: () => {} };
  drawSegment(pen, [12, 50], [88, 50], null, false);
  assert.deepEqual(drawn[0], [12, 52]);
});

test("picking uses the projected positions at every supported zoom", () => {
  const points = [[-60, 20], [10, -25], [50, 30]];
  for (const zoom of [0.5, 1, 1.25, 4]) {
    const drawn = project(points, 360, 190, zoom);
    for (const [index, [x, y]] of drawn.entries()) {
      if (x >= -18 && x <= 378 && y >= -18 && y <= 208) assert.equal(pickedNode(drawn, x, y), index);
    }
  }
  assert.equal(pickedNode([], 180, 95), -1);
});
