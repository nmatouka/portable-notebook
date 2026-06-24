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

# The vendored loader builds wheel URLs as `${self.location.origin}/_vendor/...`
# (experiment #1). Keeping the *runtime* origin makes the frontend work unchanged
# over Tauri's custom protocol on every platform, whose origin differs by engine —
# mnote://localhost on macOS/Linux (WebKit) vs http://mnote.localhost on Windows
# (WebView2). So nothing here is pinned to a specific scheme.

echo "synced $(find frontend -type f | wc -l | tr -d ' ') files into app/frontend ($(du -sh frontend | cut -f1))"
