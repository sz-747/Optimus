import assert from "node:assert/strict";
import test from "node:test";

import { RedisUrlAdapter } from "../redis-url-adapter.js";

test("REDIS_URL adapter connects once and translates the atomic-store command surface", async () => {
  const calls = [];
  const transaction = {
    set: (...args) => calls.push(["multi.set", ...args]),
    del: (...args) => calls.push(["multi.del", ...args]),
    expire: (...args) => calls.push(["multi.expire", ...args]),
    zAdd: (...args) => calls.push(["multi.zAdd", ...args]),
    zRem: (...args) => calls.push(["multi.zRem", ...args]),
    exec: async () => ["OK"],
  };
  const client = {
    isReady: false,
    isOpen: false,
    on: (name) => calls.push(["on", name]),
    connect: async () => { calls.push(["connect"]); client.isReady = true; client.isOpen = true; },
    get: async (key) => { calls.push(["get", key]); return "value"; },
    set: async (...args) => { calls.push(["set", ...args]); return "OK"; },
    incr: async (key) => { calls.push(["incr", key]); return 1; },
    expire: async (...args) => { calls.push(["expire", ...args]); return 1; },
    del: async (...args) => { calls.push(["del", ...args]); return 1; },
    zAdd: async (...args) => { calls.push(["zAdd", ...args]); return 1; },
    zRange: async (...args) => { calls.push(["zRange", ...args]); return ["one"]; },
    zRem: async (...args) => { calls.push(["zRem", ...args]); return 1; },
    eval: async (...args) => { calls.push(["eval", ...args]); return [1, 2]; },
    multi: () => transaction,
  };
  const adapter = new RedisUrlAdapter("redis://SECRET_MUST_NOT_BE_USED", client);
  assert.equal(await adapter.get("key"), "value");
  await adapter.set("key", "value", { nx: true, ex: 30 });
  assert.deepEqual(await adapter.eval("return 1", ["key"], [2]), [1, 2]);
  await adapter.zadd("queue", { score: 1, member: "command" });
  const multi = adapter.multi();
  multi.set("one", "two", { ex: 10 });
  multi.zrem("queue", "command");
  await multi.exec();

  assert.equal(calls.filter(([name]) => name === "connect").length, 1);
  assert.ok(calls.some((call) => call[0] === "set" && call[3].NX === true && call[3].EX === 30));
  assert.ok(calls.some((call) => call[0] === "eval" && call[2].keys[0] === "key" && call[2].arguments[0] === "2"));
  assert.ok(calls.some((call) => call[0] === "zAdd" && call[2][0].value === "command"));
  assert.ok(calls.some((call) => call[0] === "multi.set" && call[3].EX === 10));
  assert.ok(calls.some((call) => call[0] === "multi.zRem" && call[2][0] === "command"));
  assert.doesNotMatch(JSON.stringify(calls), /SECRET_MUST_NOT_BE_USED/);
});
