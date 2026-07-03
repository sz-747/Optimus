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
        // Load the mock (guarded) immediately before main.js so __TAURI__ exists when it evaluates.
        const loader =
          '<script type="module">if(!window.__TAURI__)await import("/mock/tauri-mock.js");</script>\n  ';
        return html.replace('<script type="module" src="./main.js">', loader + '<script type="module" src="./main.js">');
      },
    },
  ],
};
