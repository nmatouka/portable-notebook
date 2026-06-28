// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Opens a double-clicked .mnote. A .mnote is either a bare marimo .py, or a zip
// bundling notebook.py + wheels/ + manifest.json (spec §5 tier 2). The whole
// vendored frontend (marimo + Pyodide + baked wheels) is embedded and served over
// the mnote:// custom protocol; the current notebook's source is injected into
// <marimo-code>, and any wheels bundled in the .mnote are merged into the served
// pyodide-lock.json and served from /_pkg/ — so an exotic-but-pure-Python package
// loads fully offline, with the webview never touching PyPI.

mod resolver;

use include_dir::{include_dir, Dir};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Mutex, OnceLock};
use tauri::{http::Response, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

// Embedded at compile time → identical behavior in dev and bundled.
static FRONTEND: Dir = include_dir!("$CARGO_MANIFEST_DIR/../frontend");
const DEFAULT_NOTEBOOK: &str = include_str!("../../default.mnote");
const LOCK_PATH: &str = "_vendor/wasm.marimo.app/pyodide-lock.json";

// Security model (spec §7): notebook code is untrusted, so confine it to the app
// origin. `connect-src 'self'` blocks network egress from BOTH the document and
// the Pyodide worker (verified). Pyodide needs 'wasm-unsafe-eval'; marimo's
// bootstrap needs 'unsafe-inline'; JS eval is not required. Sent on every response.
// Origin-agnostic CSP (spec §7): `'self'` is whatever origin the custom protocol
// has on this engine (mnote://localhost on WebKit, http://mnote.localhost on
// WebView2). connect-src 'self' blocks network egress. Inline scripts run under a
// per-response nonce rather than 'unsafe-inline', so a notebook's rendered HTML
// can't execute inline JS in the app origin.
fn csp(nonce: Option<&str>) -> String {
    let script = match nonce {
        Some(n) => format!("'self' 'nonce-{n}' 'wasm-unsafe-eval'"),
        None => "'self' 'wasm-unsafe-eval'".to_string(),
    };
    // form-action 'self' stops a rendered <form> from POSTing to an external host
    // (form submission is NOT covered by connect-src). Navigation egress is blocked
    // separately by the window's on_navigation handler.
    format!(
        "default-src 'self'; script-src {script}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; \
         worker-src 'self' blob:; child-src 'self' blob:; form-action 'self'; \
         frame-ancestors 'none'; object-src 'none'; base-uri 'self'"
    )
}

/// A fresh CSP nonce. The browser hides the `nonce` attribute from the DOM, so
/// notebook code can't read it to forge a matching inline <script>.
fn script_nonce() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::getrandom(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Add `nonce="..."` to inline <script> tags (those without `src=`); external
/// scripts are covered by 'self'.
fn add_nonce(html: &str, nonce: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    while let Some(pos) = rest.find("<script") {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[..gt];
        if tag.contains("src=") {
            out.push_str(&rest[..=gt]);
        } else {
            out.push_str(tag);
            out.push_str(" nonce=\"");
            out.push_str(nonce);
            out.push_str("\">");
        }
        rest = &rest[gt + 1..];
    }
    out.push_str(rest);
    out
}

// Tauri serves a custom protocol under different origins per webview engine.
#[cfg(target_os = "windows")]
const WINDOW_URL: &str = "http://mnote.localhost/";
#[cfg(not(target_os = "windows"))]
const WINDOW_URL: &str = "mnote://localhost/";

/// The currently-open document: notebook source + any wheels bundled in the .mnote.
#[derive(Default)]
struct Doc {
    source: String,
    /// Opened file name for the title bar (empty = the default notebook).
    name: String,
    /// Lock entries (already pointed at /_pkg/) to merge into pyodide-lock.json.
    extra_packages: serde_json::Map<String, serde_json::Value>,
    /// Bundled wheel bytes, keyed by file basename, served from /_pkg/<base>.
    wheels: HashMap<String, Vec<u8>>,
}

/// Native title bar text: the open file (non-spoofable), or the product name.
fn window_title(name: &str) -> String {
    if name.is_empty() {
        "Carrel".to_string()
    } else {
        format!("{name} — Carrel")
    }
}
struct Current(Mutex<Doc>);

// Bounds to defuse a hostile .mnote (decompression-bomb / memory exhaustion).
const MAX_MNOTE_FILE: u64 = 512 * 1024 * 1024; // whole .mnote on disk
const MAX_TEXT: u64 = 16 * 1024 * 1024; // notebook.py / manifest.json (decompressed)
const MAX_WHEEL: u64 = 128 * 1024 * 1024; // one bundled wheel (decompressed)
const MAX_WHEELS_TOTAL: u64 = 512 * 1024 * 1024; // sum of bundled wheels
const MAX_WHEEL_COUNT: usize = 256;

/// Read up to `limit` bytes; None if the source is larger (bounds zip-bomb decompression).
fn read_capped<R: Read>(r: R, limit: u64) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    r.take(limit + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 <= limit).then_some(buf)
}

/// Parse an opened .mnote: a bare marimo .py, or a zip (notebook.py + wheels/ +
/// manifest.json). Manifest file names are rewritten to the local /_pkg/ URL the
/// handler serves; everything is held in memory (no temp files).
fn parse_mnote(bytes: Vec<u8>) -> Doc {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Doc {
            source: String::from_utf8_lossy(&bytes).into_owned(),
            ..Default::default()
        };
    }
    let mut doc = Doc::default();
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return doc;
    };

    if let Ok(f) = zip.by_name("notebook.py") {
        if let Some(b) = read_capped(f, MAX_TEXT) {
            doc.source = String::from_utf8_lossy(&b).into_owned();
        }
    }

    let manifest: Option<serde_json::Value> = zip
        .by_name("manifest.json")
        .ok()
        .and_then(|f| read_capped(f, MAX_TEXT))
        .and_then(|b| serde_json::from_slice(&b).ok());

    let wheel_names: Vec<String> = zip
        .file_names()
        .filter(|n| n.starts_with("wheels/") && n.ends_with(".whl"))
        .take(MAX_WHEEL_COUNT)
        .map(String::from)
        .collect();
    let mut total: u64 = 0;
    for n in wheel_names {
        let Ok(f) = zip.by_name(&n) else { continue };
        let Some(b) = read_capped(f, MAX_WHEEL) else { continue };
        total = total.saturating_add(b.len() as u64);
        if total > MAX_WHEELS_TOTAL {
            break;
        }
        let base = n.rsplit('/').next().unwrap_or(&n).to_string();
        doc.wheels.insert(base, b);
    }

    if let Some(serde_json::Value::Object(pkgs)) =
        manifest.as_ref().and_then(|m| m.get("packages")).cloned()
    {
        for (name, mut entry) in pkgs {
            if let Some(obj) = entry.as_object_mut() {
                let base = obj
                    .get("file_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.rsplit('/').next().unwrap_or(s).to_string())
                    .unwrap_or_default();
                // Root-relative so it resolves against packageBaseUrl's origin on
                // either engine (new URL("/_pkg/x", "<origin>/_vendor/.../full/")).
                obj.insert(
                    "file_name".into(),
                    serde_json::Value::String(format!("/_pkg/{base}")),
                );
            }
            doc.extra_packages.insert(name, entry);
        }
    }
    doc
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "zip" => "application/zip",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream", // wheels (.whl) etc.
    }
}

