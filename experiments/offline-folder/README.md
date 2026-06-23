# Experiment #1 — Offline-in-a-folder

**Goal (spec §6 step 1 / §10):** prove that a `marimo export html-wasm` notebook can run **with no network** once Pyodide and the required wheels are vendored locally. This is the only genuinely uncertain piece of the whole project; everything else is derisked only after this passes.

**Status:** 🔄 In progress.
- ✅ Export runs **online** in headless Chromium — numpy executes in WASM and the slider-driven output renders.
- ⏳ Offline vendoring + the offline-blocked re-test (Chromium **and** WebKit) not yet done.

---

## What's here

| File | Purpose |
|------|---------|
| `notebook.py` | Trivial test notebook: an interest-rate slider driving a numpy compound-interest calc. PEP 723 inline deps (sandbox style). |
| `capture.mjs` | Playwright harness. Loads the export, waits for the computed output, logs all network traffic, drives the slider, screenshots. Runs `online` (log traffic) or `offline` (abort every non-localhost request — a stricter test than "Wi-Fi off"). |
| `package.json` / `package-lock.json` | Pins Playwright. |
| `dist/` | *(gitignored)* the WASM export. Regenerate with the command below. |
| `.venv/`, `node_modules/` | *(gitignored)* toolchains. |

## Reproduce

```bash
cd experiments/offline-folder

# 1. Python env + marimo
uv venv --python 3.12 .venv
uv pip install --python .venv/bin/python marimo numpy

# 2. Export the notebook to a WASM-HTML folder
.venv/bin/marimo export html-wasm notebook.py -o dist --mode run

# 3. Node env for the test harness
npm install
npx playwright install chromium webkit

# 4. Serve + verify ONLINE (proves the export works, logs what it fetches)
.venv/bin/python -m http.server 8123 --directory dist &
node capture.mjs chromium online
```

---

## Findings so far

### 1. The export already self-vendors marimo's frontend
`marimo export html-wasm` copies marimo's entire JS/CSS frontend bundle into `dist/assets/` (~27 MB, ~680 files). So of the **three vendoring layers** the spec calls out, layer (a) — the marimo frontend — is handled for free by the export. Only the **Pyodide runtime** and the **wheels** are fetched remotely and need vendoring.

### 2. Exact remote dependency set: 18 files, 4 hosts
Captured by loading the export online and logging every request. Pyodide is **`v314.0.0`** (a Python 3.14 / ABI `2026_0` build).

| Host | What it serves |
|------|----------------|
| `cdn.jsdelivr.net/pyodide/v314.0.0/full/` | Pyodide core (`pyodide.asm.wasm`, `pyodide.asm.mjs`, `python_stdlib.zip`) + most wheels (numpy, micropip, msgspec, pyyaml, docutils, jedi, parso, packaging, pygments, pyodide_http) |
| `wasm.marimo.app` | `pyodide-lock.json` (the marimo-specific package lock) |
| `test-files.pythonhosted.org` (**TestPyPI**) | `marimo_base-0.23.10-py3-none-any.whl` |
| `files.pythonhosted.org` (PyPI) | `markdown`, `narwhals`, `pymdown_extensions` |

So the wheel set for this notebook = **marimo_base + its dependency closure + numpy**. The Pyodide *core* and *most base wheels* come from jsDelivr; only marimo itself and a few pure-Python deps come from (Test)PyPI.

### 3. The version-skew worry did NOT bite
The spec flagged that the frozen frontend version and the served wheel could drift. In practice the lock is requested as `pyodide-lock.json?v=0.23.10&pyodide=v314.0.0`, so it serves the **matching** `marimo_base-0.23.10`. (A raw fetch of the lock *without* the query returns an older `0.23.9` — so always pass the version query.)

### 4. WASM MIME is a non-issue locally
`python -m http.server` (Python 3.12) serves `.wasm` as `application/wasm`, so `WebAssembly.instantiateStreaming` is satisfied without any custom MIME handling. (The custom-protocol handler in later steps must replicate this.)

### 5. Loader strings to repoint for offline
The two worker bundles build remote URLs from these literals (must be rewritten to local paths, plus the lock's absolute (Test)PyPI URLs rewritten to local filenames):
- `dist/assets/worker-Bp53hInb.js` and `dist/assets/save-worker-Bcr7rl0C.js`
  - `` `https://cdn.jsdelivr.net/pyodide/v${…}/full/` `` → local `pyodide/` dir
  - `` `https://wasm.marimo.app/pyodide-lock.json?v=${…}&pyodide=${…}` `` → local lock

### Cross-origin isolation
As the spec predicted, single-threaded WASM means **no `SharedArrayBuffer`**, so no COOP/COEP headers were needed — the export loads and runs without cross-origin isolation. (To be re-confirmed under the custom protocol in step 3.)

---

## Open items / next steps

1. **Vendor** the 18 files into `dist/pyodide/`; rewrite the lock's absolute URLs → local filenames; patch the two worker bundles to local paths.
2. **Offline proof:** run `node capture.mjs chromium offline` and `node capture.mjs webkit offline` — both must render with **zero** non-local requests reaching the network (the harness aborts them and logs any attempt).
3. **Fix the slider selector** in `capture.mjs` — marimo's slider is a custom component, not a raw `input[type="range"]`, so the interactivity assertion currently errors (initial render is verified; live recompute is not yet).
4. **Note the version floor:** the runtime targets bleeding-edge **Python 3.14 / Pyodide v314**. Worth tracking as an aggressive minimum.
