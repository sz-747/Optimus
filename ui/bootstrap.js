const controlPlaneRoute =
  location.pathname === "/control-plane" ||
  location.search.includes("view=control-plane") ||
  !window.__TAURI__;

const application = controlPlaneRoute
  ? import("./control-plane-app.js")
  : import("./main.js");

application.catch((error) => {
  console.error("Optimus UI bootstrap failed:", error);
});