/// Swap the percent-encoded source held in <marimo-code ...>...</marimo-code>.
fn inject(template: &str, source: &str) -> String {
    let encoded = utf8_percent_encode(source, NON_ALPHANUMERIC).to_string();
    let (Some(open), Some(close)) = (template.find("<marimo-code"), template.find("</marimo-code>"))
    else {
        return template.to_string();
    };
    let Some(rel_gt) = template[open..close].find('>') else {
        return template.to_string();
    };
    let content_start = open + rel_gt + 1;
    let mut out = String::with_capacity(template.len() + encoded.len());
    out.push_str(&template[..content_start]);
    out.push_str(&encoded);
    out.push_str(&template[close..]);
    out
}

/// Merge the open doc's bundled package entries into the baked pyodide-lock.json.
fn merged_lock(raw: &str, extra: &serde_json::Map<String, serde_json::Value>) -> Vec<u8> {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(mut lock) => {
            if let Some(pkgs) = lock.get_mut("packages").and_then(|p| p.as_object_mut()) {
                for (k, v) in extra {
                    pkgs.insert(k.clone(), v.clone());
                }
            }
            serde_json::to_vec(&lock).unwrap_or_else(|_| raw.as_bytes().to_vec())
        }
        Err(_) => raw.as_bytes().to_vec(),
    }
}

