# mnote Player (Tauri app)

Opens a double-clicked **`.mnote`** file as a live, interactive marimo notebook — fully offline, in a native webview. Built on the vendored offline runtime proven in [experiment #1](../experiments/offline-folder/).

> **Status: Steps 2–4 complete & verified on macOS** ([build plan](../offline-marimo-player-spec.md)). Launch bare → a default notebook; double-click a `.mnote` (or `open -a "mnote Player" file.mnote`) → that notebook opens live. All three paths verified by screenshot: bare launch (`$1,628.89`), open-in-running-app, and cold double-click via the file association. Windows/WebView2 not yet built.

## How it works

A `.mnote` is simply a **marimo `.py` notebook** (UTF-8 source). The player owns the runtime; the file is just the payload.

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
  • index.html → inject current notebook source into <marimo-code> (percent-encoded)
  • /assets/*, /_vendor/*  → embedded bytes (Pyodide, wheels, marimo frontend)
        │
        ▼
marimo-wasm boots Pyodide, runs the injected notebook — offline.
```

Key points:
- **One baked runtime plays any notebook.** Injecting the source into `<marimo-code>` server-side avoids re-exporting per file (the [injection mechanism](../experiments/offline-folder/inject.py) was de-risked first).
- **`mnote://` custom protocol** (Step 3) serves the whole frontend from a compile-time `include_dir!` embed, with correct MIME (incl. `application/wasm`). This also re-confirms Web Workers + streaming WASM work over a custom scheme.
- The vendored loader base is pinned to `mnote://localhost` (by [sync-frontend.sh](sync-frontend.sh)) so `new URL(wheel, base)` always has a valid absolute base under the custom scheme.

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

## Not done yet
- Windows/WebView2 build + the registry-based file association + single-instance forwarding.
- Tighten CSP to an explicit `wasm-unsafe-eval` policy (currently disabled).
- Package-resolution tiers / `.mnote` zip container with bundled wheels (spec §5).
- Cosmetic: WebKit font 404s (experiment finding #8); window title doesn't reflect the open file.
