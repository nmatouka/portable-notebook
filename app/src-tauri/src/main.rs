// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Opens a double-clicked .mnote. A .mnote is either a bare marimo .py, or a zip
// bundling notebook.py + wheels/ + manifest.json (spec §5 tier 2). The whole
// vendored frontend (marimo + Pyodide + baked wheels) is embedded and served over
// the mnote:// custom protocol; the current notebook's source is injected into
// <marimo-code>, and any wheels bundled in the .mnote are merged into the served
// pyodide-lock.json and served from /_pkg/ — so an exotic-but-pure-Python package
// loads fully offline, with the webview never touching PyPI.

use include_dir::{include_dir, Dir};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use tauri::{http::Response, Manager, WebviewUrl, WebviewWindowBuilder};

// Embedded at compile time → identical behavior in dev and bundled.
static FRONTEND: Dir = include_dir!("$CARGO_MANIFEST_DIR/../frontend");
const DEFAULT_NOTEBOOK: &str = include_str!("../../default.mnote");
const LOCK_PATH: &str = "_vendor/wasm.marimo.app/pyodide-lock.json";

// Security model (spec §7): notebook code is untrusted, so confine it to the app
// origin. `connect-src 'self'` blocks network egress from BOTH the document and
// the Pyodide worker (verified). Pyodide needs 'wasm-unsafe-eval'; marimo's
// bootstrap needs 'unsafe-inline'; JS eval is not required. Sent on every response.
const CSP: &str = "default-src 'self' mnote://localhost; \
script-src 'self' mnote://localhost 'unsafe-inline' 'wasm-unsafe-eval'; \
style-src 'self' mnote://localhost 'unsafe-inline'; \
img-src 'self' mnote://localhost data: blob:; \
font-src 'self' mnote://localhost data:; \
connect-src 'self' mnote://localhost; \
worker-src 'self' mnote://localhost blob:; \
child-src 'self' mnote://localhost blob:; \
object-src 'none'; base-uri 'self'";

/// The currently-open document: notebook source + any wheels bundled in the .mnote.
#[derive(Default)]
struct Doc {
    source: String,
    /// Lock entries (already pointed at /_pkg/) to merge into pyodide-lock.json.
    extra_packages: serde_json::Map<String, serde_json::Value>,
    /// Bundled wheel bytes, keyed by file basename, served from /_pkg/<base>.
    wheels: HashMap<String, Vec<u8>>,
}
struct Current(Mutex<Doc>);

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

    if let Ok(mut f) = zip.by_name("notebook.py") {
        let mut s = String::new();
        let _ = f.read_to_string(&mut s);
        doc.source = s;
    }

    let manifest: Option<serde_json::Value> = zip.by_name("manifest.json").ok().and_then(|mut f| {
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        serde_json::from_str(&s).ok()
    });

    let wheel_names: Vec<String> = zip
        .file_names()
        .filter(|n| n.starts_with("wheels/") && n.ends_with(".whl"))
        .map(String::from)
        .collect();
    for n in wheel_names {
        if let Ok(mut f) = zip.by_name(&n) {
            let mut b = Vec::new();
            if f.read_to_end(&mut b).is_ok() {
                let base = n.rsplit('/').next().unwrap_or(&n).to_string();
                doc.wheels.insert(base, b);
            }
        }
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
                obj.insert(
                    "file_name".into(),
                    serde_json::Value::String(format!("mnote://localhost/_pkg/{base}")),
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

/// Read a .mnote from disk, make it current, and reload the window.
fn load_file(app: &tauri::AppHandle, path: &str) {
    if let Ok(bytes) = std::fs::read(path) {
        *app.state::<Current>().0.lock().unwrap() = parse_mnote(bytes);
        if let Some(win) = app.get_webview_window("main") {
            // Show the opened file in the native title bar — a non-spoofable
            // affordance for which file's (untrusted) content is running.
            let name = std::path::Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "notebook".into());
            let _ = win.set_title(&format!("{name} — mnote Player"));
            let _ = win.eval("window.location.reload()");
        }
    }
}

fn main() {
    let default_doc = Doc {
        source: DEFAULT_NOTEBOOK.to_string(),
        ..Default::default()
    };

    tauri::Builder::default()
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
            let resp = |ct: &str, body: Vec<u8>| {
                Response::builder()
                    .header("Content-Type", ct)
                    .header("Content-Security-Policy", CSP)
                    .body(body)
                    .unwrap()
            };

            // Wheels bundled in the open .mnote (tier 2).
            if let Some(base) = rel.strip_prefix("_pkg/") {
                let state = app.state::<Current>();
                let doc = state.0.lock().unwrap();
                return match doc.wheels.get(base) {
                    Some(b) => resp("application/octet-stream", b.clone()),
                    None => Response::builder().status(404).body(Vec::new()).unwrap(),
                };
            }

            // index.html with the current notebook injected.
            if rel == "index.html" {
                let tmpl = FRONTEND
                    .get_file("index.html")
                    .and_then(|f| f.contents_utf8())
                    .unwrap_or("");
                let src = app.state::<Current>().0.lock().unwrap().source.clone();
                return resp("text/html", inject(tmpl, &src).into_bytes());
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
        .setup(|app| {
            // Windows/Linux (and some macOS launches) pass the file as argv[1].
            if let Some(arg) = std::env::args().nth(1) {
                load_file(app.handle(), &arg);
            }
            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::CustomProtocol("mnote://localhost/".parse().unwrap()),
            )
            .title("mnote Player")
            .inner_size(1000.0, 760.0)
            .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the mnote player")
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
