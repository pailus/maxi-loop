# Production deployment

Single host, Docker Compose (`docker-compose.prod.yml`: PostgreSQL 16 + the
`maxi-loop` image), behind nginx with TLS. The app listens on loopback only
(`127.0.0.1:8110`); PostgreSQL is not published at all.

The current production host is `loop.maxiflow.id` (Cloudflare proxied, SSL
mode Full, so the origin must answer on both 80 and 443).

## Routine deploy

```bash
DEPLOY_HOST=user@host deploy/deploy.sh
```

Builds the image locally (`Dockerfile`: npm build → cargo release → Debian
slim, non-root uid 10001), `docker save | ssh docker load`, uploads the compose
file, `docker compose up -d`, and waits for `/healthz`. Migrations run on
startup. The previously running image is kept as `maxi-loop:previous`.

Rollback:

```bash
ssh user@host 'docker tag maxi-loop:previous maxi-loop:latest && cd maxi-loop && docker compose -f docker-compose.prod.yml up -d'
```

Only forward-compatible migrations can be rolled back this way; restore a DB
backup otherwise.

## One-time server setup

1. `~/maxi-loop/` (mode 0700) with `.env` (mode 0600), generated on the server:
   `POSTGRES_USER`, `POSTGRES_PASSWORD` (`openssl rand -hex 32`, URL-safe),
   `POSTGRES_DB`, `PUBLIC_BASE_URL=https://<domain>`, `SINGLE_TENANT_MODE`,
   `LOG_FORMAT=json`, `RUST_LOG`, optional `SMTP_*`. `DATABASE_URL`,
   `COOKIE_SECURE=true` and `INGEST_SPOOL_DIR` are set by the compose file.
   **Never put `BOOTSTRAP_ADMIN_*` in `.env`**: startup bootstrap upserts that
   user and would reset its password on every restart.
2. TLS: `sudo certbot certonly --nginx --cert-name <domain> -d <domain>`.
3. nginx: copy `deploy/nginx/loop.maxiflow.id.conf` to
   `/etc/nginx/conf.d/`, then `sudo nginx -t && sudo systemctl reload nginx`.
4. First deploy (`deploy/deploy.sh`), then create the first admin once:

   ```bash
   cd ~/maxi-loop
   umask 077
   ADMIN_PW=$(openssl rand -base64 24)
   docker compose -f docker-compose.prod.yml run --rm --no-deps \
     -e BOOTSTRAP_ADMIN_EMAIL=admin@example.com \
     -e BOOTSTRAP_ADMIN_PASSWORD="$ADMIN_PW" \
     -e BOOTSTRAP_ADMIN_NAME=Administrator \
     app bootstrap
   printf 'email=admin@example.com\npassword=%s\n' "$ADMIN_PW" > ADMIN_CREDENTIALS
   ```

   Log in, change the password (Settings → password), then delete
   `ADMIN_CREDENTIALS`.

## Operations

- Logs: `docker logs -f maxi-loop-app` (JSON), capped at 3×50 MB.
- Backup: `docker exec maxi-loop-db pg_dump -U maxi_loop -Fc maxi_loop > maxi-loop-$(date +%F).dump`
- Ingest spool (named volume `maxi-loop_spool`, 0700) holds raw payloads only
  until they are written to the DB; it is replayed on startup.
