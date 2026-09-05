# Rust API

The complete rustdoc for the crate is published alongside this site.

[:octicons-arrow-right-24: Open the Rust API reference](../rust/xsdkit/index.html){ .md-button .md-button--primary }

## The shape of it

`src/lib.rs` re-exports everything that is public API. If a name is not
re-exported there, it is not part of the compatibility promise.

| Start at | For |
|---|---|
| [`SchemaSetBuilder`](../rust/xsdkit/struct.SchemaSetBuilder.html) | Reading documents into a schema set |
| [`Schemas`](../rust/xsdkit/model/struct.Schemas.html) | The compiled model, and every query over it |
| [`Diagnostics`](../rust/xsdkit/diagnostics/struct.Diagnostics.html) | What went wrong, with codes and spans |
| [`Value`](../rust/xsdkit/values/enum.Value.html) | A typed XSD value |
| [`ContentMatcher`](../rust/xsdkit/content/struct.ContentMatcher.html) | Stepping a content model by hand |

### Modules worth knowing

| Module | |
|---|---|
| [`model`](../rust/xsdkit/model/index.html) | The component types and `Schemas` itself |
| [`content`](../rust/xsdkit/content/index.html) | Content models, automata, UPA |
| [`instance`](../rust/xsdkit/instance/index.html) | Streaming validation and the PSVI |
| [`values`](../rust/xsdkit/values/index.html) | Typed values and facet checking |
| [`atomic`](../rust/xsdkit/atomic/index.html) | The 14 datatypes implemented from the specification |
| [`datatypes`](../rust/xsdkit/datatypes/index.html) | The 50 built-ins and their derivation graph |
| [`diagnostics`](../rust/xsdkit/diagnostics/index.html) | Codes, severities, spans |
| [`load`](../rust/xsdkit/load/index.html) | Resolvers, conformance modes, versions |
| [`names`](../rust/xsdkit/names/index.html) | Interned qualified names |
| [`regex`](../rust/xsdkit/regex/index.html) | XSD patterns, transpiled |

## Ids, not references

Components are held in arenas and addressed by `Copy` ids — `TypeId`,
`ElementId`, `ParticleId` and the rest. A component graph is cyclic (a type can
contain an element of that type), so Rust references would mean `Rc<RefCell<…>>`
everywhere; ids keep the model `Send + Sync`, cheap to copy, and cheap to
compare.

Ids are indexes into the `Schemas` that produced them. `Index` is implemented
for each of them, so the arena reads as a lookup:

```rust
# use xsdkit::Schemas;
# fn demo(schemas: &Schemas, element: xsdkit::ElementId) {
let decl = &schemas[element];
let ty   = &schemas[decl.type_id];
# }
```

An id from one `Schemas` used against another is a programming error, not a
runtime check — treat them as belonging to the set they came from.

## Doc comments are the reference

The crate is documented inline and the docs build runs with
`RUSTDOCFLAGS="-D warnings"` in CI, so a broken intra-doc link fails the build
rather than rotting quietly.

```bash
cargo doc --open
```
