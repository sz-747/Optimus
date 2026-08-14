const RELAY_BASE = "/api/control-plane";

export function createControlPlaneTransport({ onConnection, onRevision, onCommand = () => {} }) {
  if (window.__TAURI__) return tauriTransport(onConnection, onRevision);
  return relayTransport(onConnection, onRevision, onCommand);
}

function tauriTransport(onConnection, onRevision) {
  const { invoke, Channel } = window.__TAURI__.core;
  return {
    mode: "desktop",
    async snapshot() {
      const snapshot = await invoke("control_plane_snapshot");
      onConnection("connected");
      return snapshot;
    },
    async subscribe() {
      const channel = new Channel();
      channel.onmessage = ({ revision }) => onRevision(revision);
      await invoke("listen_control_plane", { onUpdate: channel });
    },
    async startRun(payload) {
      const session = await invoke("control_session_start", {
        repoRoot: payload.repoRoot,
        name: payload.sessionName,
      });
      const agent = await invoke("control_agent_prepare", {
        sessionId: idValue(session.id),
        parentId: null,
        name: payload.agentName,
        task: payload.task,
        command: payload.command,
        branch: payload.branch,
        kind: payload.kind,
        fileScope: payload.fileScope,
      });
      return invoke("control_agent_start", { agentId: idValue(agent.id) });
    },
    async stopAgent(agentId) {
      return invoke("control_agent_stop", { agentId: numericId(agentId) });
    },
    authenticated() { return true; },
    async authenticate() {},
  };
}

function relayTransport(onConnection, onRevision, onCommand) {
  let stopped = false;
  let sessionActive = false;
  let eventPromise = null;
  let eventController = null;
  let eventGeneration = 0;
  let commandGeneration = 0;
  let lastRevision = 0;
  const deviceId = localStorage.getItem("optimus.deviceId") || "primary";
  const request = async (path, options = {}) => {
    const response = await fetch(`${RELAY_BASE}${path}`, {
      ...options,
      credentials: "same-origin",
      headers: { ...options.headers },
    });
    if (!response.ok) {
      const payload = await response.json().catch(() => null);
      const error = new Error(payload?.error?.message || `relay ${response.status}`);
      error.status = response.status;
      error.code = payload?.error?.code || "relay_error";
      if (response.status === 401) {
        sessionActive = false;
        stopEvents();
        onConnection("offline");
      }
      throw error;
    }
    return response;
  };

  function stopEvents() {
    eventGeneration += 1;
    eventController?.abort();
    eventController = null;
    eventPromise = null;
  }

  async function eventLoop(generation) {
    let backoff = 500;
    while (!stopped && sessionActive && generation === eventGeneration) {
      const controller = new AbortController();
      let reader = null;
      try {
        eventController = controller;
        const response = await request(`/events?after=${lastRevision}`, {
          headers: { Accept: "text/event-stream", "Last-Event-ID": String(lastRevision) },
          signal: controller.signal,
        });
        if (!response.headers.get("content-type")?.includes("text/event-stream")) {
          throw new Error("relay event stream has an invalid content type");
        }
        backoff = 500;
        reader = response.body.getReader();
        const decoder = new TextDecoder();
        let pending = "";
        while (!stopped && sessionActive && generation === eventGeneration) {
          const result = await readWithHeartbeat(reader, 16_000);
          if (result.done) throw new Error("relay event stream closed");
          pending += decoder.decode(result.value, { stream: true }).replaceAll("\r\n", "\n");
          const events = pending.split("\n\n");
          pending = events.pop() || "";
          for (const event of events) {
            let eventId = 0;
            let eventName = "message";
            let message = null;
            for (const line of event.split("\n")) {
              if (line.startsWith("id:")) eventId = Number(line.slice(3).trim()) || 0;
              if (line.startsWith("event:")) eventName = line.slice(6).trim();
              if (line.startsWith("data:")) message = JSON.parse(line.slice(5).trim());
            }
            if (eventName === "presence") {
              onConnection(message?.online ? "connected" : "offline");
              continue;
            }
            const revision = Math.max(eventId, Number(message?.revision) || 0);
            if (revision > lastRevision) {
              lastRevision = revision;
              onRevision(revision);
            }
          }
        }
      } catch (error) {
        if (stopped || !sessionActive || generation !== eventGeneration || error.name === "AbortError") break;
        console.warn("control-plane relay reconnect:", error.message || error);
        onConnection("stale");
        await delay(withJitter(backoff));
        backoff = Math.min(backoff * 2, 8_000);
      } finally {
        controller.abort();
        await reader?.cancel().catch(() => {});
        if (eventController === controller) eventController = null;
      }
    }
    if (generation === eventGeneration) eventPromise = null;
  }

  function subscribeOnce() {
    if (!sessionActive || stopped || eventPromise) return;
    const generation = ++eventGeneration;
    eventPromise = eventLoop(generation);
  }

  async function watchCommand(initial) {
    const generation = commandGeneration;
    let command = initial;
    let backoff = 750;
    onCommand(command);
    while (!stopped && sessionActive && generation === commandGeneration && !terminal(command.status)) {
      await delay(backoff);
      try {
        const response = await request(`/commands?id=${encodeURIComponent(command.commandId)}`);
        command = await response.json();
        onCommand(command);
        backoff = 750;
      } catch (error) {
        if (stopped || !sessionActive || generation !== commandGeneration) return;
        console.warn("control-plane command status retry:", error.message || error);
        backoff = Math.min(backoff * 2, 8_000);
      }
    }
  }

  async function submitCommand(input) {
    const response = await request("/commands", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": crypto.randomUUID() },
      body: JSON.stringify(input),
    });
    const command = await response.json();
    void watchCommand(command);
    return command;
  }

  return {
    mode: "web",
    async snapshot() {
      const response = await request("/snapshot");
      const snapshot = await response.json();
      sessionActive = true;
      lastRevision = Math.max(lastRevision, Number(snapshot.revision) || 0);
      return snapshot;
    },
    async subscribe() {
      stopped = false;
      subscribeOnce();
    },
    async startRun(payload) {
      return submitCommand({
          type: "start_run",
          payload: {
            workspaceId: payload.workspaceId,
            profileId: payload.profileId,
            sessionName: payload.sessionName,
            agents: payload.agents,
          },
      });
    },
    async stopAgent(agentId) {
      return submitCommand({ type: "stop_agent", payload: { agentId } });
    },
    authenticated() { return sessionActive; },
    async authenticate(token) {
      stopEvents();
      const response = await fetch(`${RELAY_BASE}/auth`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ token: token.trim(), deviceId }),
      });
      if (!response.ok) {
        const payload = await response.json().catch(() => null);
        throw new Error(payload?.error?.message || `relay ${response.status}`);
      }
      sessionActive = true;
      stopped = false;
    },
    close() {
      stopped = true;
      commandGeneration += 1;
      stopEvents();
    },
  };
}

function terminal(status) {
  return ["succeeded", "failed", "rejected", "expired"].includes(status);
}

async function readWithHeartbeat(reader, timeoutMs) {
  let timer;
  try {
    return await Promise.race([
      reader.read(),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("relay heartbeat timed out")), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function withJitter(milliseconds) {
  return Math.round(milliseconds * (0.8 + Math.random() * 0.4));
}

function numericId(value) {
  return Number(String(value).replace(/^agent-/, ""));
}

function idValue(value) {
  if (typeof value === "number") return value;
  if (value && typeof value === "object" && "0" in value) return Number(value[0]);
  return Number(String(value).replace(/^[^-]+-/, ""));
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
