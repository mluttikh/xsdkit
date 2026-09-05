# Loading schemas

## The three sources

A schema can come from a path, from text you already hold, or from bytes whose
encoding you would rather not guess at.

=== "Python"

    ```python
    import xsdkit

    xsdkit.SchemaSet.from_file("report.xsd")
    xsdkit.SchemaSet.from_string(xsd_text, uri="report.xsd")
    xsdkit.SchemaSet.from_bytes(raw, uri="report.xsd")
    ```

=== "Rust"

    ```rust
    use xsdkit::SchemaSetBuilder;

    let schemas = SchemaSetBuilder::new()
        .file("report.xsd")
        .build()?;
    # Ok::<_, xsdkit::Diagnostics>(())
    ```

    ```rust
    # use xsdkit::SchemaSetBuilder;
    # let xsd_text = "";
    # let raw: Vec<u8> = Vec::new();
    SchemaSetBuilder::new().text(xsd_text, "report.xsd");
    SchemaSetBuilder::new().bytes(raw, "report.xsd");
    ```

The `uri` is not decoration. It is what diagnostics point at and what relative
`schemaLocation` hints resolve against, so giving a real one to a string you
loaded from somewhere else makes every later error message useful.

Several documents can go into one set — which is the normal case when a schema
family has no single root:

```rust
# use xsdkit::SchemaSetBuilder;
let schemas = SchemaSetBuilder::new()
    .file("common.xsd")
    .file("orders.xsd")
    .file("shipping.xsd")
    .build()?;
# Ok::<_, xsdkit::Diagnostics>(())
```

## Encodings are detected, not assumed

`from_bytes` reads the byte-order mark, then the XML declaration, then falls
back to UTF-8. Bytes that contradict the encoding they claim are an **error**,
not a document quietly full of `U+FFFD` replacement characters — a schema that
silently loses a character in a `pattern` facet is worse than one that fails
to load.

```python
from pathlib import Path
schemas = xsdkit.SchemaSet.from_bytes(Path("report.xsd").read_bytes())
```

In Python, `validate` and `iter_typed` take `str` or `bytes` for the same
reason: hand them the bytes and let the library read the declaration.

## Finding the other documents

`xs:include`, `xs:import`, `xs:redefine` and `xs:override` all name a
`schemaLocation`. The specification is explicit that this is a *hint* — a
processor is free to ignore it and use its own copy — which is why every
resolution strategy here is yours to choose.

### Search paths

```python
schemas = xsdkit.SchemaSet.from_file(
    "report.xsd",
    search_paths=["schemas/", "vendor/schemas/"],
)
```

Locations are tried relative to the referring document first, then against
each search path in order.

### A resolver

When the documents are not on disk at all — in a zip, in a database, behind an
HTTP client you control, pinned to versions you vendored — supply a resolver.
It is a function of `(location, base)`.

```python
import zipfile

with zipfile.ZipFile("schemas.zip") as z:
    schemas = xsdkit.SchemaSet.from_string(
        main_xsd,
        resolver=lambda location, base: z.read(location),
    )
```

Return `bytes` (best — the encoding is then detected), or `str`, or a
`(uri, document)` pair to record where it was actually found so diagnostics
name the right file. Raise to say it could not be resolved; your exception
message becomes the diagnostic.

!!! warning "A resolver replaces the filesystem"

    It is an alternative to `search_paths`, not a layer on top of it. Once you
    supply one, it is asked for everything, and nothing falls back to disk.

=== "Rust"

    ```rust
    use xsdkit::{Resolver, SchemaSetBuilder};

    struct Vendored;

    impl Resolver for Vendored {
        fn resolve(&self, location: &str, _base: Option<&str>) -> Result<(String, Vec<u8>), String> {
            let path = format!("vendor/{location}");
            std::fs::read(&path)
                .map(|bytes| (path.clone(), bytes))
                .map_err(|e| format!("{path}: {e}"))
        }
    }

    let schemas = SchemaSetBuilder::new()
        .resolver(Vendored)
        .file("report.xsd")
        .build()?;
    # Ok::<_, xsdkit::Diagnostics>(())
    ```

