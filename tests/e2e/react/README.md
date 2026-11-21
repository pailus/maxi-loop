# React Official `@sentry/react` E2E Verification

Verifies that the official browser [`@sentry/react`](https://www.npmjs.com/package/@sentry/react) SDK works end-to-end against a running `maxi-loop` server **in real Headless Chrome** across origins, covering:

- **Cross-origin browser CORS** on `/api/{project_id}/envelope/` (`OPTIONS` preflight + `Access-Control-Allow-Origin: *` + `Access-Control-Expose-Headers: X-Sentry-Rate-Limits, Retry-After`, while `/api/v1/*` stays strictly same-origin)
- **Production Vite build + CI Source Map upload**: uploads the real minified bundle's `.js.map` via `POST /api/v1/projects/{slug}/releases/{version}/sourcemaps` using a per-project write API token (`Authorization: Bearer mlapi_...`)
- **Minified React error capture & source map resolution**: verifies 3× `TypeError` + 1× `RangeError` from minified `dist/assets/index-*.js` are resolved back to original `src/checkout.ts` with `context_line`, `pre_context`, `post_context`, `in_app = true`, and `sourcemap_resolved = true`, and that sensitive `password`/`api_key`/`token` fields are scrubbed to `[Filtered]`
- **Browser → FastAPI distributed trace waterfall**: Chrome executes a real `fetch` inside `@sentry/react` (`browserTracingIntegration`), propagating `sentry-trace` + `baggage` headers to a live FastAPI backend (`sentry-sdk`); verifies a 2-service distributed waterfall linking Chrome (`ui.checkout.flow` → `http.client`) to FastAPI (`http.server` → `db.sql.query`)
- **Negative auth check**: verifies a tampered DSN in Chrome ingests `0` issues and `0` transactions

## Running

```bash
npm --prefix tests/e2e/react install
npm --prefix tests/e2e/react run build

MAXI_LOOP_BASE_URL=http://127.0.0.1:8080 \
MAXI_LOOP_ADMIN_EMAIL=admin@maxi-loop.local \
MAXI_LOOP_ADMIN_PASSWORD='<your-admin-password>' \
tests/e2e/python/.venv/bin/python tests/e2e/verify_react_sdk.py
```
