import * as Sentry from "@sentry/react";
import { createRoot } from "react-dom/client";
import { App } from "./App";

const cfg = window.__MAXI_LOOP_E2E__;
if (cfg) {
  Sentry.init({
    dsn: cfg.dsn,
    release: cfg.release,
    environment: cfg.environment,
    tracesSampleRate: 1.0,
    integrations: [
      Sentry.browserTracingIntegration({
        instrumentPageLoad: false,
        instrumentNavigation: false,
      }),
    ],
    tracePropagationTargets: [cfg.backendUrl],
  });
}

const rootEl = document.getElementById("root");
if (rootEl) {
  createRoot(rootEl).render(<App />);
}