### The network is off

The built-in `FileResolver` refuses `http://` and `https://` outright:

```text
error[XSD1101]: refusing to fetch `http://www.w3.org/2001/xml.xsd` over the network;
                supply a resolver or a local copy
  --> report.xsd:3
  help: `schemaLocation` is a hint; add a search path or a custom Resolver
```

Fetching a schema over the network at load time makes your build depend on
someone else's uptime and turns a schema reference into a remote code path.
If you want it, write four lines of resolver and own the decision. See
[Security](project/security.md).

One special case needs no fetching at all: the `xml:` namespace. `xml:lang`,
`xml:space`, `xml:base` and `xml:id` are predeclared, so a schema that imports
`xml.xsd` works without it being present.

## Composition

All four composition mechanisms are implemented, including the awkward ones.

| Directive | What it does |
|---|---|
| `xs:include` | Adds components from a document with the *same* target namespace |
| `xs:import` | Makes another namespace's components referenceable |
| `xs:redefine` | Includes a document and *replaces* some of its definitions (XSD 1.0) |
| `xs:override` | The same idea, redesigned to be comprehensible (XSD 1.1) |

**Chameleon includes** work: a document with no `targetNamespace` of its own is
absorbed into its includer's namespace. The same file included by two
different namespaces yields two distinct sets of components, which is the
behaviour the specification requires and a classic source of bugs.

```python
for d in schemas.documents:
    print(d.uri, d.target_namespace, "chameleon" if d.chameleon else "")
```

Circular include graphs terminate. So do circular type derivations, and every
other graph walk in the library — see [Security](project/security.md).

!!! note "`redefine` has a rule that catches everyone"

    Inside `xs:redefine`, a reference to the name being redefined means the
    **original**. `<complexType name="T"><extension base="T">` extends the
    included `T`, not the one being declared. `xs:override` deliberately has no
    such rule: there, references mean the new components.

## Options

Every loader takes the same set.

| Option | Default | Meaning |
|---|---|---|
| `search_paths` | none | Directories to try for `schemaLocation` hints |
| `resolver` | filesystem | Replaces resolution entirely |
| `conformance` | `strict` | `lax` downgrades some errors — see below |
| `version` | `"1.0"` | `"1.1"` turns on XSD 1.1 — see [XSD 1.1](xsd11.md) |
| `nodes_limit` | 10,000,000 | Cap on XML nodes per document |

### Strict and lax

`strict` refuses to hand back a schema that had any error. `lax` downgrades the
violations that still permit building usable components — a dangling `import`
being the common one — so you get the model *and* the list of what was wrong.

Real schemas ship with broken references often enough that the mode earns its
keep.

=== "Python"

    ```python
    schemas, diagnostics = xsdkit.load("vendor/partial.xsd", conformance="lax")

    for d in diagnostics:
        print(d)
    # error[XSD1201]: no type named `{urn:vendor}Missing`
    #   --> vendor/partial.xsd:12
    ```

    `load` and `load_string` return the diagnostics instead of raising.
    `SchemaSet.from_file` raises `SchemaError` — which carries the full list on
    its `.diagnostics` — so use whichever matches whether imperfection is
    expected.

=== "Rust"

    ```rust
    use xsdkit::{Conformance, SchemaSetBuilder};

    let (schemas, diagnostics) = SchemaSetBuilder::new()
        .conformance(Conformance::Lax)
        .file("vendor/partial.xsd")
        .build_with_warnings();
    ```

    `build()` returns `Result<Schemas, Diagnostics>`; `build_with_warnings()`
    always returns a `Schemas` alongside everything that was found.

## Cost

Loading is linear in the size of the documents and is the expensive half;
querying afterwards is not. A 3,000-declaration schema compiles in about 15 ms.
Build once and keep the result — see [Performance](project/performance.md).

## Next

- [Querying the model](querying.md) — what to do with the result.
- [Diagnostics](diagnostics.md) — reading what went wrong.
