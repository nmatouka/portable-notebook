// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Step 4: open a double-clicked .mnote file.
//
// The whole vendored frontend (marimo runtime + Pyodide + wheels, proven offline
// in experiment #1) is embedded in the binary and served over a custom `mnote://`
// protocol. For the document request we inject the *currently open* notebook's
// source into the <marimo-code> element, so one baked runtime can play any
// notebook. A .mnote is simply a marimo .py notebook (UTF-8 source).

use include_dir::{include_dir, Dir};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::sync::Mutex;
use tauri::{http::Response, Manager, WebviewUrl, WebviewWindowBuilder};

// Embedded at compile time → identical behavior in dev and bundled, no resource-path juggling.
static FRONTEND: Dir = include_dir!("$CARGO_MANIFEST_DIR/../frontend");
// Shown when launched without a file.
const DEFAULT_NOTEBOOK: &str = include_str!("../../default.mnote");

// Security model (spec §7): notebook code is untrusted (it arrives in files), so
// confine it to the app origin. `connect-src 'self'` is the load-bearing control —
// it blocks network egress from BOTH the document and the Pyodide worker, so a
// hostile notebook cannot exfiltrate (verified in experiment #1). Pyodide needs
// 'wasm-unsafe-eval'; marimo's bootstrap needs 'unsafe-inline' scripts; JS eval
// is NOT required. Sent on every response so the worker inherits it too.
const CSP: &str = "default-src 'self' mnote://localhost; \
script-src 'self' mnote://localhost 'unsafe-inline' 'wasm-unsafe-eval'; \
style-src 'self' mnote://localhost 'unsafe-inline'; \
img-src 'self' mnote://localhost data: blob:; \
font-src 'self' mnote://localhost data:; \
connect-src 'self' mnote://localhost; \
worker-src 'self' mnote://localhost blob:; \
child-src 'self' mnote://localhost blob:; \
object-src 'none'; base-uri 'self'";

/// The notebook source currently loaded into the player.
struct Current(Mutex<String>);

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
/// marimo decodes it with decodeURIComponent(), so over-encoding is harmless.
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

/// Read a .mnote (marimo .py) from disk, make it current, and reload the window.
fn load_file(app: &tauri::AppHandle, path: &str) {
    if let Ok(src) = std::fs::read_to_string(path) {
        *app.state::<Current>().0.lock().unwrap() = src;
        if let Some(win) = app.get_webview_window("main") {
            // Show the opened file in the native title bar — a non-spoofable
            // affordance for *which* file's (untrusted) content is running.
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
    tauri::Builder::default()
        .manage(Current(Mutex::new(DEFAULT_NOTEBOOK.to_string())))
        .register_uri_scheme_protocol("mnote", |ctx, req| {
            let app = ctx.app_handle();
            let path = req.uri().path();
            let rel = if path == "/" || path.is_empty() {
                "index.html"
            } else {
                path.trim_start_matches('/')
            };

            // Defensive: the frontend is an embedded virtual tree (no real FS to
            // escape to), but reject traversal anyway.
            if rel.contains("..") {
                return Response::builder().status(403).body(Vec::new()).unwrap();
            }

            if rel == "index.html" {
                let tmpl = FRONTEND
                    .get_file("index.html")
                    .and_then(|f| f.contents_utf8())
                    .unwrap_or("");
                let src = app.state::<Current>().0.lock().unwrap().clone();
                return Response::builder()
                    .header("Content-Type", "text/html")
                    .header("Content-Security-Policy", CSP)
                    .body(inject(tmpl, &src).into_bytes())
                    .unwrap();
            }

            match FRONTEND.get_file(rel) {
                Some(f) => Response::builder()
                    .header("Content-Type", mime_for(rel))
                    .header("Content-Security-Policy", CSP)
                    .body(f.contents().to_vec())
                    .unwrap(),
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
