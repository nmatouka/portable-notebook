#!/usr/bin/env bash
# Vendor every remote asset the WASM export fetches into dist/_vendor/<host>/<path>,
# then rewrite the worker bundles + lock JSON to point at those local paths.
#
# Strategy: host-mirroring. Each remote URL https://H/P is mirrored at
# dist/_vendor/H/P, and every "https://H/" prefix is rewritten to "/_vendor/H/".
# Because host+path are preserved, every URL the Pyodide loader constructs
# (indexURL, packageBaseUrl, lock file, absolute wheel URLs) resolves locally
# with no need to model packageBaseUrl derivation.
#
# Re-run after a fresh `marimo export html-wasm`. Requires network (downloads once).
set -euo pipefail
cd "$(dirname "$0")"

DIST=dist
VENDOR="$DIST/_vendor"
URLS=external-urls.txt

[ -d "$DIST" ] || { echo "no dist/ — run the marimo export first"; exit 1; }
[ -f "$URLS" ] || { echo "no $URLS — run: node capture.mjs chromium online"; exit 1; }

echo "==> mirroring $(grep -c . "$URLS") URLs into $VENDOR/"
rm -rf "$VENDOR"; mkdir -p "$VENDOR"
while IFS= read -r url; do
  url="${url%$'\r'}"                    # strip CR if checked out CRLF (Windows)
  [ -z "$url" ] && continue
  noscheme="${url#http*://}"
  host="${noscheme%%/*}"
  pathq="${noscheme#*/}"
  path="${pathq%%\?*}"                 # drop query string for the on-disk name
  dest="$VENDOR/$host/$path"
  mkdir -p "$(dirname "$dest")"
  curl -fsSL "$url" -o "$dest"
  echo "    $host/$path  ($(du -h "$dest" | cut -f1))"
done < "$URLS"

echo "==> rewriting worker bundles (cdn.jsdelivr.net, wasm.marimo.app -> /_vendor/)"
for w in "$DIST"/assets/worker-*.js "$DIST"/assets/save-worker-*.js; do
  [ -e "$w" ] || continue
  # jsdelivr base is used as packageBaseUrl in `new URL(wheel, base)`, which REQUIRES
  # an absolute base — so inject the runtime origin (portable: static server now,
  # app:// custom protocol later). The marimo lock is only fetch()'d, so a
  # root-relative path is fine there.
  perl -pi -e 's#https://cdn\.jsdelivr\.net/#\${self.location.origin}/_vendor/cdn.jsdelivr.net/#g;
               s#https://wasm\.marimo\.app/#/_vendor/wasm.marimo.app/#g' "$w"
  echo "    patched $(basename "$w")"
done

echo "==> rewriting vendored lock JSON (absolute wheel hosts -> /_vendor/)"
LOCK="$VENDOR/wasm.marimo.app/pyodide-lock.json"
perl -pi -e 's#https://test-files\.pythonhosted\.org/#/_vendor/test-files.pythonhosted.org/#g;
             s#https://files\.pythonhosted\.org/#/_vendor/files.pythonhosted.org/#g;
             s#https://cdn\.jsdelivr\.net/#/_vendor/cdn.jsdelivr.net/#g;
             s#https://wasm\.marimo\.app/#/_vendor/wasm.marimo.app/#g' "$LOCK"

# Integrity: every pinned file must match the committed checksums, so a tampered or
# MITM'd download (or a silent upstream change) fails the build instead of being
# embedded into the app. The immutable runtime (versioned wheels + Pyodide core) is
# pinned; the dynamically-served pyodide-lock.json is not (it would flake, and the
# wheels it references are themselves pinned). Regenerate deliberately on a real
# upstream change:
#   (cd dist/_vendor && find . -type f ! -name 'pyodide-lock.json' | sort | xargs shasum -a 256) > external-sha256.txt
echo "==> verifying vendored files against external-sha256.txt"
sums="$PWD/external-sha256.txt"
# macOS ships `shasum`; Windows git-bash / Linux ship `sha256sum` — same format.
if command -v shasum >/dev/null 2>&1; then shacheck="shasum -a 256 -c"
elif command -v sha256sum >/dev/null 2>&1; then shacheck="sha256sum -c"
else echo "    ERROR: no sha256 tool (shasum/sha256sum) found." >&2; exit 1; fi
if (cd "$VENDOR" && $shacheck "$sums" >/dev/null 2>&1); then
  echo "    all $(grep -c . "$sums") files match"
else
  echo "    ERROR: vendored-file checksum mismatch — tampering or an upstream change. Aborting." >&2
  exit 1
fi

echo "==> done. vendored size: $(du -sh "$VENDOR" | cut -f1)"
echo "    remaining https:// refs in workers (should be non-pyodide only):"
grep -rhoE 'https://(cdn\.jsdelivr\.net|wasm\.marimo\.app)[^"'"'"'`]*' "$DIST"/assets/worker-*.js "$DIST"/assets/save-worker-*.js 2>/dev/null | sort -u | sed 's/^/      /' || echo "      none"
