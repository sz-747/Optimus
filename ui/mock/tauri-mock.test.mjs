// Self-check for the mock's split-tree math (the only non-trivial logic here). Run: node this file.
// Guards that spawn/split/close reshape the tree and that rects mirror the backend's even splits.
import assert from "node:assert/strict";
import { core } from "./tauri-mock.js";

const { invoke } = core;
const noArgs = { onOutput: { onmessage: null } };

// spawn → one full-bounds pane
let r = await invoke("spawn", noArgs);
let t = r.tree;
assert.equal(t.panes.length, 1, "spawn yields one pane");
assert.deepEqual(t.panes[0].rect, { x: 0, y: 0, width: 1, height: 1 }, "single pane fills bounds");
assert.equal(t.dividers.length, 0, "no dividers with one pane");

// split right → two side-by-side halves + one vertical divider, new pane focused
r = await invoke("split", { direction: "right", ...noArgs });
t = r.tree;
assert.equal(t.panes.length, 2, "split adds a pane");
const left = t.panes.find((p) => p.rect.x === 0);
const right = t.panes.find((p) => p.rect.x === 0.5);
assert.deepEqual(left.rect, { x: 0, y: 0, width: 0.5, height: 1 }, "left half");
assert.deepEqual(right.rect, { x: 0.5, y: 0, width: 0.5, height: 1 }, "right half");
assert.equal(t.dividers.length, 1, "one divider");
assert.equal(t.dividers[0].orientation, "vertical", "vertical divider");
assert.equal(t.focused_pane, right.pane_id, "new pane is focused");

// split down on the focused (right) pane → it halves vertically; left pane untouched
r = await invoke("split", { direction: "down", ...noArgs });
t = r.tree;
assert.equal(t.panes.length, 3, "second split adds a third pane");
assert.ok(t.panes.some((p) => p.rect.x === 0 && p.rect.width === 0.5 && p.rect.height === 1), "left pane unchanged");
assert.ok(t.panes.some((p) => p.rect.x === 0.5 && Math.abs(p.rect.height - 0.5) < 1e-9), "right column split in half");
assert.equal(t.dividers.length, 2, "two dividers");

// capacity tracks live pane count
const cap = await invoke("capacity_state");
assert.equal(cap.used, 3, "capacity counts panes");
assert.equal(cap.label, "3 / 8 terminals", "capacity label");

// close focused pane → back to two, sibling reclaims the space
r = await invoke("close_focused");
assert.equal(r.closed.length, 1, "close reports the freed surface");
assert.equal(r.reseeded, false, "not reseeded while other panes remain");
assert.equal(r.tree.panes.length, 2, "close drops a pane");

// close down to the last pane → reseeds a fresh one (never-contentless invariant)
r = await invoke("close_focused");
r = await invoke("close_focused");
assert.equal(r.reseeded, true, "closing the last pane reseeds");
assert.equal(r.tree.panes.length, 1, "a fresh pane replaces the emptied tree");

console.log("tauri-mock self-check OK");
