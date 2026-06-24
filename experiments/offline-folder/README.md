# Experiment #1 — Offline-in-a-folder ✅ PASSED

**Goal (spec §6 step 1 / §10):** prove that a `marimo export html-wasm` notebook can run **with no network** once Pyodide and the required wheels are vendored locally. This is the only genuinely uncertain piece of the whole project; everything else is derisked only after this passes.

## Result

**Passes on both engines — Chromium and WebKit — with zero network.** Tested by aborting *every* non-localhost request at the browser layer (stricter than "Wi-Fi off"):

| | Chromium | WebKit |
|---|---|---|
| Loaded offline | ✅ | ✅ |
| numpy computed in WASM | `$1,628.89` | `$1,628.89` |
| Slider → live recompute | `$3,394.57` | `$3,394.57` |
| External requests | **0** | **0** |
| Requests blocked (needed but missing) | **0** | **0** |

The spec's success criterion — *"if the slider works offline, the only real unknown is beaten"* — is met. Cleared to proceed to Step 2 (Tauri wrap).

---

## What's here

| File | Purpose |
|------|---------|
| `notebook.py` | Test notebook: an interest-rate slider driving a numpy compound-interest calc. PEP 723 inline deps. |
| `external-urls.txt` | The 18 remote URLs the export fetches (captured online). Input to `vendor.sh`. |
| `vendor.sh` | Mirrors those 18 URLs into `dist/_vendor/<host>/<path>` and rewrites the worker bundles + lock JSON to local paths. Re-run after a fresh export. |
| `capture.mjs` | Playwright harness. `online` logs traffic + writes the URL list; `offline` aborts every non-local request. Verifies initial render **and** slider-driven recompute on `chromium`/`webkit`. |
| `dist/` | *(gitignored)* the WASM export + `dist/_vendor/` (the vendored runtime). Regenerate with the commands below. |

## Reproduce

```bash
cd experiments/offline-folder

uv venv --python 3.12 .venv
uv pip install --python .venv/bin/python marimo numpy
.venv/bin/marimo export html-wasm notebook.py -o dist --mode run

npm install && npx playwright install chromium webkit

# serve + capture the remote URL list (online, once)
.venv/bin/python -m http.server 8123 --directory dist &
node capture.mjs chromium online        # confirms it works + records external-urls.txt

# vendor everything locally, then prove it offline
bash vendor.sh
node capture.mjs chromium offline
node capture.mjs webkit   offline
```

---

## Findings

### 1. The export self-vendors marimo's frontend (layer 1 is free)
`marimo export html-wasm` copies marimo's entire JS/CSS frontend into `dist/assets/` (~27 MB). Of the spec's **three vendoring layers**, only the **Pyodide runtime** and the **wheels** are fetched remotely and need vendoring.

### 2. Exact remote set: 18 files, 4 hosts (Pyodide `v314.0.0`, Python 3.14 / ABI `2026_0`)

| Host | What it serves |
|------|----------------|
| `cdn.jsdelivr.net/pyodide/v314.0.0/full/` | Pyodide core (`pyodide.asm.wasm`, `pyodide.asm.mjs`, `python_stdlib.zip`) + base wheels (numpy, micropip, msgspec, pyyaml, docutils, jedi, parso, packaging, pygments, pyodide_http) |
| `wasm.marimo.app` | `pyodide-lock.json` |
| `test-files.pythonhosted.org` (TestPyPI) | `marimo_base-0.23.10` |
| `files.pythonhosted.org` (PyPI) | `markdown`, `narwhals`, `pymdown_extensions` |

Relative wheel names in the lock resolve against the **jsDelivr** base (`packageBaseUrl`); only marimo and the 3 PyPI deps carry absolute URLs.

### 3. Vendoring strategy: host-mirroring
Mirror every URL `https://H/P` → `dist/_vendor/H/P`, then rewrite `https://H/` → local in the worker bundles and the lock JSON. Preserving host+path means every URL the loader *constructs* lines up automatically — no need to model `packageBaseUrl` derivation.

