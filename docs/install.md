# Installation

!!! note "Not yet published"

    `xsdkit` has not been released to [crates.io](https://crates.io) or
    [PyPI](https://pypi.org) yet. The commands in the first tab of each block
    are what installation *will* look like; until then, install from the
    repository.

## Python

=== "From PyPI"

    ```bash
    pip install xsdkit
    ```

=== "From the repository"

    ```bash
    pip install "git+https://github.com/mluttikh/xsdkit"
    ```

    A Rust toolchain is needed to build from source; wheels will not require one.

Python 3.9 and newer. The extension is built against the stable ABI
(`abi3`), so one wheel per platform serves every supported Python — upgrading
your interpreter does not mean waiting for a new release.

The package ships type stubs and a `py.typed` marker, so `mypy` and `pyright`
check calls into it like any other typed library.

```python
import xsdkit

xsdkit.__version__
```

## Rust

=== "From crates.io"

    ```bash
    cargo add xsdkit
    ```

=== "From the repository"

    ```toml
    [dependencies]
    xsdkit = { git = "https://github.com/mluttikh/xsdkit" }
    ```

The minimum supported Rust version is **1.87**, checked in CI on every commit
rather than merely claimed in `Cargo.toml`.

There are no optional features to choose between for ordinary use. The
`python` and `extension-module` features exist to build the Python bindings
and are not something a Rust dependent turns on.

## Building the repository

```bash
git clone https://github.com/mluttikh/xsdkit
cd xsdkit

cargo test                      # the Rust side
maturin develop --release       # build and install the Python extension
pytest python/tests -q          # the Python side
```

`maturin develop` builds a **debug** binary unless you pass `--release`, and
the difference is large enough to mislead you if you are measuring anything.

### The conformance suite

The W3C XML Schema Test Suite is 231 MB and is not vendored. Point `XSDTESTS`
at a clone and the suite runs; leave it unset and those tests skip.

```bash
git clone --depth 1 https://github.com/w3c/xsdtests /tmp/xsdtests
export XSDTESTS=/tmp/xsdtests

cargo test --test w3c_suite -- --nocapture                      # schemas
cargo test --release --test w3c_suite -- --ignored --nocapture  # documents
```

### The documentation

This site is built with [Material for MkDocs](https://squidfunk.github.io/mkdocs-material/),
with the Rust API reference mounted underneath it.

```bash
pip install -r docs/requirements.txt
./scripts/build-docs.sh          # builds rustdoc + the site into site/
mkdocs serve                     # live preview on http://127.0.0.1:8000
```

The Python reference is generated from the built extension module, so
`maturin develop` has to have run first.
