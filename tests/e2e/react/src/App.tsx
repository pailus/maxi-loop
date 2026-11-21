import * as Sentry from "@sentry/react";
import { useEffect, useState } from "react";
import { calculateCartSubtotal, validateDiscountRate } from "./checkout";

export interface E2EConfig {
  dsn: string;
  release: string;
  environment: string;
  backendUrl: string;
  mode: "full" | "invalid-dsn";
  piiSecret: string;
  callbackUrl: string;
}

declare global {
  interface Window {
    __MAXI_LOOP_E2E__?: E2EConfig;
  }
}

async function notifyDone(callbackUrl: string, payload: Record<string, unknown>): Promise<void> {
  await fetch(callbackUrl, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
}

export function App() {
  const [status, setStatus] = useState("running");

  useEffect(() => {
    const cfg = window.__MAXI_LOOP_E2E__;
    if (!cfg) {
      setStatus("missing-config");
      return;
    }

    let cancelled = false;
    void (async () => {
      try {
        if (cfg.mode === "invalid-dsn") {
          await Sentry.startSpan(
            {
              name: "GET /react/invalid-dsn",
              op: "navigation",
              forceTransaction: true,
            },
            async () => {
              try {
                calculateCartSubtotal(9999, undefined);
              } catch (err) {
                Sentry.captureException(err);
              }
            },
          );
          await Sentry.flush(5000);
          if (!cancelled) {
            setStatus("done");
            await notifyDone(cfg.callbackUrl, { ok: true, mode: cfg.mode });
          }
          return;
        }

        // 1. Trigger 3 distinct-order TypeError crashes in minified checkout.ts (with sensitive PII extra).
        //    Using distinct order IDs exercises real client transport (avoiding @sentry/browser's
        //    consecutive-identical dedupeIntegration) while verifying Maxi Loop groups all 3 into 1 issue.
        for (const orderId of [1001, 1002, 1003]) {
          try {
            calculateCartSubtotal(orderId, undefined);
          } catch (err) {
            Sentry.withScope((scope) => {
              scope.setTag("flow", "checkout");
              scope.setExtra("order_id", orderId);
              scope.setExtra("password", cfg.piiSecret);
              scope.setExtra("api_key", cfg.piiSecret);
              Sentry.captureException(err);
            });
          }
        }

        // 2. Trigger 1 distinct RangeError crash in minified checkout.ts.
        try {
          validateDiscountRate(150);
        } catch (err) {
          Sentry.withScope((scope) => {
            scope.setTag("flow", "coupon");
            scope.setExtra("token", cfg.piiSecret);
            Sentry.captureException(err);
          });
        }

        // 3. Execute a real browser transaction calling the FastAPI backend so @sentry/react
        //    propagates `sentry-trace` + `baggage` headers across origins.
        let browserTraceId = "";
        let backendTraceId = "";
        let backendParentSpanId = "";

        await Sentry.startSpan(
          {
            name: "GET /react/checkout",
            op: "ui.checkout.flow",
            forceTransaction: true,
          },
          async (rootSpan) => {
            browserTraceId = rootSpan.spanContext().traceId;
            const resp = await fetch(`${cfg.backendUrl}/api/checkout/42`, {
              method: "GET",
              headers: {
                Accept: "application/json",
              },
            });
            if (!resp.ok) {
              throw new Error(`backend returned HTTP ${resp.status}`);
            }
            const data = (await resp.json()) as {
              trace_id?: string;
              parent_span_id?: string;
            };
            backendTraceId = data.trace_id ?? "";
            backendParentSpanId = data.parent_span_id ?? "";
          },
        );

        const flushed = await Sentry.flush(8000);
        if (!cancelled) {
          setStatus("done");
          await notifyDone(cfg.callbackUrl, {
            ok: true,
            mode: cfg.mode,
            flushed,
            browser_trace_id: browserTraceId,
            backend_trace_id: backendTraceId,
            backend_parent_span_id: backendParentSpanId,
          });
        }
      } catch (err) {
        const message = err instanceof Error ? `${err.name}: ${err.message}` : String(err);
        if (!cancelled) {
          setStatus(`error: ${message}`);
          await notifyDone(cfg.callbackUrl, { ok: false, error: message });
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  return <main data-e2e-status={status}>Maxi Loop React E2E: {status}</main>;
}
