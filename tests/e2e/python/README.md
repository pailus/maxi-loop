# Python Official `sentry-sdk` E2E Verification (FastAPI & Django)

Verifies that the official Python [`sentry-sdk`](https://pypi.org/project/sentry-sdk/) works against a running `maxi-loop` server **by changing only the DSN**, covering **both Error Tracking and Performance Monitoring**:

- **FastAPI** (`FastApiIntegration`):
  - Errors: repeated `ZeroDivisionError` occurrences grouped into 1 issue (`event_count = 3`) + separate `ValueError` issue (`event_count = 1`)
  - Performance: `GET /api/fastapi/orders/{order_id}` with `db.sql.query` + `http.client` spans (`p50 >= 500ms`, `error_count = 0`) and `GET /billing/divide/{invoice_id}` (`3/3` error transactions)
- **Django** (`DjangoIntegration`, real WSGI handler via `get_wsgi_application()`):
  - Errors: repeated `RuntimeError` occurrences grouped into 1 issue (`event_count = 2`) + separate `KeyError` issue (`event_count = 1`)
  - Performance: `GET /django/orders/{order_id}/` continuing the FastAPI trace via `sentry-trace` + `baggage` headers with `db.sql.query` span (`p50 >= 500ms`) and `GET /django/checkout/{order_id}/` (`2/2` error transactions)
- **Cross-service distributed trace waterfall**: verifies FastAPI (`http.server` → `http.client`) linked cleanly to Django (`http.server` → `db.sql.query`) under one shared `trace_id`
- **Stack trace & PII scrubbing**: verifies `in_app` frames with `context_line`, and confirms `Authorization`, `Cookie`, and sensitive span `data` (`password`, `api_key`) are scrubbed to `[Filtered]` in both issues and sampled traces
- **Per-project API tokens**: reads issues and performance metrics using scoped `Authorization: Bearer mlapi_...` tokens
- **Negative auth check**: verifies a tampered DSN key ingests `0` issues and `0` transactions

## Running

```bash
uv venv tests/e2e/python/.venv
uv pip install --python tests/e2e/python/.venv/bin/python -r tests/e2e/python/requirements.txt

MAXI_LOOP_BASE_URL=http://127.0.0.1:8080 \
MAXI_LOOP_ADMIN_EMAIL=admin@maxi-loop.local \
MAXI_LOOP_ADMIN_PASSWORD='<your-admin-password>' \
tests/e2e/python/.venv/bin/python tests/e2e/python/verify_sentry_sdk.py
```
