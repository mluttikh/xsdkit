#!/usr/bin/env bash
# Fetches the commit of the W3C XML Schema Test Suite that the baselines in
# tests/conformance/ were blessed against, into the directory given.
#
# The pin lives in tests/conformance/SUITE, beside the baselines it describes:
# a baseline means something only against one snapshot of the suite, and a
# clone of its default branch drifts from it. One commit is 16 MB and a few
# seconds; a full clone is 231 MB.
#
#     scripts/fetch-w3c-suite.sh /tmp/xsdtests
set -euo pipefail

dest=${1:?usage: scripts/fetch-w3c-suite.sh DIR}
repo=$(cd "$(dirname "$0")/.." && pwd)
suite=$(grep -v '^#' "$repo/tests/conformance/SUITE" | tr -d '[:space:]')
echo "pinned at $suite"
git init -q "$dest"
git -C "$dest" remote add origin https://github.com/w3c/xsdtests
git -C "$dest" fetch -q --depth 1 origin "$suite"
git -C "$dest" checkout -q FETCH_HEAD
