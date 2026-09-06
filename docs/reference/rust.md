# Rust API

The complete rustdoc for the crate is published alongside this site.

[:octicons-arrow-right-24: Open the Rust API reference](../rust/xsdkit/index.html){ .md-button .md-button--primary }

## The shape of it

`src/lib.rs` re-exports everything that is public API. If a name is not
re-exported there, it is not part of the compatibility promise.

| Start at | For |
|---|---|
| [`SchemaSetBuilder`](../rust/xsdkit/struct.SchemaSetBuilder.html) | Reading documents into a schema set |
| [`Compilation`](../rust/xsdkit/struct.Compilation.html) | What compiling produced: the components *and* every diagnostic |
| [`Schemas`](../rust/xsdkit/model/struct.Schemas.html) | The compiled model, and every query over it |
| [`ElementRef`](../rust/xsdkit/refs/struct.ElementRef.html) | Following a schema without touching an id |
| [`DocumentValidator`](../rust/xsdkit/instance/struct.DocumentValidator.html) | Checking a document, and the typed PSVI |
| [`ValueValidator`](../rust/xsdkit/validate/struct.ValueValidator.html) | Checking one lexical form against a simple type |
| [`Diagnostics`](../rust/xsdkit/diagnostics/struct.Diagnostics.html) | What went wrong, with codes and spans |
| [`Value`](../rust/xsdkit/values/enum.Value.html) | A typed XSD value |

### Modules worth knowing

| Module | |
|---|---|
| [`model`](../rust/xsdkit/model/index.html) | The component types and `Schemas` itself |
| [`refs`](../rust/xsdkit/refs/index.html) | The navigable view: `ElementRef`, `TypeRef`, `ChildRef` |
| [`content`](../rust/xsdkit/content/index.html) | Content models, matching, UPA |
| [`instance`](../rust/xsdkit/instance/index.html) | Streaming validation and the PSVI |
| [`values`](../rust/xsdkit/values/index.html) | Typed values and facet checking |
| [`atomic`](../rust/xsdkit/atomic/index.html) | The 14 datatypes implemented from the specification |
| [`datatypes`](../rust/xsdkit/datatypes/index.html) | The 50 built-ins and their derivation graph |
| [`diagnostics`](../rust/xsdkit/diagnostics/index.html) | Codes, severities, spans |
| [`names`](../rust/xsdkit/names/index.html) | Interned qualified names |
| [`regex`](../rust/xsdkit/regex/index.html) | XSD patterns, transpiled |

Configuration — `Version`, `Conformance`, `Resolver`, `FileResolver` — lives at
the crate root rather than in a module named after the phase that consumes it.

## Two ways to reach a component

Components are held in arenas and addressed by `Copy` ids — `TypeId`,
`ElementId`, `ParticleId` and the rest. A component graph is cyclic (a type can
contain an element of that type), so Rust references would mean `Rc<RefCell<…>>`
everywhere; ids keep the model `Send + Sync`, cheap to copy, and cheap to
compare.

Ids are not what you want to *ask questions with*, though, so there is a view
over them. An `ElementRef` is a borrow of the schema plus an id — two words,
`Copy`, no allocation and no reference counting — and following one reads the
way the schema reads:

```rust
use xsdkit::Schemas;

fn describe(schemas: &Schemas) -> Option<()> {
    let report = schemas.element(Some("urn:example"), "report")?;
    for child in report.children() {
        println!("{}: {}", child.local_name(), child.type_of().display_name());
    }
    Some(())
}
```

The arena underneath is never more than a method call away. `Index` is
implemented for every id, name lookups have `_id` forms that hand ids back, and
`Schemas::get` turns an id into a reference:

```rust
use xsdkit::{ElementId, Schemas};

fn by_id(schemas: &Schemas, element: ElementId) {
    let decl = &schemas[element];             // the raw component
    let ty = &schemas[decl.type_id];
    let same = schemas.get(element);          // …or a reference to it
    assert_eq!(same.name(), decl.name);
}
```

An id from one `Schemas` used against another is a programming error, not a
runtime check — treat them as belonging to the set they came from. References
carry their schema, so comparing two of them accounts for it.

## Cargo features

| Feature | |
|---|---|
| `serde` | `Serialize`/`Deserialize` for `Schemas`, so a large schema set is compiled once and loaded thereafter |
| `python` | Builds the PyO3 extension module; not for library use |

## Doc comments are the reference

The crate is documented inline and the docs build runs with
`RUSTDOCFLAGS="-D warnings"` in CI, so a broken intra-doc link fails the build
rather than rotting quietly. The Rust snippets on *this* site are compiled too,
by `scripts/check-doc-snippets.py`, for the same reason.

```bash
cargo doc --open
```
