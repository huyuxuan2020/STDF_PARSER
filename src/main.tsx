import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import type { StdfApi } from "./types";

// Dev-only design preview: `vite dev` + `?mock` runs the app against the test
// fixtures so every view is reachable in a plain browser without the Tauri
// backend. The DEV guard keeps the fixtures out of production bundles; the
// mock is exposed as window.mockApi so emitComplete()/emitProgress() can be
// driven from the console.
async function resolveApi(): Promise<StdfApi | undefined> {
  if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("mock")) {
    const { createMockApi } = await import("./test/fixtures");
    const mock = createMockApi();
    (window as Window & { mockApi?: StdfApi }).mockApi = mock;
    return mock;
  }
  return undefined;
}

resolveApi().then((api) => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App api={api} />
    </React.StrictMode>
  );
});
