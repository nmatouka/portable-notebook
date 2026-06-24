# mnote Player (Tauri app)

Opens a double-clicked **`.mnote`** file as a live, interactive marimo notebook — fully offline, in a native webview. Built on the vendored offline runtime proven in [experiment #1](../experiments/offline-folder/).

> **Status: Steps 2–4 complete & verified on macOS** ([build plan](../offline-marimo-player-spec.md)). Launch bare → a default notebook; double-click a `.mnote` (or `open -a "mnote Player" file.mnote`) → that notebook opens live. All three paths verified by screenshot: bare launch (`$1,628.89`), open-in-running-app, and cold double-click via the file association. Windows/WebView2 not yet built.

## How it works

A `.mnote` is either a bare **marimo `.py` notebook**, or a **zip** bundling `notebook.py` + `wheels/` + a `manifest.json` of lock entries (for pure-Python packages beyond the baked set — spec §5 tier 2). The player owns the runtime; the file is just the payload. Build a bundle with [`tools/mnote-pack.py`](../tools/mnote-pack.py).

```
double-click foo.mnote
        │
        ▼
macOS Apple Event (RunEvent::Opened)  ─┐   argv[1] on Windows/Linux
        │                              │
        ▼                              ▼
   read file ──► store as "current notebook" ──► reload webview
        │
        ▼
window loads  mnote://localhost/
        │
        ▼
custom-protocol handler serves the EMBEDDED frontend:
  • index.html        → inject current notebook source into <marimo-code> (percent-encoded)
  • pyodide-lock.json → merge in the .mnote's bundled-package entries (tier 2)
  • /_pkg/*           → wheels bundled in the open .mnote (tier 2)
  • /assets/*, /_vendor/* → embedded bytes (Pyodide, baked wheels, marimo frontend)
        │
        ▼
marimo-wasm boots Pyodide, runs the injected notebook — offline.
```

Key points:
- **One baked runtime plays any notebook.** Injecting the source into `<marimo-code>` server-side avoids re-exporting per file (the [injection mechanism](../experiments/offline-folder/inject.py) was de-risked first).
- **`mnote://` custom protocol** (Step 3) serves the whole frontend from a compile-time `include_dir!` embed, with correct MIME (incl. `application/wasm`). This also re-confirms Web Workers + streaming WASM work over a custom scheme.
- The vendored loader base is pinned to `mnote://localhost` (by [sync-frontend.sh](sync-frontend.sh)) so `new URL(wheel, base)` always has a valid absolute base under the custom scheme.
- **Package resolution (spec §5).** Tier 1 (baked wheels) works out of the box. **Tier 2** is implemented: a `.mnote` zip carries extra pure-Python wheels, which the player merges into the served lock and serves from `/_pkg/`, so micropip installs them locally — offline, no PyPI. micropip checks the lock before the network, which is what makes this work. Tier 3 (on-demand download by the Rust backend, gated) is future work.

## Layout

```
app/
  src-tauri/
    src/main.rs         protocol handler + inject() + open-file (Opened/argv)
    tauri.conf.json     mnote:// window, .mnote fileAssociations, CSP off (Pyodide)
    Cargo.toml          tauri, include_dir, percent-encoding, url
    icons/
  default.mnote         notebook shown when launched without a file
  placeholder-dist/     unused stub (Tauri requires a frontendDist); real frontend is embedded
  frontend/             (gitignored) the vendored export — embedded via include_dir!
  sync-frontend.sh      copy experiments/offline-folder/dist -> frontend/ + pin loader base
```

## Build & run

Requires the Rust toolchain (`rustup`) and Xcode CLT.

```bash
../experiments/offline-folder/vendor.sh   # produce the vendored export (once)
./sync-frontend.sh                        # copy + pin it into frontend/ (embedded at build)

cd src-tauri && cargo run                 # dev run (opens default notebook)
npx @tauri-apps/cli@2 build --debug --bundles app   # build mnote Player.app
open "target/debug/bundle/macos/mnote Player.app" --args /path/to/foo.mnote
```

## Security model (spec §7)

A `.mnote` carries **untrusted code** (the recipient didn't write it) and is deliberately designed to look like a document — so network egress is denied by default.

**Enforced**
- **No network egress** — the load-bearing control. Every response from the `mnote://` handler carries a strict CSP whose key directive is `connect-src 'self'`, so a hostile notebook cannot `fetch()` out. Verified against this exact policy in the browser (experiment #1): exfiltration is refused from **both** the document and the Pyodide **worker** (where the notebook's Python runs). The legit runtime is unaffected — `'wasm-unsafe-eval'` (Pyodide) + `'unsafe-inline'` scripts (marimo bootstrap); JS `eval` is not needed — and the default notebook still computes under the CSP in the app.
- **Sandboxed runtime** — Pyodide/WASM: no real filesystem, no subprocesses, no threads.
- **Read-only, local-only serving** — the handler returns only bytes from the embedded frontend, with a `..` traversal guard; it never touches disk or network.
- **No host bridge** — the notebook gets only `core:default` Tauri capabilities; it cannot call the Rust backend.
- **Trust affordance** — the native title bar shows the open file name (`foo.mnote — mnote Player`), which page content cannot spoof.

**Known limitations / deferred**
- Under WebKit a CSP-blocked fetch can leave the promise *pending*, so a hostile (or network-dependent) notebook may **hang** instead of erroring. No data leaks; the UX is just poor — and notebooks that genuinely need the network aren't a fit for the offline player anyway.
- `script-src` still allows `'unsafe-inline'` (marimo's baked bootstrap). Hardening to nonces/hashes is future work and doesn't affect egress.
- When tier-3 package downloads land (spec §5), the **Rust backend** — not the notebook — will fetch them, gated by a one-time confirmation; the webview CSP stays strict.
- In-content UI spoofing (a notebook drawing fake dialogs) is mitigated only by the title bar.

Test fixture: [notebook-egress.mnote](../experiments/offline-folder/notebook-egress.mnote); CSP iteration used [csp-server.py](../experiments/offline-folder/csp-server.py).

## Not done yet
- **Tier-3** package resolution: the Rust backend downloads a missing wheel from PyPI once, caches it (shared, version-keyed), and serves it via `mnote://`, gated by a one-time confirmation. Tiers 1–2 are done.
- Windows/WebView2 build + the registry-based file association + single-instance forwarding.
- CSP hardening (nonces instead of `'unsafe-inline'`); graceful handling of network-blocked notebooks (vs hang).
- Cosmetic: WebKit font 404s; the title bar doesn't reflect the file on *cold* open (only when opened into an already-running window).
