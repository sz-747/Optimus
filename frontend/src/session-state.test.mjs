import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { addSession, createSessionState, removeSession, selectSession } from "./session-state.js";

test("a fresh workspace has no selected terminal", () => {
  assert.deepEqual(createSessionState(), { sessions: [], selectedId: null });
});

test("creating a terminal selects it", () => {
  const state = addSession(createSessionState(), { id: 7, title: "Terminal 7" });
  assert.equal(state.selectedId, 7);
  assert.equal(state.sessions.length, 1);
});

test("closing the selected terminal selects its next neighbour", () => {
  const initial = {
    sessions: [
      { id: 1, title: "Terminal 1" },
      { id: 2, title: "Terminal 2" },
      { id: 3, title: "Terminal 3" },
    ],
    selectedId: 2,
  };
  const state = removeSession(initial, 2);
  assert.equal(state.selectedId, 3);
  assert.deepEqual(state.sessions.map((session) => session.id), [1, 3]);
});

test("closing the final terminal restores the empty state", () => {
  const state = removeSession(
    selectSession(createSessionState([{ id: 1, title: "Terminal 1" }]), 1),
    1,
  );
  assert.deepEqual(state, { sessions: [], selectedId: null });
});

test("the desktop interface uses Tauri's supported module API", async () => {
  const source = await readFile(fileURLToPath(new URL("./main.js", import.meta.url)), "utf8");
  assert.doesNotMatch(source, /window\.__TAURI__/);
  assert.match(source, /@tauri-apps\/api\/core/);
});

test("terminal children launch at lower priority without blocking the interface", async () => {
  const source = await readFile(
    fileURLToPath(new URL("../../shell/src/main.rs", import.meta.url)),
    "utf8",
  );

  assert.match(source, /BELOW_NORMAL_PRIORITY_CLASS/);
  assert.match(source, /\.spawn\(\)/);
});