/// Package names whose wheel is physically vendored in the embedded frontend, so
/// they load offline with no tier-2/3 help. (The lock lists ~373 names but only a
/// handful of wheels are actually shipped.)
fn baked_names() -> &'static HashSet<String> {
    static BAKED: OnceLock<HashSet<String>> = OnceLock::new();
    BAKED.get_or_init(|| {
        fn walk(dir: &Dir, out: &mut HashSet<String>) {
            for f in dir.files() {
                if let Some(n) = f.path().file_name().and_then(|s| s.to_str()) {
                    if n.ends_with(".whl") {
                        out.insert(n.to_string());
                    }
                }
            }
            for d in dir.dirs() {
                walk(d, out);
            }
        }
        let mut wheels = HashSet::new();
        walk(&FRONTEND, &mut wheels);

        let mut baked = HashSet::new();
        let raw = FRONTEND.get_file(LOCK_PATH).and_then(|f| f.contents_utf8()).unwrap_or("{}");
        if let Ok(serde_json::Value::Object(pkgs)) = serde_json::from_str::<serde_json::Value>(raw)
            .map(|v| v.get("packages").cloned().unwrap_or_default())
        {
            for (name, e) in pkgs {
                let base = e
                    .get("file_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("");
                if wheels.contains(base) {
                    baked.insert(resolver::norm(&name));
                }
            }
        }
        baked
    })
}

/// Fill `doc` with wheels for any declared dependency that is neither baked nor
/// already bundled — downloading the pure-Python closure from PyPI once (gated +
/// cached) if needed. The webview never fetches; only this backend does (spec §7).
fn resolve_missing(app: &tauri::AppHandle, doc: &mut Doc) {
    let bundled: HashSet<String> = doc.extra_packages.keys().map(|k| resolver::norm(k)).collect();
    let missing: Vec<String> = resolver::pep723_deps(&doc.source)
        .into_iter()
        .filter(|d| {
            let n = resolver::norm(d);
            !baked_names().contains(&n) && !bundled.contains(&n)
        })
        .collect();
    if missing.is_empty() {
        return;
    }

    let cache = app
        .path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("mnote-wheels");

    // Tier-3 gate: an untrusted file is driving a download — confirm before any
    // network fetch. Already-cached packages need no prompt (offline reuse).
    let need_net: Vec<&String> = missing
        .iter()
        .filter(|m| !resolver::is_cached(&resolver::norm(m), &cache))
        .collect();
    if !need_net.is_empty() {
        let list = need_net.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
        // Debug-only test hook so the download path can be exercised headlessly;
        // release builds always prompt.
        let auto = cfg!(debug_assertions) && std::env::var_os("MNOTE_AUTO_DOWNLOAD").is_some();
        let ok = auto
            || app
                .dialog()
                .message(format!(
                    "This notebook needs {} package(s) that aren't bundled:\n\n{list}\n\nDownload them once from PyPI? They'll be cached for offline use.",
                    need_net.len()
                ))
                .title("Download packages?")
                .buttons(MessageDialogButtons::OkCancel)
                .blocking_show();
        if !ok {
            return; // declined — the notebook may fail to import; the user's choice
        }
    }

    // Surface progress in the title bar while the (blocking) download runs.
    if !need_net.is_empty() {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.set_title(&format!(
                "Downloading {} package(s)… — Carrel",
                need_net.len()
            ));
        }
    }

    for r in resolver::resolve_closure(&missing, &cache, baked_names()) {
        doc.wheels.insert(r.filename, r.bytes);
        doc.extra_packages.insert(r.name, r.entry);
    }
}

