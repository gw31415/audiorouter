import { StrictMode } from "react";
import { createRoot, hydrateRoot } from "react-dom/client";
import { renderToString } from "react-dom/server";
import App from "./App";
import "./index.css";

// Build-time prerender entry — called by vite-prerender-plugin.
// Runs in Node (no DOM), so only renderToString is needed.
export function prerender(): string {
  return renderToString(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

// Browser entry — runs only in the client.
if (typeof document !== "undefined") {
  const rootEl = document.getElementById("root")!;
  const app = (
    <StrictMode>
      <App />
    </StrictMode>
  );

  // When built with vite-prerender-plugin, #root contains pre-rendered HTML.
  // hydrateRoot attaches React to the existing DOM without a full re-render.
  // In dev (no pre-rendered content), fall back to createRoot.
  if (rootEl.hasChildNodes()) {
    hydrateRoot(rootEl, app);
  } else {
    createRoot(rootEl).render(app);
  }
}
