# Carrel

[![CI](https://github.com/nmatouka/portable-notebook/actions/workflows/ci.yml/badge.svg)](https://github.com/nmatouka/portable-notebook/actions/workflows/ci.yml)

**Carrel** opens [marimo](https://marimo.io) notebooks by **double-click — fully offline, with live interactivity**. No terminal, no hosting, no internet required.

An author bundles a notebook as a single `.mnote` file; a recipient installs Carrel **once**, then double-clicks any `.mnote` and it runs live and offline (sliders, inputs, and recomputation all work). The runtime lives in the installed app; the file is just the portable payload — the same bargain as Wolfram's old CDF Player, but built on open standards (HTML / WASM / Python via [Pyodide](https://pyodide.org)) instead of a proprietary engine.

> **Try it:** a notebook running entirely in your browser via WebAssembly — the engine Carrel makes work *offline* — is live at **https://nmatouka.github.io/portable-notebook/**. The desktop app and full status are below.

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
5. Polish — base wheel set ✅, **3-tier resolver + shared cache ✅**, security model ✅. Remaining: download-progress indicator, CSP hardening, Windows build.

## Status

- ✅ Spec drafted and reviewed (scope: macOS + Windows).
- ✅ Experiment #1 — marimo WASM export runs **fully offline** (load + live slider recompute, zero network) on both Chromium and WebKit. See the [experiment write-up](experiments/offline-folder/README.md).
- ✅ Step 2 — wrapped in a [Tauri shell](app/): the built `.app` opens the notebook in a native WebKit window, numpy computes, slider drives live recompute — all offline.
- ✅ Steps 3 & 4 — the [app](app/) serves the embedded frontend over a dedicated **`mnote://` custom protocol** and **opens a double-clicked `.mnote`** by injecting its source into the runtime. Verified on macOS: bare launch shows a default notebook; double-click (or open) a `.mnote` and it runs live (e.g. a Celsius→Fahrenheit notebook computing `68.0 °F`). **This is the core vision working end-to-end.**
- ✅ Step 5 (security model, §7) — the player **denies notebook network egress** via a strict CSP (`connect-src 'self'`), verified to block exfiltration from both the document and the Pyodide worker. Runtime is sandboxed (Pyodide/WASM), serving is read-only/local, the notebook gets no host bridge, and the title bar shows the open file. See the [app security model](app/README.md#security-model-spec-7).
- ✅ Step 5 (package resolution, §5) — **all three tiers**: baked, bundled in the `.mnote` ([`tools/mnote-pack.py`](tools/mnote-pack.py)), and **on-demand PyPI download** by the Rust backend (gated by a one-time confirmation, cached for offline reuse). Verified on macOS: a `cowsay` notebook downloads its wheel on first open (sha256-checked) and runs from cache offline thereafter; the webview never touches PyPI (the CSP forbids it).
- 🔄 Cross-platform: the app is now **origin-agnostic** (works under both WebKit's `mnote://localhost` and WebView2's `http://mnote.localhost`), with `tauri-plugin-single-instance` forwarding double-clicked files on Windows/Linux; a [CI matrix](.github/workflows/ci.yml) compiles it on macOS **and** Windows — **both green**. Windows *runtime* verification (on a real WebView2 machine) and installer bundling are still pending.
- ✅ Polish — CSP `script-src` hardened to a per-response **nonce** (no `'unsafe-inline'`, verified marimo still runs); the title bar follows the open file (incl. cold opens) via `on_page_load`; a tier-3 download shows "Downloading N package(s)…" in the title.
- ⏭️ Remaining: Windows *runtime* verification + installer bundling (need a Windows machine); minor items (tier-3 version solving, WebKit font 404s).

The product name is still undecided. The file extension is **`.mnote`** (finalized 2026-06-23) — a config value (file-association manifest + a constant in the player), not an architectural choice.

## Live demo

A notebook running entirely in the browser via WebAssembly — the same engine the desktop player makes work *offline* — is published to GitHub Pages from [`site/`](site/) by the [Pages workflow](.github/workflows/pages.yml): **https://nmatouka.github.io/portable-notebook/**

## License

This project's own source is **source-available under [PolyForm Noncommercial 1.0.0](LICENSE)** — free to use, modify, and share for any noncommercial purpose; commercial use is not granted.

> Note: strictly, a "no commercial use" license is *not* OSI-approved "open source" (the [Open Source Definition](https://opensource.org/osd) forbids field-of-use restrictions). PolyForm Noncommercial is the standard, well-drafted choice for *source-available, noncommercial* software. If you'd rather it be true open source (which allows commercial use), MIT/Apache-2.0 is the swap.

Bundled and built-upon components (marimo, Pyodide, Tauri, Python wheels, …) keep their own — generally permissive — licenses; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
