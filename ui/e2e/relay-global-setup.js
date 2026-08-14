import { closeRelayServer, startRelayServer as startLocalRelayServer } from "./relay-server.mjs";

export default async function startRelayServer() {
  await startLocalRelayServer();
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch("http://127.0.0.1:4174/api/control-plane/health");
      if (response.ok) return closeRelayServer;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("relay test server did not become ready");
}
