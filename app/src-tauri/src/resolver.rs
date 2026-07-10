// Tier-3 package resolution (spec §5): for a declared dependency that is neither
// baked into the player nor bundled in the .mnote, the *Rust backend* (not the
// notebook — the webview CSP forbids that) downloads the wheels once, caches them,
// and returns pyodide-lock entries pointing at the local /_pkg/ URLs. The player
// merges those into the served lock, exactly like tier-2, so micropip installs them
// locally and offline thereafter.
//
// Two sources, tried in order:
//   1. The Pyodide lock catalog — packages the player's own pyodide-lock.json knows
//      about (numpy, pandas, matplotlib, scipy, …). Only ~14 of the ~373 listed
//      wheels are physically baked; the rest have file_names the vendor step
//      rewrote to local /_vendor/ paths that 404 when absent. For those we download
//      the *exact* Pyodide-built wheel from its original CDN URL (reconstructed from
//      the rewritten path) and verify it against the lock's sha256. This is what
//      lets marimo gallery examples — which lean on the scientific stack — open.
//   2. PyPI — for a declared dep the lock doesn't list, fetch its latest pure-Python
//      (py3-none-any) wheel. Non-pure-Python packages absent from the lock can't run
//      under Pyodide and are skipped.
//
// v1 scope: latest version, no version solving (the lock pins the only version its
// Pyodide can actually run, which is what we want anyway).

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
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

/// A package the player's own pyodide-lock.json describes: the exact wheel Pyodide
/// can run, plus where to fetch it and how to verify it.
pub struct LockPkg {
    /// Original remote URL, reconstructed from the vendor-rewritten `/_vendor/…` path.
    pub url: String,
    /// Wheel basename (also the /_pkg/ name the player serves it under).
    pub filename: String,
    /// PyPI/CDN-published sha256 from the lock, checked after download.
    pub sha256: Option<String>,
    /// Direct dependencies (normalized), for closure traversal.
    pub depends: Vec<String>,
    /// The lock entry verbatim; served back with only `file_name` repointed to /_pkg/.
    pub entry: serde_json::Value,
}

/// Build the catalog (normalized name → [`LockPkg`]) from the player's served
/// pyodide-lock.json. `vendor.sh` rewrote every `https://H/P` wheel URL to
/// `/_vendor/H/P`, so a rewritten `file_name` reconstructs to `https://H/P`.
pub fn build_catalog(lock_json: &str) -> HashMap<String, LockPkg> {
    let mut out = HashMap::new();
    let Ok(serde_json::Value::Object(pkgs)) = serde_json::from_str::<serde_json::Value>(lock_json)
        .map(|v| v.get("packages").cloned().unwrap_or_default())
    else {
        return out;
    };
    for (name, entry) in pkgs {
        let Some(file_name) = entry.get("file_name").and_then(|v| v.as_str()) else {
            continue;
        };
        // Only entries the vendor step rewrote to a local mirror path can be turned
        // back into a fetchable URL; anything else we can't source here.
        let Some(hostpath) = file_name.strip_prefix("/_vendor/") else {
            continue;
        };
        let url = format!("https://{hostpath}");
        let filename = hostpath.rsplit('/').next().unwrap_or(hostpath).to_string();
        let sha256 =
            entry.get("sha256").and_then(|h| h.as_str()).map(str::to_owned);
        let depends = entry
            .get("depends")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|d| d.as_str()).map(norm).collect())
            .unwrap_or_default();
        out.insert(norm(&name), LockPkg { url, filename, sha256, depends, entry });
    }
    out
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

/// Download a wheel, bounded by MAX_WHEEL_DL so a slow/huge host can't exhaust memory.
fn download(url: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    agent().get(url).call().ok()?.into_reader().take(MAX_WHEEL_DL + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_WHEEL_DL).then_some(bytes)
}

/// Persist a downloaded wheel so the next open resolves it offline.
fn cache_write(cache: &Path, norm_name: &str, filename: &str, bytes: &[u8]) {
    let dir = cache.join(norm_name);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(filename), bytes);
}

