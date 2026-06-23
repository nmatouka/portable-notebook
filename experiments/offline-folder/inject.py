#!/usr/bin/env python3
"""Inject a marimo notebook's source into a baked WASM-export index.html.

This is the core of "open an arbitrary .mnote": the player keeps ONE baked
frontend (vendored runtime + assets) and, at open time, swaps the notebook
source held in <marimo-code hidden="">...</marimo-code> (percent-encoded).

Usage: inject.py <notebook.(py|mnote)> <template-index.html> <out-index.html>
"""
import sys
import re
import urllib.parse
import pathlib

nb, tmpl, out = sys.argv[1], sys.argv[2], sys.argv[3]
src = pathlib.Path(nb).read_text(encoding="utf-8")
# marimo reads the element with decodeURIComponent(); encodeURIComponent-equivalent
# encoding (uppercase %XX, only unreserved left raw) decodes cleanly.
encoded = urllib.parse.quote(src, safe="")

html = pathlib.Path(tmpl).read_text(encoding="utf-8")
new_html, n = re.subn(
    r"(<marimo-code\b[^>]*>).*?(</marimo-code>)",
    lambda m: m.group(1) + encoded + m.group(2),
    html,
    count=1,
    flags=re.DOTALL,
)
if n != 1:
    sys.exit("error: <marimo-code> element not found in template")

pathlib.Path(out).write_text(new_html, encoding="utf-8")
print(f"injected {len(src)} chars of source ({len(encoded)} encoded) into {out}")