/// Read a .mnote from disk, make it current, and reload the window.
///
/// Runs on a background thread: resolve_missing() may show a modal dialog and
/// block on a network download, and a blocking dialog on the main (event-loop)
/// thread deadlocks the webview.
fn load_file(app: &tauri::AppHandle, path: &str) {
    let app = app.clone();
    let path = path.to_string();
    std::thread::spawn(move || {
        // Refuse an absurdly large file before reading it all into memory.
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_MNOTE_FILE {
            return;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let mut doc = parse_mnote(bytes);
        doc.name = std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        resolve_missing(&app, &mut doc);
        *app.state::<Current>().0.lock().unwrap() = doc;
        // The title bar follows the open file via on_page_load (below), which also
        // covers cold opens where the file arrives before the window exists.
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.eval("window.location.reload()");
        }
    });
}

/// Bundle a notebook's source + its non-baked pure-Python wheel closure into a
/// self-contained .mnote zip (the authoring path; mirrors tools/mnote-pack.py).
/// Returns the number of wheels bundled.
fn export_mnote(source: &str, out: &std::path::Path, cache: &std::path::Path) -> Result<usize, String> {
    let missing: Vec<String> = resolver::pep723_deps(source)
        .into_iter()
        .filter(|d| !baked_names().contains(&resolver::norm(d)))
        .collect();
    let resolved = resolver::resolve_closure(&missing, cache, baked_names());

    let file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = Default::default();

    zip.start_file("notebook.py", opts).map_err(|e| e.to_string())?;
    zip.write_all(source.as_bytes()).map_err(|e| e.to_string())?;

    let mut packages = serde_json::Map::new();
    for r in &resolved {
        zip.start_file(format!("wheels/{}", r.filename), opts).map_err(|e| e.to_string())?;
        zip.write_all(&r.bytes).map_err(|e| e.to_string())?;
        // The manifest carries the bare basename; the player rewrites it to /_pkg/.
        let mut entry = r.entry.clone();
        if let Some(o) = entry.as_object_mut() {
            o.insert("file_name".into(), serde_json::Value::String(r.filename.clone()));
        }
        packages.insert(r.name.clone(), entry);
    }
    let manifest = serde_json::json!({ "packages": packages });
    zip.start_file("manifest.json", opts).map_err(|e| e.to_string())?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).unwrap_or_default()).map_err(|e| e.to_string())?;
    zip.finish().map_err(|e| e.to_string())?;
    Ok(resolved.len())
}

