# Offline Marimo Notebook Player — Build Specification

*(Working title — the product name is still undecided. The file extension is **`.mnote`**, finalized 2026-06-23.)*

**A desktop app that opens marimo notebooks by double-click, fully offline, with live interactivity.**

This document is a build brief for an implementing agent (Claude Code). It contains the full context, the architecture decision, the package-resolution design, a staged build plan that derisks the hard part first, and the known gotchas. Read it top to bottom before scaffolding anything.

> **✅ File extension: `.mnote`** *(finalized 2026-06-23).*
> Use `.mnote` everywhere — the payload extension, the file-association manifest, and the constant in the player. It remains a config value, not an architectural one; changing it later is still a contained find-and-replace.
>
> Why `.mnote` satisfies the original decision criteria:
> - **Available** — checked against the extension databases (fileinfo.com, file-extensions.org, file.org): no current software claims `.mnote` (nearest hits are unrelated formats like `.motn` and `.mno`). Clean to claim.
> - **Not `.py`** — signals "a document you open," not intimidating "source code," and avoids being hijacked by an installed editor/Python. The internal payload is still a marimo `.py` the user never sees (same way a `.docx` is secretly a zip of XML).
> - **Brand-neutral enough** — reads as a generic "note" format and does not spell out or obviously reference marimo. *(Minor: an insider might read the leading `m` as "marimo" — judged acceptable, and reversible if it ever matters.)*
> - Lowercase, letters only, pleasant to read.

---

## 1. Goal

Build an open-source desktop application that lets a user **double-click a `.mnote` file and have a marimo notebook open as a live, interactive document — with no terminal, no hosting, and no internet connection required.**

The intended end-user flow:

1. An author creates a marimo notebook and bundles it as a `.mnote` file.
2. A recipient installs this player app **once**.
3. From then on, the recipient double-clicks any `.mnote` file and it opens live and offline. Sliders, inputs, and recomputation all work.

