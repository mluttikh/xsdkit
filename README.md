# xsdkit

[![CI](https://github.com/mluttikh/xsdkit/actions/workflows/ci.yml/badge.svg)](https://github.com/mluttikh/xsdkit/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A **generic XSD reader**: parse W3C XML Schema into a queryable schema
component model, in Rust and (soon) Python.

> **Status: early.** The component model, document loading, reference
> resolution and content-model compilation work, and are tested against real
> schemas. Python bindings are next, then instance validation and the units
> layer — see [DESIGN.md](DESIGN.md) §3.14.

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
- **Composition** — `include`, `import`, and **chameleon includes**, where a
  document with no `targetNamespace` is absorbed into its includer's. Circular
  graphs terminate.
- **Resolution** — references, attribute-group flattening (transitive),
  substitution-group closure (transitive, skipping abstract heads),
  `keyref` → `key`.
- **Content models** compiled to Glushkov position automata, with
  **Unique Particle Attribution** checking falling out of the same structure.
  Extension appends to the base's content; restriction replaces it.
  `xs:all` gets per-member counters rather than `n!` regex paths.
- **Diagnostics** with stable codes, source spans and help text. Every error
  is reported, not just the first.

## Roadmap

| | | |
|---|---|---|
| ✅ | Component model, loading, composition | done |
| ✅ | Content automata, UPA | done |
| → | **Python bindings** | next |
| | Instance validation, typed reading (PSVI) | |
| | Unit binding extraction (GML, Energistics, `appinfo`) | |
| | XSD 1.1 | |
| | `xsd2arrow`, a separate package | |

`redefine`/`override` are currently read as plain includes, with a warning.
Code generation is permanently out of scope.

**Known limitation:** schema documents must currently be UTF-8. A schema
declaring `encoding="ISO-8859-1"` fails to load, and reports the failure as a
missing file. Fixed in the next release, alongside the Python bindings — the
`Resolver` trait has to return bytes rather than text, which is a breaking
change worth making before an API gets wrapped.

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

## Design

[DESIGN.md](DESIGN.md) reviews the XSD format and 17 implementations across
8 languages, and lays out the staged plan this crate follows.

## License

MIT
