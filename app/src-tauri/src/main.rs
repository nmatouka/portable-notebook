// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Step 2: the shell just opens a webview onto the bundled, vendored marimo
// export (configured via `frontendDist` in tauri.conf.json). No custom
// protocol or file association yet — those are Steps 3 and 4.
fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running the mnote player");
}
