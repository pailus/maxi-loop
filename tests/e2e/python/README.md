# Python Official `sentry-sdk` E2E Verification (FastAPI & Django)

Verifies that the official Python [`sentry-sdk`](https://pypi.org/project/sentry-sdk/) works against a running `maxi-loop` server **by changing only the DSN**, covering:

- **FastAPI** (`FastApiIntegration`): repeated `ZeroDivisionError` occurrences grouped into 1 issue (`event_count = 3`) + separate `ValueError` issue (`event_count = 1`)
- **Django** (`DjangoIntegration`): repeated `RuntimeError` occurrences grouped into 1 issue (`event_count = 2`) + separate `KeyError` issue (`event_count = 1`)
- **Stack trace & source context**: verifies `in_app` frames with `context_line`
- **PII scrubbing**: verifies `Authorization` and `Cookie` headers sent with `send_default_pii=True` are scrubbed to `[Filtered]` before persistence
- **Negative auth check**: verifies a tampered DSN key ingests 0 events

## Running

```bash
uv venv tests/e2e/python/.venv
uv pip install --python tests/e2e/python/.venv/bin/python -r tests/e2e/python/requirements.txt

MAXI_LOOP_BASE_URL=http://127.0.0.1:8080 \
MAXI_LOOP_ADMIN_EMAIL=admin@maxi-loop.local \
MAXI_LOOP_ADMIN_PASSWORD='<your-admin-password>' \
tests/e2e/python/.venv/bin/python tests/e2e/python/verify_sentry_sdk.py
```
