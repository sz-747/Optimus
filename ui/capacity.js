// The always-visible capacity meter (CLAUDE.md thesis): an 8-pip safe-zone bar + "X / Y terminals"
// label + SAFE-ZONE/AT-CAP chip, bound to the backend CapacityView. Presentation lives in the
// backend (build_capacity_view, matching the ported view model); this just paints the snapshot.
const { invoke } = window.__TAURI__.core;

const PIP_COUNT = 8;

const meter = document.getElementById("cap-meter");
const pipsEl = meter.querySelector(".pips");
const labelEl = meter.querySelector(".cap-label");
const chip = document.getElementById("safe-zone");

const pips = Array.from({ length: PIP_COUNT }, () => {
  const p = document.createElement("span");
  p.className = "pip";
  pipsEl.append(p);
  return p;
});

export function renderCapacity(view) {
  const filled = Math.round(view.fraction * PIP_COUNT);
  pips.forEach((p, i) => p.classList.toggle("on", i < filled));
  meter.className = `cap level-${view.level}`;
  labelEl.textContent = view.label;
  chip.textContent = view.at_cap ? "AT CAP" : "SAFE ZONE";
  chip.className = `chip level-${view.level}`;
  chip.title = view.hint || "";
}

// Fetch, paint, and return the view so callers (e.g. a cap-refusal handler) can read at_cap/hint.
export function refreshCapacity() {
  return invoke("capacity_state")
    .then((view) => {
      renderCapacity(view);
      return view;
    })
    .catch((e) => {
      console.error("capacity:", e);
      return null;
    });
}

export function initCapacity() {
  refreshCapacity();
  // The static cap only shifts on spawn/release (which also refresh), but poll slowly so a future
  // memory-pressure tightening still surfaces without a structural op. ponytail: 1.5s is plenty.
  setInterval(refreshCapacity, 1500);
}
