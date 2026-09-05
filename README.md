# xsdkit

[![CI](https://github.com/mluttikh/xsdkit/actions/workflows/ci.yml/badge.svg)](https://github.com/mluttikh/xsdkit/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A **generic XSD reader**: parse W3C XML Schema into a queryable schema
component model, in Rust and Python.

> **Status: early.** The component model, document loading, reference
> resolution, content-model compilation and the Python bindings work, and are
> tested against real schemas. Instance validation and the units layer come
> next — see [DESIGN.md](DESIGN.md) §3.14.

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
[`xsd-parser`](https://crates.io/crates/xsd-parser)'"'"'s job. Generating
`xml2arrow` YAML is `xsd2arrow`'"'"'s — a separate package, so that reading a
schema never pulls in `arrow`.

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
let ty = schemas[report].type_id;

// Which children may appear, may they repeat, may they be absent?
// Answered from the compiled automaton, with substitution groups expanded
// and inherited content included.
for child in schemas.possible_children(ty) {
    println!(
        "{}  repeating={}  optional={}",
        schemas.display_name(schemas[child].name),
        schemas.child_repeats(ty, child),
        schemas.child_is_optional(ty, child),
    );
}

// Does a sequence of children satisfy the content model?
let mut m = schemas.match_content(ty).unwrap();
let ok = m.step(schemas.qname(Some("urn:example"), "title").unwrap()) && m.accepts_end();
```

### Python

```bash
pip install xsdkit
```

```python
import xsdkit

schemas = xsdkit.SchemaSet.from_file("report.xsd", search_paths=["schemas/"])
report = schemas.element("urn:example", "report")

for child in report.type.children:
    print(child.local_name,
          "repeats" if report.type.repeats(child) else "once",
          "optional" if report.type.optional(child) else "required")

# Does a child sequence satisfy the content model?
report.type.accepts(["{urn:example}title", "{urn:example}count"])
```

Validate a document, and read it into typed values:

```python
report = schemas.validate(open("report.xml").read())
report.is_valid           # False
for d in report.errors:
    print(d)              # error[XSD2004]: `{urn:example}count`: ... --> :3

events, report = schemas.read_typed(open("report.xml").read())
for ev in events:
    if ev.kind == "text":
        print(type(ev.value).__name__, ev.value)
# int       42
# Decimal   3.14
# datetime  2024-12-30 12:39:15+00:00
# date      2024-03-31
```

Values arrive as native Python types, not strings to re-parse. Pass
`on_event=` to stream instead of collecting.

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
  `xsi:type`, `xsi:nil`, substitution groups and wildcards.
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
| | `xsd2arrow`, a separate package | |

Code generation is permanently out of scope.

### Units of measure

There is no unit layer, on purpose. No standard says *where* a unit lives in
an XSD — it was proposed to the XML Schema WG in 1999 and not adopted, and
UnitsML has been a committee draft since 2011. Only the vocabulary is
standardised (UCUM, UN/CEFACT Rec. 20), never the slot.

`xsdkit` exposes the facts instead: attribute uses folded down the derivation
chain, `fixed` and `default` values, enumeration facets, `appinfo` verbatim,
and schema-supplied attribute values flagged in the PSVI. Turning those into
your schema family's convention is about fifteen lines — see
[`examples/units.rs`](examples/units.rs):

```bash
cargo run --example units -- schemas/measures.xsd
# {urn:rig}Depth      fixed        m
# {urn:rig}Pressure   enumerated   ["Pa", "hPa", "bar"]
# {gml}MeasureType    per-instance @uom
```

A built-in heuristic would be right most of the time and silently wrong the
rest, and a wrong unit is worse than no unit.

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
| valid schemas accepted | **99.1%** (5,198 / 5,247) |
| invalid schemas rejected | **59.4%** (285 / 480) |

The gap is the honest description of what this is. `xsdkit` reads real
schemas well; it does not yet enforce most of the specification's *validity
constraints*, so a schema it accepts is not thereby a valid schema. If you
need a conformance checker, use Xerces or Saxon; if you need to read a schema
that already works, this is built for that.

Document validation is the other half of the suite — 21,533 scored cases,
95.0% correct:

| | |
|---|---|
| valid documents accepted | **95.7%** (11,381 / 11,890) |
| invalid documents rejected | **94.2%** (9,080 / 9,643) |

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