### 4. The one real gotcha: `new URL(wheel, base)` needs an **absolute** base
Pyodide loads the **core** by string concatenation (a root-relative `/_vendor/...` path works), but loads **wheels** via `new URL(fileName, packageBaseUrl)` — and a root-relative base throws `Failed to construct 'URL': Invalid base URL`. Fix: inject the runtime origin so the base is absolute. The jsDelivr base lives in a backtick template literal, so:

```
https://cdn.jsdelivr.net/  →  ${self.location.origin}/_vendor/cdn.jsdelivr.net/
```

`self.location.origin` is **portable**: it's the static-server origin now and the `app://` custom-protocol origin in Step 3. The marimo lock is only `fetch()`'d, so it stays a plain root-relative path.

### 5. WASM MIME is a non-issue locally
`python -m http.server` (3.12) serves `.wasm` as `application/wasm`, satisfying `WebAssembly.instantiateStreaming` with no custom handling. (The Step-3 custom protocol must replicate this.)

### 6. Cross-origin isolation not needed (as predicted)
Single-threaded WASM ⇒ no `SharedArrayBuffer` ⇒ no COOP/COEP headers. The export loads under both engines without cross-origin isolation. (Re-confirm under the custom protocol in Step 3.)

### 7. The version-skew worry did not bite
The lock is requested as `pyodide-lock.json?v=0.23.10&pyodide=v314.0.0` and serves the matching `marimo_base-0.23.10`. (Fetching the lock *without* the query returns an older `0.23.9` — always pass the version query.)

### 8. WebKit-only quirk (cosmetic, non-blocking)
WebKit requests three fonts (`PTSans`, `Lora`) at the **root** path instead of `/assets/...` → local 404s. They're cosmetic (the compute path is unaffected and text renders), but worth fixing later by correcting the font `url()` base. **Flag for Step 2/3** since engine-specific path quirks are exactly what cross-engine support must catch.

### 9. The marimo slider lives in shadow DOM
The thumb is `span[role="slider"]` inside the `<marimo-slider>` web component's open shadow root — invisible to `document.querySelector` but reachable by Playwright locators (which pierce open shadow DOM). Drive it by focusing `[role="slider"]` and pressing Arrow keys.

---

## Package resolution (derisk for spec §5)

How does a notebook use a package that **isn't** baked in?

- For a non-baked dep, marimo-wasm uses **micropip → the PyPI Simple API**: `pypi.org/simple/<pkg>/`, then `<wheel>.metadata`, then the wheel from `files.pythonhosted.org` (observed with a `cowsay` notebook, online).
- **micropip checks the Pyodide lock first.** Adding a package to the served `pyodide-lock.json` with a locally-available wheel makes micropip install it from there — **no PyPI query**. Verified: the `cowsay` notebook ran **offline** (0 external requests) once `cowsay` was added to the lock with a local wheel.

So the resolver is the same dynamic-serving trick used for `index.html`, applied to the lock:

> On open, the player resolves the notebook's declared deps; for any not already baked, it makes the wheel available locally and **adds a lock entry** pointing at it. micropip loads everything locally. The webview never touches PyPI (consistent with the egress-blocking CSP) — only the Rust backend may fetch.

**Three tiers** (spec §5), landing in one shared, version-keyed wheel cache:
1. **Baked** — already in the frontend's lock. Offline. *(works today)*
2. **Bundled in the `.mnote`** — `.mnote` becomes a zip (`notebook.py` + `wheels/` + a `manifest.json` of lock entries); the player merges them in. Offline, no download; the author resolves the closure at authoring time.
3. **Fetched on demand** — the Rust backend downloads the wheel from PyPI once, caches, serves it via `mnote://`, gated by a one-time confirmation. Online only for the first open.

Tier-1 works today; tiers 2–3 are the implementation ahead. Fixtures: `notebook-cowsay.mnote`, `pkg-probe.mjs`.

## Carry-forward for later steps
- **Version floor:** the runtime targets bleeding-edge **Python 3.14 / Pyodide v314**. Track as an aggressive minimum.
- **Portability of the fix:** the `${self.location.origin}` rewrite already works for any origin, so Step 3's `app://` protocol should need no change to the vendoring — only a handler that serves `/_vendor/...` with correct MIME.
- **WebKit font 404s** (finding #8) to resolve before shipping.
- `vendor.sh` is **not idempotent** — it rewrites `https://` → local, so re-run it only on a freshly exported `dist/`.
