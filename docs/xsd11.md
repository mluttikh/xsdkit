# XSD 1.1

XSD 1.1 is **opt-in**. Reading a 1.1 document as 1.0 is not an error — the
specification designed the new constructs to be ignorable — so the version is a
decision you make, not one inferred from the file.

=== "Python"

    ```python
    schemas = xsdkit.SchemaSet.from_file("report.xsd", version="1.1")
    ```

=== "Rust"

    ```rust
    use xsdkit::{SchemaSetBuilder, Version};

    let schemas = SchemaSetBuilder::new()
        .version(Version::Xsd11)
        .file("report.xsd")
        .compile().into_result()?;
    # Ok::<_, xsdkit::Diagnostics>(())
    ```

Where a 1.1 construct is silently ignored under 1.0, you get a warning rather
than nothing:

```text
warning[XSD1104]: `xs:defaultAttributes` is an XSD 1.1 construct and is ignored
  --> report.xsd:2
  help: build with `Version::Xsd11` to process it
```

## What turning it on gives you

### Open content

A complex type can accept elements its content model never mentioned, either
interleaved with the declared ones or only at the end.

```xml
<xs:complexType name="T">
  <xs:openContent mode="interleave">
    <xs:any namespace="##other" processContents="lax"/>
  </xs:openContent>
  <xs:sequence>
    <xs:element name="a" type="xs:string"/>
  </xs:sequence>
</xs:complexType>
```

```python
doc = '<e xmlns="urn:t" xmlns:o="urn:other"><a>x</a><o:extra>y</o:extra></e>'

xsdkit.SchemaSet.from_string(xsd, version="1.0").validate(doc).is_valid  # False
xsdkit.SchemaSet.from_string(xsd, version="1.1").validate(doc).is_valid  # True
```

`xs:defaultOpenContent` applies the same rule to every complex type in a
document, so a schema family can be made extensible in one line.

### Default attributes

`<xs:schema defaultAttributes="tns:Common">` attaches an attribute group to
every complex type in the document — the version-stamp-on-everything pattern,
without repeating it on all two hundred types.

### Relaxed Unique Particle Attribution

XSD 1.0 rejects a content model where a wildcard and an element could both
match the same child. 1.1 keeps the model and gives the **element** priority.

```xml
<xs:sequence>
  <xs:element name="title" type="xs:string"/>
  <xs:any namespace="##any" processContents="lax" minOccurs="0"/>
</xs:sequence>
```

Ambiguous under 1.0 (`XSD1304`); legal under 1.1, where `title` wins.

### Conditional inclusion

The `vc:` attributes let one document serve both versions. `vc:minVersion`,
`vc:maxVersion`, `vc:typeAvailable` and `vc:facetAvailable` decide whether an
element is read at all, so a schema can carry a 1.1 refinement and a 1.0
fallback side by side.

### Richer wildcards

- `notNamespace` — everything *except* the listed namespaces.
- Element Declarations Consistent, checked when a document reaches it rather
  than when the schema is read: a wildcard may admit a name the content model
  also declares, and that is an error only if no value could satisfy both.
- `explicitTimezone` — `required` insists a temporal value name an instant,
  `prohibited` says it is meant to be read locally.
- `notQName` — everything except the listed names, including the special
  `##defined` (any element declared as a global) and `##definedSibling` (any
  element declared as a sibling in this content model).

### `xs:precisionDecimal`

The 1.1 numeric type that remembers its precision: `1.50` and `1.5` are equal
in value and distinguishable in form, which is the point of it.

```python
schemas.type("http://www.w3.org/2001/XMLSchema", "precisionDecimal").validate("1.50")
# '1.50' — trailing zero preserved
```

### Local target namespaces

`targetNamespace` on a local element or attribute declaration, which 1.0 does
not allow.

## What is not implemented

Two 1.1 features are read into the model and **not evaluated**:

| Feature | Status |
|---|---|
| `xs:assert` / `xs:assertion` | Stored on the type; never checked |
| Conditional type assignment (`xs:alternative`) | Parsed; the alternative is never selected |

Both need an XPath 2.0 subset, which is a substantial piece of work in its own
right and is the next thing on the roadmap. Until then, a document that
violates an assertion validates successfully.

This is the honest caveat on turning 1.1 on: everything above works, and
assertions quietly do not. The
[conformance figures](project/conformance.md) count the cases this costs.