This is acceptable and intended: the runtime lives in the installed app, not in the file. The file is a portable payload; the player supplies the runtime. (This is the same bargain as Wolfram's old CDF Player, but built on open standards — HTML/WASM/Python — instead of a proprietary engine.)

---

## 2. Background: the two problems we are solving

marimo can already export a notebook to a self-contained WASM HTML file (`marimo export html-wasm`). That file runs Python in the browser via Pyodide. But it has two limitations that block the "double-click a local file" experience:

**Problem A — `file://` has no HTTP origin.**
Double-clicking an HTML file opens it under the `file://` scheme, which the browser treats as a null/opaque origin with no headers. This breaks the things the WASM app needs: ES module loading (CORS-blocked from a null origin), `fetch()` of sibling files (blocked, especially in Chrome), Web Workers (can't spawn from `file://`), and correct `application/wasm` MIME typing (no headers to declare it). **Fix: serve over a real origin (HTTP, or a custom secure protocol). Any real origin satisfies the browser — internet not required.**

**Problem B — the runtime and wheels are fetched from a CDN at load time.**
The exported HTML is lightweight because it does *not* embed the heavy parts. The Pyodide runtime (tens of MB) and Python package wheels are pulled from a remote CDN when the page loads. With no internet, initialization fails. **Fix: vendor Pyodide and the wheels locally so nothing is fetched remotely.**

These two problems are independent. Problem A is about *origin*, Problem B is about *bundled assets*. The architecture below solves both.

---

## 3. Architecture decision

### Do NOT fork Chromium.

A Chromium fork would mean maintaining a divergent copy of a massive codebase and chasing its security patches forever, just to alter the same-origin policy — which would punch a hole in the exact boundary that keeps the engine safe, and *still* wouldn't solve Problem B. Wrong tool. Reject this approach.

### Instead: wrap a native webview and supply what it asks for.

The elegant path uses two insights:

**Insight 1 — a custom secure protocol replaces the local web server AND the CDN interception.**
Both Tauri and Electron let you register a custom URI scheme (e.g. `app://` (placeholder scheme name)) and mark it as a **secure, standard** origin. A page served over that scheme is a trusted context with full access to ES modules, Web Workers, `fetch`, and streaming WASM. This solves Problem A with no separate localhost HTTP server and no port management — the protocol handler *is* the server, built into the webview. Because everything it serves comes from the app's bundled asset folder, it also contributes to solving Problem B (offline by construction). **Confirm this per-platform before relying on it:** native webviews are not identical — WKWebView (macOS) and WebView2 (Windows) differ, and Tauri's custom-protocol origin string itself differs (`scheme://localhost` on macOS vs `http://scheme.localhost` on Windows). marimo runs Pyodide in a Web Worker, so verify worker spawning + streaming WASM over the custom scheme on **each** target OS. Treat this as a second real unknown, not a given (see §6).

**Insight 2 — the app owns the runtime; the `.mnote` is just the notebook payload.**
Do not post-process each exported HTML file to rewrite CDN links. Flip the model: the **player bundles marimo's WASM frontend + Pyodide + a base wheel set once**, into the app. The `.mnote` is then primarily the notebook itself — ideally a marimo `.py` notebook authored with `--sandbox` so it carries its dependency list inline (PEP 723). The player reads the declared deps, resolves the wheels locally, and loads the notebook into its own runtime. This keeps `.mnote` files tiny and self-describing, eliminates per-file URL rewriting, and mirrors how marimo.app / molab already work (host provides runtime, notebook is payload) — just packaged for offline.

**Note that marimo itself runs *inside* Pyodide** — the kernel is a Python package executing in the WASM runtime. So "the app bundles the runtime" means bundling **three coupled layers**: (a) marimo's frontend JS/CSS bundle, (b) the Pyodide runtime, and (c) a wheel set that **includes the `marimo` wheel and its dependencies**, not just numpy/pandas. These three are version-locked: the player ships a frozen marimo+Pyodide tuple, and a notebook authored against a newer marimo runs against the bundled version. Make that a conscious decision, not an accident.

### Recommended stack: **Tauri** (not Electron)

- Tauri uses the OS's **native** webview (WebView2 on Windows, WebKit on macOS — WebKitGTK on Linux if added later; see §8), so the *shell* binary is a few MB — not a second copy of a browser. **But the vendored runtime is the real weight:** Pyodide core plus a tier-1 wheel set (pandas, scipy, scikit-learn, matplotlib are each tens of MB) realistically makes a **~150–250 MB installer**. Budget for this honestly — it argues for vendoring tier-1 wheels on disk but lazy-loading them into the runtime on first import, not eagerly on every open.
- First-class support for custom protocols, file associations, and "open file" events.
- Emits native installers per OS (`.dmg` for macOS, `.msi`/`.exe` for Windows) out of the box; `.AppImage`/`.deb` are available if Linux is added later.
- You write very little Rust — essentially a custom-protocol handler and a file-open handler.

Electron is a valid fallback (identical rendering everywhere) but costs ~100MB+ for a bundled Chromium you don't need.

**Bonus simplification:** marimo's WASM mode is single-threaded (no threading/multiprocessing). That means **no `SharedArrayBuffer` requirement**, which means you can **skip the COOP/COEP cross-origin-isolation header dance** that normally plagues Pyodide deployments. Do not waste time on cross-origin isolation; it isn't needed.

---

## 4. The `.mnote` file format

- At its core, a `.mnote` is a marimo notebook (`.py`, authored with `--sandbox` so dependencies are declared inline via PEP 723).
- It MAY optionally bundle wheels for exotic packages it needs (see tier 2 below). A reasonable container is a zip with a documented layout (e.g. `notebook.py` + `wheels/` + a small `manifest.json`), but the simplest viable v1 is just the sandboxed `.py` with no bundled wheels. Decide the container format early and document it.
- The file must be **portable across OSes**. Only the *player* is per-platform; the file is not.

---

## 5. Package resolution: three-tier hybrid

When a notebook needs a package, the player checks tiers in order and stops at the first hit.

1. **Baked into the app** — **`marimo` itself and its dependencies** (required — the kernel runs in Pyodide), plus the common scientific set: `numpy`, `pandas`, `matplotlib`, `scipy`, `scikit-learn` (tune this list). Vendored on disk so they're always offline. Note "always present" means *present on disk*, not loaded into every runtime: **lazy-load each wheel on first import** so opening a notebook doesn't pay the cost of scipy+sklearn it never uses. Covers the majority of notebooks.
2. **Bundled in the `.mnote`** — if a file declares an exotic package, it can carry that wheel inside itself. Player finds it locally; still fully offline.
3. **Fetched on demand** — if a package is in neither place, download once via `micropip` and cache it. Online only for that first fetch; offline forever after. **Bounded by Pyodide:** this only works for pure-Python wheels and packages in Pyodide's own distribution — arbitrary PyPI packages with C extensions Pyodide hasn't built will *not* install, so surface that clearly rather than failing mid-run. Treat the tier-3 fetch as **gated by user confirmation** (see §7, Security model): an untrusted file should never silently trigger a download.

### Why this is low-effort
- Pyodide's `micropip` already does "fetch a wheel and install it" — that's tier 3, mostly **already built**.
- marimo's WASM mode already calls `micropip` for missing imports.
- Your real work is inserting tiers 1 and 2 **ahead** of tier 3: point the resolver at local caches first, fall through to network only if both miss.

### Resolver shape
A small function: `resolve(package_name, version) -> wheel`, trying **app cache → file bundle → download**, returning the first wheel found.

### Two things to build in from the start (annoying to retrofit)
- **One shared cache, keyed by package name + version.** Tier-2 bundles and tier-3 downloads land in the same store, so a package fetched for one notebook is instantly available to the next. Version-keying avoids two notebooks silently colliding on different versions of the same lib.
- **Up-front dependency resolution.** Because a sandboxed `.mnote` declares its full dependency list inline, resolve everything *before* running cells. Then either proceed fully offline, or clearly tell the user "this notebook needs X — one-time download" before execution. No mid-run cell death because a wheel was missing.

### Visible state indicator
Show whether a file is "instantly offline" or "needs a quick download." A small "preparing — downloading N packages" line on first open, then nothing on subsequent opens. Keeps it honest and unsurprising.

---

## 6. Staged build plan (derisk the hard parts first)

**Do not build the polished version first.** Prove the uncertain pieces in isolation before polishing. But there are **three** real unknowns, not one — and two of them hide inside steps that look like boilerplate. Offline vendoring is only the first; loading an arbitrary notebook into the app-owned runtime, and the custom protocol behaving correctly inside native webviews, are the other two. Spike each before assuming it's free.

1. **Offline-in-a-folder (critical experiment #1 — offline vendoring).**
   Take one `marimo export html-wasm` output. Vendor all three layers it needs — marimo's frontend bundle, the Pyodide runtime, and the wheels (incl. the `marimo` wheel itself) — and rewrite references to relative paths. Serve the folder with any dumb static server (`python -m http.server`) and confirm the slider recomputes with **no network**.
   - ⚠️ **Disabling Wi-Fi is not a reliable offline test.** The browser may serve CDN assets cached from an earlier online run, giving a false pass. Use a fresh browser profile / hard cache-clear and confirm **zero** non-local requests in the DevTools Network panel.
   - **Validate on both engines early.** Run this proof in **WebKit (macOS)** and **Chromium (Windows WebView2)** — different WASM/worker implementations, and this is the cheapest place to catch an engine-specific failure before it's buried under the Tauri shell (see §8).
   - **Also lock the `.mnote` container format before Step 4** (it's painful to retrofit): bare `.py` vs a zip with `notebook.py` + `wheels/` + `manifest.json`. It dictates the resolver and the open-file path. Simplest viable v1 is the sandboxed `.py`; decide and document now, not during polish.

2. **Wrap it.**
   Drop that working folder into a Tauri shell that opens a webview to it. Now it's a double-clickable app instead of a terminal command.

3. **Swap static serving for the `app://` (placeholder scheme name) custom protocol — critical experiment #2.**
   Removes the bundled server and gives a clean secure origin. **This is a real unknown, not a swap:** confirm Web Workers + streaming WASM + `fetch` all work over the custom scheme on each target OS (see §3, Insight 1). Expect to fight Tauri's default CSP here — Pyodide needs `script-src 'wasm-unsafe-eval'` to compile and run WASM, and the stock strict CSP will block it.

4. **Add the `.mnote` file association + open-file handler — critical experiment #3.**
   Switch from "runtime opens a baked-in notebook" to "runtime **injects** whichever notebook was double-clicked into the bundled kernel." **This is the second hidden unknown:** feeding a raw `.py` into a pre-bundled marimo-wasm runtime is a different mechanism than serving a pre-baked export, and is *not* validated by Step 1 — spike it separately. Also handle the macOS gotcha: a double-click arrives as an Apple Event that can fire **before** the webview is ready, so buffer the requested path until the frontend signals ready (Windows passes it via argv). **This step delivers the core vision.**

5. **Polish.**
   - Ship the base wheel set (tier 1), lazy-loaded on first import.
   - Implement the three-tier resolver + shared version-keyed cache.
   - Add graceful fallback: try local cache, fetch-and-cache once if online (gated by user confirmation), show a clear message if a needed wheel is missing offline.
   - Add the state indicator.

---

## 7. Security model

This product is, structurally, **"double-click a file from someone and it runs their Python."** §1 deliberately makes the file read as a benign *document*, not as code — which is exactly the social-engineering shape that demands an explicit trust model. Settle these before shipping to anyone but yourself:

- **What Pyodide does and doesn't sandbox.** Pyodide runs in the webview's sandbox: no real filesystem, no subprocesses, no threading. That contains a lot. It does **not** by itself stop network egress (`fetch` / `micropip`), CPU abuse, or convincing in-document UI.
- **Network egress policy (decide explicitly).** A notebook can `fetch()` and exfiltrate or phone home. Lock this down at the webview layer via CSP / the Tauri allowlist. Default to **no notebook-initiated network**, with tier-3 wheel fetches as the one controlled exception.
- **Tier-3 downloads are driven by untrusted files.** A `.mnote` declares its deps and the player fetches them. Gate this behind the "this notebook needs X — one-time download" confirmation (§5): it is both good UX *and* the security boundary. Never silently auto-download because a file asked.
- **UI spoofing.** A live notebook renders arbitrary UI inside what the user perceives as a trusted viewer (fake dialogs, phishing forms). Consider a persistent, non-spoofable window chrome that marks content as untrusted.
- **Open-source / anonymous release doesn't change any of the above.** Bundled deps (marimo Apache-2.0, Pyodide, Tauri) still carry attribution / NOTICE obligations even when released anonymously.

None of this needs heavy machinery for a v1 aimed at your own machines or a classroom. But the moment the audience is "files from strangers," the egress policy and the tier-3 gate are the load-bearing controls — design them in, don't bolt them on.

---

## 8. Platform support: macOS + Windows (Linux deferred)

**Initial targets are macOS and Windows. Linux is explicitly deferred** — a reversible, additive decision (see "Keeping the Linux door open" below). Dropping Linux removes the riskiest engine (version-fragmented WebKitGTK, the most likely place Pyodide/WASM misbehaves) and the worst packaging case (system `libwebkit2gtk` dependency, AppImage). It does **not** collapse the work to one platform, because:

**You still have two engines.** macOS WKWebView is **WebKit**; Windows WebView2 is **Chromium**. Different JS/WASM/worker implementations, so "works on Mac" still does not imply "works on Windows." The custom-protocol + streaming-WASM + Web Worker behavior must be validated on both — which is why critical-experiment #1 (§6) should run on both engines.

### File association (the main per-OS work)

| | macOS | Windows |
|---|---|---|
| **Register** | UTI in Info.plist (`CFBundleDocumentTypes` + an exported UTI for `.mnote`) | Registry (`HKCR\.mnote`, ProgID, `shell\open\command`), written by the installer |
| **Path arrives as** | Apple Event — can fire *before* the webview is ready | `argv[1]` |
| **App already running** | event routed to existing instance | new process spawns → must forward |

Tauri v2's `bundle.fileAssociations` config generates both the Info.plist entries and the Windows registry keys. The runtime work you still own:
- **Single-instance + path forwarding** (`tauri-plugin-single-instance`) so a second double-click reuses the running app instead of opening a duplicate.
- **macOS open-event buffering** — hold the requested path until the frontend signals ready (the §6 gotcha).

### Build & distribution

- **No cross-compiling.** Build each target on its own OS via a **CI matrix** (`macos-latest`, `windows-latest`). Two runners now; adding `ubuntu-latest` later is the bulk of re-adding Linux.
- **Installers:** `.dmg`/`.app` (macOS); `.msi` + NSIS `.exe` (Windows). Bundle the **WebView2 Evergreen bootstrapper** for older Windows machines that lack it.
- **Signing per OS:** macOS Developer ID cert + **notarization + stapling** (modern Gatekeeper hard-blocks un-notarized apps, doesn't just warn); Windows Authenticode (EV cert for instant SmartScreen reputation). Wire both into CI.

### The platform-agnostic part

The ~150–250 MB of Pyodide + wheels is WASM/JS/data — **identical bytes on both platforms** — carried via Tauri `resources`. Per-OS paths (the tier-3 cache, app data) go through Tauri's `app_cache_dir` / `app_data_dir`; never hardcode.

### Keeping the Linux door open (free insurance)

Two habits cost nothing on Mac/Windows and make a later Linux add trivial:
- **Exact-case asset paths** — Linux is case-sensitive; Mac/Windows are not, so a case mismatch passes locally and 404s only on Linux. Get it right from the start.
- **Use Tauri path APIs and Rust `PathBuf`** (never string-joined `/`) for all filesystem access.

Re-adding Linux later = WebKitGTK validation of experiment #1 + freedesktop `.desktop`/MIME registration + an `ubuntu-latest` CI runner. Nothing in a Mac+Windows design forecloses it.

---

## 9. Known caveats / non-goals

- **First-install friction is real but normal.** The first time, the recipient downloads and installs the app. Because it's a niche app, expect an "unidentified developer" warning unless code-signed (Apple ~$99/yr; Windows certs vary). Fine for friends/internal use; sign it for strangers or a product.
- **Cross-platform = build per OS.** The `.mnote` is portable; the player is not. Initial targets are macOS + Windows (Linux deferred); each is built on its own OS via a CI matrix — see §8 for the full breakdown.
- **Not a fit for "email a stranger a file they open with zero setup."** For that, host the WASM export and send a link. This player is for cases where a one-time install is acceptable (own machines, a classroom, an internal team, a kiosk, a paid product).
- **Package coverage is bounded by Pyodide.** Pure-Python wheels on PyPI plus the Pyodide-supported set work; some packages won't. No threading/multiprocessing. 2GB memory ceiling in WASM.

---

## 10. First task for the implementing agent

Start with **Step 1 only**: produce a working *offline-in-a-folder* proof. Concretely:

1. Create a trivial marimo notebook with a slider that drives a computed output (e.g. an interest-rate calculation) and one common dependency (e.g. numpy).
2. Export it with `marimo export html-wasm`.
3. Vendor all three layers — marimo's frontend bundle, the Pyodide runtime, and the required wheels (**including the `marimo` wheel itself**, since the kernel runs in Pyodide) — locally; rewrite references to relative paths.
4. Serve the folder with a static server and confirm it runs **with networking genuinely disabled** — use a fresh browser profile / cache so a prior online run can't mask a missing asset, and verify **zero** non-local requests in DevTools.
5. Report back what had to be vendored, any path/MIME issues hit, and whether single-threaded mode avoided cross-origin-isolation headers as expected.

Only proceed to Tauri wrapping (Step 2) once Step 1 is confirmed working offline.

---

## 11. Reference notes

- marimo WASM export: `marimo export html-wasm notebook.py -o <folder>` (supports `--mode edit` for a playground feel; default is app/run mode).
- Author share-ready notebooks with `marimo edit --sandbox notebook.py` so dependencies are inlined (PEP 723) and resolvable up front.
- WASM is powered by Pyodide; preinstalled packages include numpy, scipy, scikit-learn, pandas, matplotlib. Missing packages are installed via `micropip` at runtime — this is the hook tier 3 builds on.
- WASM HTML "must be served over HTTP, cannot be opened from `file://`" — this is exactly the constraint the custom protocol resolves.
