// Tier-3 package resolution (spec §5): for a declared dependency that is neither
// baked into the player nor bundled in the .mnote, the *Rust backend* (not the
// notebook — the webview CSP forbids that) resolves the pure-Python closure from
// PyPI, downloads the wheels once, caches them, and returns pyodide-lock entries
// pointing at the local /_pkg/ URLs. The player merges those into the served lock,
// exactly like tier-2, so micropip installs them locally and offline thereafter.
//
// v1 scope: pure-Python (py3-none-any) wheels, latest version, no version solving.

use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_WHEEL_DL: u64 = 128 * 1024 * 1024; // cap a single downloaded wheel

/// HTTP client with timeouts, so a hung/slow host can't stall the download thread.
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .build()
}

/// PEP 503 name normalization.
pub fn norm(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        let c = if matches!(c, '_' | '.' | '-') { '-' } else { c.to_ascii_lowercase() };
        if c == '-' {
            if !prev_dash {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

/// Extract the `dependencies = [...]` list from a notebook's PEP 723 header.
pub fn pep723_deps(source: &str) -> Vec<String> {
    let Some(start) = source.find("# /// script") else {
        return vec![];
    };
    let body_start = start + "# /// script".len();
    let Some(rel_end) = source[body_start..].find("# ///") else {
        return vec![];
    };
    let block: String = source[body_start..body_start + rel_end]
        .lines()
        .map(|l| l.strip_prefix("# ").or_else(|| l.strip_prefix('#')).unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");
    let Some(d) = block.find("dependencies") else {
        return vec![];
    };
    let Some(lb) = block[d..].find('[') else {
        return vec![];
    };
    let Some(rb) = block[d + lb..].find(']') else {
        return vec![];
    };
    block[d + lb + 1..d + lb + rb]
        .split(',')
        .filter_map(|s| {
            let name: String = s
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .chars()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

pub struct Resolved {
    pub name: String,
    pub filename: String,
    pub bytes: Vec<u8>,
    pub depends: Vec<String>,
    pub entry: serde_json::Value,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// (depends, imports) read from a wheel's .dist-info METADATA / top_level.txt.
fn wheel_meta(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let (mut depends, mut imports) = (Vec::new(), Vec::new());
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())) else {
        return (depends, imports);
    };
    let names: Vec<String> = zip.file_names().map(String::from).collect();
    if let Some(meta) = names.iter().find(|n| n.ends_with(".dist-info/METADATA")) {
        if let Ok(mut f) = zip.by_name(meta) {
            let mut s = String::new();
            let _ = f.read_to_string(&mut s);
            for line in s.lines() {
                if let Some(rest) = line.strip_prefix("Requires-Dist:") {
                    if rest.contains(';') {
                        continue; // skip optional/marker/extra deps in v1
                    }
                    let n: String = rest
                        .trim()
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
                        .collect();
                    if !n.is_empty() {
                        depends.push(norm(&n));
                    }
                }
            }
        }
    }
    if let Some(tl) = names.iter().find(|n| n.ends_with(".dist-info/top_level.txt")) {
        if let Ok(mut f) = zip.by_name(tl) {
            let mut s = String::new();
            let _ = f.read_to_string(&mut s);
            imports = s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        }
    }
    (depends, imports)
}

fn cached_wheel(norm_name: &str, cache: &Path) -> Option<PathBuf> {
    std::fs::read_dir(cache.join(norm_name))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().map_or(false, |x| x == "whl"))
}

/// Is the package's wheel already in the cache (so it loads offline)?
pub fn is_cached(norm_name: &str, cache: &Path) -> bool {
    cached_wheel(norm_name, cache).is_some()
}

/// Resolve one package: load from cache, else download the latest pure-Python
/// wheel from PyPI and cache it. Returns None if it can't be satisfied.
fn resolve_one(norm_name: &str, cache: &Path) -> Option<Resolved> {
    let (filename, bytes) = if let Some(p) = cached_wheel(norm_name, cache) {
        (p.file_name()?.to_string_lossy().into_owned(), std::fs::read(&p).ok()?)
    } else {
        let http = agent();
        let json: serde_json::Value = http
            .get(&format!("https://pypi.org/pypi/{norm_name}/json"))
            .call()
            .ok()?
            .into_json()
            .ok()?;
        let w = json.get("urls")?.as_array()?.iter().find(|u| {
            u.get("filename").and_then(|f| f.as_str()).map_or(false, |f| f.ends_with("-py3-none-any.whl"))
        })?;
        let filename = w.get("filename")?.as_str()?.to_string();
        // Defensive: never let a server-provided name escape the cache directory.
        if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
            return None;
        }
        let url = w.get("url")?.as_str()?;
        let expected =
            w.get("digests").and_then(|d| d.get("sha256")).and_then(|h| h.as_str()).map(str::to_owned);

        let mut bytes = Vec::new();
        http.get(url).call().ok()?.into_reader().take(MAX_WHEEL_DL + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > MAX_WHEEL_DL {
            return None;
        }
        // Integrity: verify the bytes against PyPI's published hash (beyond TLS).
        if let Some(exp) = &expected {
            if !sha256_hex(&bytes).eq_ignore_ascii_case(exp) {
                return None;
            }
        }
        let dir = cache.join(norm_name);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(&filename), &bytes);
        (filename, bytes)
    };

    let version = filename.split('-').nth(1).unwrap_or("0").to_string();
    let (depends, imports) = wheel_meta(&bytes);
    let entry = serde_json::json!({
        "name": norm_name,
        "version": version,
        "file_name": format!("/_pkg/{filename}"),
        "install_dir": "site",
        "package_type": "package",
        "sha256": sha256_hex(&bytes),
        "unvendored_tests": false,
        "imports": if imports.is_empty() { vec![norm_name.replace('-', "_")] } else { imports },
        "depends": depends.clone(),
    });
    Some(Resolved { name: norm_name.to_string(), filename, bytes, depends, entry })
}

/// Resolve the pure-Python closure of `top`, skipping anything in `baked`.
pub fn resolve_closure(top: &[String], cache: &Path, baked: &HashSet<String>) -> Vec<Resolved> {
    let mut out = Vec::new();
    let mut seen = baked.clone();
    let mut queue: VecDeque<String> = top.iter().map(|s| norm(s)).collect();
    while let Some(n) = queue.pop_front() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(r) = resolve_one(&n, cache) {
            for d in &r.depends {
                if !seen.contains(d) {
                    queue.push_back(d.clone());
                }
            }
            out.push(r);
        }
    }
    out
}
