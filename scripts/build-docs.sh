#!/usr/bin/env bash
# Builds the documentation site: the Rust API reference, mounted under the
# MkDocs site so both halves live at one URL.
#
#   site/            the guide and the Python reference
#   site/rust/       rustdoc
#
# The Python reference is generated from the *built* extension module, so the
# wheel has to be installed first — `maturin develop` before running this.
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> rustdoc"
# Cleaned first: `cargo doc` writes over what it regenerates but never removes
# what it no longer generates, and this tree is copied wholesale into the site.
# A module that has since been made private leaves a directory behind, and the
# reference page went on linking to one.
rm -rf target/doc
# Denied warnings here too: a broken intra-doc link should fail the docs build,
# not ship as a dead link on the website.
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

echo "==> staging rustdoc into docs/rust"
rm -rf docs/rust
cp -R target/doc docs/rust
# `cargo doc` writes no root index for a single crate, so the mounted tree
# would 404 at /rust/. Send it to the crate.
cat > docs/rust/index.html <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>Redirecting to the xsdkit crate documentation</title>
<meta http-equiv="refresh" content="0; url=xsdkit/index.html">
<link rel="canonical" href="xsdkit/index.html">
<p>Redirecting to <a href="xsdkit/index.html">the xsdkit crate documentation</a>.</p>
HTML

echo "==> mkdocs"
python3 -m mkdocs build --strict

echo "==> done: site/"
