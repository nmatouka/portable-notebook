# Portable Notebook

A desktop app that opens [marimo](https://marimo.io) notebooks by **double-click — fully offline, with live interactivity**. No terminal, no hosting, no internet required.

An author bundles a notebook as a single file; a recipient installs this player **once**, then double-clicks any such file and it runs live and offline (sliders, inputs, and recomputation all work). The runtime lives in the installed app; the file is just the portable notebook payload — the same bargain as Wolfram's old CDF Player, but built on open standards (HTML / WASM / Python via [Pyodide](https://pyodide.org)) instead of a proprietary engine.

> **Status: early. No app yet.** The repo currently holds the build specification and the first derisking experiment. See below.

## How it works (the short version)

A naive "double-click the HTML export" approach is blocked by two independent problems, both solved by the architecture in the spec:

- **Origin** — `file://` is a null origin, which breaks ES modules, `fetch`, Web Workers, and WASM MIME. *Fix: serve over a custom secure protocol inside a native webview.*
- **Remote assets** — marimo's WASM export pulls Pyodide and Python wheels from a CDN at load. *Fix: vendor Pyodide + a base wheel set into the app.*

Recommended stack is **Tauri** (native webview, small shell binary). Initial targets are **macOS + Windows**; Linux is deferred (reversible). The full rationale, package-resolution design, security model, and staged build plan are in the spec.

## Repository layout

| Path | What it is |
|------|------------|
| [`offline-marimo-player-spec.md`](offline-marimo-player-spec.md) | The build specification — read this first. Architecture, file format, 3-tier package resolution, security model, platform support, and the staged build plan. |
| [`experiments/offline-folder/`](experiments/offline-folder/) | **Critical experiment #1** — proving a marimo WASM export can run with no network. See its [README](experiments/offline-folder/README.md). |

## Build plan, at a glance

The spec stages the work to derisk the genuinely uncertain parts first:

1. **Offline-in-a-folder** — prove Pyodide + wheels load with no network. *(✅ passed — see the experiment)*
2. Wrap the working folder in a Tauri shell. *(✅ done)*
3. Replace static serving with a custom secure protocol. *(✅ done — `mnote://`)*
4. Add the file association + open-file handler (loads whichever notebook was double-clicked). *(✅ done — verified on macOS)*
5. Polish — base wheel set, 3-tier resolver + shared cache, graceful fallback, state indicator. *(next)*

## Status

- ✅ Spec drafted and reviewed (scope: macOS + Windows).
- ✅ Experiment #1 — marimo WASM export runs **fully offline** (load + live slider recompute, zero network) on both Chromium and WebKit. See the [experiment write-up](experiments/offline-folder/README.md).
- ✅ Step 2 — wrapped in a [Tauri shell](app/): the built `.app` opens the notebook in a native WebKit window, numpy computes, slider drives live recompute — all offline.
- ✅ Steps 3 & 4 — the [app](app/) serves the embedded frontend over a dedicated **`mnote://` custom protocol** and **opens a double-clicked `.mnote`** by injecting its source into the runtime. Verified on macOS: bare launch shows a default notebook; double-click (or open) a `.mnote` and it runs live (e.g. a Celsius→Fahrenheit notebook computing `68.0 °F`). **This is the core vision working end-to-end.**
- ⏭️ Next: Step 5 polish — the 3-tier package resolver + shared cache, the security model (§7), CSP tightening, and a Windows/WebView2 build.

The product name is still undecided. The file extension is **`.mnote`** (finalized 2026-06-23) — a config value (file-association manifest + a constant in the player), not an architectural choice.
