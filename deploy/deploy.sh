#!/usr/bin/env bash
# Build the production image locally, ship it to the server over SSH and
# (re)start the stack. One-time server setup (dir, .env, TLS, nginx, admin):
# see deploy/README.md.
#
# Usage: DEPLOY_HOST=user@host deploy/deploy.sh [--skip-build]
set -euo pipefail

: "${DEPLOY_HOST:?set DEPLOY_HOST=user@host}"
REMOTE_DIR="${REMOTE_DIR:-maxi-loop}"   # relative to the remote user's home
HOST_PORT="${MAXI_LOOP_HOST_PORT:-8110}"
IMAGE="maxi-loop:latest"

cd "$(dirname "$0")/.."

if [[ "${1:-}" != "--skip-build" ]]; then
  echo "==> building $IMAGE"
  DOCKER_BUILDKIT=1 docker build -t "$IMAGE" .
fi

echo "==> keeping the running image as maxi-loop:previous (rollback)"
ssh "$DEPLOY_HOST" "docker image inspect $IMAGE >/dev/null 2>&1 && docker tag $IMAGE maxi-loop:previous || true"

echo "==> shipping image"
docker save "$IMAGE" | gzip -1 | ssh "$DEPLOY_HOST" 'gunzip | docker load'
# Podman tags local builds as localhost/<name>; normalise for the compose file.
ssh "$DEPLOY_HOST" "docker image inspect localhost/$IMAGE >/dev/null 2>&1 \
  && docker tag localhost/$IMAGE $IMAGE && docker rmi localhost/$IMAGE >/dev/null || true"

echo "==> uploading compose file"
scp -q docker-compose.prod.yml "$DEPLOY_HOST:$REMOTE_DIR/docker-compose.prod.yml"

echo "==> restarting stack"
ssh "$DEPLOY_HOST" "cd $REMOTE_DIR && test -f .env && docker compose -f docker-compose.prod.yml up -d --remove-orphans"

echo "==> waiting for /healthz"
for _ in $(seq 1 30); do
  if ssh "$DEPLOY_HOST" "curl -fsS http://127.0.0.1:$HOST_PORT/healthz" >/dev/null 2>&1; then
    echo "==> healthy"
    exit 0
  fi
  sleep 2
done
echo "!! not healthy after 60s; logs: ssh $DEPLOY_HOST 'docker logs --tail 100 maxi-loop-app'" >&2
exit 1
