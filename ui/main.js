// P1 dev stub: spawn a shell, stream PTY bytes into a <pre>. xterm.js replaces this in P4.
const { invoke, Channel } = window.__TAURI__.core;

const out = document.createElement("pre");
out.id = "dev-output";
document.getElementById("app").append(out);

const decoder = new TextDecoder();

async function boot() {
  const onOutput = new Channel();
  onOutput.onmessage = (bytes) => {
    console.log("pty bytes:", bytes.length);
    out.textContent += decoder.decode(new Uint8Array(bytes), { stream: true });
  };
  const onEvent = new Channel();
  onEvent.onmessage = (msg) => console.log("engine event:", msg);

  const id = await invoke("dev_spawn_shell", { onOutput, onEvent });
  console.log("surface", id, "spawned");

  // Echo keystrokes straight through so the shell is minimally usable in the stub.
  document.addEventListener("keydown", (e) => {
    if (e.key.length === 1 && !e.ctrlKey && !e.altKey) {
      invoke("dev_send_text", { id, text: e.key });
    } else if (e.key === "Enter") {
      invoke("dev_send_text", { id, text: "\r" });
    } else if (e.key === "Backspace") {
      invoke("dev_send_text", { id, text: "\x7f" });
    }
  });
}

boot().catch((e) => {
  out.textContent = `boot failed: ${e}`;
  console.error(e);
});
