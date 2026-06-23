#!/usr/bin/env bash
# Populate app/frontend/ with the proven, vendored offline export from
# experiment #1. This is the literal "drop that working folder into a Tauri
# shell" step. frontend/ is generated (gitignored); re-run after re-vendoring.
set -euo pipefail
cd "$(dirname "$0")"

SRC=../experiments/offline-folder/dist
if [ ! -d "$SRC/_vendor" ]; then
  echo "error: $SRC/_vendor not found."
  echo "       run experiments/offline-folder/vendor.sh first to produce the vendored export."
  exit 1
fi

rm -rf frontend
cp -R "$SRC" frontend

# The app serves the frontend over the mnote:// custom protocol. Pin the vendored
# loader's absolute base to that scheme so `new URL(wheel, base)` always has a
# valid hierarchical base, regardless of how self.location.origin resolves under
# a custom scheme. (Experiment #1 left it as ${self.location.origin} for the
# static-server case.)
for w in frontend/assets/worker-*.js frontend/assets/save-worker-*.js; do
  [ -e "$w" ] || continue
  perl -pi -e 's#\$\{self\.location\.origin\}/_vendor/#mnote://localhost/_vendor/#g' "$w"
done

echo "synced $(find frontend -type f | wc -l | tr -d ' ') files into app/frontend ($(du -sh frontend | cut -f1)), pinned loader base to mnote://localhost"
