import { resolve } from "node:path";
import { createServer } from "vite";

const controlPlaneUrl = "http://127.0.0.1:4173/control-plane?demo=1";

async function serverIsReady() {
  try {
    const response = await fetch(controlPlaneUrl, { signal: AbortSignal.timeout(1_000) });
    return response.ok && response.headers.get("content-type")?.includes("text/html");
  } catch {
    return false;
  }
}

export default async function startControlPlane() {
  if (await serverIsReady()) return undefined;

  const server = await createServer({
    root: resolve("ui"),
    logLevel: "warn",
    server: {
      host: "127.0.0.1",
      port: 4173,
      strictPort: true,
    },
  });
  await server.listen();

  return async () => {
    await server.close();
  };
}