/// Resolve one package. Prefer the Pyodide lock catalog (the exact wheel Pyodide can
/// run — this is what unblocks the scientific stack that gallery examples use); fall
/// back to the latest pure-Python wheel on PyPI. Loads from cache when present, else
/// downloads and caches. Returns None if it can't be satisfied.
fn resolve_one(norm_name: &str, cache: &Path, catalog: &HashMap<String, LockPkg>) -> Option<Resolved> {
    if let Some(pkg) = catalog.get(norm_name) {
        let bytes = if let Some(p) = cached_wheel(norm_name, cache) {
            std::fs::read(&p).ok()?
        } else {
            let bytes = download(&pkg.url)?;
            // Integrity: verify against the lock's published hash (beyond TLS), the
            // same guarantee the vendored/baked wheels get at build time.
            if let Some(exp) = &pkg.sha256 {
                if !sha256_hex(&bytes).eq_ignore_ascii_case(exp) {
                    return None;
                }
            }
            cache_write(cache, norm_name, &pkg.filename, &bytes);
            bytes
        };
        // Serve the lock entry verbatim (correct version, deps, imports, shared-lib
        // flags), only repointing file_name at the local /_pkg/ copy.
        let mut entry = pkg.entry.clone();
        if let Some(o) = entry.as_object_mut() {
            o.insert(
                "file_name".into(),
                serde_json::Value::String(format!("/_pkg/{}", pkg.filename)),
            );
        }
        return Some(Resolved {
            name: norm_name.to_string(),
            filename: pkg.filename.clone(),
            bytes,
            depends: pkg.depends.clone(),
            entry,
        });
    }

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

/// Resolve the closure of `top`, skipping anything in `baked`. Each package is
/// sourced from the Pyodide lock catalog when listed there, otherwise from PyPI.
pub fn resolve_closure(
    top: &[String],
    cache: &Path,
    baked: &HashSet<String>,
    catalog: &HashMap<String, LockPkg>,
) -> Vec<Resolved> {
    let mut out = Vec::new();
    let mut seen = baked.clone();
    let mut queue: VecDeque<String> = top.iter().map(|s| norm(s)).collect();
    while let Some(n) = queue.pop_front() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(r) = resolve_one(&n, cache, catalog) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // A lock shaped like the vendor-rewritten one: every wheel URL was rewritten to
    // a local /_vendor/<host>/<path> mirror, so the host+path reconstructs the URL.
    const LOCK: &str = r#"{
      "packages": {
        "numpy":  {"name":"numpy","version":"2.4.3","file_name":"/_vendor/cdn.jsdelivr.net/pyodide/v314.0.0/full/numpy-2.4.3-cp314-cp314-pyemscripten_2026_0_wasm32.whl","sha256":"aaaa","depends":[],"imports":["numpy"]},
        "pandas": {"name":"pandas","version":"2.2.3","file_name":"/_vendor/cdn.jsdelivr.net/pyodide/v314.0.0/full/pandas-2.2.3-cp314-cp314-pyemscripten_2026_0_wasm32.whl","sha256":"bbbb","depends":["numpy","python-dateutil","pytz"],"imports":["pandas"]},
        "pytz":   {"name":"pytz","version":"2025.2","file_name":"/_vendor/files.pythonhosted.org/packages/xx/pytz-2025.2-py3-none-any.whl","sha256":"cccc","depends":[]},
        "weird":  {"name":"weird","version":"1.0","file_name":"weird-1.0-py3-none-any.whl","depends":[]}
      }
    }"#;

    #[test]
    fn catalog_reconstructs_pyodide_wheel_urls() {
        let cat = build_catalog(LOCK);
        // A Pyodide-built wheel that isn't baked is now sourceable — this is the fix.
        let p = &cat["pandas"];
        assert_eq!(
            p.url,
            "https://cdn.jsdelivr.net/pyodide/v314.0.0/full/pandas-2.2.3-cp314-cp314-pyemscripten_2026_0_wasm32.whl"
        );
        assert_eq!(p.filename, "pandas-2.2.3-cp314-cp314-pyemscripten_2026_0_wasm32.whl");
        assert_eq!(p.sha256.as_deref(), Some("bbbb"));
        assert_eq!(p.depends, vec!["numpy", "python-dateutil", "pytz"]);
        // pythonhosted-hosted entries round-trip the same way.
        assert_eq!(cat["pytz"].url, "https://files.pythonhosted.org/packages/xx/pytz-2025.2-py3-none-any.whl");
    }

    #[test]
    fn catalog_skips_unsourceable_entries() {
        let cat = build_catalog(LOCK);
        // A bare-filename entry (no /_vendor/ prefix) can't be turned into a URL.
        assert!(!cat.contains_key("weird"));
    }

    #[test]
    fn pep723_reads_marimo_example_header() {
        let src = "# /// script\n# dependencies = [\"pandas\", \"matplotlib==3.9\", \"altair\"]\n# ///\nimport marimo\n";
        let mut deps = pep723_deps(src);
        deps.sort();
        assert_eq!(deps, vec!["altair", "matplotlib", "pandas"]);
    }
}
