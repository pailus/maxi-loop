#!/usr/bin/env bash
# init.sh — Run this at the start of every work session: bash .harness/init.sh
# Purpose: make sure the environment is ready and show a summary of the
# project state before the coding assistant starts working.

set -euo pipefail

# Always work from the project root, wherever this script is called from.
HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$HARNESS_DIR/.."

echo "== Harness Init =="
echo "Project: Maxi Loop (detected stack: rust (from project type))"

# 1. Check basic tools are available (adjust to the project's stack)
command -v git >/dev/null 2>&1 && echo "[ok] git: $(git --version)" || echo "[!!] git not found"

# 2. Install dependencies if there is a package manager (uncomment per stack)
# npm install
# pip install -r requirements.txt

# 3. Short git status
echo ""
echo "== Git status =="
git status --short || true

# 4. Secret hygiene: .env files must never be tracked by git
echo ""
echo "== Secrets check =="
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  tracked_env="$(git ls-files | grep -E '(^|/)\.env(\.[^/]*)?$' | grep -vE '\.(example|sample|template)$' || true)"
  if [ -n "$tracked_env" ]; then
    echo "[!!] .env file(s) tracked by git — remove from the index and rotate the secrets:"
    echo "$tracked_env" | sed 's/^/     /'
  else
    echo "[ok] no .env file tracked by git"
  fi
  if [ -f .env ] && ! git check-ignore -q .env; then
    echo "[!!] .env exists but is not in .gitignore"
  fi
else
  echo "(not a git repository — skipped)"
fi

# 5. Feature-list summary (needs jq; skipped if missing)
echo ""
echo "== Feature status =="
if command -v jq >/dev/null 2>&1 && [ -f .harness/feature-list.json ]; then
  jq -r '.features[] | "\(.status)\t\(.id)\t\(.title)"' .harness/feature-list.json
else
  echo "(jq not available or .harness/feature-list.json not found — check manually)"
fi

# 6. Harness housekeeping
echo ""
echo "== Harness =="
if [ -f .harness/progress.md ]; then
  entries="$(grep -c '^### ' .harness/progress.md || true)"
  if [ "${entries:-0}" -gt 13 ]; then
    echo "[!!] progress.md has ${entries} entries — move the oldest to .harness/progress-archive.md (keep ~10)"
  else
    echo "[ok] progress.md: ${entries} entries"
  fi
fi
if [ -f .harness/ARCHITECTURE.md ] && grep -q 'NOT FILLED YET' .harness/ARCHITECTURE.md; then
  echo "[!!] .harness/ARCHITECTURE.md is not filled yet — fill it before other work"
fi

# 7. Reminder of the files to read
echo ""
echo "== Read before starting =="
echo "1. .harness/AGENTS.md"
echo "2. .harness/progress.md (top entry)"
echo "3. .harness/session-handoff.md"
echo "4. .harness/ARCHITECTURE.md (project map)"
echo "5. .harness/SECURITY.md / ENGINEERING.md / decisions.md (when the task touches them)"
echo ""
echo "Init done."
