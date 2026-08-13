#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(dirname "$script_dir")"
desktop_dir="$project_root/desktop"

cd "$desktop_dir"
npm run build
npm run preview -- --host 127.0.0.1 &
preview_pid=$!
trap 'kill "$preview_pid" 2>/dev/null || true' EXIT

for attempt in {1..60}; do
  if curl -fsS http://127.0.0.1:4173 >/dev/null; then
    break
  fi
  sleep 0.25
done

VELGRINOR_SCREENSHOT_URL=http://127.0.0.1:4173 node scripts/capture-screenshots.mjs
