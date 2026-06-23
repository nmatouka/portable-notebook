# mnote Player (Tauri shell) — Step 2

Wraps the proven, vendored offline marimo export (from [experiment #1](../experiments/offline-folder/)) in a [Tauri v2](https://tauri.app) native-webview shell, so the notebook opens as a **double-clickable app** instead of a terminal `http.server` command.

> **Status: Step 2 — ✅ verified** ([build plan](../offline-marimo-player-spec.md)). The built `.app` opens the notebook in a native macOS WebKit window; numpy computes and the slider drives live recompute (`$1,628.89` at 5% → `$5,233.84` at 18%) — all offline, served over Tauri's `tauri://localhost` asset protocol. This shell loads a single baked-in notebook; loading an arbitrary double-clicked `.mnote` (Step 4) and a dedicated `app://` protocol (Step 3) are not here yet.
>
> **Bonus finding:** because Tauri's asset protocol *is* a custom scheme, this already answers the Step-3 unknown for macOS/WebKit — **Web Workers + streaming WASM work over it**, with the `${self.location.origin}` rewrite from experiment #1 adapting to the `tauri://` origin unchanged. (Still to confirm on Windows/WebView2.)

## Layout

```
app/
  src-tauri/
    Cargo.toml          Rust deps (tauri v2)
    build.rs            tauri-build
    tauri.conf.json     window + frontendDist + bundle config
    src/main.rs         minimal shell: open a webview onto the frontend
    capabilities/       Tauri v2 ACL (core defaults only — no IPC yet)
    icons/              app icons (generated from marimo's logo)
  frontend/             (gitignored) the vendored export — see sync-frontend.sh
  sync-frontend.sh      copies experiments/offline-folder/dist -> frontend/
```

`frontend/` is the **exact** vendored folder proven offline in experiment #1, copied in wholesale. Tauri serves it through its built-in asset protocol (`tauri://localhost` on macOS, `http://tauri.localhost` on Windows) — which conveniently also exercises the Step-3 question early: *do Web Workers + streaming WASM work over a custom protocol?*

The vendored worker rewrite uses `${self.location.origin}`, so it adapts to Tauri's protocol origin with no change (the payoff from experiment #1's portability fix).

## Build & run

Requires the Rust toolchain (`rustup`) and Xcode CLT on macOS.

```bash
# 1. produce the vendored export (once), then copy it in
../experiments/offline-folder/vendor.sh      # if not already vendored
./sync-frontend.sh

# 2. run in dev (opens the window)
cd src-tauri && cargo run
#   or, via the Tauri CLI:    npx @tauri-apps/cli@2 dev

# 3. produce installers (.app / .dmg on macOS)
npx @tauri-apps/cli@2 build
```

## Config notes

- **CSP is disabled** (`app.security.csp: null`) for now so Pyodide's WASM compile/eval and workers load freely. Step 3/polish will tighten this to an explicit policy including `script-src 'wasm-unsafe-eval'` (the gotcha the spec flags).
- **No IPC yet** — the frontend is a self-contained notebook, so the capability grants only `core:default`.
- **identifier** `app.mnote.player` — placeholder; revisit before any public release.
