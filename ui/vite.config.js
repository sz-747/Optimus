// The frontend talks to window.__TAURI__.core, which only exists inside the Tauri window. For
// `npm run dev` in a plain browser, inject the stateful mock (ui/mock/tauri-mock.js) so the chrome
// renders and reacts. `apply: "serve"` scopes this to the dev server only — production `vite build`
// never sees the mock, keeping index.html pristine and the installer clean.
export default {
  plugins: [
    {
      name: "inject-tauri-mock",
      apply: "serve",
      transformIndexHtml(html) {
        // Replace the main.js tag with one module that imports the mock THEN main.js — sequential
        // awaits guarantee __TAURI__ is installed before main.js evaluates (no cross-script ordering
        // assumption). main.js's first line destructures window.__TAURI__.core, so order is load-bearing.
        const loader =
          '<script type="module">\n' +
          '    if (!window.__TAURI__) await import("/mock/tauri-mock.js");\n' +
          '    await import("/main.js");\n' +
          "  </script>";
        return html.replace('<script type="module" src="./main.js"></script>', loader);
      },
    },
  ],
};
