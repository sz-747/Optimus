import { createClient } from "redis";

export class RedisUrlAdapter {
  constructor(url, client = null) {
    this.client = client || createClient({
      url,
      socket: {
        connectTimeout: 3_000,
        reconnectStrategy: (retries) => Math.min(100 * 2 ** retries, 3_000),
      },
    });
    this.connecting = null;
    this.client.on("error", (error) => {
      console.error("Optimus Redis connection error:", error?.code || error?.name || "unknown");
    });
  }

  async get(key) {
    return (await this.#ready()).get(key);
  }

  async set(key, value, options = {}) {
    return (await this.#ready()).set(key, value, nodeOptions(options));
  }

  async incr(key) {
    return (await this.#ready()).incr(key);
  }

  async expire(key, seconds) {
    return (await this.#ready()).expire(key, seconds);
  }

  async del(...keys) {
    return (await this.#ready()).del(keys.flat());
  }

  async zadd(key, entry) {
    return (await this.#ready()).zAdd(key, [{ score: entry.score, value: String(entry.member) }]);
  }

  async zrange(key, start, stop) {
    return (await this.#ready()).zRange(key, start, stop);
  }

  async zrem(key, ...members) {
    return (await this.#ready()).zRem(key, members.flat().map(String));
  }

  async eval(script, keys, args) {
    return (await this.#ready()).eval(script, { keys, arguments: args.map(String) });
  }

  multi() {
    const operations = [];
    const chain = {};
    for (const name of ["set", "del", "expire", "zadd", "zrem"]) {
      chain[name] = (...args) => {
        operations.push([name, args]);
        return chain;
      };
    }
    chain.exec = async () => {
      const transaction = (await this.#ready()).multi();
      for (const [name, args] of operations) {
        if (name === "set") transaction.set(args[0], args[1], nodeOptions(args[2] || {}));
        if (name === "del") transaction.del(args.flat());
        if (name === "expire") transaction.expire(args[0], args[1]);
        if (name === "zadd") transaction.zAdd(args[0], [{ score: args[1].score, value: String(args[1].member) }]);
        if (name === "zrem") transaction.zRem(args[0], args.slice(1).flat().map(String));
      }
      return transaction.exec();
    };
    return chain;
  }

  async #ready() {
    if (this.client.isReady || this.client.isOpen) return this.client;
    this.connecting ||= this.client.connect().finally(() => { this.connecting = null; });
    await this.connecting;
    return this.client;
  }
}

function nodeOptions(options) {
  const translated = {};
  if (options.nx) translated.NX = true;
  if (options.ex != null) translated.EX = Number(options.ex);
  return translated;
}