fn main() {
    // Headless authoring CLI: `carrel export <notebook.(py|mnote)> <out.mnote>`.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("export") {
        let (Some(inp), Some(outp)) = (args.get(2), args.get(3)) else {
            eprintln!("usage: carrel export <notebook.(py|mnote)> <out.mnote>");
            std::process::exit(2);
        };
        let bytes = std::fs::read(inp).unwrap_or_else(|e| {
            eprintln!("cannot read {inp}: {e}");
            std::process::exit(1);
        });
        let source = parse_mnote(bytes).source;
        let cache = std::env::temp_dir().join("carrel-export-wheels");
        match export_mnote(&source, std::path::Path::new(outp), &cache) {
            Ok(n) => {
                println!("wrote {outp} ({n} bundled wheel(s))");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("export failed: {e}");
                std::process::exit(1);
            }
        }
    }

    let default_doc = Doc {
        source: DEFAULT_NOTEBOOK.to_string(),
        ..Default::default()
    };

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    // Windows/Linux deliver a double-clicked file to a NEW process; forward it to
    // the already-running instance and focus it. (No-op on macOS.)
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(path) = argv.get(1) {
                load_file(app, path);
            }
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .manage(Current(Mutex::new(default_doc)))
        .register_uri_scheme_protocol("mnote", |ctx, req| {
            let app = ctx.app_handle();
            let path = req.uri().path();
            let rel = if path == "/" || path.is_empty() {
                "index.html"
            } else {
                path.trim_start_matches('/')
            };
            if rel.contains("..") {
                return Response::builder().status(403).body(Vec::new()).unwrap();
            }
            // index.html: inject the current notebook + a per-response script nonce.
            if rel == "index.html" {
                let tmpl = FRONTEND
                    .get_file("index.html")
                    .and_then(|f| f.contents_utf8())
                    .unwrap_or("");
                let src = app.state::<Current>().0.lock().unwrap().source.clone();
                let n = script_nonce();
                let html = add_nonce(&inject(tmpl, &src), &n);
                return Response::builder()
                    .header("Content-Type", "text/html")
                    .header("Content-Security-Policy", csp(Some(&n)))
                    .body(html.into_bytes())
                    .unwrap();
            }

            let res_csp = csp(None);
            let resp = |ct: &str, body: Vec<u8>| {
                Response::builder()
                    .header("Content-Type", ct)
                    .header("Content-Security-Policy", res_csp.clone())
                    .body(body)
                    .unwrap()
            };

            // Wheels bundled/cached for the open .mnote (tier 2/3).
            if let Some(base) = rel.strip_prefix("_pkg/") {
                let state = app.state::<Current>();
                let doc = state.0.lock().unwrap();
                return match doc.wheels.get(base) {
                    Some(b) => resp("application/octet-stream", b.clone()),
                    None => Response::builder().status(404).body(Vec::new()).unwrap(),
                };
            }

            // pyodide-lock.json with bundled packages merged in.
            if rel == LOCK_PATH {
                let raw = FRONTEND
                    .get_file(LOCK_PATH)
                    .and_then(|f| f.contents_utf8())
                    .unwrap_or("{}");
                let extra = app.state::<Current>().0.lock().unwrap().extra_packages.clone();
                return resp("application/json", merged_lock(raw, &extra));
            }

            match FRONTEND.get_file(rel) {
                Some(f) => resp(mime_for(rel), f.contents().to_vec()),
                None => Response::builder().status(404).body(Vec::new()).unwrap(),
            }
        })
        .on_page_load(|webview, _payload| {
            // Keep the native title in sync with the open file. Fires on the
            // initial load and on every reload, so it also fixes the cold-open
            // case where the file arrives (via Opened) before the window exists.
            let app = webview.app_handle();
            let name = app.state::<Current>().0.lock().unwrap().name.clone();
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_title(&window_title(&name));
            }
        })
        .setup(|app| {
            // Windows/Linux (and some macOS launches) pass the file as argv[1].
            if let Some(arg) = std::env::args().nth(1) {
                load_file(app.handle(), &arg);
            }
            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::CustomProtocol(WINDOW_URL.parse().unwrap()),
            )
            .title("Carrel")
            .inner_size(1000.0, 760.0)
            // Confine the webview to the app's own origin. A top-level navigation
            // (or a form submit, which navigates) to an external host is how an
            // untrusted notebook could exfiltrate or load a phishing page despite
            // connect-src; deny anything that isn't the mnote:// app origin.
            .on_navigation(|url| {
                matches!(url.scheme(), "mnote" | "about")
                    || url.host_str() == Some("mnote.localhost")
            })
            .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Carrel")
        .run(|_app, _event| {
            // macOS delivers double-clicked files as an Apple Event → RunEvent::Opened.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                if let Some(p) = urls.into_iter().find_map(|u| u.to_file_path().ok()) {
                    load_file(_app, &p.to_string_lossy());
                }
            }
        });
}
