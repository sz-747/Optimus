// In-app toasts: transient cards for surfaced notifications. The backend pushes policy-approved
// toasts (show_toast) down a persistent Channel as they're recorded — no polling. Each toast
// auto-dismisses; click to dismiss early.
const { invoke, Channel } = window.__TAURI__.core;

const TOAST_MS = 5000;
const stack = document.getElementById("toasts");

function showToast(t) {
  const el = document.createElement("div");
  el.className = "toast";

  const title = document.createElement("div");
  title.className = "toast-title";
  title.textContent = t.title;
  el.append(title);

  const detail = [t.subtitle, t.body].filter(Boolean).join(" — ");
  if (detail) {
    const body = document.createElement("div");
    body.className = "toast-body";
    body.textContent = detail;
    el.append(body);
  }

  const dismiss = () => el.remove();
  el.addEventListener("click", dismiss);
  const timer = setTimeout(dismiss, TOAST_MS);
  el.addEventListener("click", () => clearTimeout(timer), { once: true });

  stack.append(el);
}

// Raise a toast from the frontend (not the backend push channel) — e.g. a safe-zone cap refusal,
// which has no notification behind it but still needs to tell the user why nothing happened.
export function notify(title, body) {
  showToast({ title, subtitle: "", body: body || "" });
}

export function initToasts() {
  const channel = new Channel();
  channel.onmessage = (toasts) => toasts.forEach(showToast);
  invoke("listen_notifications", { onToast: channel }).catch((e) => console.error("toasts:", e));
}
