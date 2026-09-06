# xsdkit

[![CI](https://github.com/mluttikh/xsdkit/actions/workflows/ci.yml/badge.svg)](https://github.com/mluttikh/xsdkit/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-mluttikh.github.io%2Fxsdkit-0f766e.svg)](https://mluttikh.github.io/xsdkit/)

A **generic XSD reader**: parse W3C XML Schema into a queryable schema
component model, in Rust and Python.

**[Documentation](https://mluttikh.github.io/xsdkit/)** — guide, Python API
reference and the full rustdoc.

> **Status: early.** The component model, document loading, reference
> resolution, content-model compilation, instance validation and the Python
> bindings work, and are measured against the W3C XML Schema Test Suite on
> every change. XSD 1.1 assertions and conditional type assignment come next.

## Why

XSD is three languages: schema *documents*, schema *components*, and
validation *semantics* defined over those components. The specification
defines every rule against the middle layer — and in Rust, nothing exposes
it. `xsd-parser` builds a codegen-shaped intermediate that discards
validation semantics; `uppsala` builds a model internally but exposes only
`validate()`. Neither can answer *"what are the possible children of this
element, and can they repeat?"*

`xsdkit` builds that layer and hands it to you. Python is no better served:
`xmlschema` is complete and the only real option, and by its own benchmarks
runs 40–75× slower than lxml.

It is deliberately a *reader*, not a toolchain. Code generation is
[`xsd-parser`](https://crates.io/crates/xsd-parser)'s job, and generating a
config or a binding for some particular downstream reader belongs in a library
of its own — so that reading a schema never pulls in dependencies you did not
ask for.

## Usage

```toml
[dependencies]
xsdkit = "0.1"
```

```rust
use xsdkit::SchemaSetBuilder;

let schemas = SchemaSetBuilder::new()
    .search_path("schemas/")
    .file("report.xsd")
    .build()?;

let report = schemas.element(Some("urn:example"), "report").unwrap();

// Which children may appear, may they repeat, may they be absent?
// Answered from the compiled automaton, with substitution groups expanded
// and inherited content included — all three from one pass over the model.
for child in report.children() {
    println!(
        "{}  repeating={}  optional={}",
        child.display_name(),
        child.repeats(),
        child.optional(),
    );
}

// Attributes, and the type of each.
for a in report.attributes() {
    println!("@{} {}", a.local_name(), a.type_of().display_name());
}

// Does a sequence of children satisfy the content model?
let ty = report.type_of().id();
let mut m = schemas.match_content(ty).unwrap();
let ok = m.step(schemas.qname(Some("urn:example"), "title").unwrap()) && m.accepts_end();
```

Names resolve to references — `ElementRef`, `TypeRef`, `ChildRef` — each a
borrow of the schema plus an id, so following a schema costs no allocation
and no reference counting. Components still live in arenas addressed by
`Copy` id: `element_id` and its siblings hand those back directly, every
reference exposes `.id()`, and `schemas.get(id)` goes the other way.

Ask about a whole type's children at once rather than child by child. Each
of the singular predicates walks the content model, so a type with hundreds
of children — ordinary in GML, UBL or WITSML — pays for hundreds of walks;
`children()` does the same work in one, and is about 40× faster on those
schemas.

### Caching a compiled schema

Compiling is the expensive step. The `serde` feature makes `Schemas` — and
every component it holds — serializable, so a large schema set is compiled
once and loaded thereafter:

```toml
xsdkit = { version = "0.1", features = ["serde"] }
```

```rust
let cached = postcard::to_allocvec(&schemas)?;
let schemas: xsdkit::Schemas = postcard::from_bytes(&cached)?;
```

Any serde format works, self-describing ones included. On a 900 KB schema of
2,000 types this is a 7x speedup — 31 ms to compile against 4.5 ms to load —
at the cost of a cache several times the size of the source XSD. Measure it
on your own schema before deciding:

```
cargo run --release --features serde --example cache -- main.xsd [search/path ...]
```

The format is not stable across versions of xsdkit: names are interned and
every component refers to them by index, so a cache is only meaningful
alongside the code that wrote it. Key it on the crate version and rebuild on
a miss.

### Python

```bash
pip install xsdkit
```

```python
import xsdkit

schemas = xsdkit.SchemaSet.from_file("report.xsd", search_paths=["schemas/"])
report = schemas["{urn:example}report"]

report.tree()          # or print(...); it renders in a notebook either way
                       # in Jupyter, a collapsible colour-coded tree
# report
#   title: xs:string
#   item+
#     @sku
#     price: xs:decimal
#     note?: xs:string

report["item"]["price"].type.qname   # walk by name, no `.type` hop
[child.local_name for child in report]
report["item"].repeats               # occurrence belongs to the pair,
report["item"]["note"].optional      # and a child carries its own

len(schemas)                         # globals this schema declares
"{urn:example}report" in schemas     # a mapping: dict(schemas) works too

# Does a child sequence satisfy the content model?
report.type.accepts(["{urn:example}title", "{urn:example}count"])
```

Validate a document, and read it into typed values:

```python
report = schemas.validate(open("report.xml").read())
report.is_valid           # False
for d in report.errors:
    print(d)              # error[XSD2004]: `{urn:example}count`: ... --> :3

for ev in schemas.iter_typed(open("report.xml").read()):
    if ev.kind == "text":
        print(type(ev.value).__name__, ev.value)
# int       42
# Decimal   3.14
# datetime  2024-12-30 12:39:15+00:00
# date      2024-03-31
```

Values arrive as native Python types, not strings to re-parse. `iter_typed`
composes with `enumerate`, `itertools` and generator expressions, and carries
the outcome on its `.report` — before the loop as well as after.

XSD 1.1 is opt-in, as it is in Rust, and documents may be bytes whose encoding
is detected rather than assumed:

```python
schemas = xsdkit.SchemaSet.from_file("report.xsd", version="1.1")
schemas.validate(Path("report.xml").read_bytes())
```

Schemas need not be on disk. A resolver is a function of `(location, base)`
that returns the document, or raises to say it could not be found:

```python
with zipfile.ZipFile("schemas.zip") as z:
    schemas = xsdkit.SchemaSet.from_string(main, resolver=lambda loc, _: z.read(loc))
```

Schemas that are expected to be imperfect return their diagnostics instead of
raising:

```python
schemas, diagnostics = xsdkit.load("vendor/partial.xsd", conformance="lax")
for d in diagnostics:
    print(d)          # error[XSD1201]: ...  --> file.xsd:12
```

Inspect a schema from the command line:

```bash
cargo run --example inspect -- schemas/report.xsd --lax
```

## What works today

- **The component model** — types, elements, attributes, particles, model
  groups, wildcards, identity constraints, notations, annotations; all seven
  symbol spaces kept separate.
- **All 50 built-ins** as real components, so `xs:string` resolves exactly
  like a user type. 19 primitives, the 1.1 additions, and the derivation
  chains between them.
- **Facets** with correct composition: patterns OR within a restriction step
  and AND across steps; the innermost enumeration wins; `whiteSpace` applied
  before lexical parsing.
- **Composition** — `include`, `import`, `redefine` and `override`, including
  **chameleon includes**, where a document with no `targetNamespace` is
  absorbed into its includer's. Circular graphs terminate.
- **Resolution** — references, attribute-group flattening (transitive),
  substitution-group closure (transitive, skipping abstract heads),
  `keyref` → `key`.
- **Instance validation** in one streaming pass over `quick-xml`, with a
  typed PSVI: values arrive as `Value::Integer(42)`, not `"42"`. Handles
  `xsi:type` — prefix, derivation, `block` and abstractness — `xsi:nil`,
  substitution groups and wildcards.
- **Content models** compiled to Glushkov position automata, with
  **Unique Particle Attribution** checking falling out of the same structure.
  Extension appends to the base's content; restriction replaces it.
  `xs:all` gets per-member counters rather than `n!` regex paths.
- **XSD 1.1**, opt-in via `Version::Xsd11`: `openContent`,
  `defaultOpenContent`, `defaultAttributes`, and the relaxed UPA rule where an
  element particle beats a competing wildcard.
- **Diagnostics** with stable codes, source spans and help text. Every error
  is reported, not just the first.

## Roadmap

| | | |
|---|---|---|
| ✅ | Component model, loading, composition | done |
| ✅ | Content automata, UPA | done |
| ✅ | Python bindings, type stubs, encoding detection | done |
| ✅ | Instance validation, typed reading (PSVI) | done |
| ✅ | `redefine` / `override` | done |
| ✅ | XSD 1.1 open content, default attributes, relaxed UPA | done |
| → | **XSD 1.1 assertions and conditional type assignment** | next |
| | Identity constraint enforcement | |

Code generation is permanently out of scope.

### Encodings

Document encodings are detected from a byte-order mark, then the XML
declaration, then UTF-8; bytes that contradict the encoding they claim are an
error rather than a document quietly full of replacement characters.

## Diagnostics

Building returns every diagnostic at once:

```text
error[XSD1201]: no type named `{urn:example}Missing`
  --> schemas/report.xsd:14
  help: check the spelling, or add an xs:import for its namespace
```

`Conformance::Lax` downgrades violations that still permit building
components — real schemas ship with dangling imports often enough that the
mode earns its keep.

```rust
use xsdkit::{SchemaSetBuilder, Conformance};

let (schemas, diagnostics) = SchemaSetBuilder::new()
    .conformance(Conformance::Lax)
    .file("vendor/partial.xsd")
    .build_with_warnings();
```

## Conformance

Measured against the **W3C XML Schema Test Suite** (5,727 scored schema
cases from NIST, Microsoft, IBM, Sun, Boeing and Saxonica):

| | |
|---|---|
| valid schemas accepted | **99.7%** (5,231 / 5,247) |
| invalid schemas rejected | **66.7%** (320 / 480) |

The gap is the honest description of what this is. `xsdkit` reads real
schemas well; it does not yet enforce most of the specification's *validity
constraints*, so a schema it accepts is not thereby a valid schema. If you
need a conformance checker, use Xerces or Saxon; if you need to read a schema
that already works, this is built for that.

Document validation is the other half of the suite — 21,575 scored cases,
99.0% correct:

| | |
|---|---|
| valid documents accepted | **99.5%** (11,846 / 11,907) |
| invalid documents rejected | **98.4%** (9,517 / 9,668) |

```bash
git clone --depth 1 https://github.com/w3c/xsdtests /tmp/xsdtests
export XSDTESTS=/tmp/xsdtests
cargo test --test w3c_suite -- --nocapture              # schemas, seconds
cargo test --release --test w3c_suite -- --ignored --nocapture   # documents
```

## Security

Schemas arrive from elsewhere as often as documents do.

- **No network by default.** `FileResolver` refuses `http(s)://`; supply your
  own [`Resolver`] to opt in.
- **No external entities.** Not a setting — `roxmltree` performs no I/O, so
  they cannot be fetched. Internal DTD subsets *are* accepted, because real
  schemas use them (the W3C's own among them), with entity-reference-loop
  detection closing the billion-laughs vector.
- **Bounded work.** A per-document node cap (`nodes_limit`), an
  include-nesting cap, and cycle guards on every graph walk.
- **Fuzzed.** Four `cargo-fuzz` targets cover the loader, the pattern
  transpiler, value parsing and instance validation, seeded from the W3C
  suite. Every finding has a named regression test; see `fuzz/`.

## Design

[DESIGN.md](DESIGN.md) reviews the XSD format and 17 implementations across
8 languages, and lays out the staged plan this crate follows.

## License

MIT
