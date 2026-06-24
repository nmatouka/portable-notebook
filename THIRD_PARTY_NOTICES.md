# Third-party notices

The [LICENSE](LICENSE) (PolyForm Noncommercial 1.0.0) covers **this project's own
source**. It does **not** relicense the third-party software the player bundles or
builds upon — each component remains under its own license, which is generally
permissive (Apache-2.0 / MIT / BSD / MPL-2.0) and compatible with redistribution.

Most of these are **not committed to this repository** — the offline frontend
(`app/frontend/`) is generated at build time (see `app/sync-frontend.sh`) and
embedded into the binary. They are listed here because **distributed builds bundle
them**, and anyone redistributing a build must carry the corresponding notices.

## Bundled into the player / runtime

| Component | Role | License |
|-----------|------|---------|
| [marimo](https://github.com/marimo-team/marimo) | notebook kernel + web frontend (runs in Pyodide) | Apache-2.0 |
| [Pyodide](https://github.com/pyodide/pyodide) | CPython compiled to WebAssembly | MPL-2.0 |
| CPython standard library | shipped inside Pyodide | PSF License |
| [NumPy](https://numpy.org) | bundled wheel (tier-1) | BSD-3-Clause |
| micropip, packaging, pyyaml, docutils, jedi, parso, pygments, msgspec, pyodide-http | bundled wheels (deps of the above) | various (BSD / MIT / Apache-2.0 / PSF) |
| [KaTeX](https://katex.org) | math rendering in the marimo frontend | MIT |
| Fira Mono, PT Sans, Lora | bundled fonts | SIL Open Font License 1.1 |

Packages a notebook fetches on demand (tier-3) or carries itself (tier-2) are the
respective authors' work under their own licenses; this project only caches and
serves them.

## Build / desktop shell

| Component | Role | License |
|-----------|------|---------|
| [Tauri](https://github.com/tauri-apps/tauri) (+ `wry`, `tao`) | native-webview app shell | MIT or Apache-2.0 |
| Rust crates: `include_dir`, `percent-encoding`, `url`, `zip`, `sha2`, `getrandom`, `ureq`, `serde`, `serde_json`, `tauri-plugin-dialog`, `tauri-plugin-single-instance` | see `app/src-tauri/Cargo.lock` | mostly MIT or Apache-2.0 |
| System WebView (WebKit on macOS/Linux, WebView2 on Windows) | provided by the OS, not bundled | — |

Full license texts are available from each project. When you distribute a built
artifact (e.g. an installer or the WASM export), include the notices required by
these licenses (notably the Apache-2.0 NOTICE for marimo and the MPL-2.0 source
availability for Pyodide).
